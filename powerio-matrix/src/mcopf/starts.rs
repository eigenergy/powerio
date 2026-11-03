//! No-load start values for transformer networks.
//!
//! The start rows are fixed source voltages, closed branch equalities and the
//! voltage part of each transformer row (tap multiplier one, currents zero).
//! The system is rectangular and usually rank deficient: a delta winding or a
//! floating secondary leaves a common mode free. Elimination in terminal
//! order with partial pivoting picks the free columns; every free start is
//! zero and the pivot columns follow by back substitution.
//!
//! Each row touches a handful of terminals and elimination over branch and
//! winding rows keeps them short, so the rows are stored sparse with an index
//! of the rows that hold each terminal. Memory follows the stored entries, not
//! rows times terminals as in the dense Gauss-Jordan elimination this
//! replaces. The column order, pivot rule and forward arithmetic are the dense
//! ones, so the same columns are free and the starts agree to rounding.
use super::{
    BTreeSet, McOpfBranch, McOpfTerminal, McOpfTransformer, Result, join, root, unsupported,
};

/// Pivot magnitude below which a column is treated as dependent.
const PIVOT_TOL: f64 = 1e-12;

/// One real start row: sparse terminal coefficients sorted by terminal, and
/// the complex right hand side `[real, imaginary]`.
#[derive(Clone, Debug, Default)]
struct StartRow {
    coeffs: Vec<(usize, f64)>,
    rhs: [f64; 2],
}

impl StartRow {
    /// Sum duplicate terminals in their given order and drop exact zeros,
    /// which are inert in the elimination.
    fn from_terms(terms: impl IntoIterator<Item = (usize, f64)>, rhs: [f64; 2]) -> Self {
        let mut terms: Vec<_> = terms.into_iter().collect();
        terms.sort_by_key(|&(k, _)| k);
        let mut coeffs: Vec<(usize, f64)> = Vec::with_capacity(terms.len());
        for (k, c) in terms {
            match coeffs.last_mut() {
                Some(last) if last.0 == k => last.1 += c,
                _ => coeffs.push((k, 0.0 + c)),
            }
        }
        coeffs.retain(|&(_, c)| c != 0.0);
        Self { coeffs, rhs }
    }

    fn get(&self, col: usize) -> f64 {
        self.coeffs
            .binary_search_by_key(&col, |&(k, _)| k)
            .map_or(0.0, |i| self.coeffs[i].1)
    }
}

/// Solve the no-load voltage equations for starts. Free common modes are zero;
/// these starts never impose a physical voltage reference on the OPF.
pub(super) fn initialize(
    branches: &[McOpfBranch],
    transformers: &[McOpfTransformer],
    terminals: &mut [McOpfTerminal],
) -> Result<()> {
    let rows = start_rows(branches, transformers, terminals)?;
    let pivots = solve_sparse(terminals.len(), rows);
    apply(terminals, &pivots);
    Ok(())
}

/// Build the start rows in the reference order (fixed terminals, closed
/// branch conductors, transformer voltage rows) and refuse a terminal with no
/// path to a fixed voltage.
fn start_rows(
    branches: &[McOpfBranch],
    transformers: &[McOpfTransformer],
    terminals: &[McOpfTerminal],
) -> Result<Vec<StartRow>> {
    let n = terminals.len();
    let mut rows = Vec::new();
    let mut connected: Vec<_> = (0..n).collect();
    for (k, t) in terminals.iter().enumerate() {
        if let Some(v) = t.fixed {
            rows.push(StartRow::from_terms([(k, 1.0)], v));
        }
    }
    for br in branches.iter().filter(|b| !b.open) {
        for (&a, &b) in br.from.iter().zip(&br.to) {
            rows.push(StartRow::from_terms([(a, 1.0), (b, -1.0)], [0.0; 2]));
            join(&mut connected, a, b);
        }
    }
    for tx in transformers {
        for coil in &tx.coils {
            if let Some(q) = coil.negative {
                join(&mut connected, coil.positive, q);
            }
            join(&mut connected, tx.coils[0].positive, coil.positive);
        }
        for (index, eq) in tx.equations.iter().enumerate() {
            if eq.voltage.is_empty() {
                continue;
            }
            let terms = eq
                .voltage
                .iter()
                .chain(
                    tx.tap_equations
                        .get(index)
                        .into_iter()
                        .flat_map(|r| &r.voltage),
                )
                .map(|&(k, c)| (k, c[0]));
            rows.push(StartRow::from_terms(terms, [0.0; 2]));
        }
    }
    let anchors: BTreeSet<_> = terminals
        .iter()
        .enumerate()
        .filter(|(_, t)| t.fixed.is_some())
        .map(|(k, _)| root(&mut connected, k))
        .collect();
    for (k, t) in terminals.iter().enumerate() {
        if !anchors.contains(&root(&mut connected, k)) {
            return Err(unsupported(&t.bus, "unreferenced transformer island"));
        }
    }
    Ok(rows)
}

/// Zero every free start, then write each pivot column's value.
fn apply(terminals: &mut [McOpfTerminal], pivots: &[(usize, [f64; 2])]) {
    for t in terminals.iter_mut().filter(|t| t.fixed.is_none()) {
        t.start = [0.0, 0.0];
    }
    for &(c, value) in pivots {
        if terminals[c].fixed.is_none() {
            terminals[c].start = value;
        }
    }
}

/// Sparse Gaussian elimination over terminal columns `0..n` followed by back
/// substitution. Returns each pivot column with its value when every free
/// column is zero.
///
/// The forward pass is the dense reference's: rows keep a stable id and
/// `order` maps a position to a row id, so the pivot search and its row swap
/// match it exactly (the candidate with the largest magnitude at or below the
/// pivot position wins, the later position on a tie) and the rows below each
/// pivot receive the same updates. The dense reference also eliminated each
/// pivot column from the rows above it; doing that here would make every
/// earlier pivot row on a series path of transformers hold the current
/// column, which is quadratic work. Back substitution reaches the same values
/// to rounding.
fn solve_sparse(n: usize, rows: Vec<StartRow>) -> Vec<(usize, [f64; 2])> {
    solve_sparse_counted(n, rows).0
}

/// [`solve_sparse`], also returning the peak number of stored coefficients.
fn solve_sparse_counted(n: usize, mut rows: Vec<StartRow>) -> (Vec<(usize, [f64; 2])>, usize) {
    let mut stored: usize = rows.iter().map(|r| r.coeffs.len()).sum();
    let mut peak = stored;
    let mut order: Vec<usize> = (0..rows.len()).collect();
    let mut position = order.clone();
    // Row ids that may hold an entry in a column; checked when consumed.
    let mut by_column: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (id, row) in rows.iter().enumerate() {
        for &(k, _) in &row.coeffs {
            by_column[k].push(id);
        }
    }
    let mut pivot = 0;
    let mut pivots = Vec::new();
    let mut scratch = Vec::new();
    for col in 0..n {
        let mut holders = std::mem::take(&mut by_column[col]);
        holders.sort_unstable();
        holders.dedup();
        // Rows already pivoted stay as they are for back substitution.
        holders.retain(|&id| position[id] >= pivot && rows[id].get(col) != 0.0);
        let best = holders.iter().copied().max_by(|&a, &b| {
            rows[a]
                .get(col)
                .abs()
                .total_cmp(&rows[b].get(col).abs())
                .then(position[a].cmp(&position[b]))
        });
        let Some(best) = best.filter(|&id| rows[id].get(col).abs() > PIVOT_TOL) else {
            // A dependent column. An unpivoted row never reads an entry left
            // of its own pivot column, so these entries are dropped.
            for &id in &holders {
                rows[id].coeffs.retain(|&(k, _)| k != col);
            }
            stored -= holders.len();
            continue;
        };
        let displaced = order[pivot];
        let at = position[best];
        order.swap(pivot, at);
        position[best] = pivot;
        position[displaced] = at;

        // Normalize the pivot row and keep only the columns ahead of it.
        let pivot_row = &mut rows[best];
        let d = pivot_row.get(col);
        let before = pivot_row.coeffs.len();
        pivot_row.coeffs.retain(|&(k, _)| k > col);
        for entry in &mut pivot_row.coeffs {
            entry.1 /= d;
        }
        for v in &mut pivot_row.rhs {
            *v /= d;
        }
        stored -= before - pivot_row.coeffs.len();
        let active = pivot_row.coeffs.clone();
        let active_rhs = pivot_row.rhs;
        for &id in holders.iter().filter(|&&id| id != best) {
            let row = &mut rows[id];
            let f = row.get(col);
            let before = row.coeffs.len();
            eliminate(row, f, col, &active, &mut scratch, |k| {
                by_column[k].push(id);
            });
            for (v, p) in row.rhs.iter_mut().zip(active_rhs) {
                *v -= f * p;
            }
            stored = stored + row.coeffs.len() - before;
            peak = peak.max(stored);
        }
        pivots.push((best, col));
        pivot += 1;
    }
    // Back substitution with every free column at zero.
    let mut value = vec![[0.0; 2]; n];
    for &(id, col) in pivots.iter().rev() {
        let row = &rows[id];
        let mut v = row.rhs;
        for &(k, a) in &row.coeffs {
            v[0] -= a * value[k][0];
            v[1] -= a * value[k][1];
        }
        value[col] = v;
    }
    let values = pivots
        .into_iter()
        .map(|(_, col)| (col, value[col]))
        .collect();
    (values, peak)
}

/// `row -= f * active` for an unpivoted row holding `col`, where `active`
/// holds the normalized pivot row's entries right of `col`. The row's own
/// entry at `col` cancels exactly (`r - r * 1`) and is dropped. A new entry
/// calls `fill` with its column; an exact zero is dropped.
fn eliminate(
    row: &mut StartRow,
    f: f64,
    col: usize,
    active: &[(usize, f64)],
    scratch: &mut Vec<(usize, f64)>,
    mut fill: impl FnMut(usize),
) {
    scratch.clear();
    let mut a = active.iter().peekable();
    for &(k, r) in row.coeffs.iter().filter(|e| e.0 > col) {
        while let Some(&&(j, p)) = a.peek()
            && j < k
        {
            scratch.push((j, 0.0 - f * p));
            fill(j);
            a.next();
        }
        match a.peek() {
            Some(&&(j, p)) if j == k => {
                scratch.push((k, r - f * p));
                a.next();
            }
            _ => scratch.push((k, r)),
        }
    }
    for &(j, p) in a {
        scratch.push((j, 0.0 - f * p));
        fill(j);
    }
    scratch.retain(|&(_, c)| c != 0.0);
    std::mem::swap(&mut row.coeffs, scratch);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcopf::{
        BoundedLoadLawPolicy, McAcOpfAssemblyOptions, McOpfComplexRow, McOpfTransformerCoil,
        build_mc_ac_opf_preparation,
    };
    use powerio_prob::McAcOpfInstance;
    use std::time::{Duration, Instant};

    /// The dense Gauss-Jordan elimination the sparse solve replaced, kept as
    /// the oracle: rows times `n + 2` doubles, cubic time.
    fn solve_dense(n: usize, sparse: &[StartRow]) -> Vec<(usize, [f64; 2])> {
        let mut rows: Vec<Vec<f64>> = sparse
            .iter()
            .map(|r| {
                let mut row = vec![0.0; n + 2];
                for &(k, c) in &r.coeffs {
                    row[k] = c;
                }
                row[n] = r.rhs[0];
                row[n + 1] = r.rhs[1];
                row
            })
            .collect();
        let mut pivot = 0;
        let mut pivots = Vec::new();
        for col in 0..n {
            let best = (pivot..rows.len())
                .max_by(|&a, &b| rows[a][col].abs().total_cmp(&rows[b][col].abs()));
            let Some(best) = best.filter(|&r| rows[r][col].abs() > PIVOT_TOL) else {
                continue;
            };
            rows.swap(pivot, best);
            let d = rows[pivot][col];
            for v in &mut rows[pivot][col..] {
                *v /= d;
            }
            let row = rows[pivot].clone();
            for (j, r) in rows.iter_mut().enumerate() {
                if j != pivot {
                    let f = r[col];
                    for k in col..n + 2 {
                        r[k] -= f * row[k];
                    }
                }
            }
            pivots.push((pivot, col));
            pivot += 1;
        }
        pivots
            .into_iter()
            .map(|(r, c)| (c, [rows[r][n], rows[r][n + 1]]))
            .collect()
    }

    fn assert_close(sparse: &[(usize, [f64; 2])], dense: &[(usize, [f64; 2])]) {
        assert_eq!(
            sparse.iter().map(|p| p.0).collect::<Vec<_>>(),
            dense.iter().map(|p| p.0).collect::<Vec<_>>(),
            "pivot columns differ"
        );
        for (s, d) in sparse.iter().zip(dense) {
            for i in 0..2 {
                assert!(
                    (s.1[i] - d.1[i]).abs() <= 1e-12,
                    "terminal {}: sparse {:?} dense {:?}",
                    s.0,
                    s.1,
                    d.1
                );
            }
        }
    }

    fn fixture(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/dist")
            .join(rel)
    }

    fn parse(
        rel: &str,
        format: &str,
    ) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
        let source = powerio_core::Source::open(fixture(rel))
            .unwrap()
            .with_format(powerio_core::FormatId::new(format).unwrap());
        powerio_dist::parse(source).unwrap()
    }

    /// Parse a distribution fixture and prepare it, after dropping the
    /// OpenDSS commands, options, untyped objects and retained source fields
    /// the preparation refuses. None of them enters a start row.
    fn preparation(rel: &str, format: &str) -> crate::mcopf::McAcOpfPreparation {
        let mut network = parse(rel, format).into_value();
        network.untyped_objects_mut().clear();
        network.commands_mut().clear();
        network.options_mut().clear();
        network.extras_mut().clear();
        network
            .sources_mut()
            .iter_mut()
            .for_each(|e| e.extras.clear());
        network
            .buses_mut()
            .iter_mut()
            .for_each(|e| e.extras.clear());
        network
            .line_codes_mut()
            .iter_mut()
            .for_each(|e| e.extras.clear());
        network
            .lines_mut()
            .iter_mut()
            .for_each(|e| e.extras.clear());
        network
            .switches_mut()
            .iter_mut()
            .for_each(|e| e.extras.clear());
        network
            .loads_mut()
            .iter_mut()
            .for_each(|e| e.extras.clear());
        network
            .capacitors_mut()
            .iter_mut()
            .for_each(|e| e.extras.clear());
        network
            .shunts_mut()
            .iter_mut()
            .for_each(|e| e.extras.clear());
        for t in network.transformers_mut() {
            t.extras
                .retain(|k, _| super::super::transformer::INTERPRETED_EXTRAS.contains(&k.as_str()));
        }
        let options = McAcOpfAssemblyOptions::new(2401.8, 1.0e6)
            .with_bounded_load_laws(BoundedLoadLawPolicy::ProjectToNominal);
        match build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(network).unwrap(),
            &options,
        ) {
            Ok(p) => p,
            Err(e) => panic!("{rel}: {e}"),
        }
    }

    /// The sparse solve reproduces the dense elimination on the IEEE 13 and
    /// 123 node feeders, regulators and the floating delta-delta XFM1
    /// included, and on the PMD IEEE 13 case.
    #[test]
    fn sparse_starts_match_the_dense_elimination_on_ieee_feeders() {
        for (rel, format) in [
            ("opendss/ieee13/IEEE13Nodeckt.dss", "dss"),
            ("opendss/ieee123/IEEE123Master.dss", "dss"),
            ("pmd/ieee13.json", "pmd"),
        ] {
            let prep = preparation(rel, format);
            assert!(!prep.transformers.is_empty(), "{rel} has no transformer");
            let n = prep.terminals.len();
            let rows = start_rows(&prep.branches, &prep.transformers, &prep.terminals).unwrap();
            let dense = solve_dense(n, &rows);
            let sparse = solve_sparse(n, rows);
            assert_close(&sparse, &dense);
            let mut expected = prep.terminals.clone();
            apply(&mut expected, &dense);
            for (got, want) in prep.terminals.iter().zip(&expected) {
                for i in 0..2 {
                    assert!((got.start[i] - want.start[i]).abs() <= 1e-12, "{rel}");
                }
            }
        }
    }

    fn terminal(bus: usize, fixed: Option<[f64; 2]>) -> McOpfTerminal {
        McOpfTerminal {
            bus: format!("b{bus}"),
            terminal: "1".into(),
            grounded: false,
            fixed,
            start: [0.0; 2],
        }
    }

    /// A chain of single-phase stages. Each stage is a closed two-wire line
    /// into an ideal coil pair; every secondary floats with the next stage's
    /// primary, so each stage leaves one common mode free. Ratios alternate
    /// 1:2 and 2:1. `2 + 4 * stages` terminals.
    fn chain(stages: usize) -> (Vec<McOpfTerminal>, Vec<McOpfBranch>, Vec<McOpfTransformer>) {
        let mut terminals = vec![terminal(0, Some([1.0, 0.0])), terminal(0, Some([0.0, 0.0]))];
        let mut branches = Vec::new();
        let mut transformers = Vec::new();
        let mut head = (0, 1);
        for s in 0..stages {
            let base = terminals.len();
            for _ in 0..4 {
                terminals.push(terminal(base, None));
            }
            let (p, q, x, y) = (base, base + 1, base + 2, base + 3);
            let square = || vec![vec![0.0; 2]; 2];
            branches.push(McOpfBranch {
                identity: format!("line:{s}"),
                from: vec![head.0, head.1],
                to: vec![p, q],
                r: vec![vec![0.01, 0.0], vec![0.0, 0.01]],
                x: square(),
                g_from: square(),
                b_from: square(),
                g_to: square(),
                b_to: square(),
                current_max: vec![None; 2],
                apparent_max: vec![None; 2],
                open: false,
            });
            let ratio = if s % 2 == 0 { 0.5 } else { 2.0 };
            transformers.push(McOpfTransformer {
                identity: format!("t{s}"),
                coils: vec![
                    McOpfTransformerCoil {
                        positive: p,
                        negative: Some(q),
                    },
                    McOpfTransformerCoil {
                        positive: x,
                        negative: Some(y),
                    },
                ],
                equations: vec![McOpfComplexRow {
                    voltage: vec![
                        (x, [1.0, 0.0]),
                        (y, [-1.0, 0.0]),
                        (p, [-ratio, 0.0]),
                        (q, [ratio, 0.0]),
                    ],
                    current: Vec::new(),
                }],
                ports: Vec::new(),
                tap_bounds: None,
                tap_nominal: 1.0,
                tap_inverse: false,
                tap_equations: Vec::new(),
            });
            head = (x, y);
        }
        (terminals, branches, transformers)
    }

    fn last_secondary(terminals: &[McOpfTerminal], transformers: &[McOpfTransformer]) -> f64 {
        let coil = &transformers.last().unwrap().coils[1];
        terminals[coil.positive].start[0] - terminals[coil.negative.unwrap()].start[0]
    }

    /// The synthetic chain agrees with the dense oracle, leaves one free
    /// column per floating stage, and its last secondary carries the source
    /// voltage after an even number of stages.
    #[test]
    fn sparse_starts_match_the_dense_elimination_on_a_floating_chain() {
        let (mut terminals, branches, transformers) = chain(40);
        let rows = start_rows(&branches, &transformers, &terminals).unwrap();
        let n = terminals.len();
        let dense = solve_dense(n, &rows);
        assert_eq!(dense.len(), n - 40);
        assert_close(&solve_sparse(n, rows), &dense);
        initialize(&branches, &transformers, &mut terminals).unwrap();
        let across = last_secondary(&terminals, &transformers);
        assert!((across - 1.0).abs() < 1e-12, "{across}");
    }

    /// 2,002 terminals: the sparse solve holds two coefficients per terminal
    /// at its peak (4,002) and took 0.3 ms in a release build. The dense
    /// elimination held 1,502 rows of 2,004 doubles (24 MB) and took 1.1 s.
    #[test]
    fn sparse_starts_scale_to_thousands_of_terminals() {
        let (mut terminals, branches, transformers) = chain(500);
        let n = terminals.len();
        assert_eq!(n, 2002);
        let started = Instant::now();
        let rows = start_rows(&branches, &transformers, &terminals).unwrap();
        let dense_entries = rows.len() * (n + 2);
        let (pivots, peak) = solve_sparse_counted(n, rows);
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
        assert!(
            peak * 500 < dense_entries,
            "{peak} stored vs {dense_entries}"
        );
        apply(&mut terminals, &pivots);
        let across = last_secondary(&terminals, &transformers);
        assert!((across - 1.0).abs() < 1e-12, "{across}");
    }
}

//! Internal DC solver factors: the signed bus by branch incidence matrix and
//! positive branch weights.
//!
//! Edge orientation is fixed to MATPOWER's from→to: column `e` of `A` has
//! `+1` at the from bus (tail) and `−1` at the to bus (head). Columns run
//! over the accepted in-service branches in `case.branches` order.

use sprs::CsMat;

use powerio_tx::BranchSusceptanceFormula;

use crate::Result;
use crate::indexed::IndexedNetwork;
use crate::matrix::triplet::CooBuilder;

use super::BuildOptions;

/// The incidence factorization used by sensitivity builders.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub(crate) struct IncidenceParts {
    /// Signed incidence `A`, shape `n × m`.
    pub(crate) a: CsMat<f64>,
    /// Positive Laplacian edge weights `b_e`, length `m`: the factor weight
    /// a sparse solver uses (`|b|` of the PowerModels susceptance). The
    /// signed susceptance lives on the DC operator surface.
    pub(crate) b: Vec<f64>,
    /// Source-network branch indices for the accepted incidence columns.
    /// Entries align with columns of `a` and `b`; filtered branches are absent.
    pub(crate) branch_indices: Vec<usize>,
}

impl IncidenceParts {
    #[inline]
    pub(crate) fn n(&self) -> usize {
        self.a.rows()
    }

    #[inline]
    pub(crate) fn m(&self) -> usize {
        self.a.cols()
    }
}

/// Build the internal bus by branch incidence factor and positive weights.
///
/// Self-loops (from == to) are dropped. A branch whose reactance is too small
/// to divide by has no DC susceptance the Laplacian can carry; it is skipped
/// when `opts.skip_zero_impedance` is true and rejected with
/// [`powerio_tx::Error::ZeroImpedance`] when it is false. A tap ratio under the same bound
/// is [`powerio_tx::Error::DegenerateTap`] either way, as it is in Y_bus.
pub(crate) fn build_incidence(
    case: &IndexedNetwork,
    formula: BranchSusceptanceFormula,
    opts: &BuildOptions,
) -> Result<IncidenceParts> {
    let n = case.n();

    // Pass 1: resolve and filter, fixing the column order.
    let mut cols: Vec<Column> = Vec::new();
    for (idx, br) in case.in_service_branches() {
        let i = case
            .bus_index(br.from)
            .ok_or(powerio_tx::Error::UnknownBus {
                bus_id: br.from,
                element_index: idx,
            })?;
        let j = case.bus_index(br.to).ok_or(powerio_tx::Error::UnknownBus {
            bus_id: br.to,
            element_index: idx,
        })?;
        // Zero impedance in every sense the selected formula can act on:
        // `x = 1e-300` gives a finite weight of `1e300` that annihilates
        // every real branch sharing a diagonal with it. The guard reads only
        // the denominator the selected formula divides by, so a value the
        // formula never reads cannot reject a branch (#324): the reciprocal
        // rules bound the reactance, and the series formula bounds the whole
        // impedance magnitude.
        let degenerate = match formula {
            formula if formula.reads_resistance() => {
                br.r.hypot(br.x) < crate::matrix::MIN_DIVISIBLE_MAGNITUDE
            }
            _ => br.x.abs() < crate::matrix::MIN_DIVISIBLE_MAGNITUDE,
        };
        if i == j || degenerate {
            if i != j && degenerate && !opts.skip_zero_impedance {
                return Err(powerio_tx::Error::ZeroImpedance { row: idx }.into());
            }
            continue;
        }
        // Only the two tap-reading formulas divide by the tap, so only they
        // can be bounded or rejected by one (#324).
        let tap = if formula.reads_tap() {
            br.calc_divisible_tap(idx)?
        } else {
            1.0
        };
        // The incidence parts carry the internal positive factor weight (the
        // Laplacian edge weight a sparse solver factors); public PowerModels
        // sign results are the DC operator surface's to emit.
        let b_e = formula.calc_solver_edge_weight(br.r, br.x, tap);
        // A NaN reactance slips past the guard above and poisons the whole
        // Laplacian.
        if !b_e.is_finite() {
            return Err(powerio_tx::Error::NonFiniteSusceptance { row: idx }.into());
        }
        cols.push(Column {
            i,
            j,
            b_e,
            branch_index: idx,
        });
    }

    // Pass 2: assemble.
    let a = assemble_bus_branch_incidence(n, cols.iter().map(|col| (col.i, col.j)));
    let b = cols.iter().map(|col| col.b_e).collect();
    let branch_indices = cols.iter().map(|col| col.branch_index).collect();

    Ok(IncidenceParts {
        a,
        b,
        branch_indices,
    })
}

/// The signed bus by branch incidence matrix over `n` buses, `n × m` CSR for
/// the `m` columns `endpoints` yields: column `e` has `+1` at its from bus
/// and `-1` at its to bus. A self-loop column (`from == to`) sums to an empty
/// column. Within each bus row the columns ascend.
///
/// Every incidence matrix in the crate (the sensitivity factor, the DC
/// operators, a DC preparation's) is this one counting sort straight into
/// CSR: one pass counts each bus row, a second places the columns.
///
/// # Panics
/// Panics if an endpoint is `>= n`.
pub(crate) fn assemble_bus_branch_incidence(
    n: usize,
    endpoints: impl ExactSizeIterator<Item = (usize, usize)> + Clone,
) -> CsMat<f64> {
    assemble_by_columns(
        n,
        endpoints.map(|(from, to)| {
            if from == to {
                [None, None]
            } else {
                [Some((from, 1.0)), Some((to, -1.0))]
            }
        }),
    )
}

/// The bus by element indicator over `n` buses, `n × k` CSR with a `1` at
/// row `bus_of[e]` of column `e`: the generator to bus map `Cg` of a
/// preparation.
///
/// # Panics
/// Panics if a bus is `>= n`.
pub(crate) fn assemble_bus_indicator(n: usize, bus_of: &[usize]) -> CsMat<f64> {
    assemble_by_columns(n, bus_of.iter().map(|&bus| [Some((bus, 1.0))]))
}

/// CSR over `n` rows from columns given in order, each with at most `K`
/// entries in distinct rows, by a counting sort: the row pointer from one
/// count pass, then the columns placed in ascending order.
fn assemble_by_columns<const K: usize>(
    n: usize,
    columns: impl ExactSizeIterator<Item = [Option<(usize, f64)>; K]> + Clone,
) -> CsMat<f64> {
    let m = columns.len();
    let mut indptr = vec![0_usize; n + 1];
    for column in columns.clone() {
        for (row, _) in column.into_iter().flatten() {
            assert!(row < n, "incidence row {row} out of range for {n} rows");
            indptr[row + 1] += 1;
        }
    }
    for row in 0..n {
        indptr[row + 1] += indptr[row];
    }
    let nnz = indptr[n];
    let mut next = indptr[..n].to_vec();
    let mut indices = vec![0_usize; nnz];
    let mut data = vec![0.0; nnz];
    for (index, column) in columns.enumerate() {
        for (row, value) in column.into_iter().flatten() {
            indices[next[row]] = index;
            data[next[row]] = value;
            next[row] += 1;
        }
    }
    CsMat::new((n, m), indptr, indices, data)
}

struct Column {
    i: usize,
    j: usize,
    b_e: f64,
    branch_index: usize,
}

/// The constant term of a DC branch flow, `-w * shift`, for positive factor
/// weight `w` and phase shift `shift` (radians): the flow at equal angles.
/// The one spelling of the offset, shared by every DC flow, offset and phase
/// shift injection in the crate.
#[inline]
pub(crate) fn calc_dc_flow_offset(weight: f64, shift: f64) -> f64 {
    -weight * shift
}

/// DC branch flows `w (theta_from - theta_to) - w shift` at bus angles
/// `theta` (radians), one per `((from, to), w, shift)` column, with `w` the
/// positive factor weight. The one flow from angles in the crate:
/// `DcOpfPreparation::calc_branch_flow` and
/// `DcOperators::calc_branch_flow_dc` both read it.
pub(crate) fn calc_dc_branch_flow(
    theta: &[f64],
    columns: impl Iterator<Item = ((usize, usize), f64, f64)>,
) -> Vec<f64> {
    columns
        .map(|((from, to), weight, shift)| {
            weight * (theta[from] - theta[to]) + calc_dc_flow_offset(weight, shift)
        })
        .collect()
}

/// Sparse diagonal matrix from `values` (square, `len × len`).
pub fn calc_diagonal(values: &[f64]) -> CsMat<f64> {
    let n = values.len();
    let mut d = CooBuilder::with_capacity(n, n);
    for (k, &v) in values.iter().enumerate() {
        d.add(k, k, v);
    }
    d.finish_csr()
}

/// `B = diag(b)`, shape `m × m`.
pub fn calc_susceptance_diagonal(b: &[f64]) -> CsMat<f64> {
    calc_diagonal(b)
}

/// Calculate the branch flow matrix used by solver preparations,
/// `Diagonal(w) * Aᵀ`, shape `m × n`, where `w` contains positive factor
/// weights.
///
/// A complete affine branch flow also adds `-b * shift`; problem
/// preparations expose that term through
/// `DcOpfPreparation::calc_branch_flow_offset`. In the public PowerModels sign
/// spelling the same flow is `p_branch = -Bf va + b .* shift` with negated
/// susceptances. [`DcOpfMatrices::branch_flow_matrix`](crate::DcOpfMatrices)
/// is this matrix over a preparation's incidence and weights.
///
/// # Panics
/// Panics if `susceptance_magnitude.len()` is not the incidence column count.
#[must_use]
pub fn calc_solver_branch_flow_matrix(
    bus_branch_incidence: &CsMat<f64>,
    susceptance_magnitude: &[f64],
) -> CsMat<f64> {
    let diagonal = calc_susceptance_diagonal(susceptance_magnitude);
    let transpose = bus_branch_incidence.transpose_view().to_csr();
    &diagonal * &transpose
}

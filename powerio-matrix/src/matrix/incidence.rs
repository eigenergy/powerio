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
    /// Dense from bus of each column, the `+1` row of `a`.
    pub(crate) from: Vec<usize>,
    /// Dense to bus of each column, the `-1` row of `a`.
    pub(crate) to: Vec<usize>,
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
        if i == j {
            continue;
        }
        // The one DC column rule: a degenerate impedance is `None`, a tap
        // or weight the formula cannot use is an error. The shift is not
        // part of the incidence factor.
        let Some(column) = formula.calc_dc_column(br, idx, case.network().is_normalized())? else {
            if !opts.skip_zero_impedance {
                return Err(powerio_tx::Error::ZeroImpedance { row: idx }.into());
            }
            continue;
        };
        let b_e = column.weight;
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
    let from = cols.iter().map(|col| col.i).collect();
    let to = cols.iter().map(|col| col.j).collect();

    Ok(IncidenceParts {
        a,
        b,
        branch_indices,
        from,
        to,
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
    let mut flows = Vec::new();
    calc_dc_branch_flow_into(theta, columns, &mut flows);
    flows
}

/// [`calc_dc_branch_flow`] into a reused buffer, which it clears first: the
/// form a builder evaluating many angle vectors (the PTDF and LODF columns)
/// calls so each column allocates nothing.
pub(crate) fn calc_dc_branch_flow_into(
    theta: &[f64],
    columns: impl Iterator<Item = ((usize, usize), f64, f64)>,
    flows: &mut Vec<f64>,
) {
    flows.clear();
    flows.extend(columns.map(|((from, to), weight, shift)| {
        weight * (theta[from] - theta[to]) + calc_dc_flow_offset(weight, shift)
    }));
}

/// The incidence product `A x` over `n` buses, one per `((from, to), x)`
/// column: each column's value leaves at its from bus (`+x`) and arrives at
/// its to bus (`-x`). A self-loop column is empty, so its value cancels at its
/// own bus. The one scatter of a branch vector onto buses in the crate:
/// [`calc_dc_phase_shift_withdrawal`] and `DcOperators::calc_bus_injection_dc`
/// both read it.
pub(crate) fn calc_bus_scatter(
    n: usize,
    columns: impl Iterator<Item = ((usize, usize), f64)>,
) -> Vec<f64> {
    let mut buses = vec![0.0; n];
    for ((from, to), value) in columns {
        if from == to {
            continue;
        }
        buses[from] += value;
        buses[to] -= value;
    }
    buses
}

/// The nodal phase shift withdrawal `A (-w .* shift)` over `n` buses, one
/// per `((from, to), w, shift)` column with `w` the positive factor weight:
/// each column's flow offset ([`calc_dc_flow_offset`]) scattered by
/// [`calc_bus_scatter`]. The one phase shift withdrawal in the crate: the DC
/// preparation's `p_shift`, `DcOpfPreparation::calc_phase_shift_withdrawal`
/// and `DcOperators::calc_bus_phase_shift_injection` all read it.
pub(crate) fn calc_dc_phase_shift_withdrawal(
    n: usize,
    columns: impl Iterator<Item = ((usize, usize), f64, f64)>,
) -> Vec<f64> {
    calc_bus_scatter(
        n,
        columns.map(|(endpoints, weight, shift)| (endpoints, calc_dc_flow_offset(weight, shift))),
    )
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

/// Calculate the branch flow matrix `Diagonal(w) * Aᵀ`, shape `m × n`. Solver
/// preparations pass positive factor weights; `DcOperators` passes the public
/// PowerModels susceptances for its public `Bf`. The one branch flow matrix
/// assembly in the crate.
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

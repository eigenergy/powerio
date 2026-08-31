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
            BranchSusceptanceFormula::SeriesSusceptance => {
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
        // Only `TapAdjustedReactance` divides by the tap, so only it can be
        // bounded or rejected by one (#324); the other formulas never read
        // the value.
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
        cols.push(Column { i, j, b_e });
    }

    // Pass 2: assemble.
    let m = cols.len();
    let mut a = CooBuilder::with_capacity_rect(n, m, 2 * m);
    let mut b = Vec::with_capacity(m);
    for (k, col) in cols.iter().enumerate() {
        a.add(col.i, k, 1.0);
        a.add(col.j, k, -1.0);
        b.push(col.b_e);
    }

    Ok(IncidenceParts {
        a: a.finish_csr(),
        b,
    })
}

struct Column {
    i: usize,
    j: usize,
    b_e: f64,
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

/// The angle dependent branch flow matrix `B Aᵀ`, shape `m × n`, over the
/// internal positive factor weights.
///
/// A complete affine branch flow also adds `-b * shift`; problem
/// preparations expose that term through
/// `DcOpfPreparation::calc_branch_flow_offset`. In the public PowerModels sign
/// spelling the same flow is `p_branch = -Bf va + b .* shift` with negated
/// susceptances.
pub fn calc_branch_flow_matrix(
    bus_branch_incidence: &CsMat<f64>,
    susceptance_magnitude: &[f64],
) -> CsMat<f64> {
    let diagonal = calc_susceptance_diagonal(susceptance_magnitude);
    let transpose = bus_branch_incidence.transpose_view().to_csr();
    &diagonal * &transpose
}

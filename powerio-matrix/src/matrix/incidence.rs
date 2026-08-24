//! DC network primitives: the signed incidence matrix `A`, branch weights `w`,
//! the flow map `diag(w) Aᵀ`, and the phase shift injection.
//!
//! Edge orientation is fixed to MATPOWER's from→to: column `e` of `A` has
//! `+1` at the from bus (tail) and `−1` at the to bus (head). Columns run
//! over in-service branches in `case.branches` order; `branch_of_col` maps a
//! column back to its source branch index.

use sprs::CsMat;

pub use powerio::DcConvention;

use crate::Result;
use crate::indexed::IndexedNetwork;
use crate::matrix::triplet::CooBuilder;

use super::{BuildOptions, ZeroImpedanceSkips};

/// The incidence factorization of a case under one DC convention.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DcSolverData {
    /// Signed incidence `A`, shape `n × m`.
    pub a: CsMat<f64>,
    /// Internal DC branch weights `w_e`, length `m`.
    pub branch_weight: Vec<f64>,
    /// Phase shift bus injection, length `n`. All zeros unless the selected
    /// convention carries shifts and the case has a phase shifter.
    pub p_shift: Vec<f64>,
    /// Column `k` → index into `case.branches`.
    pub branch_of_col: Vec<usize>,
    /// In-service branch rows skipped because their DC denominator is zero.
    pub skipped_zero_impedance: ZeroImpedanceSkips,
}

impl DcSolverData {
    #[inline]
    pub fn n(&self) -> usize {
        self.a.rows()
    }

    #[inline]
    pub fn m(&self) -> usize {
        self.a.cols()
    }
}

/// Build `A`, the branch weights, the phase shift injection, and the
/// column→branch map.
///
/// Self-loops (from == to) are dropped. A branch whose reactance is too small
/// to divide by has no finite branch weight the solver matrix can carry; it is skipped
/// when `opts.skip_zero_impedance` is true and rejected with
/// [`powerio::Error::ZeroImpedance`] when it is false. A tap ratio under the same bound
/// is [`powerio::Error::DegenerateTap`] either way, as it is in Y_bus.
pub fn build_incidence(
    case: &IndexedNetwork,
    conv: DcConvention,
    opts: &BuildOptions,
) -> Result<DcSolverData> {
    let n = case.n();

    // Pass 1: resolve and filter, fixing the column order.
    let mut cols: Vec<Column> = Vec::new();
    let mut skipped_zero_impedance = Vec::new();
    for (idx, br) in case.in_service_branches() {
        let i = case.bus_index(br.from).ok_or(powerio::Error::UnknownBus {
            bus_id: br.from,
            element_index: idx,
        })?;
        let j = case.bus_index(br.to).ok_or(powerio::Error::UnknownBus {
            bus_id: br.to,
            element_index: idx,
        })?;
        // Zero impedance in every sense the builder can act on: `x = 1e-300`
        // gives a finite `w = 1e300` that annihilates every real branch sharing
        // a diagonal with it. Exact zero used to be the whole test.
        let degenerate_x = br.x.abs() < crate::matrix::MIN_DIVISIBLE_MAGNITUDE;
        if i == j || degenerate_x {
            if i != j && degenerate_x {
                if !opts.skip_zero_impedance {
                    return Err(powerio::Error::ZeroImpedance { row: idx }.into());
                }
                skipped_zero_impedance.push(idx);
            }
            continue;
        }
        // `Matpower` divides the branch weight by the tap, so it is bounded here
        // by the same rule Y_bus and the instance builders apply.
        let weight = conv.branch_weight(br.r, br.x, br.divisible_tap(idx)?);
        // A NaN reactance slips past the guard above and poisons the whole
        // solver matrix.
        if !weight.is_finite() {
            return Err(powerio::Error::NonFiniteSusceptance { row: idx }.into());
        }
        // angle_radians, not to_radians: a normalized network's shift is
        // already in radians, so converting again would double-scale it.
        let shift_rad = if conv.includes_phase_shifts() {
            case.angle_radians(br.shift)
        } else {
            0.0
        };
        cols.push(Column {
            i,
            j,
            weight,
            shift_rad,
            branch: idx,
        });
    }

    // Pass 2: assemble.
    let m = cols.len();
    let mut a = CooBuilder::with_capacity_rect(n, m, 2 * m);
    let mut branch_weight = Vec::with_capacity(m);
    let mut p_shift = vec![0.0; n];
    let mut branch_of_col = Vec::with_capacity(m);
    for (k, col) in cols.iter().enumerate() {
        a.add(col.i, k, 1.0);
        a.add(col.j, k, -1.0);
        branch_weight.push(col.weight);
        branch_of_col.push(col.branch);
        if col.shift_rad != 0.0 {
            // MATPOWER makeBdc: Pbusinj = (Cf − Ct)ᵀ (b ⊙ (−shift)). Column k
            // of (Cf − Ct) is e_from − e_to.
            p_shift[col.i] -= col.weight * col.shift_rad;
            p_shift[col.j] += col.weight * col.shift_rad;
        }
    }

    Ok(DcSolverData {
        a: a.finish_csr(),
        branch_weight,
        p_shift,
        branch_of_col,
        skipped_zero_impedance: ZeroImpedanceSkips::new(skipped_zero_impedance),
    })
}

struct Column {
    i: usize,
    j: usize,
    weight: f64,
    shift_rad: f64,
    branch: usize,
}

/// Sparse diagonal matrix from `values` (square, `len × len`).
pub fn diagonal(values: &[f64]) -> CsMat<f64> {
    let n = values.len();
    let mut d = CooBuilder::with_capacity(n, n);
    for (k, &v) in values.iter().enumerate() {
        d.add(k, k, v);
    }
    d.finish_csr()
}

/// `diag(w)`, shape `m × m`.
pub fn weight_diagonal(branch_weight: &[f64]) -> CsMat<f64> {
    diagonal(branch_weight)
}

/// The angle dependent flow map `diag(w) Aᵀ`, shape `m × n`.
///
/// A complete affine branch flow also adds `-w * shift`; problem instances
/// expose that term through `DcOpfInstance::branch_flow_offset`.
pub fn build_flow_map(a: &CsMat<f64>, branch_weight: &[f64]) -> CsMat<f64> {
    let d = weight_diagonal(branch_weight);
    let at = a.transpose_view().to_csr();
    &d * &at
}

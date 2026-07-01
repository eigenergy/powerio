//! DC network primitives: the signed incidence matrix `A`, branch
//! susceptances `b`, the flow map `B Aᵀ`, and the phase shift injection.
//!
//! Edge orientation is fixed to MATPOWER's from→to: column `e` of `A` has
//! `+1` at the from bus (tail) and `−1` at the to bus (head). Columns run
//! over in-service branches in `case.branches` order; `branch_of_col` maps a
//! column back to its source branch index.

use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::matrix::triplet::CooBuilder;
use crate::{Error, Result};

use super::{BuildOptions, ZeroImpedanceSkips};

/// DC susceptance convention for `b_e` and the Laplacian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum DcConvention {
    /// `b_e = 1/x_e`; taps and phase shifts ignored: the textbook DC power
    /// flow weight.
    #[default]
    PaperPure,
    /// `b_e = 1/(x_e·τ_e)` with a phase shift injection vector, matching
    /// MATPOWER `makeBdc`.
    Matpower,
}

/// The incidence factorization of a case under one DC convention.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IncidenceParts {
    /// Signed incidence `A`, shape `n × m`.
    pub a: CsMat<f64>,
    /// Branch susceptances `b_e`, length `m`.
    pub b: Vec<f64>,
    /// Phase shift bus injection, length `n`. All zeros unless the MATPOWER
    /// convention is used and shifters are present.
    pub p_shift: Vec<f64>,
    /// Column `k` → index into `case.branches`.
    pub branch_of_col: Vec<usize>,
    /// In-service branch rows skipped because their DC denominator is zero.
    pub skipped_zero_impedance: ZeroImpedanceSkips,
}

impl IncidenceParts {
    #[inline]
    pub fn n(&self) -> usize {
        self.a.rows()
    }

    #[inline]
    pub fn m(&self) -> usize {
        self.a.cols()
    }
}

/// Build `A`, `b`, the phase shift injection, and the column→branch map.
///
/// Self-loops (from == to) are dropped. Branches with `x == 0` have no finite DC
/// susceptance; they are skipped when `opts.skip_zero_impedance` is true and
/// rejected with [`Error::ZeroImpedance`] when it is false.
pub fn build_incidence(
    case: &IndexedNetwork,
    conv: DcConvention,
    opts: &BuildOptions,
) -> Result<IncidenceParts> {
    let n = case.n();

    // Pass 1: resolve and filter, fixing the column order.
    let mut cols: Vec<Column> = Vec::new();
    let mut skipped_zero_impedance = Vec::new();
    for (idx, br) in case.in_service_branches() {
        let i = case.bus_index(br.from).ok_or(Error::UnknownBus {
            bus_id: br.from,
            element_index: idx,
        })?;
        let j = case.bus_index(br.to).ok_or(Error::UnknownBus {
            bus_id: br.to,
            element_index: idx,
        })?;
        if i == j || br.x == 0.0 {
            if i != j && br.x == 0.0 {
                if !opts.skip_zero_impedance {
                    return Err(Error::ZeroImpedance { row: idx });
                }
                skipped_zero_impedance.push(idx);
            }
            continue;
        }
        let b_e = match conv {
            DcConvention::PaperPure => 1.0 / br.x,
            DcConvention::Matpower => 1.0 / (br.x * br.effective_tap()),
        };
        // A NaN reactance slips past the `x == 0.0` guard above, and a
        // denormal `x` yields Inf; either poisons the whole Laplacian.
        if !b_e.is_finite() {
            return Err(Error::NonFiniteSusceptance { row: idx });
        }
        let shift_rad = match conv {
            DcConvention::PaperPure => 0.0,
            // angle_radians, not to_radians: a normalized network's shift is
            // already in radians, so converting again would double-scale it.
            DcConvention::Matpower => case.angle_radians(br.shift),
        };
        cols.push(Column {
            i,
            j,
            b_e,
            shift_rad,
            branch: idx,
        });
    }

    // Pass 2: assemble.
    let m = cols.len();
    let mut a = CooBuilder::with_capacity_rect(n, m, 2 * m);
    let mut b = Vec::with_capacity(m);
    let mut p_shift = vec![0.0; n];
    let mut branch_of_col = Vec::with_capacity(m);
    for (k, col) in cols.iter().enumerate() {
        a.add(col.i, k, 1.0);
        a.add(col.j, k, -1.0);
        b.push(col.b_e);
        branch_of_col.push(col.branch);
        if col.shift_rad != 0.0 {
            // MATPOWER makeBdc: Pbusinj = (Cf − Ct)ᵀ (b ⊙ (−shift)). Column k
            // of (Cf − Ct) is e_from − e_to.
            p_shift[col.i] -= col.b_e * col.shift_rad;
            p_shift[col.j] += col.b_e * col.shift_rad;
        }
    }

    Ok(IncidenceParts {
        a: a.finish_csr(),
        b,
        p_shift,
        branch_of_col,
        skipped_zero_impedance: ZeroImpedanceSkips::new(skipped_zero_impedance),
    })
}

struct Column {
    i: usize,
    j: usize,
    b_e: f64,
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

/// `B = diag(b)`, shape `m × m`.
pub fn susceptance_diag(b: &[f64]) -> CsMat<f64> {
    diagonal(b)
}

/// The flow map `B Aᵀ`, shape `m × n`: `f = (B Aᵀ) θ`.
pub fn build_flow_map(a: &CsMat<f64>, b: &[f64]) -> CsMat<f64> {
    let d = susceptance_diag(b);
    let at = a.transpose_view().to_csr();
    &d * &at
}

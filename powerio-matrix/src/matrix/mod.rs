//! Sparse matrix builders for MATPOWER cases.
//!
//! Sign convention: the susceptance matrix has the form `B = A diag(b) Aᵀ`
//! with the node-by-edge incidence `A` (n×m) and per-edge weight `b_e = x/(r²+x²)`
//! (see `bprime.rs` for the entry-level form). Resulting matrices satisfy positive
//! diagonal, negative off-diagonal, `diag = sum of |off-diagonal|` — the
//! positive (M-matrix) Laplacian form SDDM solvers expect.

mod adjacency;
mod bdoubleprime;
mod bprime;
pub mod incidence;
// The DC-OPF interior-point operators are experimental and off by default,
// built only under `--features kkt`, which the default build and the main CI
// jobs skip.
#[cfg(feature = "kkt")]
pub mod kkt;
mod lacpf;
pub mod laplacian;
mod lindist3flow;
pub mod opf;
pub mod sensitivity;
pub mod triplet;
mod ybus;

#[cfg(test)]
mod tests;

pub use adjacency::build_adjacency;
pub use bdoubleprime::build_bdoubleprime;
pub use bprime::build_bprime;
pub use incidence::{
    DcConvention, IncidenceParts, build_flow_map, build_incidence, susceptance_diag,
};
pub use lacpf::build_lacpf;
pub use laplacian::{
    GroundedIndexMap, build_weighted_laplacian, ground_at, ground_at_each, reference_indicator,
    unit_vector,
};
pub use lindist3flow::{
    LinDist3FlowMatrices, build_lindist3flow, build_lindist3flow_spanning_tree,
};
pub use opf::{BusCosts, GenCosts, OpfInstance, Units, build_opf_instance};
pub use sensitivity::{build_lodf, build_ptdf, build_ptdf_lodf};
pub use ybus::{YbusParts, build_ybus};
// Crate-internal: the gridfm columnar export reuses the per-branch admittance and
// flow kernels so its branch table and Y_bus agree with `build_ybus` by construction.
#[cfg(feature = "gridfm")]
pub(crate) use ybus::{YbusFlags, branch_admittance, branch_flows};

use sprs::CsMat;

/// Which FDPF scheme to use for B'.
///
/// - `Bx`: B' uses `-x / (r² + x²)` (what most modern solvers do).
/// - `Xb`: B' uses `-1 / x` (original Stott/Alsac 1974). Requires `x ≠ 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum Scheme {
    #[default]
    Bx,
    Xb,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuildOptions {
    pub scheme: Scheme,
    /// Apply tap ratios when building B″ and Y-bus. (B′ always ignores taps.)
    pub include_taps: bool,
    /// Apply phase shifts when building Y-bus. (B′/B″ always ignore shifts.)
    pub include_shifts: bool,
    /// Drop branches whose `r² + x² = 0` (true) or error out (false).
    pub skip_zero_impedance: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            scheme: Scheme::Bx,
            include_taps: true,
            include_shifts: true,
            skip_zero_impedance: true,
        }
    }
}

/// Common stats over a sparse matrix used by the TUI and `meta.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct MatrixStats {
    pub n: usize,
    pub nnz: usize,
    pub min_diag: f64,
    pub max_diag: f64,
    /// Smallest `D_ii - sum_j |O_ij|` across all rows. Negative means
    /// the matrix is not diagonally dominant.
    pub min_dd_margin: f64,
    /// Whether all off-diagonals are ≤ 0 (M-matrix sign pattern).
    pub m_matrix_sign: bool,
    pub frobenius_norm: f64,
}

impl MatrixStats {
    pub fn from_csr(a: &CsMat<f64>) -> Self {
        let n = a.rows();
        let mut min_diag = f64::INFINITY;
        let mut max_diag = f64::NEG_INFINITY;
        let mut min_dd = f64::INFINITY;
        let mut m_sign = true;
        let mut fro_sq = 0.0_f64;

        for (row_idx, row) in a.outer_iterator().enumerate() {
            let mut diag = 0.0_f64;
            let mut off_abs = 0.0_f64;
            for (col, &v) in row.iter() {
                fro_sq += v * v;
                if col == row_idx {
                    diag = v;
                } else {
                    off_abs += v.abs();
                    if v > 0.0 {
                        m_sign = false;
                    }
                }
            }
            min_diag = min_diag.min(diag);
            max_diag = max_diag.max(diag);
            min_dd = min_dd.min(diag - off_abs);
        }

        Self {
            n,
            nnz: a.nnz(),
            min_diag,
            max_diag,
            min_dd_margin: min_dd,
            m_matrix_sign: m_sign,
            frobenius_norm: fro_sq.sqrt(),
        }
    }
}

/// Negate every stored value of a sparse matrix in place. Used where the input
/// is owned and consumed straight away (B″ and the `YbusB` pipeline arm), so no
/// clone of the structure is needed.
pub(crate) fn negate_into(mut a: CsMat<f64>) -> CsMat<f64> {
    a.data_mut().iter_mut().for_each(|v| *v = -*v);
    a
}

/// Whether a matrix is SDDM (symmetric diagonally dominant M-matrix).
/// Useful as a quick sanity check before feeding it to an SDDM solver.
pub fn sddm_check(a: &CsMat<f64>) -> bool {
    let stats = MatrixStats::from_csr(a);
    stats.m_matrix_sign && stats.min_dd_margin >= -1e-12 && stats.min_diag > 0.0
}

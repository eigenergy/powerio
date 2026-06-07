//! The weighted Laplacian `L = A diag(w) Aᵀ`, slack grounding, and the
//! index bookkeeping for round-tripping a grounded solve back to full size.
//!
//! Built from the same `A`, `w` factors the incidence module produces, so
//! `L` and its slack-grounded form share an exact factorization.

use sprs::CsMat;

use crate::matrix::incidence::diagonal;
use crate::matrix::triplet::CooBuilder;

/// `L = A diag(w) Aᵀ` (n×n). With `w = b` this is the DC Laplacian; with
/// `w = b²·θ_f⁻¹` it is the reweighted Laplacian `L₁` from the KKT system.
pub fn build_weighted_laplacian(a: &CsMat<f64>, w: &[f64]) -> CsMat<f64> {
    let d = diagonal(w);
    let at = a.transpose_view().to_csr();
    a * &(&d * &at)
}

/// Delete row `r` and column `r` from a square matrix, returning the
/// `(n−1)×(n−1)` grounded matrix. Used to remove the slack bus so a singular
/// Laplacian becomes SPD.
///
/// # Panics
///
/// Panics if `r >= matrix.rows()`: with no row/column to remove the result
/// would be silently the wrong shape.
pub fn ground_at(matrix: &CsMat<f64>, r: usize) -> CsMat<f64> {
    let n = matrix.rows();
    debug_assert_eq!(n, matrix.cols(), "ground_at expects a square matrix");
    // Hard assert (not debug-only): with `r >= n` no row/column is removed but
    // the builder is sized `n-1`, silently producing a wrong-shaped matrix in
    // release. `ground_at` is `pub`, so guard the contract unconditionally.
    assert!(
        r < n,
        "ground_at: index {r} out of range for {n}x{n} matrix"
    );
    let mut g = CooBuilder::new(n.saturating_sub(1));
    for (&v, (i, j)) in matrix {
        if i == r || j == r {
            continue;
        }
        let ii = if i > r { i - 1 } else { i };
        let jj = if j > r { j - 1 } else { j };
        g.add(ii, jj, v);
    }
    g.finish_csr()
}

/// Maps indices between the full `[0, n)` space and the grounded `[0, n−1)`
/// space (row/column `r` removed). Used by the DC-OPF interior-point operators
/// (the `kkt` feature) to round-trip a grounded solve back to full size.
#[derive(Debug, Clone, Copy)]
pub struct GroundMap {
    pub n: usize,
    pub r: usize,
}

impl GroundMap {
    #[inline]
    pub fn new(n: usize, r: usize) -> Self {
        Self { n, r }
    }

    /// Full index → grounded index. `None` for the grounded-out bus `r`.
    #[inline]
    pub fn full_to_reduced(&self, i: usize) -> Option<usize> {
        match i {
            _ if i == self.r => None,
            _ if i > self.r => Some(i - 1),
            _ => Some(i),
        }
    }

    /// Grounded index → full index.
    #[inline]
    pub fn reduced_to_full(&self, i: usize) -> usize {
        if i >= self.r { i + 1 } else { i }
    }
}

/// The unit vector `e_r`, length `n`.
pub fn unit_vector(n: usize, r: usize) -> Vec<f64> {
    let mut e = vec![0.0; n];
    if r < n {
        e[r] = 1.0;
    }
    e
}

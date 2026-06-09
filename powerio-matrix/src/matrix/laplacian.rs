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
/// Laplacian becomes SPD. The single-reference case of [`ground_at_each`].
///
/// # Panics
///
/// Panics if `r >= matrix.rows()`: with no row/column to remove the result
/// would be silently the wrong shape.
pub fn ground_at(matrix: &CsMat<f64>, r: usize) -> CsMat<f64> {
    ground_at_each(matrix, &[r])
}

/// Delete every row and column in `refs` from a square matrix, returning the
/// grounded matrix of side `n − k`, where `k` is the count of distinct
/// in-range references. Grounding one bus per connected component turns a
/// singular Laplacian SPD; grounding several buses within one component is the
/// distributed-slack solve (their angles are tied to the same reference and the
/// absorbed power splits by electrical distance).
///
/// # Panics
///
/// Panics if any reference is `>= matrix.rows()`: the builder is sized `n − k`,
/// so an out-of-range index would silently yield the wrong shape.
pub fn ground_at_each(matrix: &CsMat<f64>, refs: &[usize]) -> CsMat<f64> {
    ground_with(matrix, &Grounding::new(refs))
}

/// A sorted, de-duplicated set of grounded indices and the reduced-index map it
/// induces: drop the grounded rows/columns and shift the survivors down to a
/// dense `[0, n − k)` range. The PTDF builder shares it, so the row/column
/// removal lives in one place.
pub(crate) struct Grounding {
    grounds: Vec<usize>,
}

impl Grounding {
    pub(crate) fn new(refs: &[usize]) -> Self {
        let mut grounds = refs.to_vec();
        // Sorted + de-duplicated so the shift is monotone and a repeated
        // reference doesn't over-count the removal.
        grounds.sort_unstable();
        grounds.dedup();
        Self { grounds }
    }

    /// Number of grounded indices `k`.
    pub(crate) fn len(&self) -> usize {
        self.grounds.len()
    }

    /// The largest grounded index, or `None` if nothing is grounded.
    pub(crate) fn max(&self) -> Option<usize> {
        self.grounds.last().copied()
    }

    /// Full index → reduced index, or `None` for a grounded index. The shift is
    /// `i − (number of grounds strictly below i)`.
    pub(crate) fn reduced(&self, i: usize) -> Option<usize> {
        if self.grounds.binary_search(&i).is_ok() {
            None
        } else {
            Some(i - self.grounds.partition_point(|&g| g < i))
        }
    }
}

/// Drop the rows and columns a [`Grounding`] marks, shifting survivors down.
///
/// # Panics
///
/// Panics if a grounded index is `>= matrix.rows()`: the builder is sized
/// `n − k`, so an out-of-range index would silently yield the wrong shape.
pub(crate) fn ground_with(matrix: &CsMat<f64>, g: &Grounding) -> CsMat<f64> {
    let n = matrix.rows();
    debug_assert_eq!(n, matrix.cols(), "ground_with expects a square matrix");
    // Hard assert (not debug-only): an out-of-range index removes no row/column
    // yet shrinks the builder. These are `pub` entry points, so guard the
    // contract unconditionally.
    if let Some(last) = g.max() {
        assert!(
            last < n,
            "ground_with: index {last} out of range for {n}x{n} matrix"
        );
    }
    let mut out = CooBuilder::new(n.saturating_sub(g.len()));
    for (&v, (i, j)) in matrix {
        if let (Some(ri), Some(rj)) = (g.reduced(i), g.reduced(j)) {
            out.add(ri, rj, v);
        }
    }
    out.finish_csr()
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

/// The reference indicator, length `n`: `1` at every grounded (slack) bus, `0`
/// elsewhere. The multi-reference form of [`unit_vector`]; a downstream solver
/// reads it to recover which buses were grounded.
pub fn reference_indicator(n: usize, refs: &[usize]) -> Vec<f64> {
    let mut e = vec![0.0; n];
    for &r in refs {
        if r < n {
            e[r] = 1.0;
        }
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grounding_reduced_shifts_survivors() {
        // Ground indices 1 and 3 of a 5-wide space: survivors 0,2,4 -> 0,1,2.
        let g = Grounding::new(&[1, 3]);
        assert_eq!(g.len(), 2);
        assert_eq!(g.reduced(0), Some(0));
        assert_eq!(g.reduced(1), None);
        assert_eq!(g.reduced(2), Some(1));
        assert_eq!(g.reduced(3), None);
        assert_eq!(g.reduced(4), Some(2));
    }

    #[test]
    fn grounding_sorts_and_dedups() {
        // Unsorted, repeated input collapses so the shift stays monotone and a
        // repeated reference doesn't over-remove a row/column.
        let g = Grounding::new(&[3, 1, 3]);
        assert_eq!(g.len(), 2);
        assert_eq!(g.max(), Some(3));
        assert_eq!(g.reduced(2), Some(1));
        assert_eq!(g.reduced(4), Some(2));
    }

    fn diag_matrix(vals: &[f64]) -> CsMat<f64> {
        let mut b = CooBuilder::new(vals.len());
        for (i, &v) in vals.iter().enumerate() {
            b.add(i, i, v);
        }
        b.finish_csr()
    }

    #[test]
    fn ground_at_each_removes_rows_and_cols() {
        let m = diag_matrix(&[10.0, 20.0, 30.0, 40.0]);
        // Ground index 1: a 3x3 with diag 10,30,40 (survivors shifted down).
        let g1 = ground_at_each(&m, &[1]);
        assert_eq!((g1.rows(), g1.cols()), (3, 3));
        assert_eq!(g1.get(0, 0), Some(&10.0));
        assert_eq!(g1.get(1, 1), Some(&30.0));
        assert_eq!(g1.get(2, 2), Some(&40.0));
        // Ground 0 and 2 from an unsorted set: a 2x2 with diag 20,40.
        let g2 = ground_at_each(&m, &[2, 0]);
        assert_eq!((g2.rows(), g2.cols()), (2, 2));
        assert_eq!(g2.get(0, 0), Some(&20.0));
        assert_eq!(g2.get(1, 1), Some(&40.0));
    }

    #[test]
    fn reference_indicator_marks_each_ref() {
        assert_eq!(reference_indicator(4, &[0, 2]), vec![1.0, 0.0, 1.0, 0.0]);
        // Out-of-range refs are ignored, not a panic.
        assert_eq!(reference_indicator(3, &[5]), vec![0.0, 0.0, 0.0]);
        // The single-reference case is exactly unit_vector.
        assert_eq!(reference_indicator(3, &[1]), unit_vector(3, 1));
    }
}

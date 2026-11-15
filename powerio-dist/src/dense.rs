//! Small dense matrix algebra shared by every conductor sized calculation:
//! the series impedance inverses of lines and transformers, the potential
//! coefficient inverses of overhead geometry, and Kron reduction.

use std::ops::{Add, Div, Mul, Neg, Sub};

use num_complex::Complex64;

/// A scalar [`invert_dense_matrix`] works over: real (`f64`) or complex
/// (`Complex64`).
pub trait DenseScalar:
    Copy
    + PartialEq
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
{
    /// The additive identity.
    const ZERO: Self;
    /// The multiplicative identity.
    const ONE: Self;
    /// The magnitude a pivot is compared by.
    fn magnitude(self) -> f64;
    /// Whether every component is finite.
    fn is_finite(self) -> bool;
    /// The magnitude as `√(re² + im²)` without rescaling, so an extreme
    /// component's square overflows or underflows as Free Pascal's `Cabs`
    /// does ([`DensePivot::OpenDssDiagonal`]). The absolute value for a real.
    fn unscaled_magnitude(self) -> f64;
    /// The reciprocal, formed so that neither an extreme magnitude's square
    /// nor its reciprocal overflows or underflows (`Complex::finv`).
    #[must_use]
    fn reciprocal(self) -> Self;
    /// The quotient by Smith's ratio division, the order Free Pascal's
    /// `ucomplex` divides in ([`DensePivot::OpenDssDiagonal`]). Plain
    /// division for a real.
    #[must_use]
    fn ratio_quotient(self, divisor: Self) -> Self;
}

impl DenseScalar for f64 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    fn magnitude(self) -> f64 {
        self.abs()
    }
    fn is_finite(self) -> bool {
        f64::is_finite(self)
    }
    fn unscaled_magnitude(self) -> f64 {
        self.abs()
    }
    fn reciprocal(self) -> Self {
        1.0 / self
    }
    fn ratio_quotient(self, divisor: Self) -> Self {
        self / divisor
    }
}

impl DenseScalar for Complex64 {
    const ZERO: Self = Complex64::new(0.0, 0.0);
    const ONE: Self = Complex64::new(1.0, 0.0);
    fn magnitude(self) -> f64 {
        self.norm()
    }
    fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
    fn unscaled_magnitude(self) -> f64 {
        self.norm_sqr().sqrt()
    }
    fn reciprocal(self) -> Self {
        self.finv()
    }
    fn ratio_quotient(self, divisor: Self) -> Self {
        if divisor.re.abs() > divisor.im.abs() {
            let ratio = divisor.im / divisor.re;
            let scale = divisor.re + divisor.im * ratio;
            Complex64::new(
                (self.re + self.im * ratio) / scale,
                (self.im - self.re * ratio) / scale,
            )
        } else {
            let ratio = divisor.re / divisor.im;
            let scale = divisor.im + divisor.re * ratio;
            Complex64::new(
                (self.im + self.re * ratio) / scale,
                (-self.re + self.im * ratio) / scale,
            )
        }
    }
}

/// The pivot order of [`invert_dense_matrix`]'s elimination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DensePivot {
    /// Gauss-Jordan elimination with partial pivoting: each column's pivot
    /// is the first entry of largest [`DenseScalar::magnitude`] at or below
    /// the diagonal, the pivot row is scaled by the pivot and subtracted
    /// from every other row.
    Partial,
    /// OpenDSS's `TcMatrix.Invert` (DSS C-API 0.14.5 `Ucmatrix.pas`): an in
    /// place sweep that pivots on the diagonal only, each step on the first
    /// unused diagonal entry of strictly largest
    /// [`DenseScalar::unscaled_magnitude`], eliminating
    /// `a_ij − a_ik a_kj / a_kk` with [`DenseScalar::ratio_quotient`]. With
    /// both floors zero it refuses where the engine reports an inversion
    /// error: no unused diagonal entry is nonzero, so a zero self impedance
    /// with nonzero mutuals is refused although row pivoting inverts it,
    /// while a pivot left at rounding noise is accepted. It refuses as well
    /// a pivot that is not finite and an unused entry an elimination makes
    /// nonfinite.
    OpenDssDiagonal,
}

/// Why [`eliminate_dense_matrix`] refused a matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DenseRefusal {
    /// The matrix is not square, or a pivot is at or below its floor
    /// (including an exact zero and, under
    /// [`DensePivot::OpenDssDiagonal`], no nonzero unused diagonal entry).
    Singular,
    /// A pivot, or an entry a later pivot reads, is not finite.
    NonFinite,
}

/// The inverse of a small dense square matrix by Gauss-Jordan elimination
/// in the `pivot` order ([`DensePivot`]). `None` when a pivot's magnitude is
/// NaN or infinite, `singular_pivot` or smaller, or `relative_pivot` times
/// the largest original magnitude in its column or smaller. Each caller
/// states the pivot its quantity is singular at: an absolute
/// `singular_pivot` (ohms for a series impedance, a dimensionless
/// coefficient for a potential matrix) and a `relative_pivot` that measures
/// cancellation within a column against that column's own scale (the dense
/// PTDF fallback passes `n ε`, so a column eliminated to rounding noise is
/// singular while a wide but valid spread of magnitudes stays invertible).
/// Zero for both refuses only exact singularity. A one by one matrix
/// inverts as its scaled reciprocal ([`DenseScalar::reciprocal`]). An empty
/// matrix inverts to an empty matrix.
///
/// The one dense inverse: line and switch series admittances, the open line
/// Schur complement, overhead line potential coefficients (OpenDSS and
/// GridLAB-D geometry), the PMD transformer leakage round trip, Kron
/// reduction ([`crate::kron::calc_kron_reduced_impedance`]), the
/// transformer winding admittance and the dense PTDF fallback of
/// `powerio-matrix` read it with [`DensePivot::Partial`]; the OpenDSS
/// source and line primitive check reads it with
/// [`DensePivot::OpenDssDiagonal`].
#[must_use]
pub fn invert_dense_matrix<T: DenseScalar>(
    matrix: &[Vec<T>],
    pivot: DensePivot,
    singular_pivot: f64,
    relative_pivot: f64,
) -> Option<Vec<Vec<T>>> {
    eliminate_dense_matrix(matrix, pivot, singular_pivot, relative_pivot).ok()
}

/// [`invert_dense_matrix`] stating why it refused.
pub(crate) fn eliminate_dense_matrix<T: DenseScalar>(
    matrix: &[Vec<T>],
    pivot: DensePivot,
    singular_pivot: f64,
    relative_pivot: f64,
) -> Result<Vec<Vec<T>>, DenseRefusal> {
    let n = matrix.len();
    if matrix.iter().any(|row| row.len() != n) {
        return Err(DenseRefusal::Singular);
    }
    let floors: Vec<f64> = (0..n)
        .map(|column| {
            let scale = matrix
                .iter()
                .fold(0.0_f64, |largest, row| largest.max(row[column].magnitude()));
            singular_pivot.max(relative_pivot * scale)
        })
        .collect();
    let check = |magnitude: f64, column: usize| {
        if !magnitude.is_finite() {
            Err(DenseRefusal::NonFinite)
        } else if magnitude <= floors[column] {
            Err(DenseRefusal::Singular)
        } else {
            Ok(())
        }
    };
    if let [row] = matrix
        && let [value] = row.as_slice()
    {
        let magnitude = match pivot {
            DensePivot::Partial => value.magnitude(),
            DensePivot::OpenDssDiagonal => value.unscaled_magnitude(),
        };
        if magnitude.is_nan() {
            return Err(DenseRefusal::Singular);
        }
        check(magnitude, 0)?;
        return Ok(vec![vec![value.reciprocal()]]);
    }
    match pivot {
        DensePivot::Partial => eliminate_partial(matrix, check),
        DensePivot::OpenDssDiagonal => eliminate_diagonal(matrix, check),
    }
}

fn eliminate_partial<T: DenseScalar>(
    matrix: &[Vec<T>],
    check: impl Fn(f64, usize) -> Result<(), DenseRefusal>,
) -> Result<Vec<Vec<T>>, DenseRefusal> {
    let n = matrix.len();
    let mut work: Vec<Vec<T>> = matrix.to_vec();
    let mut inverse: Vec<Vec<T>> = (0..n)
        .map(|row| {
            (0..n)
                .map(|column| if row == column { T::ONE } else { T::ZERO })
                .collect()
        })
        .collect();
    for pivot in 0..n {
        let mut best = pivot;
        for row in pivot + 1..n {
            if work[row][pivot].magnitude() > work[best][pivot].magnitude() {
                best = row;
            }
        }
        check(work[best][pivot].magnitude(), pivot)?;
        work.swap(pivot, best);
        inverse.swap(pivot, best);
        let lead = work[pivot][pivot];
        for column in 0..n {
            work[pivot][column] = work[pivot][column] / lead;
            inverse[pivot][column] = inverse[pivot][column] / lead;
        }
        for row in 0..n {
            if row == pivot {
                continue;
            }
            let factor = work[row][pivot];
            if factor == T::ZERO {
                continue;
            }
            for column in 0..n {
                let (lead_work, lead_inverse) = (work[pivot][column], inverse[pivot][column]);
                work[row][column] = work[row][column] - factor * lead_work;
                inverse[row][column] = inverse[row][column] - factor * lead_inverse;
            }
        }
    }
    Ok(inverse)
}

/// `TcMatrix.Invert`'s sweep ([`DensePivot::OpenDssDiagonal`]).
fn eliminate_diagonal<T: DenseScalar>(
    matrix: &[Vec<T>],
    check: impl Fn(f64, usize) -> Result<(), DenseRefusal>,
) -> Result<Vec<Vec<T>>, DenseRefusal> {
    let n = matrix.len();
    let mut a: Vec<Vec<T>> = matrix.to_vec();
    let mut used = vec![false; n];
    for _ in 0..n {
        // The first unused diagonal entry of strictly largest magnitude; a
        // NaN entry is never selected, and none selected is the engine's
        // zero pivot.
        let mut selected = None;
        let mut largest = 0.0;
        for (index, done) in used.iter().enumerate() {
            let magnitude = a[index][index].unscaled_magnitude();
            if !done && magnitude > largest {
                largest = magnitude;
                selected = Some(index);
            }
        }
        let k = selected.ok_or(DenseRefusal::Singular)?;
        check(largest, k)?;
        used[k] = true;
        for i in (0..n).filter(|&i| i != k) {
            for j in (0..n).filter(|&j| j != k) {
                let correction = (a[i][k] * a[k][j]).ratio_quotient(a[k][k]);
                a[i][j] = a[i][j] - correction;
                // Only the unused block decides later pivots.
                if !used[i] && !used[j] && !a[i][j].is_finite() {
                    return Err(DenseRefusal::NonFinite);
                }
            }
        }
        let lead = -a[k][k].reciprocal();
        a[k][k] = lead;
        for i in (0..n).filter(|&i| i != k) {
            a[i][k] = a[i][k] * lead;
            a[k][i] = a[k][i] * lead;
        }
    }
    for row in &mut a {
        for value in row.iter_mut() {
            *value = -*value;
        }
    }
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_real_and_complex_matrices_and_refuses_singular_pivots() {
        let real = invert_dense_matrix(
            &[vec![0.0, 2.0], vec![4.0, 0.0]],
            DensePivot::Partial,
            0.0,
            0.0,
        )
        .unwrap();
        assert_eq!(real, vec![vec![0.0, 0.25], vec![0.5, 0.0]]);
        let z = Complex64::new(1.0, 2.0);
        let complex = invert_dense_matrix(&[vec![z]], DensePivot::Partial, 0.0, 0.0).unwrap();
        assert_eq!(complex[0][0], z.finv());
        assert!(
            invert_dense_matrix(
                &[vec![1.0, 2.0], vec![2.0, 4.0]],
                DensePivot::Partial,
                0.0,
                0.0
            )
            .is_none()
        );
        assert!(invert_dense_matrix(&[vec![1e-15]], DensePivot::Partial, 1e-14, 0.0).is_none());
        assert!(invert_dense_matrix(&[vec![1.0, 2.0]], DensePivot::Partial, 0.0, 0.0).is_none());
        assert_eq!(
            invert_dense_matrix::<f64>(&[], DensePivot::Partial, 0.0, 0.0),
            Some(Vec::new())
        );
        // A non finite pivot is refused at every size, the scalar included.
        assert!(
            invert_dense_matrix(&[vec![f64::INFINITY]], DensePivot::Partial, 0.0, 0.0).is_none()
        );
        assert!(invert_dense_matrix(&[vec![f64::NAN]], DensePivot::Partial, 0.0, 0.0).is_none());
        // A column cancelled to rounding noise is refused only when the
        // caller states a relative pivot.
        let noisy = [vec![1.0, 1.0], vec![1.0, 1.0 + f64::EPSILON]];
        assert!(invert_dense_matrix(&noisy, DensePivot::Partial, 0.0, 0.0).is_some());
        assert!(
            invert_dense_matrix(&noisy, DensePivot::Partial, 0.0, 2.0 * f64::EPSILON).is_none()
        );
    }

    #[test]
    fn the_opendss_diagonal_sweep_inverts_and_refuses_as_the_engine_does() {
        let c = Complex64::new;
        let matrix = vec![
            vec![c(0.3, 1.1), c(0.1, 0.4), c(0.05, 0.3)],
            vec![c(0.1, 0.4), c(0.35, 1.0), c(0.1, 0.35)],
            vec![c(0.05, 0.3), c(0.1, 0.35), c(0.4, 1.2)],
        ];
        let partial = invert_dense_matrix(&matrix, DensePivot::Partial, 0.0, 0.0).unwrap();
        let diagonal = invert_dense_matrix(&matrix, DensePivot::OpenDssDiagonal, 0.0, 0.0).unwrap();
        for (p, d) in partial.iter().flatten().zip(diagonal.iter().flatten()) {
            assert!((p - d).norm() < 1e-12, "{p} {d}");
        }
        // A zero self impedance with nonzero mutuals: row pivoting inverts
        // it, the diagonal sweep meets no nonzero pivot (OpenDSS errors 325
        // and 183).
        let crossed = vec![
            vec![c(0.0, 0.0), c(1.0, 1.0)],
            vec![c(1.0, 1.0), c(0.0, 0.0)],
        ];
        assert!(invert_dense_matrix(&crossed, DensePivot::Partial, 0.0, 0.0).is_some());
        assert_eq!(
            eliminate_dense_matrix(&crossed, DensePivot::OpenDssDiagonal, 0.0, 0.0),
            Err(DenseRefusal::Singular)
        );
        // A pivot left at rounding noise is nonzero, so the engine accepts it.
        let noisy = [vec![1.0, 1.0], vec![1.0, 1.0 + f64::EPSILON]];
        assert!(invert_dense_matrix(&noisy, DensePivot::OpenDssDiagonal, 0.0, 0.0).is_some());
        // A one by one infinity or NaN is refused.
        assert_eq!(
            eliminate_dense_matrix(
                &[vec![f64::INFINITY]],
                DensePivot::OpenDssDiagonal,
                0.0,
                0.0
            ),
            Err(DenseRefusal::NonFinite)
        );
        assert_eq!(
            eliminate_dense_matrix(&[vec![f64::NAN]], DensePivot::OpenDssDiagonal, 0.0, 0.0),
            Err(DenseRefusal::Singular)
        );
        assert_eq!(
            eliminate_dense_matrix(&[vec![2.0]], DensePivot::OpenDssDiagonal, 0.0, 0.0),
            Ok(vec![vec![0.5]])
        );
    }
}

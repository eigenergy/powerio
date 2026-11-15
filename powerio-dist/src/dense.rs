//! Small dense matrix algebra shared by every conductor sized calculation:
//! the series impedance inverses of lines and transformers, the potential
//! coefficient inverses of overhead geometry, and Kron reduction.

use std::ops::{Add, Div, Mul, Sub};

use num_complex::Complex64;

/// A scalar [`invert_dense_matrix`] works over: real (`f64`) or complex
/// (`Complex64`).
pub trait DenseScalar:
    Copy + PartialEq + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self>
{
    /// The additive identity.
    const ZERO: Self;
    /// The multiplicative identity.
    const ONE: Self;
    /// The magnitude a pivot is compared by.
    fn magnitude(self) -> f64;
    /// The reciprocal, formed so that neither an extreme magnitude's square
    /// nor its reciprocal overflows or underflows (`Complex::finv`).
    #[must_use]
    fn reciprocal(self) -> Self;
}

impl DenseScalar for f64 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    fn magnitude(self) -> f64 {
        self.abs()
    }
    fn reciprocal(self) -> Self {
        1.0 / self
    }
}

impl DenseScalar for Complex64 {
    const ZERO: Self = Complex64::new(0.0, 0.0);
    const ONE: Self = Complex64::new(1.0, 0.0);
    fn magnitude(self) -> f64 {
        self.norm()
    }
    fn reciprocal(self) -> Self {
        self.finv()
    }
}

/// The inverse of a small dense square matrix by Gauss-Jordan elimination
/// with partial pivoting: each column's pivot is the first entry of largest
/// magnitude at or below the diagonal. `None` when a pivot's magnitude is
/// `singular_pivot` or smaller, or NaN, so zero refuses only exact
/// singularity and each caller states the pivot its quantity is singular
/// at (ohms for a series impedance, a dimensionless coefficient for a
/// potential matrix). A one by one matrix inverts as its scaled reciprocal
/// ([`DenseScalar::reciprocal`]). An empty matrix inverts to an empty matrix.
///
/// The one dense inverse: line and switch series admittances, the open line
/// Schur complement, overhead line potential coefficients (OpenDSS and
/// GridLAB-D geometry), the PMD transformer leakage round trip, Kron
/// reduction ([`crate::kron::calc_kron_reduced_impedance`]) and the
/// transformer winding admittance read it.
#[must_use]
pub fn invert_dense_matrix<T: DenseScalar>(
    matrix: &[Vec<T>],
    singular_pivot: f64,
) -> Option<Vec<Vec<T>>> {
    let n = matrix.len();
    if matrix.iter().any(|row| row.len() != n) {
        return None;
    }
    if let [row] = matrix
        && let [value] = row.as_slice()
    {
        let magnitude = value.magnitude();
        if magnitude.is_nan() || magnitude <= singular_pivot {
            return None;
        }
        return Some(vec![vec![value.reciprocal()]]);
    }
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
        let magnitude = work[best][pivot].magnitude();
        if magnitude.is_nan() || magnitude <= singular_pivot {
            return None;
        }
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
    Some(inverse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_real_and_complex_matrices_and_refuses_singular_pivots() {
        let real = invert_dense_matrix(&[vec![0.0, 2.0], vec![4.0, 0.0]], 0.0).unwrap();
        assert_eq!(real, vec![vec![0.0, 0.25], vec![0.5, 0.0]]);
        let z = Complex64::new(1.0, 2.0);
        let complex = invert_dense_matrix(&[vec![z]], 0.0).unwrap();
        assert_eq!(complex[0][0], z.finv());
        assert!(invert_dense_matrix(&[vec![1.0, 2.0], vec![2.0, 4.0]], 0.0).is_none());
        assert!(invert_dense_matrix(&[vec![1e-15]], 1e-14).is_none());
        assert!(invert_dense_matrix(&[vec![1.0, 2.0]], 0.0).is_none());
        assert_eq!(invert_dense_matrix::<f64>(&[], 0.0), Some(Vec::new()));
    }
}

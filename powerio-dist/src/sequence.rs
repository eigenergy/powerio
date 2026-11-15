//! Symmetrical components: the one transform pair between sequence and
//! phase quantities.
//!
//! A transposed (symmetric) element with positive sequence value `z₁` and
//! zero sequence value `z₀` has the phase matrix with self entries
//! `(2 z₁ + z₀) / 3` and mutual entries `(z₀ − z₁) / 3`; the inverse reads
//! `z₁ = self − mutual` and `z₀ = self + 2 mutual`. A full three phase
//! matrix or phasor vector transforms with Fortescue's operator
//! `a = e^{j 2π/3}`.

use std::ops::{Add, Div, Mul, Sub};

use num_complex::Complex64;

use crate::VoltageSource;

/// The self (diagonal) phase entry of a transposed element:
/// `(2 z₁ + z₀) / 3`.
#[must_use]
pub fn calc_sequence_self<T>(positive: T, zero: T) -> T
where
    T: Copy + Add<Output = T> + Mul<f64, Output = T> + Div<f64, Output = T>,
{
    (positive * 2.0 + zero) / 3.0
}

/// The mutual (off diagonal) phase entry of a transposed element:
/// `(z₀ − z₁) / 3`.
#[must_use]
pub fn calc_sequence_mutual<T>(positive: T, zero: T) -> T
where
    T: Copy + Sub<Output = T> + Div<f64, Output = T>,
{
    (zero - positive) / 3.0
}

/// The `n × n` phase matrix of a transposed element from its positive and
/// zero sequence values ([`calc_sequence_self`] on the diagonal,
/// [`calc_sequence_mutual`] off it). The one sequence to phase expansion:
/// the OpenDSS line code and line readers and the pandapower reader read
/// it, for impedance, resistance, reactance and susceptance alike.
#[must_use]
pub fn calc_phase_from_sequence<T>(n: usize, positive: T, zero: T) -> Vec<Vec<T>>
where
    T: Copy + Add<Output = T> + Sub<Output = T> + Mul<f64, Output = T> + Div<f64, Output = T>,
{
    let mutual = calc_sequence_mutual(positive, zero);
    let own = calc_sequence_self(positive, zero);
    (0..n)
        .map(|i| (0..n).map(|j| if i == j { own } else { mutual }).collect())
        .collect()
}

/// The positive and zero sequence values `(z₁, z₀)` of a transposed element
/// from its self and mutual phase entries: `(self − mutual, self + 2 mutual)`.
/// The one phase to sequence inverse of [`calc_phase_from_sequence`]: the
/// OpenDSS source writer and the SINCAL multiconductor writer read it.
#[must_use]
pub fn calc_sequence_from_phase<T>(own: T, mutual: T) -> (T, T)
where
    T: Copy + Add<Output = T> + Sub<Output = T> + Mul<f64, Output = T>,
{
    (own - mutual, own + mutual * 2.0)
}

/// Fortescue's operator `a = e^{j 2π/3}` and `a²`.
fn rotation() -> (Complex64, Complex64) {
    let a = Complex64::from_polar(1.0, 2.0 * std::f64::consts::PI / 3.0);
    (a, a * a)
}

/// The zero, positive and negative sequence components of three phase
/// phasors `[a, b, c]`: `V₀ = (Va + Vb + Vc)/3`, `V₁ = (Va + a Vb + a² Vc)/3`,
/// `V₂ = (Va + a² Vb + a Vc)/3`.
#[must_use]
pub fn calc_sequence_components(phase: [Complex64; 3]) -> [Complex64; 3] {
    let (a, a2) = rotation();
    let [va, vb, vc] = phase;
    [
        (va + vb + vc) / 3.0,
        (va + a * vb + a2 * vc) / 3.0,
        (va + a2 * vb + a * vc) / 3.0,
    ]
}

/// The sequence matrix `A⁻¹ Z A` of a three phase matrix (rows and columns
/// zero, positive, negative), with `A` Fortescue's transform. A transposed
/// matrix is diagonal here, `diag(z₀, z₁, z₁)`; its off diagonal entries
/// measure the coupling between sequences.
///
/// # Panics
/// When `matrix` is smaller than three by three.
#[must_use]
pub fn calc_sequence_matrix(matrix: &[Vec<Complex64>]) -> [[Complex64; 3]; 3] {
    let (a, a2) = rotation();
    let one = Complex64::new(1.0, 0.0);
    // Columns of A: the phase phasors of a unit zero, positive and negative
    // sequence quantity; A⁻¹ = conj(A)ᵀ / 3.
    let basis = [[one, one, one], [one, a2, a], [one, a, a2]];
    let mut seq = [[Complex64::new(0.0, 0.0); 3]; 3];
    for (p, row) in seq.iter_mut().enumerate() {
        for (q, out) in row.iter_mut().enumerate() {
            for (i, matrix_row) in matrix.iter().take(3).enumerate() {
                for (j, value) in matrix_row.iter().take(3).enumerate() {
                    *out += basis[i][p].conj() * value * basis[j][q];
                }
            }
            *out /= 3.0;
        }
    }
    seq
}

impl VoltageSource {
    /// The source's line to line voltage base over its phase positions
    /// `phases` (indices into its voltage vectors), volts: `√3 |V₁|`, the
    /// magnitude of the balanced equivalent's line voltage, for three phases
    /// ([`calc_sequence_components`]); the phasor difference for two (a
    /// split phase source); the phase magnitude for one. `None` for any
    /// other count, a stated magnitude or angle that is missing or not
    /// finite, or a base that is not positive.
    ///
    /// The one source voltage base: the multiconductor to balanced lowering
    /// bases a source's zone on it.
    #[must_use]
    pub fn calc_line_to_line_voltage(&self, phases: &[usize]) -> Option<f64> {
        let phasor = |index: usize| {
            let magnitude = *self.v_magnitude.get(index)?;
            let angle = *self.v_angle.get(index)?;
            (magnitude.is_finite() && angle.is_finite())
                .then(|| Complex64::from_polar(magnitude, angle))
        };
        let base = match *phases {
            [a, b, c] => {
                let [_, positive, _] =
                    calc_sequence_components([phasor(a)?, phasor(b)?, phasor(c)?]);
                3f64.sqrt() * positive.norm()
            }
            [a, b] => (phasor(a)? - phasor(b)?).norm(),
            [a] => phasor(a)?.norm(),
            _ => return None,
        };
        (base.is_finite() && base > 0.0).then_some(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transform_pair_inverts() {
        let (positive, zero) = (Complex64::new(0.1, 0.4), Complex64::new(0.3, 1.2));
        let matrix = calc_phase_from_sequence(3, positive, zero);
        let (p, z) = calc_sequence_from_phase(matrix[0][0], matrix[0][1]);
        assert!((p - positive).norm() < 1e-15 && (z - zero).norm() < 1e-15);
        let seq = calc_sequence_matrix(&matrix);
        assert!((seq[0][0] - zero).norm() < 1e-14);
        assert!((seq[1][1] - positive).norm() < 1e-14);
        assert!(seq[0][1].norm() < 1e-14 && seq[1][2].norm() < 1e-14);
        let real = calc_phase_from_sequence(2, 1.0, 4.0);
        assert_eq!(real, vec![vec![2.0, 1.0], vec![1.0, 2.0]]);
    }

    #[test]
    fn a_balanced_source_has_its_line_voltage_as_base() {
        let third = 2.0 * std::f64::consts::PI / 3.0;
        let terminals = vec!["1".to_owned(), "2".to_owned(), "3".to_owned()];
        let mut source = VoltageSource::new(
            "s",
            "b",
            terminals,
            vec![100.0; 3],
            vec![0.0, -third, third],
        );
        let base = source.calc_line_to_line_voltage(&[0, 1, 2]).unwrap();
        assert!((base - 100.0 * 3f64.sqrt()).abs() < 1e-9);
        source.v_magnitude = vec![120.0, 120.0];
        source.v_angle = vec![0.0, std::f64::consts::PI];
        assert!((source.calc_line_to_line_voltage(&[0, 1]).unwrap() - 240.0).abs() < 1e-9);
        assert_eq!(source.calc_line_to_line_voltage(&[0, 1, 2, 3]), None);
    }
}

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

/// How a sequence to phase expansion takes its third. The spellings are
/// the same transform and can differ in the last place (and, for
/// [`SequenceThird::DivideFirst`], in where a large finite value
/// overflows); the reference engines use several, so a reader reproducing
/// one names the order its element class uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SequenceThird {
    /// Divide by three: OpenDSS `Line` (`RecalcElementData`:
    /// `Zs := (Ztemp + Cmplx(R0, X0)) / 3`) and `VSource`
    /// (`(2.0 * X1 + X0) / 3.0`), and every reader that follows no engine's
    /// rounding.
    Divide,
    /// Multiply by the rounded reciprocal `1.0 / 3.0`: OpenDSS `LineCode`
    /// alone (`CalcMatricesFromZ1Z0` multiplies by its local
    /// `OneThird := 1.0 / 3.0`).
    Reciprocal,
    /// Divide each sequence value by three before combining them, self
    /// `2 (z₁ / 3) + z₀ / 3` and mutual `z₀ / 3 − z₁ / 3`, so no
    /// intermediate sum of two large finite values overflows: the SINCAL
    /// multiconductor line and rated shunt readers.
    DivideFirst,
    /// The self entry divided by three as [`SequenceThird::Divide`] and the
    /// mutual entry as that self entry less `z₁`: OpenDSS `VSource` given
    /// its sequence impedances (`Xm := Xs − X1` in `RecalcElementData`).
    SelfLessPositive,
}

/// The self (diagonal) phase entry of a transposed element:
/// `(2 z₁ + z₀) / 3`, the third taken as `third` states.
#[must_use]
pub fn calc_sequence_self<T>(positive: T, zero: T, third: SequenceThird) -> T
where
    T: Copy + Add<Output = T> + Mul<f64, Output = T> + Div<f64, Output = T>,
{
    match third {
        SequenceThird::Divide | SequenceThird::SelfLessPositive => (positive * 2.0 + zero) / 3.0,
        SequenceThird::Reciprocal => (positive * 2.0 + zero) * (1.0 / 3.0),
        SequenceThird::DivideFirst => (positive / 3.0) * 2.0 + zero / 3.0,
    }
}

/// The mutual (off diagonal) phase entry of a transposed element:
/// `(z₀ − z₁) / 3`, the third taken as `third` states.
#[must_use]
pub fn calc_sequence_mutual<T>(positive: T, zero: T, third: SequenceThird) -> T
where
    T: Copy + Add<Output = T> + Sub<Output = T> + Mul<f64, Output = T> + Div<f64, Output = T>,
{
    match third {
        SequenceThird::Divide => (zero - positive) / 3.0,
        SequenceThird::Reciprocal => (zero - positive) * (1.0 / 3.0),
        SequenceThird::DivideFirst => zero / 3.0 - positive / 3.0,
        SequenceThird::SelfLessPositive => calc_sequence_self(positive, zero, third) - positive,
    }
}

/// The `n × n` phase matrix of a transposed element from its positive and
/// zero sequence values ([`calc_sequence_self`] on the diagonal,
/// [`calc_sequence_mutual`] off it). The one sequence to phase expansion:
/// the OpenDSS line code, line and source readers and the pandapower reader
/// read it, for impedance, resistance, reactance and susceptance alike.
#[must_use]
pub fn calc_phase_from_sequence<T>(
    n: usize,
    positive: T,
    zero: T,
    third: SequenceThird,
) -> Vec<Vec<T>>
where
    T: Copy + Add<Output = T> + Sub<Output = T> + Mul<f64, Output = T> + Div<f64, Output = T>,
{
    let mutual = calc_sequence_mutual(positive, zero, third);
    let own = calc_sequence_self(positive, zero, third);
    (0..n)
        .map(|i| (0..n).map(|j| if i == j { own } else { mutual }).collect())
        .collect()
}

/// OpenDSS's rotation operator `DSSGlobals.CALPHA`, `-0.5 + j 0.866025`:
/// rounded to six places, unlike Fortescue's exact `a`. The reference
/// engine's unequal sequence (`Z1 ≠ Z2`) source matrix uses it; replacing
/// it with `sin 120°` changes those impedances.
pub const OPENDSS_ALPHA: Complex64 = Complex64::new(-0.5, 0.866_025);

/// Entry `(i, j)`, `i ≠ j`, of the three phase circulant matrix of an
/// element with unequal sequence impedances `[z₀, z₁, z₂]` as OpenDSS
/// `VSource` forms it: `(z₀ + z₁ α^((j−i) mod 3) + z₂ α^((i−j) mod 3)) / 3`
/// with [`OPENDSS_ALPHA`]. The diagonal is `(z₂ + z₁ + z₀) / 3`.
/// [`calc_circulant_sequence_from_phase`] is its inverse.
#[must_use]
pub fn calc_circulant_phase_entry(sequence: [Complex64; 3], i: usize, j: usize) -> Complex64 {
    let [zero, positive, negative] = sequence;
    if i == j {
        return (negative + positive + zero) / 3.0;
    }
    (zero
        + positive * OPENDSS_ALPHA.powu(((j + 3 - i) % 3) as u32)
        + negative * OPENDSS_ALPHA.powu(((i + 3 - j) % 3) as u32))
        / 3.0
}

/// The sequence impedances `[z₀, z₁, z₂]` of a three phase circulant matrix
/// from its first row `[z₀₀, z₀₁, z₀₂]`, the inverse of
/// [`calc_circulant_phase_entry`] over the same [`OPENDSS_ALPHA`].
#[must_use]
pub fn calc_circulant_sequence_from_phase(row: [Complex64; 3]) -> [Complex64; 3] {
    let first_rotation = OPENDSS_ALPHA - 1.0;
    let second_rotation = OPENDSS_ALPHA * OPENDSS_ALPHA - 1.0;
    let upper_delta = 3.0 * (row[1] - row[0]);
    let lower_delta = 3.0 * (row[2] - row[0]);
    let denominator = first_rotation * first_rotation - second_rotation * second_rotation;
    let positive = (upper_delta * first_rotation - lower_delta * second_rotation) / denominator;
    let negative = (lower_delta * first_rotation - upper_delta * second_rotation) / denominator;
    let zero = 3.0 * row[0] - positive - negative;
    [zero, positive, negative]
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
        let matrix = calc_phase_from_sequence(3, positive, zero, SequenceThird::Divide);
        let (p, z) = calc_sequence_from_phase(matrix[0][0], matrix[0][1]);
        assert!((p - positive).norm() < 1e-15 && (z - zero).norm() < 1e-15);
        let seq = calc_sequence_matrix(&matrix);
        assert!((seq[0][0] - zero).norm() < 1e-14);
        assert!((seq[1][1] - positive).norm() < 1e-14);
        assert!(seq[0][1].norm() < 1e-14 && seq[1][2].norm() < 1e-14);
        let real = calc_phase_from_sequence(2, 1.0, 4.0, SequenceThird::Reciprocal);
        assert_eq!(real, vec![vec![2.0, 1.0], vec![1.0, 2.0]]);
        // The two thirds are the same transform, apart in the last place.
        let (a, b) = (0.7_f64, 0.1784_f64);
        let divide = calc_sequence_self(a, b, SequenceThird::Divide);
        let reciprocal = calc_sequence_self(a, b, SequenceThird::Reciprocal);
        assert_eq!(divide, (2.0 * a + b) / 3.0);
        assert_eq!(reciprocal, (2.0 * a + b) * (1.0 / 3.0));
        assert!((divide - reciprocal).abs() <= f64::EPSILON);
        // Dividing first keeps a large finite expansion finite.
        let large = f64::MAX / 2.0;
        assert!(calc_sequence_self(large, large, SequenceThird::Divide).is_infinite());
        let first = calc_sequence_self(large, large, SequenceThird::DivideFirst);
        assert_eq!(first, (large / 3.0) * 2.0 + large / 3.0);
        assert_eq!(
            calc_sequence_mutual(-large, large, SequenceThird::DivideFirst),
            large / 3.0 - -large / 3.0
        );
        // OpenDSS VSource's sequence setters take the mutual as Xs − X1.
        let own = calc_sequence_self(a, b, SequenceThird::SelfLessPositive);
        assert_eq!(own, divide);
        assert_eq!(
            calc_sequence_mutual(a, b, SequenceThird::SelfLessPositive),
            own - a
        );
        let sequence = [
            Complex64::new(0.3, 1.2),
            Complex64::new(0.1, 0.4),
            Complex64::new(0.12, 0.5),
        ];
        let row = [0, 1, 2].map(|j| calc_circulant_phase_entry(sequence, 0, j));
        for (back, value) in calc_circulant_sequence_from_phase(row).iter().zip(sequence) {
            assert!((back - value).norm() < 1e-12);
        }
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

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
/// with [`OPENDSS_ALPHA`], summed in `VSource.RecalcElementData`'s order,
/// the `α²` term, then the `α` term, then `z₀` (`Calpha2 * Z2 + Calpha1 *
/// Z1 + Z0` above the diagonal, `Calpha2 * Z1 + Calpha1 * Z2 + Z0` below
/// it, DSS C-API 0.14.5), with `α² = α · α`. The diagonal is
/// `(z₂ + z₁ + z₀) / 3`. [`calc_circulant_sequence_from_phase`] is its
/// inverse.
#[must_use]
pub fn calc_circulant_phase_entry(sequence: [Complex64; 3], i: usize, j: usize) -> Complex64 {
    let [zero, positive, negative] = sequence;
    if i == j {
        return (negative + positive + zero) / 3.0;
    }
    let alpha_squared = OPENDSS_ALPHA * OPENDSS_ALPHA;
    // Above the diagonal (j = i + 1 mod 3) z₁ takes α and z₂ takes α².
    let (by_alpha_squared, by_alpha) = if (j + 3 - i) % 3 == 1 {
        (negative, positive)
    } else {
        (positive, negative)
    };
    (alpha_squared * by_alpha_squared + OPENDSS_ALPHA * by_alpha + zero) / 3.0
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

/// The rotation operator `a` a symmetrical component transform is built
/// on. The variants are the same transform to within their rounding of
/// `sin 120°`; the reference tools use several, so a reader reproducing one
/// names the operator it uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SequenceRotation {
    /// Fortescue's `a = e^{j 2π/3}` from its polar form, `a² = a · a`, and
    /// the inverse `A⁻¹ = conj(A)ᵀ / 3`: every reader and model that follows
    /// no engine's rounding.
    Fortescue,
    /// `a = −½ + j √3/2` written in rectangular form, `a² = conj(a)`, and
    /// the inverse `conj(A)ᵀ / 3`: the SINCAL multiconductor transformer
    /// port expansion.
    Rectangular,
    /// OpenDSS `mathutil.pas`'s `SetAMatrix` as upstream OpenDSS builds it
    /// (and DSS C-API under its `BadPrecision` compatibility flag):
    /// `a = −½ + j 0.866025403`, truncated, `a² = conj(a)`, and `A⁻¹` the
    /// closed form of the inverse of that `A`, whose sequence rows are
    /// `[⅓, w, conj(w)]` and `[⅓, conj(w), w]` with
    /// `w = −1/6 + j / (4 · 0.866025403)`; `conj(A)ᵀ / 3` is not its inverse.
    /// `mathutil.pas` forms `Ap2s` numerically (`TcMatrix.Invert`), whose
    /// sweep differs from this closed form in the last place (its `⅓`
    /// entries and `w.re`, with residues of order `2⁻⁵⁴` in their imaginary
    /// parts), so values through this operator agree with the engine's to
    /// within that rounding, not bit for bit. The IndMach012a user model's
    /// sequence admittance reads it.
    OpenDssMathutil,
}

/// `sin 120°` as `mathutil.pas`'s `SetAMatrix` spells it.
const MATHUTIL_SINE: f64 = 0.866_025_403;

/// The transform `A` (columns: the phase phasors of a unit zero, positive
/// and negative sequence quantity) and its inverse under `rotation`.
fn transform(rotation: SequenceRotation) -> ([[Complex64; 3]; 3], [[Complex64; 3]; 3]) {
    let one = Complex64::new(1.0, 0.0);
    let (a, a2) = match rotation {
        SequenceRotation::Fortescue => {
            let a = Complex64::from_polar(1.0, 2.0 * std::f64::consts::PI / 3.0);
            (a, a * a)
        }
        SequenceRotation::Rectangular => {
            let a = Complex64::new(-0.5, crate::electrical::SQRT_3 / 2.0);
            (a, a.conj())
        }
        SequenceRotation::OpenDssMathutil => {
            let a = Complex64::new(-0.5, MATHUTIL_SINE);
            (a, a.conj())
        }
    };
    let forward = [[one, one, one], [one, a2, a], [one, a, a2]];
    let third = one / 3.0;
    let inverse = if rotation == SequenceRotation::OpenDssMathutil {
        let w = Complex64::new(-1.0 / 6.0, 1.0 / (4.0 * MATHUTIL_SINE));
        [
            [third, third, third],
            [third, w, w.conj()],
            [third, w.conj(), w],
        ]
    } else {
        [
            [third, third, third],
            [third, a / 3.0, a2 / 3.0],
            [third, a2 / 3.0, a / 3.0],
        ]
    };
    (forward, inverse)
}

/// The zero, positive and negative sequence components of three phase
/// phasors `[a, b, c]`: `V₀ = (Va + Vb + Vc)/3`, `V₁ = (Va + a Vb + a² Vc)/3`,
/// `V₂ = (Va + a² Vb + a Vc)/3`, under `rotation` (for
/// [`SequenceRotation::OpenDssMathutil`], the rows of its exact inverse).
#[must_use]
pub fn calc_sequence_components(
    phase: [Complex64; 3],
    rotation: SequenceRotation,
) -> [Complex64; 3] {
    let (forward, inverse) = transform(rotation);
    let [va, vb, vc] = phase;
    if rotation == SequenceRotation::OpenDssMathutil {
        return inverse.map(|row| row[0] * va + row[1] * vb + row[2] * vc);
    }
    let (a, a2) = (forward[2][1], forward[1][1]);
    [
        (va + vb + vc) / 3.0,
        (va + a * vb + a2 * vc) / 3.0,
        (va + a2 * vb + a * vc) / 3.0,
    ]
}

/// The three phase phasors `[a, b, c]` of zero, positive and negative
/// sequence components `[V₀, V₁, V₂]`: `Va = V₀ + V₁ + V₂`,
/// `Vb = V₀ + a² V₁ + a V₂`, `Vc = V₀ + a V₁ + a² V₂` under `rotation`, the
/// inverse of [`calc_sequence_components`]. The GridLAB-D sequence voltage
/// substation reads it.
#[must_use]
pub fn calc_phase_components(
    sequence: [Complex64; 3],
    rotation: SequenceRotation,
) -> [Complex64; 3] {
    let (forward, _) = transform(rotation);
    let (a, a2) = (forward[2][1], forward[1][1]);
    let [zero, positive, negative] = sequence;
    [
        zero + positive + negative,
        zero + a2 * positive + a * negative,
        zero + a * positive + a2 * negative,
    ]
}

/// The three phase matrix `A diag(d₀, d₁, d₂) A⁻¹` of an element whose
/// sequence networks are uncoupled, with sequence values
/// `diagonal = [d₀, d₁, d₂]` (impedances or admittances), under `rotation`.
/// For an operator whose inverse is `conj(A)ᵀ / 3` entry `(i, j)` is
/// `(d₀ + d₁ f + d₂ conj(f)) / 3` with `f = p_i conj(p_j)` and
/// `p = [1, a², a]`; for [`SequenceRotation::OpenDssMathutil`] it is
/// `Σₛ A_is dₛ A⁻¹_sj` over its exact inverse. The one sequence diagonal to
/// phase expansion: the SINCAL multiconductor transformer ports
/// ([`SequenceRotation::Rectangular`]) and the OpenDSS IndMach012a motor
/// ([`SequenceRotation::OpenDssMathutil`]) read it.
#[must_use]
pub fn calc_phase_matrix_from_sequence(
    diagonal: [Complex64; 3],
    rotation: SequenceRotation,
) -> [[Complex64; 3]; 3] {
    let (forward, inverse) = transform(rotation);
    let mut matrix = [[Complex64::default(); 3]; 3];
    for (i, row) in matrix.iter_mut().enumerate() {
        for (j, entry) in row.iter_mut().enumerate() {
            *entry = if rotation == SequenceRotation::OpenDssMathutil {
                forward[i][1] * diagonal[1] * inverse[1][j]
                    + forward[i][2] * diagonal[2] * inverse[2][j]
                    + forward[i][0] * diagonal[0] * inverse[0][j]
            } else {
                let p = [forward[0][1], forward[1][1], forward[2][1]];
                let factor = p[i] * p[j].conj();
                (diagonal[0] + diagonal[1] * factor + diagonal[2] * factor.conj()) / 3.0
            };
        }
    }
    matrix
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
    // Columns of A: the phase phasors of a unit zero, positive and negative
    // sequence quantity; A⁻¹ = conj(A)ᵀ / 3.
    let (basis, _) = transform(SequenceRotation::Fortescue);
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

/// The line to line over line to neutral ratio of `phases` phasors of one
/// magnitude spaced `2π / phases` apart, `2 sin(π / phases)` (`√3` for
/// three, `2` for two): OpenDSS `VSource`'s per phase magnitude
/// (`kVBase · PerUnit · 1000 / 2 / Sin(180 / Fnphases · π / 180)`) and the
/// CIM and MG-RAVENS `nominalVoltage` read and written over it. The one
/// polygon chord.
#[must_use]
pub fn calc_polygon_chord(phases: usize) -> f64 {
    2.0 * (std::f64::consts::PI / phases as f64).sin()
}

/// The angle, radians, of phase `phase` (counted from zero) of a balanced
/// positive sequence set of `phases` phasors whose first phase is at zero:
/// each phase lags the one before by `2π / phases`, stated in `(−π, π]`
/// (`0`, `−2π/3`, `+2π/3` for three phases), formed as `−(m · τ / n)` for
/// `m = phase mod n` with `2m < n` and as `(n − m) · τ / n` otherwise. Zero
/// for `phases` zero. The one balanced phase rotation: the GridLAB-D nominal
/// phasors and sequence voltage substation, the multiconductor OPF start
/// point, the SINCAL multiconductor source writer and the integrated
/// transmission and distribution coupling read it.
#[must_use]
pub fn calc_balanced_phase_angle(phase: usize, phases: usize) -> f64 {
    if phases == 0 {
        return 0.0;
    }
    let m = phase % phases;
    let (n, tau) = (phases as f64, std::f64::consts::TAU);
    if 2 * m < phases {
        -(m as f64 * tau / n)
    } else {
        (phases - m) as f64 * tau / n
    }
}

/// Which phasors [`VoltageSource::calc_line_to_line_voltage`] reads a
/// source's line to line base from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LineVoltageForm {
    /// The balanced equivalent of every phasor: `√3 |V₁|` for three phases
    /// ([`calc_sequence_components`], Fortescue), the phasor difference
    /// `|Va − Vb|` for two (a split phase pair). The multiconductor to
    /// balanced lowering reads it.
    PositiveSequence,
    /// The first phase's magnitude times the chord of the `n` phase polygon
    /// ([`calc_polygon_chord`]): the form an MG-RAVENS or CIM
    /// `EnergySource`, which states one magnitude for a balanced set, takes
    /// its `nominalVoltage` in. The MG-RAVENS writer reads it.
    FirstPhase,
}

impl VoltageSource {
    /// The source's line to line voltage base over its phase positions
    /// `phases` (indices into its voltage vectors), volts, in `form`
    /// ([`LineVoltageForm`]). One phase is the line to line equivalent
    /// `√3 |V|` of its magnitude, which is line to neutral
    /// ([`crate::electrical::calc_wye_line_voltage`]), in either form.
    ///
    /// Under [`LineVoltageForm::PositiveSequence`]: `√3 |V₁|` for three
    /// phases, `|Va − Vb|` for two; `None` for any other count, a stated
    /// magnitude or angle that is missing or not finite, or a base that is
    /// not positive. Under [`LineVoltageForm::FirstPhase`]: the first
    /// position's stated magnitude times `calc_polygon_chord(n)` for `n` of
    /// two or more, formed as stated (a zero, negative or nonfinite
    /// magnitude gives that base, for the caller's own check); `None` only
    /// when no position is given or its magnitude is missing.
    ///
    /// The one source voltage base: the multiconductor to balanced lowering
    /// bases a source's zone on it, and the MG-RAVENS writer states it as a
    /// source's `nominalVoltage` and seeds the source bus's `BaseVoltage`
    /// with it when the source carries no `basekv`.
    #[must_use]
    pub fn calc_line_to_line_voltage(
        &self,
        phases: &[usize],
        form: LineVoltageForm,
    ) -> Option<f64> {
        if form == LineVoltageForm::FirstPhase {
            let magnitude = *self.v_magnitude.get(*phases.first()?)?;
            return Some(if phases.len() == 1 {
                crate::electrical::calc_wye_line_voltage(magnitude)
            } else {
                magnitude * calc_polygon_chord(phases.len())
            });
        }
        let phasor = |index: usize| {
            let magnitude = *self.v_magnitude.get(index)?;
            let angle = *self.v_angle.get(index)?;
            (magnitude.is_finite() && angle.is_finite())
                .then(|| Complex64::from_polar(magnitude, angle))
        };
        let base = match *phases {
            [a, b, c] => {
                let [_, positive, _] = calc_sequence_components(
                    [phasor(a)?, phasor(b)?, phasor(c)?],
                    SequenceRotation::Fortescue,
                );
                crate::electrical::calc_wye_line_voltage(positive.norm())
            }
            [a, b] => (phasor(a)? - phasor(b)?).norm(),
            [a] => {
                phasor(a)?;
                crate::electrical::calc_wye_line_voltage(self.v_magnitude[a].abs())
            }
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
        for rotation in [
            SequenceRotation::Fortescue,
            SequenceRotation::Rectangular,
            SequenceRotation::OpenDssMathutil,
        ] {
            let phase = calc_phase_components(sequence, rotation);
            for (back, value) in calc_sequence_components(phase, rotation)
                .iter()
                .zip(sequence)
            {
                assert!((back - value).norm() < 1e-12, "{rotation:?}");
            }
            // A diag A⁻¹ applied to the phasors is A diag of their sequence.
            let matrix = calc_phase_matrix_from_sequence(sequence, rotation);
            let unit = [
                Complex64::new(1.0, 0.0),
                Complex64::new(0.2, -0.3),
                Complex64::new(-0.4, 0.1),
            ];
            let image = calc_phase_components(
                {
                    let components = calc_sequence_components(unit, rotation);
                    [
                        components[0] * sequence[0],
                        components[1] * sequence[1],
                        components[2] * sequence[2],
                    ]
                },
                rotation,
            );
            for (i, row) in matrix.iter().enumerate() {
                let value: Complex64 = row.iter().zip(unit).map(|(m, v)| m * v).sum();
                assert!((value - image[i]).norm() < 1e-12, "{rotation:?}");
            }
        }
        let row = [0, 1, 2].map(|j| calc_circulant_phase_entry(sequence, 0, j));
        for (back, value) in calc_circulant_sequence_from_phase(row).iter().zip(sequence) {
            assert!((back - value).norm() < 1e-12);
        }
    }

    #[test]
    fn the_balanced_phase_angles_are_the_positive_sequence_rotation() {
        use std::f64::consts::{PI, TAU};
        assert_eq!(
            [0, 1, 2].map(|k| calc_balanced_phase_angle(k, 3)),
            [0.0, -2.0 * PI / 3.0, 2.0 * PI / 3.0]
        );
        assert_eq!(calc_balanced_phase_angle(4, 3), -TAU / 3.0);
        assert_eq!(calc_balanced_phase_angle(1, 2), PI);
        assert_eq!(calc_balanced_phase_angle(0, 0), 0.0);
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
        let base = source
            .calc_line_to_line_voltage(&[0, 1, 2], LineVoltageForm::PositiveSequence)
            .unwrap();
        assert!((base - 100.0 * 3f64.sqrt()).abs() < 1e-9);
        source.v_magnitude = vec![120.0, 120.0];
        source.v_angle = vec![0.0, std::f64::consts::PI];
        assert!(
            (source
                .calc_line_to_line_voltage(&[0, 1], LineVoltageForm::PositiveSequence)
                .unwrap()
                - 240.0)
                .abs()
                < 1e-9
        );
        assert_eq!(
            source.calc_line_to_line_voltage(&[0, 1, 2, 3], LineVoltageForm::PositiveSequence),
            None
        );
        for form in [
            LineVoltageForm::PositiveSequence,
            LineVoltageForm::FirstPhase,
        ] {
            assert_eq!(
                source.calc_line_to_line_voltage(&[0], form),
                Some(crate::electrical::calc_wye_line_voltage(120.0))
            );
        }
        // The first phase form reads one magnitude: a split phase pair's is
        // twice it, whatever the second phasor states.
        source.v_magnitude = vec![120.0, 100.0];
        assert_eq!(
            source.calc_line_to_line_voltage(&[0, 1], LineVoltageForm::FirstPhase),
            Some(240.0)
        );
    }
}

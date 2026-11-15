//! The scalar electrical formulas both network families share: the per unit
//! impedance base, the power factor relations and the voltage power law
//! term. Each is the one implementation the transmission, distribution,
//! composition and matrix crates call.

/// The impedance base `V² / S` of a voltage base and a power base in
/// consistent units (volts and volt amperes give ohms, kilovolts and
/// megavolt amperes give ohms as well). `None` unless both bases are
/// positive and finite and the quotient is finite; the caller states what an
/// unusable base means for its own quantity.
#[must_use]
pub fn calc_impedance_base(voltage: f64, power: f64) -> Option<f64> {
    let positive = |x: f64| x.is_finite() && x > 0.0;
    if !positive(voltage) || !positive(power) {
        return None;
    }
    Some(voltage * voltage / power).filter(|base| positive(*base))
}

/// The admittance base `S / V²` of a voltage base and a power base in
/// consistent units (volts and volt amperes give siemens, kilovolts and
/// megavolt amperes give siemens as well), formed as `power / (voltage ·
/// voltage)`. `None` unless both bases are positive and finite and the
/// quotient is positive and finite. The one admittance base: dividing by it
/// takes a susceptance or conductance to per unit as dividing by
/// [`calc_impedance_base`] takes an impedance.
#[must_use]
pub fn calc_admittance_base(voltage: f64, power: f64) -> Option<f64> {
    let positive = |x: f64| x.is_finite() && x > 0.0;
    if !positive(voltage) || !positive(power) {
        return None;
    }
    Some(power / (voltage * voltage)).filter(|base| positive(*base))
}

/// The sine of a power factor's angle, `sin(acos |pf|) = √((1 − |pf|)(1 + |pf|))`,
/// formed without the cancellation of `1 − pf²` near unity. Reactive power
/// is `S` times this for an apparent power `S`. `NaN` when `|pf| > 1`.
#[must_use]
pub fn calc_power_factor_sine(pf: f64) -> f64 {
    let magnitude = pf.abs();
    ((1.0 - magnitude) * (1.0 + magnitude)).sqrt()
}

/// The reactive over active power ratio a power factor fixes, OpenDSS and
/// BMOPF by sign: `Q / P = tan(acos |pf|)` with `pf > 0`, Q on the same side
/// as P, and the opposite sign for `pf < 0`. This is
/// [`calc_power_factor_sine`]` / pf`, infinite at `pf = ±0` (signed as the
/// zero) and `NaN` when `|pf| > 1`.
#[must_use]
pub fn calc_reactive_ratio(pf: f64) -> f64 {
    calc_power_factor_sine(pf) / pf
}

/// The derivative of [`calc_reactive_ratio`] with respect to `pf`,
/// `−1 / (pf² √(1 − pf²))` on `0 < |pf| < 1`.
#[must_use]
pub fn calc_reactive_ratio_derivative(pf: f64) -> f64 {
    -1.0 / (pf * pf * calc_power_factor_sine(pf))
}

/// The coefficients `[a, b]` of a fixed power factor written as the row
/// `a P + b Q = 0`: `a = √((1 − |pf|)(1 + |pf|))` and `b = −pf`, so that
/// `Q / P = a / pf` ([`calc_reactive_ratio`]) and a tiny nonzero power factor
/// keeps a finite row.
#[must_use]
pub fn calc_power_factor_coefficients(pf: f64) -> [f64; 2] {
    [calc_power_factor_sine(pf), -pf]
}

/// The derivatives of [`calc_power_factor_coefficients`] with respect to
/// `pf`: `[−pf / √(1 − pf²), −1]` on `|pf| < 1`.
#[must_use]
pub fn calc_power_factor_coefficients_derivative(pf: f64) -> [f64; 2] {
    [-pf / calc_power_factor_sine(pf), -1.0]
}

/// Below this voltage ratio a power law term with exponent below one is
/// continued by its tangent at the floor ([`calc_voltage_power_term`]):
/// `x^γ` has an unbounded slope at zero for `γ < 1` (and is itself unbounded
/// for `γ < 0`), and an iterate can pass near zero on its way to a solution.
pub const LOAD_VOLTAGE_RATIO_FLOOR: f64 = 1e-3;

/// One voltage power law term `c x^γ` at the voltage ratio `x = |V| / V_nom`
/// and its derivative `γ c x^(γ−1)` with respect to `x`. Exponents 0, 1 and 2
/// (constant power, current and impedance) are evaluated without `powf`. For
/// any other `γ < 1` and `x` below [`LOAD_VOLTAGE_RATIO_FLOOR`] the term
/// follows its tangent at the floor, `c (f₀ + f₀' (x − x₀))`, so value and
/// slope stay finite and consistent as a voltage approaches zero.
///
/// The one voltage dependent load law evaluation: the balanced load terms,
/// the distribution ZIP, exponential, constant current and impedance laws,
/// and the multiconductor OPF load law read it.
#[must_use]
#[allow(clippy::float_cmp)] // the integer exponents are exact spellings
pub fn calc_voltage_power_term(coefficient: f64, exponent: f64, ratio: f64) -> (f64, f64) {
    let (c, g, x) = (coefficient, exponent, ratio);
    if g == 0.0 {
        (c, 0.0)
    } else if g == 1.0 {
        (c * x, c)
    } else if g == 2.0 {
        (c * x * x, 2.0 * c * x)
    } else if g < 1.0 && x < LOAD_VOLTAGE_RATIO_FLOOR {
        let f0 = LOAD_VOLTAGE_RATIO_FLOOR.powf(g);
        let d0 = g * LOAD_VOLTAGE_RATIO_FLOOR.powf(g - 1.0);
        (c * (f0 + d0 * (x - LOAD_VOLTAGE_RATIO_FLOOR)), c * d0)
    } else {
        (c * x.powf(g), c * g * x.powf(g - 1.0))
    }
}

/// The sum of voltage power law terms `[coefficient, exponent]` at the
/// voltage ratio `x`, with its derivative with respect to `x`, each term by
/// [`calc_voltage_power_term`].
#[must_use]
pub fn calc_voltage_power_terms(terms: &[[f64; 2]], ratio: f64) -> (f64, f64) {
    terms.iter().fold((0.0, 0.0), |(value, slope), &[c, g]| {
        let (v, d) = calc_voltage_power_term(c, g, ratio);
        (value + v, slope + d)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impedance_base_needs_positive_finite_bases() {
        assert_eq!(calc_impedance_base(12.47, 10.0), Some(12.47 * 12.47 / 10.0));
        assert_eq!(calc_impedance_base(0.0, 10.0), None);
        assert_eq!(calc_impedance_base(12.47, 0.0), None);
        assert_eq!(calc_impedance_base(f64::NAN, 1.0), None);
        assert_eq!(calc_impedance_base(1e300, 1e-300), None);
    }

    #[test]
    fn admittance_base_needs_positive_finite_bases() {
        assert_eq!(
            calc_admittance_base(12.47, 10.0),
            Some(10.0 / (12.47 * 12.47))
        );
        assert_eq!(calc_admittance_base(0.0, 10.0), None);
        assert_eq!(calc_admittance_base(12.47, -1.0), None);
        assert_eq!(calc_admittance_base(f64::INFINITY, 1.0), None);
        assert_eq!(calc_admittance_base(1e300, 1e-300), None);
    }

    #[test]
    fn power_factor_relations_agree_with_the_trigonometric_forms() {
        for pf in [0.9_f64, -0.85, 0.3, 1.0, -1.0] {
            let ratio = pf.abs().acos().tan().copysign(pf);
            assert!((calc_reactive_ratio(pf) - ratio).abs() < 1e-12);
            let [a, b] = calc_power_factor_coefficients(pf);
            assert!((-a / b - ratio).abs() < 1e-12);
        }
        let (pf, h) = (0.8_f64, 1e-6);
        let fd = (calc_reactive_ratio(pf + h) - calc_reactive_ratio(pf - h)) / (2.0 * h);
        assert!((calc_reactive_ratio_derivative(pf) - fd).abs() < 1e-6);
        let fd = (calc_power_factor_coefficients(pf + h)[0]
            - calc_power_factor_coefficients(pf - h)[0])
            / (2.0 * h);
        assert!((calc_power_factor_coefficients_derivative(pf)[0] - fd).abs() < 1e-6);
    }

    #[test]
    fn a_power_term_continues_by_its_tangent_below_the_floor() {
        let (v, d) = calc_voltage_power_term(2.0, 1.5, 0.9);
        assert!((v - 2.0 * 0.9_f64.powf(1.5)).abs() < 1e-15);
        assert!((d - 3.0 * 0.9_f64.sqrt()).abs() < 1e-15);
        let (v, d) = calc_voltage_power_term(1.0, 0.5, 0.0);
        assert!(v.is_finite() && d.is_finite() && v > 0.0);
        assert_eq!(calc_voltage_power_term(3.0, 0.0, 0.0), (3.0, 0.0));
        let (v, d) = calc_voltage_power_terms(&[[1.0, 2.0], [0.5, 1.0]], 0.5);
        assert!((v - 0.5).abs() < 1e-15 && (d - 1.5).abs() < 1e-15);
    }
}

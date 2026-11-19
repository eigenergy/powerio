//! The scalar electrical formulas both network families share: the per unit
//! impedance base, the power factor relations and the voltage power law
//! term. Each is the one implementation the transmission, distribution,
//! composition and matrix crates call.

use std::ops::Div;

/// The impedance base `V² / S` of a voltage base and a power base in
/// consistent units (volts and volt amperes give ohms, kilovolts and
/// megavolt amperes give ohms as well). `None` unless both bases are
/// positive and finite and the quotient is finite; the caller states what an
/// unusable base means for its own quantity. The same quantity as
/// [`calc_branch_impedance_base`] with one voltage at both ends.
#[must_use]
pub fn calc_impedance_base(voltage: f64, power: f64) -> Option<f64> {
    calc_branch_impedance_base(voltage, voltage, power)
}

/// The order in which [`calc_impedance_rebase`] forms `value · Z_base(from)
/// / Z_base(to)`. Each reader keeps the order it has always used, so its
/// results stay bit for bit the same; the quantity is the same in every
/// order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebaseOrder {
    /// `value · (Z_base(from) / Z_base(to))`, each base by
    /// [`calc_impedance_base`]; `NaN` when either base is unusable. The
    /// SINCAL balanced transformer's series impedance.
    BaseQuotient,
    /// `value · (S_to / S_from) · (V_from / V_to)²`, unchecked. The PSS/E
    /// `CM=2` exciting current and the integrated transmission and
    /// distribution transformer lowering.
    PowerRatio,
    /// `value / (S_from / S_to) · (V_from / V_to)²`, unchecked. The PSS/E
    /// `CM=2` no load loss.
    InversePowerRatio,
}

/// `value` taken from the per unit base `(from_voltage, from_power)` to the
/// base `(to_voltage, to_power)` when it is a per unit impedance, the factor
/// `Z_base(from) / Z_base(to)`; with the bases swapped it takes a per unit
/// admittance from `from` to `to`. `order` picks the operation order
/// ([`RebaseOrder`]). The one per unit rebase: the SINCAL balanced
/// transformer's series impedance, the PSS/E `CM=2` magnetizing admittance
/// and the integrated transmission and distribution transformer lowering
/// read it.
#[must_use]
pub fn calc_impedance_rebase(
    value: f64,
    from_voltage: f64,
    from_power: f64,
    to_voltage: f64,
    to_power: f64,
    order: RebaseOrder,
) -> f64 {
    let voltage_ratio = || (from_voltage / to_voltage).powi(2);
    match order {
        RebaseOrder::BaseQuotient => {
            match (
                calc_impedance_base(from_voltage, from_power),
                calc_impedance_base(to_voltage, to_power),
            ) {
                (Some(from), Some(to)) => value * (from / to),
                _ => f64::NAN,
            }
        }
        RebaseOrder::PowerRatio => value * (to_power / from_power) * voltage_ratio(),
        RebaseOrder::InversePowerRatio => value / (from_power / to_power) * voltage_ratio(),
    }
}

/// The impedance base `V₁ V₂ / S` of a branch between two voltage bases at
/// a power base, formed as `from_voltage · to_voltage / power`, in
/// consistent units: the base on which a line joining two nominal voltages
/// (an IIDM line between voltage levels) states its per unit series
/// impedance. With `from_voltage == to_voltage` it is
/// [`calc_impedance_base`]. `None` unless all three bases are positive and
/// finite and the quotient is positive and finite.
#[must_use]
pub fn calc_branch_impedance_base(from_voltage: f64, to_voltage: f64, power: f64) -> Option<f64> {
    let positive = |x: f64| x.is_finite() && x > 0.0;
    if !positive(from_voltage) || !positive(to_voltage) || !positive(power) {
        return None;
    }
    Some(calc_squared_voltage_impedance(
        from_voltage * to_voltage,
        power,
    ))
    .filter(|base| positive(*base))
}

/// The impedance `V² / S` that draws the power `power` at a voltage whose
/// squared magnitude is `voltage_squared`, formed as `voltage_squared /
/// power`, for a real or a complex power (`|V|² / conj(S)` takes
/// `conj(S)`). With an impedance in place of the power it is the power that
/// impedance draws, the same quotient. Unchecked: the caller states what an
/// unusable voltage or power means. The one voltage squared over a power:
/// [`calc_branch_impedance_base`] (over `V₁ V₂`), the distribution ZIP
/// coefficient and nominal power conversions and the OpenDSS rated reactor
/// read it.
#[must_use]
pub fn calc_squared_voltage_impedance<T>(voltage_squared: f64, power: T) -> T
where
    f64: Div<T, Output = T>,
{
    voltage_squared / power
}

/// The admittance `S / V²` that draws the power `power` at a voltage whose
/// squared magnitude is `voltage_squared`, formed as `power /
/// voltage_squared`, for a real or a complex power (`conj(S) / |V|²` takes
/// `conj(S)`). Unchecked, as [`calc_squared_voltage_impedance`]. The one
/// power over voltage squared: [`calc_rated_admittance`] is it at
/// `voltage · voltage`, and the distribution load's nominal admittance and
/// the transformer magnetizing base read it.
#[must_use]
pub fn calc_squared_voltage_admittance<T>(power: T, voltage_squared: f64) -> T
where
    T: Div<f64, Output = T>,
{
    power / voltage_squared
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
    Some(calc_rated_admittance(power, voltage)).filter(|base| positive(*base))
}

/// The admittance that draws `power` at `voltage`, `power / V²`, formed as
/// `power / (voltage · voltage)` in consistent units: a susceptance from a
/// rated reactive power (a capacitor bank or a shunt section in Mvar at
/// kilovolts), a conductance from an active power. The one power over
/// voltage squared at a real power ([`calc_squared_voltage_admittance`]):
/// [`calc_admittance_base`], the capacitor stamp, the BMOPF capacitor
/// lowering, the OpenDSS common return bank and the CGMES and PSS/E shunt
/// and SVC conversions read it.
#[must_use]
pub fn calc_rated_admittance(power: f64, voltage: f64) -> f64 {
    calc_squared_voltage_admittance(power, voltage * voltage)
}

/// The power an admittance draws at `voltage`, `admittance · V²`, formed as
/// `admittance · (voltage · voltage)`: the inverse of
/// [`calc_rated_admittance`].
#[must_use]
pub fn calc_rated_power(admittance: f64, voltage: f64) -> f64 {
    admittance * (voltage * voltage)
}

/// `√3`, the line to line over line to neutral voltage ratio of a balanced
/// three phase system.
pub const SQRT_3: f64 = 1.732_050_807_568_877_2;

/// The line to neutral voltage of a balanced three phase system from its
/// line to line voltage, `V_LL / √3`. The one line to neutral conversion
/// (OpenDSS, MG-RAVENS, pandapower, the transmission and distribution
/// coupling).
#[must_use]
pub fn calc_wye_phase_voltage(line_voltage: f64) -> f64 {
    line_voltage / SQRT_3
}

/// The line to line voltage of a balanced three phase system from its line
/// to neutral voltage, `√3 V_LN`: the inverse of [`calc_wye_phase_voltage`].
#[must_use]
pub fn calc_wye_line_voltage(phase_voltage: f64) -> f64 {
    phase_voltage * SQRT_3
}

/// The order [`calc_line_apparent_power`] multiplies in. The orders are the
/// same product and can differ in the last place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApparentPowerOrder {
    /// `factor · V · I`: every reader and writer that follows no other
    /// tool's rounding.
    FactorFirst,
    /// `I · factor · V`: PowSybl's `AbstractConverter.convertToMva`
    /// (`I / 1000 · √3 · Vnom`, the current passed in kiloamperes).
    CurrentFirst,
}

/// The apparent power of a balanced current `current` at the voltage
/// `voltage`: `factor · V · I`, with `factor` [`SQRT_3`] for a three phase
/// rating at its line to line voltage and `1` for a single phase one, in
/// consistent units (kilovolts and kiloamperes give megavolt amperes), the
/// product taken in `order`. The one rating conversion from current: the
/// CGMES, UCTE, PowSybl PSS/E and SINCAL readers, the multiconductor to
/// balanced lowering and the OpenDSS source read it.
#[must_use]
pub fn calc_line_apparent_power(
    voltage: f64,
    current: f64,
    factor: f64,
    order: ApparentPowerOrder,
) -> f64 {
    match order {
        ApparentPowerOrder::FactorFirst => factor * voltage * current,
        ApparentPowerOrder::CurrentFirst => current * factor * voltage,
    }
}

/// The balanced current of an apparent power `power` at `voltage`,
/// `S / (factor · V)`: the inverse of [`calc_line_apparent_power`]. The
/// CGMES, UCTE and SINCAL writers and the OpenDSS source and Thevenin
/// writer read it.
#[must_use]
pub fn calc_line_current(power: f64, voltage: f64, factor: f64) -> f64 {
    power / (factor * voltage)
}

/// The quadrature leg `√(m² − r²)` of a magnitude `m` and its in phase
/// part `r`, formed as `√((m − |r|)(m + |r|))`, free of the cancellation of
/// `m² − r²` near `|r| = m`, and as `m √((1 − q)(1 + q))` with
/// `q = |r| / m` where that product overflows: the reactance of a short
/// circuit test from its impedance and
/// resistance, the magnetizing current of a no load test from its exciting
/// current and loss, the reactive power of an apparent power and its active
/// part, and with `m = 1` the sine of a power factor
/// ([`calc_power_factor_sine`]). Zero when both are zero; `NaN` when
/// `|r| > m` or either is `NaN`. The caller states what an excess means for
/// its quantity.
#[must_use]
pub fn calc_quadrature(magnitude: f64, real: f64) -> f64 {
    let real = real.abs();
    let product = (magnitude - real) * (magnitude + real);
    if product.is_finite() {
        return product.sqrt();
    }
    let ratio = real / magnitude;
    magnitude * ((1.0 - ratio) * (1.0 + ratio)).sqrt()
}

/// Which ratio [`calc_impedance_from_ratio`] reads and how it forms the
/// split. The forms can differ in the last place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImpedanceRatioForm {
    /// An `R / X` ratio: `X = |Z| / hypot(R/X, 1)`, `R = |Z| (R/X) / hypot(R/X, 1)`
    /// (the SINCAL transformer, source and rated shunt readers).
    ResistanceOverReactance,
    /// An `X / R` ratio as OpenDSS `VSource` splits a short circuit
    /// impedance: `X = |Z| / √(1 + 1 / (X/R)²)`, `R = X / (X/R)`.
    OpenDssReactanceOverResistance,
}

/// The divisor `|Z| / X` of an impedance with the given ratio, formed as
/// `form` states: `hypot(R/X, 1)` or `√(1 + 1 / (X/R)²)`.
#[must_use]
pub fn calc_impedance_ratio_divisor(ratio: f64, form: ImpedanceRatioForm) -> f64 {
    match form {
        ImpedanceRatioForm::ResistanceOverReactance => ratio.hypot(1.0),
        ImpedanceRatioForm::OpenDssReactanceOverResistance => (1.0 + 1.0 / (ratio * ratio)).sqrt(),
    }
}

/// The resistance and reactance `(R, X)` of an impedance of magnitude
/// `magnitude` with the given `R / X` or `X / R` ratio, split as `form`
/// states over [`calc_impedance_ratio_divisor`]. The one impedance from a
/// magnitude and a ratio: the SINCAL multiconductor transformer, source and
/// rated shunt readers and the OpenDSS source's short circuit impedance
/// read it.
#[must_use]
pub fn calc_impedance_from_ratio(
    magnitude: f64,
    ratio: f64,
    form: ImpedanceRatioForm,
) -> (f64, f64) {
    let divisor = calc_impedance_ratio_divisor(ratio, form);
    match form {
        ImpedanceRatioForm::ResistanceOverReactance => {
            (magnitude * (ratio / divisor), magnitude / divisor)
        }
        ImpedanceRatioForm::OpenDssReactanceOverResistance => {
            let reactance = magnitude / divisor;
            (reactance / ratio, reactance)
        }
    }
}

/// The sine of a power factor's angle, `sin(acos |pf|) = √((1 − |pf|)(1 + |pf|))`,
/// formed without the cancellation of `1 − pf²` near unity: the
/// [`calc_quadrature`] of a unit magnitude. Reactive power
/// is `S` times this for an apparent power `S`. `NaN` when `|pf| > 1`.
#[must_use]
pub fn calc_power_factor_sine(pf: f64) -> f64 {
    calc_quadrature(1.0, pf)
}

/// How [`calc_reactive_ratio`] forms `tan(acos |pf|)`. The forms are the
/// same ratio and can differ in the last place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReactiveRatioForm {
    /// [`calc_power_factor_sine`]` / pf`, free of the cancellation of
    /// `1 − pf²` near unity: every reader and model that follows no
    /// engine's rounding.
    Sine,
    /// `√(1 / pf² − 1)`, negated for `pf < 0`: OpenDSS's `Load`,
    /// `Generator`, `Storage` and `PVSystem`
    /// (`kvarBase := kWBase * Sqrt(1.0 / Sqr(PFNominal) - 1.0)`, DSS C-API
    /// 0.14.5).
    OpenDss,
}

/// The reactive over active power ratio a power factor fixes, OpenDSS and
/// BMOPF by sign: `Q / P = tan(acos |pf|)` with `pf > 0`, Q on the same side
/// as P, and the opposite sign for `pf < 0`, formed as `form` states.
/// Infinite at `pf = ±0` and `NaN` when `|pf| > 1`.
#[must_use]
pub fn calc_reactive_ratio(pf: f64, form: ReactiveRatioForm) -> f64 {
    match form {
        ReactiveRatioForm::Sine => calc_power_factor_sine(pf) / pf,
        ReactiveRatioForm::OpenDss => {
            let ratio = (1.0 / (pf * pf) - 1.0).sqrt();
            if pf < 0.0 { -ratio } else { ratio }
        }
    }
}

/// The derivative of [`calc_reactive_ratio`] with respect to `pf` (either
/// form),
/// `−1 / (pf² √(1 − pf²))` on `0 < |pf| < 1`.
#[must_use]
pub fn calc_reactive_ratio_derivative(pf: f64) -> f64 {
    -1.0 / (pf * pf * calc_power_factor_sine(pf))
}

/// The coefficients `[a, b]` of a fixed power factor written as the row
/// `a P + b Q = 0`: `a = √((1 − |pf|)(1 + |pf|))` and `b = −pf`, so that
/// `Q / P = a / pf` ([`calc_reactive_ratio`]'s [`ReactiveRatioForm::Sine`]) and a tiny nonzero power factor
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

/// One voltage power law term `c x^γ` over the log voltage ratio
/// `ℓ = ln x`, formed as `c e^{γ ℓ}`, with its derivative `γ c e^{γ ℓ}` with
/// respect to `ℓ`: [`calc_voltage_power_term`] on the positive voltage
/// domain a log voltage lift defines, where no floor is needed. The one log
/// voltage term evaluation: the multiconductor OPF load parameter
/// sensitivity over its log voltage coordinate reads it.
#[must_use]
pub fn calc_log_voltage_power_term(coefficient: f64, exponent: f64, log_ratio: f64) -> (f64, f64) {
    let value = coefficient * (exponent * log_ratio).exp();
    (value, exponent * value)
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
    #[allow(clippy::float_cmp)] // each order states its exact spelling
    fn the_rebase_orders_spell_their_operations() {
        let (value, v1, s1, v2, s2) = (0.37, 13.8, 100.0, 12.47, 37.5);
        assert_eq!(
            calc_impedance_rebase(value, v1, s1, v2, s2, RebaseOrder::BaseQuotient),
            value * ((v1 * v1 / s1) / (v2 * v2 / s2))
        );
        assert_eq!(
            calc_impedance_rebase(value, v1, s1, v2, s2, RebaseOrder::PowerRatio),
            value * (s2 / s1) * (v1 / v2).powi(2)
        );
        assert_eq!(
            calc_impedance_rebase(value, v1, s1, v2, s2, RebaseOrder::InversePowerRatio),
            value / (s1 / s2) * (v1 / v2).powi(2)
        );
        assert!(calc_impedance_rebase(value, 0.0, s1, v2, s2, RebaseOrder::BaseQuotient).is_nan());
    }

    #[test]
    #[allow(clippy::float_cmp)] // each conversion states its exact spelling
    fn the_rating_conversions_spell_their_products() {
        assert_eq!(SQRT_3, 3f64.sqrt());
        assert_eq!(calc_rated_admittance(1.2, 12.47), 1.2 / (12.47 * 12.47));
        assert_eq!(calc_rated_power(0.5, 12.47), 0.5 * (12.47 * 12.47));
        assert_eq!(calc_wye_phase_voltage(12_470.0), 12_470.0 / 3f64.sqrt());
        assert_eq!(calc_wye_line_voltage(7_200.0), 7_200.0 * 3f64.sqrt());
        let (kv, amps) = (110.0, 650.0);
        assert_eq!(
            calc_line_apparent_power(kv, amps, SQRT_3, ApparentPowerOrder::FactorFirst),
            3f64.sqrt() * kv * amps
        );
        assert_eq!(
            calc_line_apparent_power(kv, amps / 1000.0, SQRT_3, ApparentPowerOrder::CurrentFirst),
            amps / 1000.0 * 3f64.sqrt() * kv
        );
        assert_eq!(
            calc_line_current(120.0, kv, SQRT_3),
            120.0 / (3f64.sqrt() * kv)
        );
    }

    #[test]
    fn the_quadrature_leg_and_the_ratio_split_close_their_triangles() {
        assert!((calc_quadrature(5.0, 3.0) - 4.0).abs() < 1e-15);
        assert!((calc_quadrature(5.0, -3.0) - 4.0).abs() < 1e-15);
        assert_eq!(calc_quadrature(0.0, 0.0), 0.0);
        assert_eq!(calc_quadrature(2.0, 2.0), 0.0);
        assert!(calc_quadrature(1.0, 1.5).is_nan());
        assert!(calc_quadrature(0.0, 1.0).is_nan());
        assert!((calc_quadrature(1e300, 6e299) - 8e299).abs() < 1e285);
        for form in [
            ImpedanceRatioForm::ResistanceOverReactance,
            ImpedanceRatioForm::OpenDssReactanceOverResistance,
        ] {
            let (r, x) = calc_impedance_from_ratio(5.0, 0.75, form);
            assert!((r.hypot(x) - 5.0).abs() < 1e-14);
            let ratio = match form {
                ImpedanceRatioForm::ResistanceOverReactance => r / x,
                _ => x / r,
            };
            assert!((ratio - 0.75).abs() < 1e-14);
        }
    }

    #[test]
    fn power_factor_relations_agree_with_the_trigonometric_forms() {
        for pf in [0.9_f64, -0.85, 0.3, 1.0, -1.0] {
            let ratio = pf.abs().acos().tan().copysign(pf);
            assert!((calc_reactive_ratio(pf, ReactiveRatioForm::Sine) - ratio).abs() < 1e-12);
            assert!((calc_reactive_ratio(pf, ReactiveRatioForm::OpenDss) - ratio).abs() < 1e-12);
            let [a, b] = calc_power_factor_coefficients(pf);
            assert!((-a / b - ratio).abs() < 1e-12);
        }
        let (pf, h) = (0.8_f64, 1e-6);
        let fd = (calc_reactive_ratio(pf + h, ReactiveRatioForm::Sine)
            - calc_reactive_ratio(pf - h, ReactiveRatioForm::Sine))
            / (2.0 * h);
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
        let (v, d) = calc_log_voltage_power_term(2.0, 1.5, 0.9_f64.ln());
        assert!((v - 2.0 * 0.9_f64.powf(1.5)).abs() < 1e-15);
        assert!((d - 1.5 * v).abs() < 1e-15);
    }
}

//! Voltage dependent load power: the one evaluation of a balanced load's
//! [`LoadVoltageModel`] at a bus voltage magnitude, with its derivative.
//!
//! With `x = vm / vn`, the bus voltage magnitude against the load's nominal
//! voltage (both per unit on the bus base), a load draws
//!
//! ```text
//! P(vm) = p - Σ c_k + Σ c_k x^γ_k        (and Q alike)
//! ```
//!
//! where `p` is the stated [`Load::p`] and each term `c_k x^γ_k` is a ZIP
//! constant current part (`γ = 1`), a ZIP constant impedance part (`γ = 2`),
//! or an exponential side (its `γ`, when not zero). The stated power stays
//! authoritative for everything beyond the terms: the ZIP constant power
//! part, an exponential `γ = 0` side, or a demand edit written onto `p`.

use crate::network::{Load, LoadVoltageModel};

/// Below this ratio `vm / vn` a term with exponent below one is continued by
/// its tangent at the floor ([`LoadVoltageTerm::calc_power`]): `x^γ` has an
/// unbounded slope at zero for `γ < 1` (and is itself unbounded for `γ < 0`),
/// and a Newton iterate can pass near zero on its way to a solution.
pub const LOAD_VOLTAGE_RATIO_FLOOR: f64 = 1e-3;

/// One voltage dependent part of a load: `coefficient · (vm / v_nom)^exponent`
/// in the load's power unit.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct LoadVoltageTerm {
    /// Reactive (`true`) or active (`false`) power.
    pub reactive: bool,
    /// The part's power at the nominal voltage.
    pub coefficient: f64,
    /// The nominal voltage, per unit on the bus base, positive.
    pub v_nom: f64,
    /// The voltage exponent (1 constant current, 2 constant impedance).
    pub exponent: f64,
}

impl LoadVoltageTerm {
    /// A term from its parts.
    #[must_use]
    pub const fn new(reactive: bool, coefficient: f64, v_nom: f64, exponent: f64) -> Self {
        Self {
            reactive,
            coefficient,
            v_nom,
            exponent,
        }
    }

    /// The term's power at magnitude `vm` (per unit) and its derivative with
    /// respect to `vm`: with `x = vm / v_nom`, `(c x^γ, γ c x^(γ−1) / v_nom)`.
    /// Exponents 1 and 2 are evaluated without `powf`. For `γ < 1` and `x`
    /// below [`LOAD_VOLTAGE_RATIO_FLOOR`] the term follows its tangent at the
    /// floor, `c (f₀ + f₀' (x − x₀))`, so value and slope stay finite and
    /// consistent as a voltage approaches zero.
    #[must_use]
    #[allow(clippy::float_cmp)] // the integer exponents are exact spellings
    pub fn calc_power(&self, vm: f64) -> (f64, f64) {
        let (c, g, vn) = (self.coefficient, self.exponent, self.v_nom);
        let x = vm / vn;
        if g == 1.0 {
            (c * x, c / vn)
        } else if g == 2.0 {
            (c * x * x, 2.0 * c * x / vn)
        } else if g < 1.0 && x < LOAD_VOLTAGE_RATIO_FLOOR {
            let f0 = LOAD_VOLTAGE_RATIO_FLOOR.powf(g);
            let d0 = g * LOAD_VOLTAGE_RATIO_FLOOR.powf(g - 1.0);
            (c * (f0 + d0 * (x - LOAD_VOLTAGE_RATIO_FLOOR)), c * d0 / vn)
        } else {
            (c * x.powf(g), c * g * x.powf(g - 1.0) / vn)
        }
    }

    /// Whether `vm` lies below the term's ratio floor: a collapsed voltage
    /// rather than an operating point.
    #[must_use]
    pub fn is_below_floor(&self, vm: f64) -> bool {
        self.exponent != 0.0 && vm / self.v_nom < LOAD_VOLTAGE_RATIO_FLOOR
    }
}

/// Why a load's voltage model cannot be evaluated.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum LoadVoltageError {
    /// A stated power, coefficient, or exponent is not finite.
    NonFiniteCoefficient,
    /// The stated nominal voltage is not positive and finite.
    InvalidNominalVoltage { stated: f64 },
    /// The stated nominal voltage reads as two different usual per unit
    /// values, in kV against the bus base and in per unit.
    AmbiguousNominalVoltage {
        stated: f64,
        base_kv: f64,
        as_kv: f64,
        as_pu: f64,
    },
    /// The stated nominal voltage is plausible neither in kV nor in per unit.
    ImplausibleNominalVoltage { stated: f64, base_kv: f64 },
}

impl std::fmt::Display for LoadVoltageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFiniteCoefficient => write!(f, "states a non-finite coefficient"),
            Self::InvalidNominalVoltage { stated } => {
                write!(f, "states a nominal voltage of {stated}")
            }
            Self::AmbiguousNominalVoltage {
                stated,
                base_kv,
                as_kv,
                as_pu,
            } => write!(
                f,
                "states a nominal voltage of {stated}, which reads as {as_kv:.4} pu in kV \
                 against the bus base of {base_kv} kV and as {as_pu:.4} pu in per unit; state \
                 it unambiguously (or leave it out for the bus base voltage)"
            ),
            Self::ImplausibleNominalVoltage { stated, base_kv } => write!(
                f,
                "states a nominal voltage of {stated}, plausible neither in kV against the bus \
                 base of {base_kv} kV nor in per unit"
            ),
        }
    }
}

impl std::error::Error for LoadVoltageError {}

/// A load's nominal voltage in per unit on its bus base.
///
/// No stated `v_nom` is the bus base voltage (1 per unit), the convention
/// every PowerIO reader writes. The balanced model does not state the unit of
/// a stated one, and sources use both, so it is read against the bus base
/// voltage `base_kv`: as kV giving `v_nom / base_kv`, or as per unit giving
/// `v_nom`. A reading is plausible in `[0.5, 2]` and usual in `[0.8, 1.25]`.
/// When the two readings agree, or only one is plausible, or only one is
/// usual (`v_nom = 0.69` on a 0.69 kV bus is 1 pu in kV, not 0.69 pu), that
/// reading is taken. When both are usual and differ (a 0.9 kV bus with
/// `v_nom = 1`), both are plausible and neither usual, or neither is
/// plausible, the nominal voltage is refused rather than guessed.
///
/// # Errors
/// A non-positive or non-finite `v_nom`, or one no reading makes plausible
/// or two readings make plausible differently.
pub fn calc_load_nominal_voltage(
    v_nom: Option<f64>,
    base_kv: f64,
) -> Result<f64, LoadVoltageError> {
    let Some(stated) = v_nom else {
        return Ok(1.0);
    };
    if !(stated.is_finite() && stated > 0.0) {
        return Err(LoadVoltageError::InvalidNominalVoltage { stated });
    }
    let plausible = |ratio: f64| (0.5..=2.0).contains(&ratio);
    let usual = |ratio: f64| (0.8..=1.25).contains(&ratio);
    let mut as_kv = (base_kv.is_finite() && base_kv > 0.0)
        .then(|| stated / base_kv)
        .filter(|&ratio| plausible(ratio));
    let mut as_pu = plausible(stated).then_some(stated);
    if let (Some(kv), Some(pu)) = (as_kv, as_pu) {
        match (usual(kv), usual(pu)) {
            (true, false) => as_pu = None,
            (false, true) => as_kv = None,
            _ => {}
        }
    }
    match (as_kv, as_pu) {
        (Some(kv), Some(pu)) if (kv - pu).abs() <= 1e-9 * kv.max(pu) => Ok(kv),
        (Some(kv), None) => Ok(kv),
        (None, Some(pu)) => Ok(pu),
        (Some(as_kv), Some(as_pu)) => Err(LoadVoltageError::AmbiguousNominalVoltage {
            stated,
            base_kv,
            as_kv,
            as_pu,
        }),
        (None, None) => Err(LoadVoltageError::ImplausibleNominalVoltage { stated, base_kv }),
    }
}

/// A load's power at a voltage magnitude, in its own unit, with the
/// derivatives with respect to the magnitude.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct LoadPowerAtVoltage {
    pub p: f64,
    pub q: f64,
    pub dp_dvm: f64,
    pub dq_dvm: f64,
}

impl Load {
    /// The voltage dependent terms of this load on a bus of base `base_kv`,
    /// in the load's power unit, or `Ok(None)` for a constant power load.
    /// A ZIP constant power part and an exponential `γ = 0` side add no
    /// term, and neither does a zero coefficient.
    ///
    /// # Errors
    /// A non-finite stated power, coefficient or exponent, or a nominal
    /// voltage [`calc_load_nominal_voltage`] refuses.
    pub fn calc_voltage_terms(
        &self,
        base_kv: f64,
    ) -> Result<Option<Vec<LoadVoltageTerm>>, LoadVoltageError> {
        let finite = |values: &[f64]| {
            if values.iter().all(|value| value.is_finite()) {
                Ok(())
            } else {
                Err(LoadVoltageError::NonFiniteCoefficient)
            }
        };
        match self.voltage_model.as_ref() {
            Some(LoadVoltageModel::Zip {
                p_constant_power,
                q_constant_power,
                p_constant_current,
                q_constant_current,
                p_constant_impedance,
                q_constant_impedance,
                v_nom,
                ..
            }) => {
                finite(&[
                    self.p,
                    self.q,
                    *p_constant_power,
                    *q_constant_power,
                    *p_constant_current,
                    *q_constant_current,
                    *p_constant_impedance,
                    *q_constant_impedance,
                ])?;
                let vn = calc_load_nominal_voltage(*v_nom, base_kv)?;
                Ok(Some(
                    [
                        (false, *p_constant_current, 1.0),
                        (true, *q_constant_current, 1.0),
                        (false, *p_constant_impedance, 2.0),
                        (true, *q_constant_impedance, 2.0),
                    ]
                    .into_iter()
                    .filter(|&(_, c, _)| c != 0.0)
                    .map(|(reactive, c, g)| LoadVoltageTerm::new(reactive, c, vn, g))
                    .collect(),
                ))
            }
            Some(LoadVoltageModel::Exponential {
                p,
                q,
                v_nom,
                gamma_p,
                gamma_q,
            }) => {
                finite(&[self.p, self.q, *p, *q, *gamma_p, *gamma_q])?;
                let vn = calc_load_nominal_voltage(*v_nom, base_kv)?;
                Ok(Some(
                    [(false, *p, *gamma_p), (true, *q, *gamma_q)]
                        .into_iter()
                        .filter(|&(_, c, g)| c != 0.0 && g != 0.0)
                        .map(|(reactive, c, g)| LoadVoltageTerm::new(reactive, c, vn, g))
                        .collect(),
                ))
            }
            _ => Ok(None),
        }
    }

    /// The power this load draws at voltage magnitude `vm` (per unit) on a bus
    /// of base `base_kv`, in its own unit, and the derivatives with respect to
    /// `vm`: the stated power less every term's nominal power, plus every
    /// term at `vm` ([`calc_voltage_terms`](Self::calc_voltage_terms)).
    ///
    /// # Errors
    /// As [`calc_voltage_terms`](Self::calc_voltage_terms).
    pub fn calc_power_at(
        &self,
        vm: f64,
        base_kv: f64,
    ) -> Result<LoadPowerAtVoltage, LoadVoltageError> {
        let mut power = LoadPowerAtVoltage {
            p: self.p,
            q: self.q,
            dp_dvm: 0.0,
            dq_dvm: 0.0,
        };
        for term in self.calc_voltage_terms(base_kv)?.unwrap_or_default() {
            let (value, slope) = term.calc_power(vm);
            if term.reactive {
                power.q += value - term.coefficient;
                power.dq_dvm += slope;
            } else {
                power.p += value - term.coefficient;
                power.dp_dvm += slope;
            }
        }
        Ok(power)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BusId;

    #[test]
    fn nominal_voltage_reads_against_the_bus_base() {
        assert_eq!(calc_load_nominal_voltage(None, 0.4), Ok(1.0));
        assert!((calc_load_nominal_voltage(Some(0.42), 0.4).unwrap() - 1.05).abs() < 1e-12);
        assert_eq!(calc_load_nominal_voltage(Some(1.0), 0.4), Ok(1.0));
        assert_eq!(calc_load_nominal_voltage(Some(0.98), 230.0), Ok(0.98));
        // A per unit nominal on a 12.47 kV bus is 1 pu, not 1/12.47 or 12.47.
        assert_eq!(calc_load_nominal_voltage(Some(1.0), 12.47), Ok(1.0));
        assert!(matches!(
            calc_load_nominal_voltage(Some(1.0), 0.9),
            Err(LoadVoltageError::AmbiguousNominalVoltage { .. })
        ));
        assert!(calc_load_nominal_voltage(Some(11.0), 230.0).is_err());
        assert!(calc_load_nominal_voltage(Some(f64::NAN), 230.0).is_err());
    }

    /// A ZIP load's power at a voltage is its constant power part plus the
    /// current and impedance parts scaled by the voltage ratio.
    #[test]
    fn zip_power_at_a_voltage() {
        let mut load = Load::new(BusId(1), 30.0, 10.0);
        load.voltage_model = Some(LoadVoltageModel::Zip {
            p_constant_power: 12.0,
            q_constant_power: 4.0,
            p_constant_current: 9.0,
            q_constant_current: 3.0,
            p_constant_impedance: 9.0,
            q_constant_impedance: 3.0,
            v_nom: None,
            load_type: None,
            scaling: None,
        });
        let power = load.calc_power_at(0.95, 230.0).unwrap();
        assert!((power.p - (12.0 + 9.0 * 0.95 + 9.0 * 0.95 * 0.95)).abs() < 1e-12);
        assert!((power.q - (4.0 + 3.0 * 0.95 + 3.0 * 0.95 * 0.95)).abs() < 1e-12);
        assert!((power.dp_dvm - (9.0 + 18.0 * 0.95)).abs() < 1e-12);
    }
}

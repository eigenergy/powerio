//! Voltage-dependent load laws, expressed as multipliers of nominal P and Q.

use crate::DistLoadVoltageModel;

impl DistLoadVoltageModel {
    /// Evaluate active/reactive power multipliers at a nonnegative voltage
    /// magnitude in volts. Coefficient and nominal-voltage arrays follow
    /// active load branches, rather than bus conductor order.
    ///
    /// For a bounded ZIP law, let `u = voltage / v_nom`. Below `v_low_pu`,
    /// both factors are `u²`. Between that threshold and `v_min_pu`, the
    /// constant-power/current contributions interpolate current magnitude
    /// from `v_low_pu` to respectively `1/v_min_pu` and `1`; the impedance
    /// term stays `u²`. Above `v_max_pu`, their power factors are respectively
    /// `(u/v_max_pu)²` and `u²/v_max_pu`. Inside the band they are `1` and `u`.
    /// Above the low threshold, a positive cutoff multiplies the result by
    /// `(1 + tanh(500 * (u - cutoff_pu))) / 2`.
    ///
    /// Returns an error for missing coefficients, invalid parameters or
    /// nonfinite evaluation instead of manufacturing a nominal load law.
    #[expect(
        clippy::too_many_lines,
        reason = "keep all branches of the bounded voltage law together for comparison with its documented equations"
    )]
    pub fn power_scale(&self, phase: usize, voltage: f64) -> Result<[f64; 2], String> {
        if !voltage.is_finite() || voltage < 0.0 {
            return Err("load voltage magnitude must be finite and nonnegative".into());
        }
        if matches!(self, Self::ConstantPower { .. }) {
            return Ok([1.0; 2]);
        }
        let nominal = *self
            .v_nom()
            .get(phase)
            .ok_or("missing nominal load voltage")?;
        if !nominal.is_finite() || nominal <= 0.0 {
            return Err("nominal load voltage must be positive and finite".into());
        }
        let voltage_pu = voltage / nominal;
        let at = |v: &[f64]| -> Result<f64, String> {
            v.get(phase)
                .copied()
                .filter(|v| v.is_finite())
                .ok_or_else(|| "missing or nonfinite load coefficient".into())
        };
        let value = match self {
            Self::ConstantPower { .. } => unreachable!(),
            Self::ConstantCurrent { .. } => [voltage_pu; 2],
            Self::ConstantImpedance { .. } => [voltage_pu * voltage_pu; 2],
            Self::Exponential {
                gamma_p, gamma_q, ..
            } => [voltage_pu.powf(at(gamma_p)?), voltage_pu.powf(at(gamma_q)?)],
            Self::Zip {
                alpha_z,
                alpha_i,
                alpha_p,
                beta_z,
                beta_i,
                beta_p,
                ..
            }
            | Self::BoundedZip {
                alpha_z,
                alpha_i,
                alpha_p,
                beta_z,
                beta_i,
                beta_p,
                ..
            } => {
                let active = [at(alpha_z)?, at(alpha_i)?, at(alpha_p)?];
                let reactive = [at(beta_z)?, at(beta_i)?, at(beta_p)?];
                let (impedance_scale, current_scale, power_scale, dropout) =
                    if let Self::BoundedZip {
                        v_low_pu: low,
                        v_min_pu: min,
                        v_max_pu: max,
                        cutoff_pu: cutoff,
                        ..
                    } = self
                    {
                        if [low, min, max, cutoff]
                            .iter()
                            .any(|v| !v.is_finite() || **v < 0.0)
                        {
                            return Err(
                                "load voltage thresholds must be finite and nonnegative".into()
                            );
                        }
                        if voltage_pu <= *low {
                            // Native low-voltage continuation is the full nominal
                            // impedance, even when ZIP coefficients do not sum to one.
                            return if (voltage_pu * voltage_pu).is_finite() {
                                Ok([voltage_pu * voltage_pu; 2])
                            } else {
                                Err("load power evaluation became nonfinite".into())
                            };
                        }
                        let (current_scale, power_scale) = if voltage_pu <= *min {
                            let weight = (voltage_pu - low) / (min - low);
                            (
                                voltage_pu * (low + (1.0 - low) * weight),
                                voltage_pu * (low + (1.0 / min - low) * weight),
                            )
                        } else if voltage_pu > *max {
                            (
                                voltage_pu * voltage_pu / if *max == 0.0 { 1.0 } else { *max },
                                voltage_pu * voltage_pu / if *max == 0.0 { 1.0 } else { max * max },
                            )
                        } else {
                            (voltage_pu, 1.0)
                        };
                        let dropout = if *cutoff > 0.0 {
                            f64::midpoint(1.0, (500.0 * (voltage_pu - cutoff)).tanh())
                        } else {
                            1.0
                        };
                        (voltage_pu * voltage_pu, current_scale, power_scale, dropout)
                    } else {
                        (voltage_pu * voltage_pu, voltage_pu, 1.0, 1.0)
                    };
                [
                    active[0] * impedance_scale
                        + active[1] * current_scale
                        + active[2] * power_scale,
                    reactive[0] * impedance_scale
                        + reactive[1] * current_scale
                        + reactive[2] * power_scale,
                ]
                .map(|v| v * dropout)
            }
        };
        if value.iter().any(|v| !v.is_finite()) {
            Err("load power evaluation became nonfinite".into())
        } else {
            Ok(value)
        }
    }

    /// Nominal law without voltage bounds. Writers use this only with an
    /// explicit diagnostic when the destination cannot carry the full law.
    #[allow(clippy::float_cmp)] // Only exact coefficient identities permit this projection.
    pub(crate) fn without_voltage_bounds(&self) -> Self {
        let Self::BoundedZip {
            v_nom,
            alpha_z,
            alpha_i,
            alpha_p,
            beta_z,
            beta_i,
            beta_p,
            cutoff_pu,
            ..
        } = self
        else {
            return self.clone();
        };
        let all = |xs: &[f64], x| !xs.is_empty() && xs.iter().all(|v| *v == x);
        if *cutoff_pu == 0.0 && all(alpha_z, 0.0) && all(beta_z, 0.0) {
            if all(alpha_i, 0.0) && all(beta_i, 0.0) && all(alpha_p, 1.0) && all(beta_p, 1.0) {
                return Self::ConstantPower {
                    v_nom: v_nom.clone(),
                };
            }
            if all(alpha_p, 0.0) && all(beta_p, 0.0) && all(alpha_i, 1.0) && all(beta_i, 1.0) {
                return Self::ConstantCurrent {
                    v_nom: v_nom.clone(),
                };
            }
        }
        Self::Zip {
            v_nom: v_nom.clone(),
            alpha_z: alpha_z.clone(),
            alpha_i: alpha_i.clone(),
            alpha_p: alpha_p.clone(),
            beta_z: beta_z.clone(),
            beta_i: beta_i.clone(),
            beta_p: beta_p.clone(),
        }
    }
}

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
    /// For a bounded exponential law the middle-band factors are
    /// `u^gamma_p` and `u^gamma_q`. Both factors become `u²` below the low
    /// threshold, `u * (low + (1/min - low) * (u-low)/(min-low))` between
    /// low and min, and `(u/max)²` above max (using `u²` when max is zero).
    /// These continuations need not join the exponential law continuously.
    ///
    /// For bounded constant power, the scale is `(u/min)²` at or below
    /// min, `1` inside the band, and `(u/max)²` above max. An optional
    /// current limit caps this scale at `u * current_limit_pu`.
    ///
    /// Returns an error for missing coefficients, invalid parameters or
    /// nonfinite evaluation instead of manufacturing a nominal load law.
    pub fn power_scale(&self, phase: usize, voltage: f64) -> Result<[f64; 2], String> {
        self.power_scale_where(phase, voltage, [true; 2])
    }

    /// [`power_scale`](Self::power_scale) for only the components `evaluate`
    /// selects, `[active power, reactive power]`. An inactive component is
    /// returned as zero without evaluating its law, so an exponent stated for
    /// a component the load does not draw (a reactive exponent of a purely
    /// active load, say) cannot make the result nonfinite. A caller that
    /// multiplies each factor by its nominal power passes `[p != 0, q != 0]`.
    #[expect(
        clippy::too_many_lines,
        reason = "keep all branches of the bounded voltage law together for comparison with its documented equations"
    )]
    pub fn power_scale_where(
        &self,
        phase: usize,
        voltage: f64,
        evaluate: [bool; 2],
    ) -> Result<[f64; 2], String> {
        if !voltage.is_finite() || voltage < 0.0 {
            return Err("load voltage magnitude must be finite and nonnegative".into());
        }
        if matches!(self, Self::ConstantPower { .. }) {
            return Ok(evaluate.map(|on| if on { 1.0 } else { 0.0 }));
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
        // An inactive component's exponent is never read; zero stands in, so
        // its factor stays finite until it is cleared below.
        let exponent = |v: &[f64], component: usize| -> Result<f64, String> {
            if evaluate[component] { at(v) } else { Ok(0.0) }
        };
        let value = match self {
            Self::ConstantPower { .. } => unreachable!(),
            Self::ConstantCurrent { .. } => [voltage_pu; 2],
            Self::ConstantImpedance { .. } => [voltage_pu * voltage_pu; 2],
            Self::Exponential {
                gamma_p, gamma_q, ..
            } => [
                voltage_pu.powf(exponent(gamma_p, 0)?),
                voltage_pu.powf(exponent(gamma_q, 1)?),
            ],
            Self::BoundedConstantPower {
                v_min_pu,
                v_max_pu,
                current_limit_pu,
                ..
            } => {
                if !v_min_pu.is_finite()
                    || *v_min_pu <= 0.0
                    || !v_max_pu.is_finite()
                    || *v_max_pu < *v_min_pu
                    || current_limit_pu.is_some_and(|v| !v.is_finite() || v <= 0.0)
                {
                    return Err("constant-power bounds and current limit must be positive, finite and ordered".into());
                }
                let scale = if voltage_pu <= *v_min_pu {
                    (voltage_pu / v_min_pu).powi(2)
                } else if voltage_pu > *v_max_pu {
                    (voltage_pu / v_max_pu).powi(2)
                } else {
                    1.0
                };
                [current_limit_pu.map_or(scale, |limit| scale.min(voltage_pu * limit)); 2]
            }
            Self::BoundedExponential {
                gamma_p,
                gamma_q,
                v_low_pu,
                v_min_pu,
                v_max_pu,
                ..
            } => bounded_exponential_scale(
                voltage_pu,
                [exponent(gamma_p, 0)?, exponent(gamma_q, 1)?],
                [*v_low_pu, *v_min_pu, *v_max_pu],
            )?,
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
                                Ok(evaluate.map(
                                    |on| {
                                        if on { voltage_pu * voltage_pu } else { 0.0 }
                                    },
                                ))
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
        let mut value = value;
        for (factor, &on) in value.iter_mut().zip(&evaluate) {
            if !on {
                *factor = 0.0;
            }
        }
        if value.iter().any(|v| !v.is_finite()) {
            Err("load power evaluation became nonfinite".into())
        } else {
            Ok(value)
        }
    }

    /// Nominal law without voltage bounds: the middle-band law, with the
    /// low-voltage transitions, high-voltage continuation and dropout removed.
    /// Any other law is returned unchanged. A caller uses this only with an
    /// explicit diagnostic when the destination cannot carry the full law.
    #[must_use]
    #[allow(clippy::float_cmp)] // Only exact coefficient identities permit this projection.
    pub fn without_voltage_bounds(&self) -> Self {
        if let Self::BoundedConstantPower { v_nom, .. } = self {
            return Self::ConstantPower {
                v_nom: v_nom.clone(),
            };
        }
        if let Self::BoundedExponential {
            v_nom,
            gamma_p,
            gamma_q,
            ..
        } = self
        {
            return Self::Exponential {
                v_nom: v_nom.clone(),
                gamma_p: gamma_p.clone(),
                gamma_q: gamma_q.clone(),
            };
        }
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

fn bounded_exponential_scale(
    u: f64,
    gamma: [f64; 2],
    bounds: [f64; 3],
) -> Result<[f64; 2], String> {
    if bounds.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err("load voltage thresholds must be finite and nonnegative".into());
    }
    let [low, min, max] = bounds;
    // Preserve the native branch order even when thresholds overlap.
    let scale = if u <= low {
        u * u
    } else if u <= min {
        u * (low + (1.0 / min - low) * (u - low) / (min - low))
    } else if u > max {
        u * u / if max == 0.0 { 1.0 } else { max * max }
    } else {
        return Ok(gamma.map(|exponent| u.powf(exponent)));
    };
    Ok([scale; 2])
}

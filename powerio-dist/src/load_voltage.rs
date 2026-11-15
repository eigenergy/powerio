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
        let nominal = self
            .nominal_voltage(phase)
            .ok_or("missing nominal load voltage")?;
        if !nominal.is_finite() || nominal <= 0.0 {
            return Err("nominal load voltage must be positive and finite".into());
        }
        let voltage_pu = voltage / nominal;
        let at = |v: &[f64]| coefficient(v, phase);
        // An inactive component's exponent is never read; zero stands in, so
        // its factor stays finite until it is cleared below.
        let exponent = |v: &[f64], component: usize| -> Result<f64, String> {
            if evaluate[component] { at(v) } else { Ok(0.0) }
        };
        let value = match self {
            Self::ConstantPower { .. } => unreachable!(),
            Self::ConstantCurrent { .. }
            | Self::ConstantImpedance { .. }
            | Self::Exponential { .. }
            | Self::Zip { .. } => {
                let mut value = [0.0; 2];
                for (component, out) in value.iter_mut().enumerate() {
                    if evaluate[component] {
                        let terms = self
                            .component_terms(phase, component == 1)?
                            .ok_or("load law has no power law terms")?;
                        *out =
                            powerio_core::electrical::calc_voltage_power_terms(&terms, voltage_pu)
                                .0;
                    }
                }
                value
            }
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
            } => {
                let gamma = [exponent(gamma_p, 0)?, exponent(gamma_q, 1)?];
                envelope_scale(voltage_pu, [*v_low_pu, *v_min_pu, *v_max_pu])?
                    .unwrap_or_else(|| gamma.map(|exponent| voltage_pu.powf(exponent)))
            }
            Self::BoundedZip {
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
                let (impedance_scale, current_scale, power_scale, dropout) = {
                    let Self::BoundedZip {
                        v_low_pu: low,
                        v_min_pu: min,
                        v_max_pu: max,
                        cutoff_pu: cutoff,
                        ..
                    } = self
                    else {
                        unreachable!()
                    };
                    {
                        if [low, min, max, cutoff]
                            .iter()
                            .any(|v| !v.is_finite() || **v < 0.0)
                        {
                            return Err(
                                "load voltage thresholds must be finite and nonnegative".into()
                            );
                        }
                        let band = envelope_band(voltage_pu, [*low, *min, *max]);
                        if band == EnvelopeBand::Low {
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
                        let (current_scale, power_scale) = match band {
                            EnvelopeBand::Transition(weight) => (
                                voltage_pu * (low + (1.0 - low) * weight),
                                transition_power_scale(voltage_pu, *low, *min, weight),
                            ),
                            EnvelopeBand::High => (
                                voltage_pu * voltage_pu / if *max == 0.0 { 1.0 } else { *max },
                                high_power_scale(voltage_pu, *max),
                            ),
                            _ => (voltage_pu, 1.0),
                        };
                        let dropout = if *cutoff > 0.0 {
                            f64::midpoint(1.0, (500.0 * (voltage_pu - cutoff)).tanh())
                        } else {
                            1.0
                        };
                        (voltage_pu * voltage_pu, current_scale, power_scale, dropout)
                    }
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

    /// The nominal voltage of channel `channel`, volts, `None` when the law
    /// states no entry for it.
    #[must_use]
    pub fn nominal_voltage(&self, channel: usize) -> Option<f64> {
        channel_entry(self.v_nom(), channel)
    }

    /// The law of channel `channel` as voltage power law terms
    /// `[fraction, exponent]`, active then reactive: each contributes
    /// `fraction · nominal power · (|V| / V_nom)^exponent`. Constant power is
    /// `[1, 0]`, constant current `[1, 1]`, constant impedance `[1, 2]`, an
    /// exponential law `[1, γ]` and a ZIP law its three fractions at
    /// exponents 2, 1 and 0. `None` for a voltage bounded law, which is
    /// piecewise. Every coefficient is required, one entry per channel, as the
    /// BMOPF schema states them.
    ///
    /// The one term form of the unbounded laws: [`power_scale`](Self::power_scale)
    /// evaluates these terms with
    /// [`powerio_core::electrical::calc_voltage_power_term`], and the
    /// multiconductor OPF load law states them in per unit.
    ///
    /// # Errors
    /// A missing or nonfinite coefficient.
    pub fn calc_power_terms(&self, channel: usize) -> Result<Option<[Vec<[f64; 2]>; 2]>, String> {
        Ok(self
            .component_terms(channel, false)?
            .zip(self.component_terms(channel, true)?)
            .map(|(active, reactive)| [active, reactive]))
    }

    fn component_terms(
        &self,
        channel: usize,
        reactive: bool,
    ) -> Result<Option<Vec<[f64; 2]>>, String> {
        let at = |v: &[f64]| coefficient(v, channel);
        Ok(Some(match self {
            Self::ConstantPower { .. } => vec![[1.0, 0.0]],
            Self::ConstantCurrent { .. } => vec![[1.0, 1.0]],
            Self::ConstantImpedance { .. } => vec![[1.0, 2.0]],
            Self::Exponential {
                gamma_p, gamma_q, ..
            } => vec![[1.0, at(if reactive { gamma_q } else { gamma_p })?]],
            Self::Zip {
                alpha_z,
                alpha_i,
                alpha_p,
                beta_z,
                beta_i,
                beta_p,
                ..
            } => {
                let [z, i, p] = if reactive {
                    [beta_z, beta_i, beta_p]
                } else {
                    [alpha_z, alpha_i, alpha_p]
                };
                vec![[at(z)?, 2.0], [at(i)?, 1.0], [at(p)?, 0.0]]
            }
            Self::BoundedZip { .. }
            | Self::BoundedExponential { .. }
            | Self::BoundedConstantPower { .. } => return Ok(None),
        }))
    }

    /// [`power_scale_where`](Self::power_scale_where) of this law inside a
    /// voltage envelope `[v_low_pu, v_min_pu, v_max_pu]`, OpenDSS's load
    /// voltage limits: both factors are `u²` (the nominal impedance) at or
    /// below `v_low_pu`, `u * (low + (1/min - low) * (u - low)/(min - low))`
    /// (current magnitude moving linearly to the constant power current at
    /// `v_min_pu`) up to `v_min_pu`, `(u/max)²` above `v_max_pu`, and this
    /// law's own factors between. A bounded exponential law is exactly its
    /// exponential law inside its own envelope; this applies the same bands
    /// to any law, as a solver's envelope option does.
    ///
    /// # Errors
    /// Thresholds that are not finite and nonnegative, or what
    /// [`power_scale_where`](Self::power_scale_where) refuses in the middle
    /// band.
    pub fn power_scale_in_envelope(
        &self,
        phase: usize,
        voltage: f64,
        envelope: [f64; 3],
        evaluate: [bool; 2],
    ) -> Result<[f64; 2], String> {
        if !voltage.is_finite() || voltage < 0.0 {
            return Err("load voltage magnitude must be finite and nonnegative".into());
        }
        let nominal = self
            .nominal_voltage(phase)
            .ok_or("missing nominal load voltage")?;
        if !nominal.is_finite() || nominal <= 0.0 {
            return Err("nominal load voltage must be positive and finite".into());
        }
        match envelope_scale(voltage / nominal, envelope)? {
            Some(scale) => {
                let value = [0, 1].map(|c| if evaluate[c] { scale[c] } else { 0.0 });
                if value.iter().any(|v| !v.is_finite()) {
                    Err("load power evaluation became nonfinite".into())
                } else {
                    Ok(value)
                }
            }
            None => self.power_scale_where(phase, voltage, evaluate),
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

/// A per channel entry: one entry per channel, in channel order, as the
/// BMOPF schema states every load coefficient ("one entry per sub-load").
fn channel_entry(values: &[f64], channel: usize) -> Option<f64> {
    values.get(channel).copied()
}

/// A finite per channel load coefficient ([`channel_entry`]).
fn coefficient(values: &[f64], channel: usize) -> Result<f64, String> {
    channel_entry(values, channel)
        .filter(|v| v.is_finite())
        .ok_or_else(|| "missing or nonfinite load coefficient".into())
}

/// Where a per unit voltage falls in a load voltage envelope
/// `[low, min, max]`, in the native branch order (so overlapping thresholds
/// keep it): at or below `low`, up to `min` (with the transition weight
/// `(u - low)/(min - low)`), above `max`, or in the middle band.
#[derive(Clone, Copy, Debug, PartialEq)]
enum EnvelopeBand {
    Low,
    Transition(f64),
    High,
    Middle,
}

fn envelope_band(u: f64, [low, min, max]: [f64; 3]) -> EnvelopeBand {
    if u <= low {
        EnvelopeBand::Low
    } else if u <= min {
        EnvelopeBand::Transition((u - low) / (min - low))
    } else if u > max {
        EnvelopeBand::High
    } else {
        EnvelopeBand::Middle
    }
}

/// The constant power factor between `low` and `min`: current magnitude
/// moving linearly from the nominal impedance's to `1/min`.
fn transition_power_scale(u: f64, low: f64, min: f64, weight: f64) -> f64 {
    u * (low + (1.0 / min - low) * weight)
}

/// The constant power factor above `max`: `(u/max)²`, `u²` for a zero `max`.
fn high_power_scale(u: f64, max: f64) -> f64 {
    u * u / if max == 0.0 { 1.0 } else { max * max }
}

/// Both factors of a law inside the envelope's outer bands, or `None` in the
/// middle band, where the law's own factors hold.
fn envelope_scale(u: f64, bounds: [f64; 3]) -> Result<Option<[f64; 2]>, String> {
    if bounds.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err("load voltage thresholds must be finite and nonnegative".into());
    }
    let [low, min, max] = bounds;
    Ok(match envelope_band(u, bounds) {
        EnvelopeBand::Low => Some([u * u; 2]),
        EnvelopeBand::Transition(weight) => Some([transition_power_scale(u, low, min, weight); 2]),
        EnvelopeBand::High => Some([high_power_scale(u, max); 2]),
        EnvelopeBand::Middle => None,
    })
}

/// The nominal admittance of a load branch drawing `power` at `v_nom`:
/// `conj(S) / V²`, siemens, the constant impedance law's admittance and the
/// current `Y U` every law reduces to at or below its low voltage
/// threshold.
#[must_use]
pub fn calc_load_nominal_admittance(
    power: num_complex::Complex64,
    v_nom: f64,
) -> num_complex::Complex64 {
    power.conj() / (v_nom * v_nom)
}

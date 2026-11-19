//! The thermal limit an OPF instance carries for one branch.
//!
//! Both OPF builders synthesize a bound for an unrated branch from
//! [`Branch::synthesize_rate_a`](powerio_tx::Branch::synthesize_rate_a), read the
//! window out of the same two fields, and apply the same unrated test, so the
//! rule lives here once.

/// The widest angle difference a branch may hold, in radians, from its
/// converted `angmin`/`angmax` pair.
///
/// `angmin == angmax == 0` states no constraint, the MATPOWER spelling that
/// `BalancedNetwork::to_normalized` widens to a pad, so a raw case and a normalized
/// one describe the same branch. Reading that pair as a zero wide window would
/// give a zero synthesized limit, which the instance then reads back as
/// unlimited. A window wider than the half turn is held at `pi` by
/// `synthesize_rate_a` itself.
pub(crate) fn angle_window(angle_min_rad: f64, angle_max_rad: f64) -> f64 {
    if angle_min_rad == 0.0 && angle_max_rad == 0.0 {
        return std::f64::consts::PI;
    }
    angle_min_rad.abs().max(angle_max_rad.abs())
}

/// Which angle difference interval a synthesized thermal limit
/// ([`Branch::synthesize_rate_a`](powerio_tx::Branch::synthesize_rate_a))
/// spans for a branch the source left unrated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SynthesizedLimitWindow {
    /// The source's stated `angmin`/`angmax`, the window every release
    /// before this option used.
    #[default]
    Stated,
    /// The angle difference bounds the preparation selects and carries in
    /// `angle_min`/`angle_max`. PowerModels pads the bounds before
    /// `calc_thermal_limits!`, so with
    /// [`AngleDifferenceBounds::PowerModelsPad`](powerio_tx::AngleDifferenceBounds::PowerModelsPad)
    /// this reproduces its limit for an unconstrained branch.
    SelectedBounds,
}

impl SynthesizedLimitWindow {
    /// Whether this is [`Self::Stated`], the default.
    #[must_use]
    pub const fn is_stated(&self) -> bool {
        matches!(self, Self::Stated)
    }
}

/// The rule an OPF builder applies to every branch's thermal bound.
///
/// The three fields are settled once per instance, so a builder holds one of
/// these across its branch loop.
pub(crate) struct ThermalLimits {
    /// Whether a branch the source left unrated gets a synthesized bound.
    pub synthesize_unrated: bool,
    /// Which angle difference interval a synthesized bound spans.
    pub window: SynthesizedLimitWindow,
    /// Multiplier that puts a stated `rate_a` in the selected unit system.
    pub power_scale: f64,
    /// Multiplier that puts an admittance in the selected unit system.
    pub admittance_scale: f64,
}

impl ThermalLimits {
    /// The bound for one branch.
    ///
    /// A branch the source left unlimited may get a synthesized bound. A
    /// stated finite zero is never synthesized. That bound is per unit power
    /// already, so it takes `admittance_scale` where a stated `rate_a` takes
    /// `power_scale`. `stated` and `selected` are the branch's source angle
    /// difference interval and the one the preparation carries, radians; the
    /// synthesized bound spans the one `window` names.
    pub(crate) fn of(
        &self,
        branch: &powerio_tx::Branch,
        stated: (f64, f64),
        selected: (f64, f64),
        from: &powerio_tx::Bus,
        to: &powerio_tx::Bus,
    ) -> crate::Result<f64> {
        let (angle_min_rad, angle_max_rad) = match self.window {
            SynthesizedLimitWindow::Stated => stated,
            SynthesizedLimitWindow::SelectedBounds => selected,
        };
        let limit = branch.rate_a;
        if limit.is_nan() || limit < 0.0 {
            return Err(powerio_tx::Error::InvalidThermalLimit { magnitude: limit }.into());
        }
        let (value, scale) = if self.synthesize_unrated && limit == f64::INFINITY {
            let window = angle_window(angle_min_rad, angle_max_rad);
            let value =
                branch.synthesize_rate_a(window, (from.vmin, from.vmax), (to.vmin, to.vmax));
            (value, self.admittance_scale)
        } else {
            (limit, self.power_scale)
        };
        let result = value * scale;
        if !scale.is_finite()
            || scale <= 0.0
            || result.is_nan()
            || result < 0.0
            || (value.is_finite() && !result.is_finite())
            || (value > 0.0 && result == 0.0)
        {
            return Err(powerio_tx::Error::InvalidThermalLimit { magnitude: result }.into());
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::angle_window;

    #[test]
    fn the_window_is_the_wider_of_the_two_magnitudes() {
        let window = 30.0_f64.to_radians();
        assert!((angle_window(-window, window) - window).abs() < 1e-15);
        assert!((angle_window(0.1, window) - window).abs() < 1e-15);
        assert!((angle_window(-window, 0.1) - window).abs() < 1e-15);
    }

    #[test]
    fn a_zero_pair_states_no_constraint() {
        assert!((angle_window(0.0, 0.0) - std::f64::consts::PI).abs() < 1e-15);
    }
}

//! The branch angle window the synthesized thermal limit reads.
//!
//! Both OPF builders synthesize a bound for an unrated branch from
//! [`Branch::synthesize_rate_a`](powerio::Branch::synthesize_rate_a), and both
//! read the window out of the same two fields, so the rule lives here once.

/// The widest angle difference a branch may hold, in radians, from its
/// converted `angmin`/`angmax` pair.
///
/// `angmin == angmax == 0` states no constraint, the MATPOWER spelling that
/// `Network::to_normalized` widens to a pad, so a raw case and a normalized
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

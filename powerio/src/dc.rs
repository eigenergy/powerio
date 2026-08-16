//! Conventions shared by DC network models and matrix builders.

use serde::{Deserialize, Serialize};

/// Electrical convention used for DC branch coefficients.
///
/// Every variant states the series susceptance `b`, which is negative for an
/// inductive branch. This is the sign PowerModels `calc_branch_y` gives, and
/// the caller that assembles a matrix negates it: powerio's bus susceptance
/// matrix takes the M-matrix form, with negative off diagonal entries and
/// positive diagonals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DcConvention {
    /// Use `b = -1/x` and ignore transformer taps and phase shifts.
    #[deprecated(
        since = "0.9.0",
        note = "use `SeriesImpedance`, which is the same rule with resistance included, or `Matpower`; removed in 1.0.0"
    )]
    PaperPure,
    /// Use `b = -1/(x tau)` and include phase shift injections, matching
    /// MATPOWER `makeBdc`.
    Matpower,
    /// Use `b = -x/(r² + x²)` and include phase shift injections.
    ///
    /// This is `Im(1/(r + jx))`, the branch's own series susceptance, so it
    /// reads the whole series impedance and not the reactance alone. It
    /// reduces to `-1/x` when the resistance is zero. A transformer tap does
    /// not scale it. Equivalent to PowerModels `calc_branch_y`, whose DC
    /// formulation uses the same quantity.
    #[default]
    SeriesImpedance,
}

impl DcConvention {
    /// The series susceptance `b` from resistance, reactance, and effective
    /// tap. Negative for an inductive branch. Only [`Self::Matpower`] reads
    /// the tap, and only [`Self::SeriesImpedance`] reads the resistance.
    #[must_use]
    #[allow(deprecated)]
    pub fn series_susceptance(self, resistance: f64, reactance: f64, effective_tap: f64) -> f64 {
        match self {
            Self::PaperPure => -1.0 / reactance,
            Self::Matpower => -1.0 / (reactance * effective_tap),
            Self::SeriesImpedance => -reactance / (resistance * resistance + reactance * reactance),
        }
    }

    /// Whether phase shifts contribute to the nodal injection vector.
    #[must_use]
    #[allow(deprecated)]
    pub fn includes_phase_shifts(self) -> bool {
        match self {
            Self::PaperPure => false,
            Self::Matpower | Self::SeriesImpedance => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resistanceless branch reads the same under both live conventions, so
    /// the new default only moves a case that carries resistance.
    #[test]
    fn series_impedance_reduces_to_minus_one_over_x() {
        let b = DcConvention::SeriesImpedance.series_susceptance(0.0, 0.25, 1.0);
        assert!((b + 4.0).abs() < 1e-12);
    }

    /// `b = Im(1/(r + jx))` is negative for an inductive branch, the sign
    /// PowerModels `calc_branch_y` gives.
    #[test]
    fn series_impedance_is_negative_for_an_inductive_branch() {
        let b = DcConvention::SeriesImpedance.series_susceptance(0.01, 0.1, 1.0);
        assert!(b < 0.0, "b = {b}");
        assert!((b + 0.1 / (0.0001 + 0.01)).abs() < 1e-12);
    }

    /// Every live convention agrees on the sign, so a caller that negates once
    /// cannot pick up a matrix of the wrong sign from the choice of variant.
    #[test]
    fn every_convention_agrees_on_the_sign() {
        #[allow(deprecated)]
        let all = [
            DcConvention::PaperPure,
            DcConvention::Matpower,
            DcConvention::SeriesImpedance,
        ];
        for convention in all {
            let b = convention.series_susceptance(0.01, 0.1, 1.0);
            assert!(b < 0.0, "{convention:?} gave {b}");
        }
    }

    /// Resistance moves the susceptance toward zero, by more as `r` grows
    /// against `x`.
    #[test]
    fn resistance_lowers_the_susceptance_magnitude() {
        let lossless = DcConvention::SeriesImpedance.series_susceptance(0.0, 0.1, 1.0);
        let lossy = DcConvention::SeriesImpedance.series_susceptance(0.1, 0.1, 1.0);
        assert!(lossy.abs() < lossless.abs());
        assert!((lossy + 5.0).abs() < 1e-12);
    }

    #[test]
    fn matpower_scales_by_the_tap() {
        let b = DcConvention::Matpower.series_susceptance(0.01, 0.2, 2.0);
        assert!((b + 2.5).abs() < 1e-12);
    }
}

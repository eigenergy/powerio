//! Conventions shared by DC network models and matrix builders.

use serde::{Deserialize, Serialize};

/// Rule for the DC branch susceptance `b`.
///
/// `b` is positive for an inductive branch, the DC model convention MATPOWER
/// `makeBdc` uses. It is the edge weight of the bus susceptance matrix and the
/// coefficient in `f = b (theta_from - theta_to)`. The AC series susceptance
/// `Im(1/(r + jx))` is its negation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DcConvention {
    /// `b = 1/x`, ignoring transformer taps and phase shifts.
    #[deprecated(
        since = "0.9.0",
        note = "use `SeriesImpedance`, which is the same rule with resistance included, or `Matpower`; removed in 1.0.0"
    )]
    PaperPure,
    /// `b = 1/(x tau)` with phase shift injections, matching MATPOWER
    /// `makeBdc`.
    Matpower,
    /// `b = x/(r² + x²)` with phase shift injections.
    ///
    /// Reads the whole series impedance and not the reactance alone, so it
    /// describes a branch with a real r/x ratio. A transformer tap does not
    /// scale it, and it reduces to `1/x` when the resistance is zero.
    /// PowerModels' DC formulation uses the same quantity.
    #[default]
    SeriesImpedance,
}

impl DcConvention {
    /// The branch susceptance from resistance, reactance, and effective tap.
    /// Only [`Self::Matpower`] reads the tap, and only
    /// [`Self::SeriesImpedance`] reads the resistance.
    #[must_use]
    #[allow(deprecated)]
    pub fn branch_susceptance(self, resistance: f64, reactance: f64, effective_tap: f64) -> f64 {
        match self {
            Self::PaperPure => 1.0 / reactance,
            Self::Matpower => 1.0 / (reactance * effective_tap),
            Self::SeriesImpedance => reactance / (resistance * resistance + reactance * reactance),
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
    fn series_impedance_reduces_to_one_over_x() {
        let b = DcConvention::SeriesImpedance.branch_susceptance(0.0, 0.25, 1.0);
        assert!((b - 4.0).abs() < 1e-12);
    }

    /// Resistance lowers the susceptance, by more as `r` grows against `x`.
    #[test]
    fn resistance_lowers_the_susceptance() {
        let lossless = DcConvention::SeriesImpedance.branch_susceptance(0.0, 0.1, 1.0);
        let lossy = DcConvention::SeriesImpedance.branch_susceptance(0.1, 0.1, 1.0);
        assert!(lossy < lossless);
        assert!((lossy - 5.0).abs() < 1e-12);
    }

    #[test]
    fn matpower_scales_by_the_tap() {
        let b = DcConvention::Matpower.branch_susceptance(0.01, 0.2, 2.0);
        assert!((b - 2.5).abs() < 1e-12);
    }
}

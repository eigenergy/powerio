//! Conventions shared by DC network models and matrix builders.

use serde::{Deserialize, Serialize};

/// The magnitude below which a reactance, an impedance, or a tap ratio stops
/// being a number a builder can divide by.
///
/// It is `f64::MIN_POSITIVE.sqrt()`: the square of anything smaller underflows
/// to zero, and the reciprocal is above 1e153, which annihilates every real
/// branch sharing its diagonal. Each builder compares a magnitude against it —
/// `|x|`, `hypot(r, x)`, the tap — never `r² + x²`, which is a square. Per unit
/// reactances run from 1e-6 to 10, so it rejects poison and nothing else.
pub const MIN_DIVISIBLE_MAGNITUDE: f64 = 1.491_668_146_240_041_3e-154;

/// The series admittance `(g, b) = (r - jx)/(r² + x²)` of an impedance, with no
/// bound applied: the caller has already decided the impedance is one to divide
/// by, and [`series_admittance_of`](crate::series_admittance_of) is the guarded
/// entry point.
///
/// `r² + x²` is not formed directly. It overflows to infinity for an impedance
/// magnitude past about 1e154 — an admittance around 1e-154, which is perfectly
/// representable — and the quotient would then read as an exact zero, dropping
/// the branch from the DC network with nothing to say it happened. Dividing by
/// the larger term first keeps both squares inside `[0, 1]`. Below that
/// magnitude the two forms agree bit for bit, so the direct one still runs.
pub(crate) fn series_admittance_parts(r: f64, x: f64) -> (f64, f64) {
    let denom = r * r + x * x;
    if denom.is_finite() {
        return (r / denom, -x / denom);
    }
    let scale = r.abs().max(x.abs());
    let (r, x) = (r / scale, x / scale);
    let denom = (r * r + x * x) * scale;
    (r / denom, -x / denom)
}

/// Rule for the DC branch susceptance `b`.
///
/// `b` is positive for an inductive branch, the DC model convention MATPOWER
/// `makeBdc` uses. It is the edge weight of the bus susceptance matrix and the
/// coefficient in `f = b (theta_from - theta_to)`. The AC series susceptance
/// `Im(1/(r + jx))` is its negation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DcConvention {
    /// `b = 1/x`, ignoring resistance, transformer taps, and phase shifts.
    ///
    /// The textbook DC linearization, which a paper reproducing a published
    /// result needs exactly as written.
    ReactanceOnly,
    /// `b = 1/(x tau)` with phase shift injections, matching MATPOWER
    /// `makeBdc`.
    Matpower,
    /// `b = x/(r² + x²)` with phase shift injections.
    ///
    /// Reads the whole series impedance, so it describes a branch with a real
    /// r/x ratio. A transformer tap does not
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
    pub fn branch_susceptance(self, resistance: f64, reactance: f64, effective_tap: f64) -> f64 {
        match self {
            Self::ReactanceOnly => 1.0 / reactance,
            Self::Matpower => 1.0 / (reactance * effective_tap),
            Self::SeriesImpedance => -series_admittance_parts(resistance, reactance).1,
        }
    }

    /// Whether phase shifts contribute to the nodal injection vector.
    #[must_use]
    pub fn includes_phase_shifts(self) -> bool {
        match self {
            Self::ReactanceOnly => false,
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

    /// An impedance well inside [`MIN_DIVISIBLE_MAGNITUDE`] whose *square* is
    /// not: `r² + x²` overflows past about 1e154 and the quotient reads as an
    /// exact zero, which drops the branch from the DC network with nothing to
    /// say so. The bound is on the magnitude precisely so the square never
    /// decides.
    #[test]
    fn an_impedance_whose_square_overflows_still_has_a_susceptance() {
        let (r, x) = (1e160, 1e160);
        assert!(r * r + x * x == f64::INFINITY, "the direct form overflows");

        let b = DcConvention::SeriesImpedance.branch_susceptance(r, x, 1.0);
        // b = x/(r² + x²) = 1/(2 · 1e160).
        assert!(
            (b / 5e-161 - 1.0).abs() < 1e-12,
            "the branch is not dropped, got {b}"
        );

        let (g, susceptance) = series_admittance_parts(r, x);
        assert!((g / 5e-161 - 1.0).abs() < 1e-12, "got {g}");
        assert!(
            (susceptance + b).abs() < 1e-175,
            "the DC rule is its negation"
        );
    }

    /// Below the overflow the scaled form is never reached, so every ordinary
    /// branch keeps the exact bits the direct quotient produced.
    #[test]
    fn the_ordinary_range_is_bit_identical_to_the_direct_quotient() {
        for (r, x) in [
            (0.01, 0.1),
            (0.03, 0.04),
            (0.0, 0.25),
            (1e-6, 1e-5),
            (7.0, 3.0),
        ] {
            let denom = r * r + x * x;
            assert_eq!(series_admittance_parts(r, x), (r / denom, -x / denom));
        }
    }
}

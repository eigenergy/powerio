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
/// The public `b` follows PowerModels: it is negative for an inductive
/// branch, the imaginary part of the series admittance the selected formula
/// models. The positive edge weight a sparse factorization uses is its
/// negation, [`solver_edge_weight`](Self::solver_edge_weight).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DcConvention {
    /// `b = -1/x`, ignoring resistance, transformer taps, and phase shifts.
    ///
    /// The textbook DC linearization, which a paper reproducing a published
    /// result needs exactly as written.
    ReactanceOnly,
    /// `b = -1/(x tau)` with phase shift injections, matching MATPOWER
    /// `makeBdc` up to MATPOWER's own sign spelling.
    ///
    /// Serialized under its own name; `Matpower`, the stored 0.9 spelling,
    /// is still read.
    #[serde(alias = "Matpower")]
    TapAdjustedReactance,
    /// `b = imag(inv(r + jx)) = -x/(r² + x²)` with phase shift injections.
    ///
    /// Reads the whole series impedance, so it describes a branch with a real
    /// r/x ratio. A transformer tap does not scale it, and it reduces to
    /// `-1/x` when the resistance is zero. This is PowerModels' DC branch
    /// susceptance exactly.
    ///
    /// Serialized under its own name; `SeriesImpedance`, the stored 0.9
    /// spelling, is still read.
    #[default]
    #[serde(alias = "SeriesImpedance")]
    SeriesSusceptance,
}

impl DcConvention {
    /// The public branch susceptance, in PowerModels signs: the imaginary
    /// part of the series admittance the selected formula models, negative
    /// for an inductive branch. Only [`Self::TapAdjustedReactance`] reads the
    /// tap, and only [`Self::SeriesSusceptance`] reads the resistance; a
    /// value the selected formula never reads cannot reject a branch.
    ///
    /// Non-finite in, non-finite out. The reciprocal rules need the guard
    /// because `1/±inf` is a finite `0.0`: a branch Y_bus rejects outright
    /// would otherwise join the DC system as a zero-weight edge with nothing
    /// to report it.
    #[must_use]
    pub fn branch_susceptance(self, resistance: f64, reactance: f64, effective_tap: f64) -> f64 {
        // Guard the denominator, not its factors: `x * tap` can overflow to
        // infinity from two finite factors and reach the same silent zero.
        let negated_reciprocal = |denominator: f64| {
            if denominator.is_finite() {
                -1.0 / denominator
            } else {
                f64::NAN
            }
        };
        match self {
            Self::ReactanceOnly => negated_reciprocal(reactance),
            Self::TapAdjustedReactance => negated_reciprocal(reactance * effective_tap),
            Self::SeriesSusceptance => series_admittance_parts(resistance, reactance).1,
        }
    }

    /// The internal positive factor weight of the same branch: the edge
    /// weight of the positive semidefinite DC Laplacian a sparse Cholesky
    /// solver factors, which is the negation of
    /// [`branch_susceptance`](Self::branch_susceptance). Public results carry
    /// PowerModels signs; a solver path fills its factor from this weight and
    /// converts sign only while writing a caller's output.
    #[must_use]
    pub fn solver_edge_weight(self, resistance: f64, reactance: f64, effective_tap: f64) -> f64 {
        -self.branch_susceptance(resistance, reactance, effective_tap)
    }

    /// Whether the selected formula reads the transformer tap, and so whether
    /// the tap can bound or reject a branch.
    #[must_use]
    pub fn reads_tap(self) -> bool {
        matches!(self, Self::TapAdjustedReactance)
    }

    /// Whether phase shifts contribute to the nodal injection vector.
    #[must_use]
    pub fn includes_phase_shifts(self) -> bool {
        match self {
            Self::ReactanceOnly => false,
            Self::TapAdjustedReactance | Self::SeriesSusceptance => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Public values carry PowerModels signs: negative for an inductive
    /// branch, `imag(inv(r + jx))` exactly. A resistanceless branch reads the
    /// same under both live conventions, so the default only moves a case
    /// that carries resistance.
    #[test]
    fn series_susceptance_reduces_to_negated_one_over_x() {
        let b = DcConvention::SeriesSusceptance.branch_susceptance(0.0, 0.25, 1.0);
        assert!((b + 4.0).abs() < 1e-12);
        // The internal factor weight is its negation.
        let weight = DcConvention::SeriesSusceptance.solver_edge_weight(0.0, 0.25, 1.0);
        assert!((weight - 4.0).abs() < 1e-12);
    }

    /// Resistance lowers the susceptance magnitude, by more as `r` grows
    /// against `x`.
    #[test]
    fn resistance_lowers_the_susceptance_magnitude() {
        let lossless = DcConvention::SeriesSusceptance.branch_susceptance(0.0, 0.1, 1.0);
        let lossy = DcConvention::SeriesSusceptance.branch_susceptance(0.1, 0.1, 1.0);
        assert!(lossy.abs() < lossless.abs());
        assert!((lossy + 5.0).abs() < 1e-12);
    }

    #[test]
    fn matpower_scales_by_the_tap() {
        let b = DcConvention::TapAdjustedReactance.branch_susceptance(0.01, 0.2, 2.0);
        assert!((b + 2.5).abs() < 1e-12);
    }

    /// Only the tap-reading formula can be rejected by a tap: the other
    /// formulas never read the value (#324).
    #[test]
    fn an_unread_tap_never_rejects_a_branch() {
        for conv in [DcConvention::ReactanceOnly, DcConvention::SeriesSusceptance] {
            assert!(!conv.reads_tap());
            let b = conv.branch_susceptance(0.01, 0.1, 1e-200);
            assert!(b.is_finite(), "{conv:?} read the tap it never divides by");
        }
        assert!(DcConvention::TapAdjustedReactance.reads_tap());
    }

    /// `1/±inf` is `0.0`, which is finite, so a branch the Y_bus builder rejects
    /// outright would enter the DC Laplacian as a zero-weight edge instead. The
    /// tap divides the same denominator, so two finite factors whose product
    /// overflows collapse the same way.
    #[test]
    fn a_non_finite_denominator_is_not_a_susceptance() {
        for x in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            for conv in [
                DcConvention::ReactanceOnly,
                DcConvention::TapAdjustedReactance,
                DcConvention::SeriesSusceptance,
            ] {
                let b = conv.branch_susceptance(0.01, x, 1.0);
                assert!(!b.is_finite(), "{conv:?} read x = {x} as b = {b}");
            }
        }
        for (x, tap) in [
            (0.1, f64::INFINITY),
            (0.1, f64::NAN),
            (1e300, 1e300),
            (1e300, -1e300),
        ] {
            let b = DcConvention::TapAdjustedReactance.branch_susceptance(0.0, x, tap);
            assert!(!b.is_finite(), "x = {x}, tap = {tap} read as b = {b}");
        }
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

        let b = DcConvention::SeriesSusceptance.branch_susceptance(r, x, 1.0);
        // b = -x/(r² + x²) = -1/(2 · 1e160).
        assert!(
            (b / -5e-161 - 1.0).abs() < 1e-12,
            "the branch is not dropped, got {b}"
        );

        let (g, susceptance) = series_admittance_parts(r, x);
        assert!((g / 5e-161 - 1.0).abs() < 1e-12, "got {g}");
        assert!(
            (susceptance - b).abs() < 1e-175,
            "the public rule is the series susceptance itself"
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

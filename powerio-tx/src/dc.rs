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
/// by, and [`calc_series_admittance_of`](crate::calc_series_admittance_of) is the
/// entry point that checks it.
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
/// negation, [`calc_solver_edge_weight`](Self::calc_solver_edge_weight).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BranchSusceptanceFormula {
    /// `b = -1/x`, ignoring resistance, transformer taps, and phase shifts.
    ///
    /// The textbook DC linearization, which a paper reproducing a published
    /// result needs exactly as written.
    ReactanceOnly,
    /// `b = -1/(x tau)` with phase shift injections, matching MATPOWER
    /// `makeBdc` up to MATPOWER's own sign spelling.
    TapAdjustedReactance,
    /// `b = imag(inv(r + jx)) = -x/(r² + x²)` with phase shift injections.
    ///
    /// Reads the whole series impedance, so it describes a branch with a real
    /// r/x ratio. A transformer tap does not scale it, and it reduces to
    /// `-1/x` when the resistance is zero. The coefficient matches PowerModels DCP; its phase-shift
    /// treatment does not. Use `SeriesSusceptanceNoPhaseShift` for DCP.
    #[default]
    SeriesSusceptance,
    /// `b = -x/((r² + x²) tau)` with phase shift injections (ignore conductance).
    TapAdjustedSeriesSusceptance,
    /// Series susceptance, ignoring taps and phase shifts, matching PowerModels
    /// DCP on the same branch orientation and impedance reference side.
    /// PowerModels may reverse antiparallel transformers during parsing, scaling
    /// their impedance by the original tap squared; this tap-ignoring coefficient
    /// then changes. The two tap-adjusted conventions are invariant under that
    /// complete reversal transformation.
    SeriesSusceptanceNoPhaseShift,
}

impl BranchSusceptanceFormula {
    /// The public branch susceptance, in PowerModels signs: the imaginary
    /// part of the series admittance the selected formula models, negative
    /// for an inductive branch. The dependency helpers
    /// state which input values can reject a branch.
    ///
    /// Non-finite in, non-finite out. The reciprocal rules need the guard
    /// because `1/±inf` is a finite `0.0`: a branch Y_bus rejects outright
    /// would otherwise join the DC system as a zero-weight edge with nothing
    /// to report it.
    #[must_use]
    pub fn calc_branch_susceptance(
        self,
        resistance: f64,
        reactance: f64,
        effective_tap: f64,
    ) -> f64 {
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
            Self::SeriesSusceptance | Self::SeriesSusceptanceNoPhaseShift => {
                series_admittance_parts(resistance, reactance).1
            }
            Self::TapAdjustedSeriesSusceptance => {
                if !effective_tap.is_finite() {
                    return f64::NAN;
                }
                series_admittance_parts(resistance, reactance).1 / effective_tap
            }
        }
    }

    /// The internal positive factor weight of the same branch: the edge
    /// weight of the positive semidefinite DC Laplacian a sparse Cholesky
    /// solver factors, which is the negation of
    /// [`calc_branch_susceptance`](Self::calc_branch_susceptance). Public results carry
    /// PowerModels signs; a solver path fills its factor from this weight and
    /// converts sign only while writing a caller's output.
    #[must_use]
    pub fn calc_solver_edge_weight(
        self,
        resistance: f64,
        reactance: f64,
        effective_tap: f64,
    ) -> f64 {
        -self.calc_branch_susceptance(resistance, reactance, effective_tap)
    }

    /// The derivative of [`calc_branch_susceptance`](Self::calc_branch_susceptance)
    /// with respect to the branch's series susceptance `b_s = -x/(r² + x²)`,
    /// holding the series conductance `g = r/(r² + x²)` and the tap fixed.
    ///
    /// The series formulas are linear in `b_s`: `1` and, tap adjusted,
    /// `1/tap`. The reciprocal reactance formulas read `x = -b_s/(g² + b_s²)`,
    /// so `d(-1/x)/db_s = 1 - (g/b_s)² = 1 - (r/x)²`, divided by the tap for
    /// [`Self::TapAdjustedReactance`]. A sensitivity that perturbs a branch's
    /// series susceptance multiplies by this factor to move the DC weight the
    /// formula assembles.
    ///
    /// Non-finite in, non-finite out, as the susceptance itself.
    #[must_use]
    pub fn calc_susceptance_sensitivity_to_series_susceptance(
        self,
        resistance: f64,
        reactance: f64,
        effective_tap: f64,
    ) -> f64 {
        let reciprocal_reactance = || {
            let ratio = resistance / reactance;
            1.0 - ratio * ratio
        };
        match self {
            Self::SeriesSusceptance | Self::SeriesSusceptanceNoPhaseShift => {
                if resistance.is_finite() && reactance.is_finite() {
                    1.0
                } else {
                    f64::NAN
                }
            }
            Self::TapAdjustedSeriesSusceptance => {
                if resistance.is_finite() && reactance.is_finite() && effective_tap.is_finite() {
                    1.0 / effective_tap
                } else {
                    f64::NAN
                }
            }
            Self::ReactanceOnly => reciprocal_reactance(),
            Self::TapAdjustedReactance => reciprocal_reactance() / effective_tap,
        }
    }

    /// Whether the selected formula reads the transformer tap, and so whether
    /// the tap can bound or reject a branch.
    #[must_use]
    pub fn reads_tap(self) -> bool {
        matches!(
            self,
            Self::TapAdjustedReactance | Self::TapAdjustedSeriesSusceptance
        )
    }

    /// Whether resistance participates in the series impedance coefficient.
    #[must_use]
    pub fn reads_resistance(self) -> bool {
        matches!(
            self,
            Self::SeriesSusceptance
                | Self::TapAdjustedSeriesSusceptance
                | Self::SeriesSusceptanceNoPhaseShift
        )
    }

    /// Whether phase shifts contribute to the nodal injection vector.
    #[must_use]
    pub fn includes_phase_shifts(self) -> bool {
        match self {
            Self::ReactanceOnly | Self::SeriesSusceptanceNoPhaseShift => false,
            Self::TapAdjustedReactance
            | Self::SeriesSusceptance
            | Self::TapAdjustedSeriesSusceptance => true,
        }
    }

    /// The formula's stable cross language name.
    #[must_use]
    pub fn formula_name(self) -> &'static str {
        match self {
            Self::SeriesSusceptance => "series_susceptance",
            Self::TapAdjustedReactance => "tap_adjusted_reactance",
            Self::ReactanceOnly => "reactance_only",
            Self::TapAdjustedSeriesSusceptance => "tap_adjusted_series_susceptance",
            Self::SeriesSusceptanceNoPhaseShift => "series_susceptance_no_phase_shift",
        }
    }

    /// The formula for one stable name, or `None` for an unknown name.
    #[must_use]
    pub fn from_formula_name(name: &str) -> Option<Self> {
        match name {
            "series_susceptance" => Some(Self::SeriesSusceptance),
            "tap_adjusted_reactance" => Some(Self::TapAdjustedReactance),
            "reactance_only" => Some(Self::ReactanceOnly),
            "tap_adjusted_series_susceptance" => Some(Self::TapAdjustedSeriesSusceptance),
            "series_susceptance_no_phase_shift" => Some(Self::SeriesSusceptanceNoPhaseShift),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The formula names are the cross language vocabulary; unknown names
    /// resolve to nothing rather than a default.
    #[test]
    fn formula_names_round_trip() {
        for convention in [
            BranchSusceptanceFormula::SeriesSusceptance,
            BranchSusceptanceFormula::TapAdjustedReactance,
            BranchSusceptanceFormula::ReactanceOnly,
            BranchSusceptanceFormula::TapAdjustedSeriesSusceptance,
            BranchSusceptanceFormula::SeriesSusceptanceNoPhaseShift,
        ] {
            assert_eq!(
                BranchSusceptanceFormula::from_formula_name(convention.formula_name()),
                Some(convention)
            );
        }
        assert_eq!(BranchSusceptanceFormula::from_formula_name("mystery"), None);
        assert_eq!(BranchSusceptanceFormula::from_formula_name("series"), None);
        assert_eq!(
            BranchSusceptanceFormula::from_formula_name("matpower"),
            None
        );
    }

    #[test]
    fn tap_adjusted_series_is_finite_at_large_impedance_and_ignores_no_inputs() {
        let f = BranchSusceptanceFormula::TapAdjustedSeriesSusceptance;
        let b = f.calc_branch_susceptance(1e200, 1e200, 2.0);
        assert!((b / -2.5e-201 - 1.0).abs() < 1e-14);
        for tap in [f64::INFINITY, f64::NAN, 0.0] {
            assert!(!f.calc_branch_susceptance(0.1, 0.2, tap).is_finite());
        }
        assert!(f.reads_tap() && f.reads_resistance() && f.includes_phase_shifts());
        let dcp = BranchSusceptanceFormula::SeriesSusceptanceNoPhaseShift;
        assert!(dcp.reads_resistance() && !dcp.reads_tap() && !dcp.includes_phase_shifts());
        assert!((dcp.calc_branch_susceptance(0.1, 0.2, f64::NAN) + 4.0).abs() < 1e-12);
    }

    /// The analytic sensitivity to the series susceptance matches a central
    /// difference that moves `b_s` with `g` and the tap held fixed.
    #[test]
    fn susceptance_sensitivity_matches_central_difference() {
        let formulas = [
            BranchSusceptanceFormula::SeriesSusceptance,
            BranchSusceptanceFormula::TapAdjustedReactance,
            BranchSusceptanceFormula::ReactanceOnly,
            BranchSusceptanceFormula::TapAdjustedSeriesSusceptance,
            BranchSusceptanceFormula::SeriesSusceptanceNoPhaseShift,
        ];
        for &(r, x, tap) in &[(0.01, 0.1, 1.0), (0.05, 0.08, 0.97), (0.0, 0.25, 1.05)] {
            let (g, b_s) = series_admittance_parts(r, x);
            let at = |b: f64| {
                let denom = g * g + b * b;
                (g / denom, -b / denom)
            };
            let h = 1e-6 * b_s.abs();
            let (r_hi, x_hi) = at(b_s + h);
            let (r_lo, x_lo) = at(b_s - h);
            for formula in formulas {
                let fd = (formula.calc_branch_susceptance(r_hi, x_hi, tap)
                    - formula.calc_branch_susceptance(r_lo, x_lo, tap))
                    / (2.0 * h);
                let analytic =
                    formula.calc_susceptance_sensitivity_to_series_susceptance(r, x, tap);
                assert!(
                    (fd - analytic).abs() <= 1e-6 * analytic.abs().max(1.0),
                    "{formula:?} at r={r} x={x} tap={tap}: fd {fd} vs analytic {analytic}"
                );
            }
        }
        assert!(
            BranchSusceptanceFormula::TapAdjustedReactance
                .calc_susceptance_sensitivity_to_series_susceptance(0.1, f64::NAN, 1.0)
                .is_nan()
        );
        assert!(
            BranchSusceptanceFormula::SeriesSusceptance
                .calc_susceptance_sensitivity_to_series_susceptance(f64::INFINITY, 0.1, 1.0)
                .is_nan()
        );
    }

    /// Public values carry PowerModels signs: negative for an inductive
    /// branch, `imag(inv(r + jx))` exactly. A resistanceless branch reads the
    /// same under both live conventions, so the default only moves a case
    /// that carries resistance.
    #[test]
    fn series_susceptance_reduces_to_negated_one_over_x() {
        let b = BranchSusceptanceFormula::SeriesSusceptance.calc_branch_susceptance(0.0, 0.25, 1.0);
        assert!((b + 4.0).abs() < 1e-12);
        // The internal factor weight is its negation.
        let weight =
            BranchSusceptanceFormula::SeriesSusceptance.calc_solver_edge_weight(0.0, 0.25, 1.0);
        assert!((weight - 4.0).abs() < 1e-12);
    }

    /// Resistance lowers the susceptance magnitude, by more as `r` grows
    /// against `x`.
    #[test]
    fn resistance_lowers_the_susceptance_magnitude() {
        let lossless =
            BranchSusceptanceFormula::SeriesSusceptance.calc_branch_susceptance(0.0, 0.1, 1.0);
        let lossy =
            BranchSusceptanceFormula::SeriesSusceptance.calc_branch_susceptance(0.1, 0.1, 1.0);
        assert!(lossy.abs() < lossless.abs());
        assert!((lossy + 5.0).abs() < 1e-12);
    }

    #[test]
    fn matpower_scales_by_the_tap() {
        let b =
            BranchSusceptanceFormula::TapAdjustedReactance.calc_branch_susceptance(0.01, 0.2, 2.0);
        assert!((b + 2.5).abs() < 1e-12);
    }

    /// Only the tap-reading formula can be rejected by a tap: the other
    /// formulas never read the value (#324).
    #[test]
    fn an_unread_tap_never_rejects_a_branch() {
        for conv in [
            BranchSusceptanceFormula::ReactanceOnly,
            BranchSusceptanceFormula::SeriesSusceptance,
        ] {
            assert!(!conv.reads_tap());
            let b = conv.calc_branch_susceptance(0.01, 0.1, 1e-200);
            assert!(b.is_finite(), "{conv:?} read the tap it never divides by");
        }
        assert!(BranchSusceptanceFormula::TapAdjustedReactance.reads_tap());
    }

    /// `1/±inf` is `0.0`, which is finite, so a branch the Y_bus builder rejects
    /// outright would enter the DC Laplacian as a zero-weight edge instead. The
    /// tap divides the same denominator, so two finite factors whose product
    /// overflows collapse the same way.
    #[test]
    fn a_non_finite_denominator_is_not_a_susceptance() {
        for x in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            for conv in [
                BranchSusceptanceFormula::ReactanceOnly,
                BranchSusceptanceFormula::TapAdjustedReactance,
                BranchSusceptanceFormula::SeriesSusceptance,
            ] {
                let b = conv.calc_branch_susceptance(0.01, x, 1.0);
                assert!(!b.is_finite(), "{conv:?} read x = {x} as b = {b}");
            }
        }
        for (x, tap) in [
            (0.1, f64::INFINITY),
            (0.1, f64::NAN),
            (1e300, 1e300),
            (1e300, -1e300),
        ] {
            let b =
                BranchSusceptanceFormula::TapAdjustedReactance.calc_branch_susceptance(0.0, x, tap);
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

        let b = BranchSusceptanceFormula::SeriesSusceptance.calc_branch_susceptance(r, x, 1.0);
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

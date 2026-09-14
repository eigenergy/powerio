//! Generator cost curve compilation and the policy that states which curve
//! shapes a preparation carries.
//!
//! Convexity is a property of the cost curve a source file states, not of the
//! network it belongs to. A DC power flow, a contingency screen, and a
//! nonconvex solver all read the same buses and branches and none of them read
//! the objective, so the shape of a cost row decides nothing for them.
//! [`CostCurvePolicy`] therefore separates the two questions: the build carries
//! what the data states, and the caller says whether a curve that is not convex
//! ends the build, is carried as stated, or is replaced by its lower convex
//! envelope.
//!
//! Every departure from convexity is recorded in a [`CostCurveProjection`],
//! which travels on the preparation and renders as a warning through
//! [`cost_curve_diagnostics`].

use serde::{Deserialize, Serialize};

use powerio_core::{Diagnostic, DiagnosticSeverity};
use powerio_tx::GenCost;

use crate::{Error, PiecewiseCostInvalidity, PiecewiseLinearCost, Result};

/// Which generator cost curve shapes a preparation carries.
///
/// The variants differ only in what they do with a curve that is not convex; a
/// convex curve is carried unchanged under all three and records nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CostCurvePolicy {
    /// Carry every readable curve as the data states it. The prepared arrays
    /// hold the stated breakpoints or coefficients, and each nonconvex curve
    /// is recorded as a [`CostCurveAction::Kept`] projection.
    #[default]
    Any,
    /// Carry convex curves only. A nonconvex piecewise linear row ends the
    /// build with [`Error::NonconvexPiecewiseCost`] and a concave polynomial
    /// row with [`Error::ConcaveCost`].
    ConvexOnly,
    /// Replace each nonconvex curve with its lower convex envelope over the
    /// generator's stated active power range, and record how far the
    /// replacement falls below the stated curve.
    ConvexifyLowerEnvelope,
}

impl CostCurvePolicy {
    /// The one alias table for the bindings: the `snake_case` variant names,
    /// case insensitive, with `-` and `_` ignored.
    ///
    /// # Errors
    /// A name outside the table.
    pub fn parse(name: &str) -> std::result::Result<Self, String> {
        match name.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "any" => Ok(Self::Any),
            "convexonly" => Ok(Self::ConvexOnly),
            "convexifylowerenvelope" => Ok(Self::ConvexifyLowerEnvelope),
            other => Err(format!(
                "unknown cost curve policy `{other}`; expected \"any\", \"convex-only\", or \"convexify-lower-envelope\""
            )),
        }
    }

    /// The `snake_case` name this policy serializes as.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::ConvexOnly => "convex_only",
            Self::ConvexifyLowerEnvelope => "convexify_lower_envelope",
        }
    }
}

impl std::str::FromStr for CostCurvePolicy {
    type Err = String;

    fn from_str(name: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse(name)
    }
}

impl std::fmt::Display for CostCurvePolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// How a stated cost curve departs from convexity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CostCurveDeparture {
    /// A piecewise linear row whose segment slopes decrease somewhere.
    NonconvexPiecewise,
    /// A polynomial row with a negative quadratic coefficient.
    ConcavePolynomial,
}

impl CostCurveDeparture {
    /// The property of the stated curve, for a diagnostic message.
    #[must_use]
    pub const fn summary(self) -> &'static str {
        match self {
            Self::NonconvexPiecewise => "a piecewise linear cost row whose segment slopes decrease",
            Self::ConcavePolynomial => "a concave polynomial cost row",
        }
    }
}

/// What the preparation carries in place of a curve that is not convex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CostCurveAction {
    /// The stated curve, unchanged.
    Kept,
    /// The lower convex envelope of the stated curve over the generator's
    /// stated active power range.
    LowerEnvelope,
}

/// One generator whose stated cost curve is not convex, and what the
/// preparation carries for it.
///
/// The projections of a build are collected in preparation order on
/// [`DcOpfPreparation::cost_curve_projections`](crate::DcOpfPreparation::cost_curve_projections)
/// and [`AcOpfPreparation::cost_curve_projections`](crate::AcOpfPreparation::cost_curve_projections).
/// An empty list means every carried curve is convex as stated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CostCurveProjection {
    /// Stable generator identity, as the preparation's `identities` column
    /// spells it.
    pub identity: String,
    /// Row in the source generator table.
    pub source_row: usize,
    /// The property of the stated curve that is not convex.
    pub departure: CostCurveDeparture,
    /// What the prepared columns hold for this generator.
    pub action: CostCurveAction,
    /// Largest amount, in the source cost unit, by which the carried curve
    /// lies below the stated one. Zero for [`CostCurveAction::Kept`], where the
    /// two are the same curve.
    pub projection_loss: f64,
}

/// Render the cost curve projections of a preparation as warnings.
///
/// Each projection becomes one finding under the code for the property it
/// names, at [`DiagnosticSeverity::Warning`]: the build produced usable arrays,
/// and the finding states what those arrays hold.
#[must_use]
pub fn cost_curve_diagnostics(projections: &[CostCurveProjection]) -> Vec<Diagnostic> {
    projections
        .iter()
        .map(|projection| {
            let info = match projection.departure {
                CostCurveDeparture::NonconvexPiecewise => {
                    &powerio_prob::diagnostics::codes::BUILD_INSTANCE_PIECEWISE_COST_NONCONVEX
                }
                CostCurveDeparture::ConcavePolynomial => {
                    &powerio_prob::diagnostics::codes::BUILD_INSTANCE_CONCAVE_COST
                }
            };
            let message = match projection.action {
                CostCurveAction::Kept => format!(
                    "generator `{}` states {}; the prepared objective carries it as stated and is not convex",
                    projection.identity,
                    projection.departure.summary(),
                ),
                CostCurveAction::LowerEnvelope => format!(
                    "generator `{}` states {}; the prepared objective carries its lower convex envelope, which lies up to {} below the stated curve",
                    projection.identity,
                    projection.departure.summary(),
                    projection.projection_loss,
                ),
            };
            Diagnostic::of(info, message).with_severity(DiagnosticSeverity::Warning)
        })
        .collect()
}

/// The complete supported cost data for one generator before unit scaling of
/// polynomial coefficients.
#[derive(Debug)]
pub(crate) struct GeneratorCostTerms {
    pub q: f64,
    pub c: f64,
    pub c0: f64,
    pub piecewise_linear: Option<PiecewiseLinearCost>,
    /// Set when the stated curve is not convex.
    pub projection: Option<CostCurveProjection>,
}

impl GeneratorCostTerms {
    /// The identically zero curve a feasibility objective carries.
    pub(crate) const fn zero() -> Self {
        Self {
            q: 0.0,
            c: 0.0,
            c0: 0.0,
            piecewise_linear: None,
            projection: None,
        }
    }
}

/// One generator's cost row and the context the policy needs to read it.
pub(crate) struct CostCurveInput<'a> {
    pub cost: &'a GenCost,
    /// Stable generator identity, for the projection record.
    pub identity: &'a str,
    /// Row in the source generator table.
    pub source_row: usize,
    /// `(pmin, pmax)` in the source power unit, the range a lower convex
    /// envelope of a polynomial row is taken over.
    pub bounds: (f64, f64),
    /// Multiplier from the source power unit into the preparation's unit.
    pub power_scale: f64,
    pub policy: CostCurvePolicy,
}

/// Compile one generator cost under `policy`.
///
/// The mathematical form is unchanged except where the policy replaces a
/// nonconvex curve, and every replacement is reported in
/// [`GeneratorCostTerms::projection`].
///
/// # Errors
/// [`Error::UnsupportedCostModel`] for a row that states no readable curve,
/// [`Error::InvalidPiecewiseCost`] for a malformed piecewise row, and, under
/// [`CostCurvePolicy::ConvexOnly`], [`Error::NonconvexPiecewiseCost`] or
/// [`Error::ConcaveCost`] for a curve that is not convex.
pub(crate) fn generator_cost_terms(input: &CostCurveInput<'_>) -> Result<GeneratorCostTerms> {
    match input.cost.model {
        1 => piecewise_linear_terms(input),
        2 => quadratic_terms(input),
        model => Err(Error::UnsupportedCostModel {
            gen_index: input.source_row,
            model,
            ncost: input.cost.ncost,
        }),
    }
}

fn piecewise_linear_terms(input: &CostCurveInput<'_>) -> Result<GeneratorCostTerms> {
    let CostCurveInput {
        cost,
        source_row: gen_index,
        power_scale,
        ..
    } = *input;
    if cost.ncost < 2 {
        return Err(Error::InvalidPiecewiseCost {
            gen_index,
            reason: PiecewiseCostInvalidity::FewerThanTwoBreakpoints {
                declared: cost.ncost,
            },
        });
    }
    let expected_values = cost
        .ncost
        .checked_mul(2)
        .ok_or(Error::InvalidPiecewiseCost {
            gen_index,
            reason: PiecewiseCostInvalidity::Truncated {
                expected_values: usize::MAX,
                got: cost.coeffs.len(),
            },
        })?;
    if cost.coeffs.len() < expected_values {
        return Err(Error::InvalidPiecewiseCost {
            gen_index,
            reason: PiecewiseCostInvalidity::Truncated {
                expected_values,
                got: cost.coeffs.len(),
            },
        });
    }

    let mut power = Vec::with_capacity(cost.ncost);
    let mut value = Vec::with_capacity(cost.ncost);
    for (point, pair) in cost.coeffs[..expected_values]
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
    {
        let p = pair[0] * power_scale;
        let v = pair[1];
        if !p.is_finite() || !v.is_finite() {
            return Err(Error::InvalidPiecewiseCost {
                gen_index,
                reason: PiecewiseCostInvalidity::NonFinitePoint { point },
            });
        }
        if power.last().is_some_and(|previous| p <= *previous) {
            return Err(Error::InvalidPiecewiseCost {
                gen_index,
                reason: PiecewiseCostInvalidity::NonIncreasingPower { point },
            });
        }
        power.push(p);
        value.push(v);
    }

    let Some(segment) = first_decreasing_slope(&power, &value, gen_index)? else {
        return Ok(GeneratorCostTerms {
            piecewise_linear: Some(PiecewiseLinearCost { power, value }),
            ..GeneratorCostTerms::zero()
        });
    };

    let departure = CostCurveDeparture::NonconvexPiecewise;
    let (curve, action, projection_loss) = match input.policy {
        CostCurvePolicy::ConvexOnly => {
            return Err(Error::NonconvexPiecewiseCost { gen_index, segment });
        }
        CostCurvePolicy::Any => (
            PiecewiseLinearCost { power, value },
            CostCurveAction::Kept,
            0.0,
        ),
        CostCurvePolicy::ConvexifyLowerEnvelope => {
            let (curve, loss) = lower_convex_envelope(&power, &value);
            (curve, CostCurveAction::LowerEnvelope, loss)
        }
    };
    Ok(GeneratorCostTerms {
        piecewise_linear: Some(curve),
        projection: Some(CostCurveProjection {
            identity: input.identity.to_owned(),
            source_row: gen_index,
            departure,
            action,
            projection_loss,
        }),
        ..GeneratorCostTerms::zero()
    })
}

/// The first segment whose slope falls below the preceding one, or `None` for a
/// convex curve.
///
/// The tolerance is a rounding allowance on the two slopes being compared: a
/// source that rounds its breakpoint values can state equal slopes as a pair
/// that differs in the last bits.
fn first_decreasing_slope(power: &[f64], value: &[f64], gen_index: usize) -> Result<Option<usize>> {
    let mut previous_slope: Option<f64> = None;
    for segment in 0..power.len() - 1 {
        let slope = (value[segment + 1] - value[segment]) / (power[segment + 1] - power[segment]);
        if !slope.is_finite() {
            return Err(Error::InvalidPiecewiseCost {
                gen_index,
                reason: PiecewiseCostInvalidity::NonFinitePoint { point: segment + 1 },
            });
        }
        if let Some(previous) = previous_slope {
            let roundoff = 64.0 * f64::EPSILON * previous.abs().max(slope.abs()).max(1.0);
            if previous > slope + roundoff {
                return Ok(Some(segment));
            }
        }
        previous_slope = Some(slope);
    }
    Ok(None)
}

/// The lower convex envelope of a piecewise linear curve, and the largest
/// amount by which it falls below the stated curve.
///
/// The envelope is the lower hull of the breakpoints, built by Andrew's
/// monotone chain over the already increasing power coordinate: a breakpoint
/// stays only while it lies strictly below the chord joining its neighbours.
/// Both extreme breakpoints are always kept, so the envelope spans the same
/// power range as the stated curve.
///
/// The stated curve and its envelope are both piecewise linear with
/// breakpoints among the stated ones, so their difference is largest at a
/// stated breakpoint and the reported loss is exact.
fn lower_convex_envelope(power: &[f64], value: &[f64]) -> (PiecewiseLinearCost, f64) {
    let mut hull: Vec<usize> = Vec::with_capacity(power.len());
    for point in 0..power.len() {
        while hull.len() >= 2 {
            let first = hull[hull.len() - 2];
            let middle = hull[hull.len() - 1];
            let cross = (power[middle] - power[first]) * (value[point] - value[first])
                - (value[middle] - value[first]) * (power[point] - power[first]);
            if cross > 0.0 {
                break;
            }
            hull.pop();
        }
        hull.push(point);
    }

    let mut loss = 0.0_f64;
    let mut segment = 0;
    for point in 0..power.len() {
        while segment + 2 < hull.len() && hull[segment + 1] < point {
            segment += 1;
        }
        let (left, right) = (hull[segment], hull[segment + 1]);
        let slope = (value[right] - value[left]) / (power[right] - power[left]);
        let on_envelope = value[left] + slope * (power[point] - power[left]);
        loss = loss.max(value[point] - on_envelope);
    }

    let curve = PiecewiseLinearCost {
        power: hull.iter().map(|&point| power[point]).collect(),
        value: hull.iter().map(|&point| value[point]).collect(),
    };
    (curve, loss)
}

/// `(q, c, c0)` of one generator's polynomial cost row.
///
/// A MATPOWER model 2 row often carries a leading coefficient near 1e-17 that
/// the source produced by rounding. It states a linear curve, and reading it as
/// quadratic gives `1/q` near 1e17 wherever the curvature is inverted.
fn quadratic_terms(input: &CostCurveInput<'_>) -> Result<GeneratorCostTerms> {
    let CostCurveInput {
        cost,
        source_row: gen_index,
        ..
    } = *input;
    // One rule, stated once on the hub type. Rolling it again here diverged
    // twice: the threshold applied to `2*c2` rather than to the source
    // coefficient the artifact lives in, and a longer row whose leading
    // coefficients are artifacts errored here while the hub read it.
    let (q, c, c0) = cost
        .calc_quadratic_with_constant_tol(GenCost::LEADING_COEFF_TOL)
        .ok_or(Error::UnsupportedCostModel {
            gen_index,
            model: cost.model,
            ncost: cost.ncost,
        })?;
    if q >= 0.0 {
        return Ok(GeneratorCostTerms {
            q,
            c,
            c0,
            ..GeneratorCostTerms::zero()
        });
    }

    let departure = CostCurveDeparture::ConcavePolynomial;
    let ((q, c, c0), action, projection_loss) = match input.policy {
        CostCurvePolicy::ConvexOnly => {
            return Err(Error::ConcaveCost {
                gen_index,
                c2: q / 2.0,
            });
        }
        CostCurvePolicy::Any => ((q, c, c0), CostCurveAction::Kept, 0.0),
        CostCurvePolicy::ConvexifyLowerEnvelope => {
            let (terms, loss) = concave_chord(input.bounds, (q, c, c0), gen_index)?;
            (terms, CostCurveAction::LowerEnvelope, loss)
        }
    };
    Ok(GeneratorCostTerms {
        q,
        c,
        c0,
        piecewise_linear: None,
        projection: Some(CostCurveProjection {
            identity: input.identity.to_owned(),
            source_row: gen_index,
            departure,
            action,
            projection_loss,
        }),
    })
}

/// The lower convex envelope of the concave curve `½ q p² + c p + c0` over
/// `[pmin, pmax]`, as `(q, c, c0)` with `q = 0`, and the largest amount by
/// which it falls below the stated curve.
///
/// A concave function lies above every chord of its graph, and the chord over
/// the whole interval is the greatest convex function below it, so the envelope
/// is that chord. It is linear, and `q = 0` states it exactly. Their difference
/// is a downward parabola vanishing at both ends, so the loss is
/// `-q (pmax - pmin)² / 8` at the midpoint.
///
/// # Errors
/// [`Error::UnboundedCostEnvelope`] when the range is not a finite interval:
/// the chord of a concave curve over an unbounded range is unbounded below, so
/// no lower convex envelope exists.
fn concave_chord(
    bounds: (f64, f64),
    (q, c, c0): (f64, f64, f64),
    gen_index: usize,
) -> Result<((f64, f64, f64), f64)> {
    let (pmin, pmax) = bounds;
    if !pmin.is_finite() || !pmax.is_finite() || pmax < pmin {
        return Err(Error::UnboundedCostEnvelope { gen_index });
    }
    let width = pmax - pmin;
    if width <= 0.0 {
        // The range is one point; the curve's value there is the whole cost.
        return Ok(((0.0, 0.0, 0.5 * q * pmin * pmin + c * pmin + c0), 0.0));
    }
    // The chord's slope through the two endpoint values.
    let slope = 0.5 * q * (pmin + pmax) + c;
    let intercept = 0.5 * q * pmin * pmin + c * pmin + c0 - slope * pmin;
    Ok(((0.0, slope, intercept), -q * width * width / 8.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piecewise(points: &[(f64, f64)]) -> GenCost {
        let coeffs = points
            .iter()
            .flat_map(|&(p, v)| [p, v])
            .collect::<Vec<f64>>();
        GenCost::new(1, 0.0, 0.0, coeffs)
    }

    fn compile(
        cost: &GenCost,
        policy: CostCurvePolicy,
        bounds: (f64, f64),
    ) -> Result<GeneratorCostTerms> {
        generator_cost_terms(&CostCurveInput {
            cost,
            identity: "generators:0",
            source_row: 0,
            bounds,
            power_scale: 1.0,
            policy,
        })
    }

    #[test]
    fn a_convex_piecewise_row_is_carried_unchanged_under_every_policy() {
        let cost = piecewise(&[(0.0, 0.0), (50.0, 100.0), (100.0, 300.0)]);
        for policy in [
            CostCurvePolicy::Any,
            CostCurvePolicy::ConvexOnly,
            CostCurvePolicy::ConvexifyLowerEnvelope,
        ] {
            let terms = compile(&cost, policy, (0.0, 100.0)).expect("a convex row");
            let curve = terms.piecewise_linear.expect("a piecewise curve");
            assert_eq!(curve.power, vec![0.0, 50.0, 100.0]);
            assert_eq!(curve.value, vec![0.0, 100.0, 300.0]);
            assert!(terms.projection.is_none());
        }
    }

    #[test]
    fn convex_only_refuses_a_nonconvex_piecewise_row() {
        let cost = piecewise(&[(0.0, 0.0), (50.0, 200.0), (100.0, 300.0)]);
        let error =
            compile(&cost, CostCurvePolicy::ConvexOnly, (0.0, 100.0)).expect_err("a nonconvex row");
        match error {
            Error::NonconvexPiecewiseCost { gen_index, segment } => {
                assert_eq!((gen_index, segment), (0, 1));
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn the_default_policy_carries_a_nonconvex_piecewise_row_as_stated() {
        let cost = piecewise(&[(0.0, 0.0), (50.0, 200.0), (100.0, 300.0)]);
        let terms = compile(&cost, CostCurvePolicy::Any, (0.0, 100.0)).expect("a nonconvex row");
        let curve = terms.piecewise_linear.expect("a piecewise curve");
        assert_eq!(curve.power, vec![0.0, 50.0, 100.0]);
        assert_eq!(curve.value, vec![0.0, 200.0, 300.0]);
        let projection = terms.projection.expect("a recorded departure");
        assert_eq!(projection.departure, CostCurveDeparture::NonconvexPiecewise);
        assert_eq!(projection.action, CostCurveAction::Kept);
        assert_eq!(projection.projection_loss.to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn the_lower_envelope_drops_the_breakpoint_above_the_chord() {
        let cost = piecewise(&[(0.0, 0.0), (50.0, 200.0), (100.0, 300.0)]);
        let terms = compile(&cost, CostCurvePolicy::ConvexifyLowerEnvelope, (0.0, 100.0))
            .expect("a nonconvex row");
        let curve = terms.piecewise_linear.expect("a piecewise curve");
        assert_eq!(curve.power, vec![0.0, 100.0]);
        assert_eq!(curve.value, vec![0.0, 300.0]);
        let projection = terms.projection.expect("a recorded departure");
        assert_eq!(projection.action, CostCurveAction::LowerEnvelope);
        // The dropped breakpoint sits 200 - 150 above the chord.
        assert!((projection.projection_loss - 50.0).abs() < 1e-12);
    }

    /// The envelope is the greatest convex curve below the stated one: it must
    /// be convex, never exceed the stated curve, and touch it at both ends.
    #[test]
    fn the_lower_envelope_stays_below_the_stated_curve_and_is_convex() {
        let points = [
            (0.0, 10.0),
            (20.0, 90.0),
            (40.0, 130.0),
            (60.0, 250.0),
            (80.0, 280.0),
            (100.0, 460.0),
        ];
        let (curve, loss) = lower_convex_envelope(
            &points.iter().map(|&(p, _)| p).collect::<Vec<f64>>(),
            &points.iter().map(|&(_, v)| v).collect::<Vec<f64>>(),
        );
        assert_eq!(curve.power.first(), Some(&0.0));
        assert_eq!(curve.power.last(), Some(&100.0));
        assert_eq!(curve.value.first(), Some(&10.0));
        assert_eq!(curve.value.last(), Some(&460.0));

        let slopes: Vec<f64> = (0..curve.power.len() - 1)
            .map(|i| (curve.value[i + 1] - curve.value[i]) / (curve.power[i + 1] - curve.power[i]))
            .collect();
        for pair in slopes.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "slopes {slopes:?} are not nondecreasing"
            );
        }

        let mut worst = 0.0_f64;
        for &(p, v) in &points {
            let segment = curve.power.windows(2).position(|w| p >= w[0] && p <= w[1]);
            let segment = segment.expect("the envelope spans the stated range");
            let slope = slopes[segment];
            let on_envelope = curve.value[segment] + slope * (p - curve.power[segment]);
            assert!(
                on_envelope <= v + 1e-9,
                "envelope {on_envelope} exceeds {v}"
            );
            worst = worst.max(v - on_envelope);
        }
        assert!((loss - worst).abs() < 1e-9, "loss {loss} vs {worst}");
    }

    #[test]
    fn the_default_policy_carries_a_concave_polynomial_row_as_stated() {
        let cost = GenCost::new(2, 0.0, 0.0, vec![-0.5, 5.0, 0.0]);
        let terms = compile(&cost, CostCurvePolicy::Any, (0.0, 10.0)).expect("a concave row");
        assert_eq!((terms.q, terms.c, terms.c0), (-1.0, 5.0, 0.0));
        let projection = terms.projection.expect("a recorded departure");
        assert_eq!(projection.departure, CostCurveDeparture::ConcavePolynomial);
        assert_eq!(projection.action, CostCurveAction::Kept);
    }

    #[test]
    fn a_concave_polynomial_row_convexifies_to_its_chord() {
        // ½ q p² + c p + c0 with q = -1: f(0) = 0, f(10) = -50 + 50 = 0.
        let cost = GenCost::new(2, 0.0, 0.0, vec![-0.5, 5.0, 0.0]);
        let terms = compile(&cost, CostCurvePolicy::ConvexifyLowerEnvelope, (0.0, 10.0))
            .expect("a concave row");
        assert_eq!((terms.q, terms.c, terms.c0), (0.0, 0.0, 0.0));
        let projection = terms.projection.expect("a recorded departure");
        assert_eq!(projection.action, CostCurveAction::LowerEnvelope);
        // The chord is flat at zero; the curve peaks at p = 5 with value 12.5.
        assert!((projection.projection_loss - 12.5).abs() < 1e-12);
    }

    #[test]
    fn a_concave_row_with_no_finite_range_has_no_lower_envelope() {
        let cost = GenCost::new(2, 0.0, 0.0, vec![-0.5, 5.0, 0.0]);
        let error = compile(
            &cost,
            CostCurvePolicy::ConvexifyLowerEnvelope,
            (0.0, f64::INFINITY),
        )
        .expect_err("an unbounded range");
        assert!(matches!(
            error,
            Error::UnboundedCostEnvelope { gen_index: 0 }
        ));
    }

    /// A rounding artifact states a linear curve. Read as quadratic it gives
    /// `1/q` near 1e17, which swamps the parallel sum and prices the bus as
    /// nearly free. A negative artifact is the same artifact, so it is not a
    /// concave row.
    #[test]
    fn a_tiny_negative_artifact_still_reads_as_a_linear_curve() {
        let cost = GenCost::new(2, 0.0, 0.0, vec![-1e-17, 3.0, 0.0]);
        let terms =
            compile(&cost, CostCurvePolicy::ConvexOnly, (0.0, 10.0)).expect("a model 2 row");
        assert_eq!((terms.q, terms.c, terms.c0), (0.0, 3.0, 0.0));
        assert!(terms.projection.is_none());
    }

    #[test]
    fn an_unreadable_cost_model_is_refused_under_every_policy() {
        let cost = GenCost::new(3, 0.0, 0.0, vec![1.0, 2.0, 3.0]);
        for policy in [
            CostCurvePolicy::Any,
            CostCurvePolicy::ConvexOnly,
            CostCurvePolicy::ConvexifyLowerEnvelope,
        ] {
            let error = compile(&cost, policy, (0.0, 10.0)).expect_err("model 3");
            assert!(matches!(
                error,
                Error::UnsupportedCostModel { model: 3, .. }
            ));
        }
    }

    #[test]
    fn the_policy_names_round_trip_through_the_alias_table() {
        for policy in [
            CostCurvePolicy::Any,
            CostCurvePolicy::ConvexOnly,
            CostCurvePolicy::ConvexifyLowerEnvelope,
        ] {
            assert_eq!(CostCurvePolicy::parse(policy.name()), Ok(policy));
        }
        assert_eq!(
            CostCurvePolicy::parse("Convexify-Lower-Envelope"),
            Ok(CostCurvePolicy::ConvexifyLowerEnvelope)
        );
        assert!(CostCurvePolicy::parse("convex").is_err());
    }

    #[test]
    fn a_kept_curve_and_a_replaced_one_render_as_warnings() {
        let projections = vec![
            CostCurveProjection {
                identity: "gen-a".to_owned(),
                source_row: 0,
                departure: CostCurveDeparture::NonconvexPiecewise,
                action: CostCurveAction::Kept,
                projection_loss: 0.0,
            },
            CostCurveProjection {
                identity: "gen-b".to_owned(),
                source_row: 4,
                departure: CostCurveDeparture::ConcavePolynomial,
                action: CostCurveAction::LowerEnvelope,
                projection_loss: 12.5,
            },
        ];
        let diagnostics = cost_curve_diagnostics(&projections);
        assert_eq!(diagnostics.len(), 2);
        for diagnostic in &diagnostics {
            assert_eq!(diagnostic.severity(), DiagnosticSeverity::Warning);
        }
        assert_eq!(
            diagnostics[0].code(),
            "BUILD.INSTANCE.PIECEWISE_COST_NONCONVEX"
        );
        assert_eq!(diagnostics[1].code(), "BUILD.INSTANCE.CONCAVE_COST");
        assert!(diagnostics[1].message().contains("12.5"));
    }
}

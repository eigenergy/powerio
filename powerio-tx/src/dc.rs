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
    /// The 0.8 spelling of [`DcConvention::ReactanceOnly`]. Works in
    /// expression and pattern position; goes away at 1.0.0.
    #[allow(non_upper_case_globals)]
    #[deprecated(
        since = "0.9.0",
        note = "renamed to DcConvention::ReactanceOnly in 0.9.0; the alias goes away at 1.0.0"
    )]
    pub const PaperPure: DcConvention = DcConvention::ReactanceOnly;

    /// The branch susceptance from resistance, reactance, and effective tap.
    /// Only [`Self::Matpower`] reads the tap, and only
    /// [`Self::SeriesImpedance`] reads the resistance.
    ///
    /// Non-finite in, non-finite out, which is what
    /// [`Self::SeriesImpedance`] has always done and what the callers check.
    /// The reciprocal rules need the guard because `1/±inf` is a finite `0.0`:
    /// a branch Y_bus rejects outright would otherwise join the DC Laplacian as
    /// a zero-weight edge with nothing to report it.
    #[must_use]
    pub fn branch_susceptance(self, resistance: f64, reactance: f64, effective_tap: f64) -> f64 {
        // Guard the denominator, not its factors: `x * tap` can overflow to
        // infinity from two finite factors and reach the same silent zero.
        let reciprocal = |denominator: f64| {
            if denominator.is_finite() {
                1.0 / denominator
            } else {
                f64::NAN
            }
        };
        match self {
            Self::ReactanceOnly => reciprocal(reactance),
            Self::Matpower => reciprocal(reactance * effective_tap),
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

    fn three_bus_network() -> crate::BalancedNetwork {
        use crate::{Branch, Bus, BusId, BusType};
        let mut shifted = Branch::new(BusId(2), BusId(3), 0.0, 0.2);
        shifted.shift = 30.0;
        let mut out = Branch::new(BusId(1), BusId(3), 0.01, 0.1);
        out.in_service = false;
        crate::BalancedNetwork::in_memory(
            "dc-data",
            100.0,
            vec![
                Bus::new(BusId(1), BusType::Ref, 230.0),
                Bus::new(BusId(2), BusType::Pq, 230.0),
                Bus::new(BusId(3), BusType::Pq, 230.0),
            ],
            vec![Branch::new(BusId(1), BusId(2), 0.0, 0.1), shifted, out],
        )
    }

    /// The shared assembly: PowerModels orientation, stable identity for
    /// every included and omitted row, and the shift injection
    /// `p_shift = A' (b .* shift)` with the shift read in radians.
    #[test]
    fn dc_network_data_maps_rows_and_omissions() {
        let network = three_bus_network();
        let view = crate::IndexedNetwork::new(&network);
        let data = dc_network_data(&view, DcConvention::SeriesImpedance);
        assert_eq!(data.formula, "series_susceptance");
        assert_eq!(data.from_index, vec![0, 1]);
        assert_eq!(data.to_index, vec![1, 2]);
        assert_eq!(data.row_ids, vec!["branches:0", "branches:1"]);
        assert_eq!(data.bus_ids, vec!["1", "2", "3"]);
        assert_eq!(data.omitted.len(), 1);
        assert_eq!(data.omitted[0].0, "branches:2");
        assert!(data.omitted[0].1.contains("out of service"));

        let b = data.susceptance[1];
        assert!((b - 5.0).abs() < 1e-12);
        let shift = 30.0_f64.to_radians();
        assert!((data.shift_injection[1] - (-b * shift)).abs() < 1e-12);
        assert!((data.shift_injection[2] - (b * shift)).abs() < 1e-12);
        assert!(data.shift_injection[0].abs() < 1e-15);
    }

    /// Adversarial partition audit: every branch lands in exactly one of
    /// included or omitted, whatever mix of degeneracies the case carries,
    /// and the stable IDs across both sets are unique.
    #[test]
    fn every_branch_is_included_or_omitted_exactly_once() {
        use crate::{Branch, Bus, BusId, BusType};
        let mut branches = vec![
            Branch::new(BusId(1), BusId(2), 0.0, 0.1),
            Branch::new(BusId(2), BusId(2), 0.0, 0.1),
            Branch::new(BusId(1), BusId(9), 0.01, 0.1),
            Branch::new(BusId(1), BusId(3), 0.0, 0.0),
            Branch::new(BusId(2), BusId(3), 0.0, f64::NAN),
            Branch::new(BusId(1), BusId(3), 0.02, 0.2),
        ];
        branches[5].in_service = false;
        let mut giant_tap = Branch::new(BusId(2), BusId(3), 0.0, 1.0e308);
        giant_tap.tap = 1.0e308;
        branches.push(giant_tap);
        let network = crate::BalancedNetwork::in_memory(
            "partition",
            100.0,
            vec![
                Bus::new(BusId(1), BusType::Ref, 230.0),
                Bus::new(BusId(2), BusType::Pq, 230.0),
                Bus::new(BusId(3), BusType::Pq, 230.0),
            ],
            branches,
        );
        let view = crate::IndexedNetwork::new(&network);
        for convention in [
            DcConvention::SeriesImpedance,
            DcConvention::Matpower,
            DcConvention::ReactanceOnly,
        ] {
            let data = dc_network_data(&view, convention);
            let included = data.row_ids.len();
            assert_eq!(included, data.susceptance.len());
            assert_eq!(included, data.from_index.len());
            assert_eq!(
                included + data.omitted.len(),
                network.branches().len(),
                "{convention:?}"
            );
            let mut ids: Vec<&str> = data
                .row_ids
                .iter()
                .map(String::as_str)
                .chain(data.omitted.iter().map(|(id, _)| id.as_str()))
                .collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), network.branches().len(), "{convention:?}");
            assert!(data.susceptance.iter().all(|b| b.is_finite()));
        }
    }

    /// The formula names are the cross language vocabulary; unknown names
    /// resolve to nothing rather than a default.
    #[test]
    fn formula_names_round_trip() {
        for convention in [
            DcConvention::SeriesImpedance,
            DcConvention::Matpower,
            DcConvention::ReactanceOnly,
        ] {
            assert_eq!(
                DcConvention::from_formula_name(convention.formula_name()),
                Some(convention)
            );
        }
        assert_eq!(DcConvention::from_formula_name("mystery"), None);
    }

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

    /// `1/±inf` is `0.0`, which is finite, so a branch the Y_bus builder rejects
    /// outright would enter the DC Laplacian as a zero-weight edge instead. The
    /// tap divides the same denominator, so two finite factors whose product
    /// overflows collapse the same way.
    #[test]
    fn a_non_finite_denominator_is_not_a_susceptance() {
        for x in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            for conv in [
                DcConvention::ReactanceOnly,
                DcConvention::Matpower,
                DcConvention::SeriesImpedance,
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
            let b = DcConvention::Matpower.branch_susceptance(0.0, x, tap);
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

/// The DC branch data of one balanced network under one susceptance formula,
/// with the stable element mappings that interpret every row: the one
/// assembly Rust, C, Python, and Julia all read, so names, element order,
/// and omission reasons agree across languages by construction.
///
/// Rows follow `A[e, from] = +1`, `A[e, to] = -1` (PowerModels orientation);
/// susceptance carries the PowerModels sign for the selected formula; the
/// phase shift injection is `p_shift = A' * (b .* shift)` per bus.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct DcNetworkData {
    /// From bus column per included row.
    pub from_index: Vec<usize>,
    /// To bus column per included row.
    pub to_index: Vec<usize>,
    /// Branch susceptance per included row.
    pub susceptance: Vec<f64>,
    /// Phase shift bus injection, one entry per bus.
    pub shift_injection: Vec<f64>,
    /// Stable module element ID per included row.
    pub row_ids: Vec<String>,
    /// Stable bus element ID per incidence column.
    pub bus_ids: Vec<String>,
    /// Branches the selected formula cannot represent: stable element ID and
    /// the diagnostic reason. Zero impedance branches land here by default;
    /// nothing removes them silently.
    pub omitted: Vec<(String, String)>,
    /// The selected formula's stable cross language name.
    pub formula: &'static str,
}

impl DcConvention {
    /// The formula's stable cross language name.
    #[must_use]
    pub fn formula_name(self) -> &'static str {
        match self {
            Self::SeriesImpedance => "series_susceptance",
            Self::Matpower => "tap_adjusted_reactance",
            Self::ReactanceOnly => "reactance_only",
        }
    }

    /// The convention for one stable formula name, `None` for an unknown
    /// name. Accepts the storage aliases (`series`, `matpower`).
    #[must_use]
    pub fn from_formula_name(name: &str) -> Option<Self> {
        match name {
            "series_susceptance" | "series" => Some(Self::SeriesImpedance),
            "tap_adjusted_reactance" | "matpower" => Some(Self::Matpower),
            "reactance_only" => Some(Self::ReactanceOnly),
            _ => None,
        }
    }
}

/// Assemble [`DcNetworkData`]: in-service branches in table order, self
/// loops and formula degenerate branches reported as omitted rows by stable
/// ID, never dropped silently and never replaced with an epsilon impedance.
#[must_use]
pub fn dc_network_data(
    view: &crate::IndexedNetwork<'_>,
    convention: DcConvention,
) -> DcNetworkData {
    let network = view.network();
    let n = view.n();
    let mut data = DcNetworkData {
        from_index: Vec::new(),
        to_index: Vec::new(),
        susceptance: Vec::new(),
        shift_injection: vec![0.0; n],
        row_ids: Vec::new(),
        bus_ids: network
            .buses()
            .iter()
            .map(|bus| bus.id.0.to_string())
            .collect(),
        omitted: Vec::new(),
        formula: convention.formula_name(),
    };
    for (idx, branch) in network.branches().iter().enumerate() {
        let id = branch
            .uid
            .clone()
            .unwrap_or_else(|| format!("branches:{idx}"));
        if !branch.in_service {
            data.omitted.push((id, "out of service".to_owned()));
            continue;
        }
        let (Some(i), Some(j)) = (view.bus_index(branch.from), view.bus_index(branch.to)) else {
            data.omitted
                .push((id, "references an undeclared bus".to_owned()));
            continue;
        };
        if i == j {
            data.omitted.push((id, "self loop".to_owned()));
            continue;
        }
        if branch.x.abs() < MIN_DIVISIBLE_MAGNITUDE {
            data.omitted.push((
                id,
                "zero impedance: the selected formula has no finite susceptance".to_owned(),
            ));
            continue;
        }
        let tap = match branch.divisible_tap(idx) {
            Ok(tap) => tap,
            Err(error) => {
                data.omitted.push((id, error.to_string()));
                continue;
            }
        };
        let b = convention.branch_susceptance(branch.r, branch.x, tap);
        if !b.is_finite() {
            data.omitted
                .push((id, "susceptance is not finite".to_owned()));
            continue;
        }
        let shift = if convention.includes_phase_shifts() {
            view.angle_radians(branch.shift)
        } else {
            0.0
        };
        if shift != 0.0 {
            data.shift_injection[i] -= b * shift;
            data.shift_injection[j] += b * shift;
        }
        data.from_index.push(i);
        data.to_index.push(j);
        data.susceptance.push(b);
        data.row_ids.push(id);
    }
    data
}

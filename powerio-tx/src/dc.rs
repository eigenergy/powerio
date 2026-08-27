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
    /// every included and omitted row, the per row shift in radians, and the
    /// shift injection `p_shift = -A' (b .* shift)`.
    #[test]
    fn dc_network_data_maps_rows_and_omissions() {
        let network = three_bus_network();
        let view = crate::IndexedNetwork::new(&network);
        let data = dc_network_data(&view, DcConvention::SeriesImpedance);
        assert_eq!(data.formula, "series_susceptance");
        assert_eq!(data.from_indices, vec![0, 1]);
        assert_eq!(data.to_indices, vec![1, 2]);
        assert_eq!(data.row_ids, vec!["branches:0", "branches:1"]);
        assert_eq!(data.bus_ids, vec!["1", "2", "3"]);
        assert_eq!(data.omitted.len(), 1);
        assert_eq!(data.omitted[0].0, "branches:2");
        assert!(data.omitted[0].1.contains("out of service"));

        let b = data.susceptance[1];
        assert!((b - 5.0).abs() < 1e-12);
        let shift = 30.0_f64.to_radians();
        assert!(data.shift[0].abs() < 1e-15);
        assert!((data.shift[1] - shift).abs() < 1e-12);
        assert!((data.shift_injection[1] - (-b * shift)).abs() < 1e-12);
        assert!((data.shift_injection[2] - (b * shift)).abs() < 1e-12);
        assert!(data.shift_injection[0].abs() < 1e-15);
    }

    /// The degeneracy bound follows the selected formula: a purely resistive
    /// branch has a finite (zero) series susceptance and stays an included
    /// row under the series formula, while the two reactance formulas omit
    /// it; a branch with no impedance at all is omitted under every formula.
    #[test]
    fn the_degeneracy_bound_follows_the_formula() {
        use crate::{Branch, Bus, BusId, BusType};
        let mut resistive = Branch::new(BusId(1), BusId(2), 0.05, 0.0);
        resistive.uid = Some("resistive".to_owned());
        let mut nothing = Branch::new(BusId(2), BusId(3), 0.0, 0.0);
        nothing.uid = Some("nothing".to_owned());
        let network = crate::BalancedNetwork::in_memory(
            "dc-degenerate",
            100.0,
            vec![
                Bus::new(BusId(1), BusType::Ref, 230.0),
                Bus::new(BusId(2), BusType::Pq, 230.0),
                Bus::new(BusId(3), BusType::Pq, 230.0),
            ],
            vec![resistive, nothing],
        );
        let view = crate::IndexedNetwork::new(&network);

        let series = dc_network_data(&view, DcConvention::SeriesImpedance);
        assert_eq!(series.row_ids, vec!["resistive"]);
        assert_eq!(series.from_indices, vec![0]);
        assert_eq!(series.to_indices, vec![1]);
        assert!(series.susceptance[0].abs() < 1e-15);
        assert_eq!(series.omitted.len(), 1);
        assert_eq!(series.omitted[0].0, "nothing");

        for convention in [DcConvention::Matpower, DcConvention::ReactanceOnly] {
            let data = dc_network_data(&view, convention);
            assert!(data.row_ids.is_empty(), "{convention:?}");
            let omitted: Vec<&str> = data.omitted.iter().map(|(id, _)| id.as_str()).collect();
            assert_eq!(omitted, vec!["resistive", "nothing"], "{convention:?}");
            for (_, reason) in &data.omitted {
                assert!(reason.contains("reactance"), "{reason}");
            }
        }
    }

    /// A three winding transformer's star bus and winding branches appear in
    /// every table: `bus_ids` matches the incidence column count and the
    /// winding rows are included or omitted, never absent.
    #[test]
    fn three_winding_expansion_keeps_every_table_aligned() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/data/psse/case3_3w_v33.raw"
        );
        let source = powerio_core::Source::open(std::path::Path::new(path)).unwrap();
        let module =
            crate::parse(source.with_format(powerio_core::FormatId::new("psse").unwrap())).unwrap();
        let network = module.value();
        let view = crate::IndexedNetwork::new(network);
        let data = dc_network_data(&view, DcConvention::SeriesImpedance);
        assert_eq!(data.bus_ids.len(), view.n());
        // Three declared buses plus the synthetic star bus.
        assert_eq!(data.bus_ids.len(), 4);
        assert!(
            data.row_ids.len() + data.omitted.len() >= 3,
            "winding branches missing: {} rows, {} omitted",
            data.row_ids.len(),
            data.omitted.len()
        );
        for index in &data.from_indices {
            assert!(*index < data.bus_ids.len());
        }
        for index in &data.to_indices {
            assert!(*index < data.bus_ids.len());
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
/// phase shift injection is `p_shift = -A' * (b .* shift)` per bus (the
/// MATPOWER `makeBdc` sign), and the complete affine branch flow is
/// `p_branch = -b .* (va_from - va_to) - b .* shift`, so
/// `A' * p_branch` equals the angle terms plus `shift_injection`. Rows and
/// columns describe the analysis network after three winding transformer
/// expansion.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct DcNetworkData {
    /// From bus column per included row.
    pub from_indices: Vec<usize>,
    /// To bus column per included row.
    pub to_indices: Vec<usize>,
    /// Branch susceptance per included row.
    pub susceptance: Vec<f64>,
    /// Phase shift angle per included row, radians; `0` for an unshifted
    /// branch or a formula that excludes shifts.
    pub shift: Vec<f64>,
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
/// The degeneracy bound applies to the magnitude the selected formula
/// actually divides by: the series impedance magnitude for
/// [`DcConvention::SeriesImpedance`], the reactance alone for the two
/// reactance formulas.
#[must_use]
pub fn dc_network_data(
    view: &crate::IndexedNetwork<'_>,
    convention: DcConvention,
) -> DcNetworkData {
    let network = view.network();
    let n = view.n();
    let mut data = DcNetworkData {
        from_indices: Vec::new(),
        to_indices: Vec::new(),
        susceptance: Vec::new(),
        shift: Vec::new(),
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
        let degenerate = match convention {
            DcConvention::SeriesImpedance => branch.r.hypot(branch.x) < MIN_DIVISIBLE_MAGNITUDE,
            DcConvention::Matpower | DcConvention::ReactanceOnly => {
                branch.x.abs() < MIN_DIVISIBLE_MAGNITUDE
            }
        };
        if degenerate {
            let reason = match convention {
                DcConvention::SeriesImpedance => {
                    "zero impedance: the series impedance magnitude is below the divisibility \
                     floor"
                }
                DcConvention::Matpower | DcConvention::ReactanceOnly => {
                    "zero reactance: the selected formula divides by reactance"
                }
            };
            data.omitted.push((id, reason.to_owned()));
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
        let row_shift = if convention.includes_phase_shifts() {
            view.angle_radians(branch.shift)
        } else {
            0.0
        };
        if row_shift != 0.0 {
            data.shift_injection[i] -= b * row_shift;
            data.shift_injection[j] += b * row_shift;
        }
        data.from_indices.push(i);
        data.to_indices.push(j);
        data.susceptance.push(b);
        data.shift.push(row_shift);
        data.row_ids.push(id);
    }
    data
}

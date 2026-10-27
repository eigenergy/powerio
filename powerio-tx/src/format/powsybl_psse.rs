//! The rules PowSybl's psse-converter applies between IIDM and PSS/E, shared by
//! the XIIDM and PSS/E readers and writers so the two directions stay inverse.
//!
//! Each rule cites the powsybl-core `psse-converter` class and method it
//! follows (package `com.powsybl.psse.converter`).

use std::collections::HashMap;

use powerio_core::ComponentId;

use crate::network::{
    BalancedNetwork, BranchRatingSet, BusId, LoadingLimits, TapChangerKind,
    TapChangerRegulationMode, TransformerControl, TransformerControlMode,
};

/// The apparent power of a current limit at a nominal voltage:
/// `√3 · Vnom · I / 1000` MVA for `I` in A and `Vnom` in kV
/// (`AbstractConverter.convertToMva`).
pub(super) fn current_limit_mva(current_a: f64, nominal_kv: f64) -> f64 {
    current_a / 1000.0 * 3f64.sqrt() * nominal_kv
}

/// The MVA ratings of one set of current limits, permanent and temporary,
/// sorted ascending as PowSybl fills `RATE1` to `RATE12`
/// (`AbstractConverter.getSortedRates(CurrentLimits, double)` and
/// `setSortedRatesToPsseRates`). A temporary limit with no value, which IIDM
/// reads as unlimited, states no rating.
pub(super) fn sorted_current_rates(limits: &LoadingLimits, nominal_kv: f64) -> Vec<f64> {
    let mut rates = limits
        .permanent_limit
        .into_iter()
        .chain(limits.temporary_limits.iter().map(|limit| limit.value))
        .filter(|value| value.is_finite() && *value > 0.0 && *value < f64::MAX)
        .map(|value| current_limit_mva(value, nominal_kv))
        .collect::<Vec<_>>();
    rates.sort_by(f64::total_cmp);
    rates
}

/// The MVA ratings PSS/E states for each branch that carries current limits
/// and no MVA rating. PowSybl's exporter takes the selected current limits of
/// side 1 at its nominal voltage, and side 2's only when side 1 has none
/// (`LineConverter.createRates`, `TransformerConverter.createRates`). The
/// selected groups retained from an IIDM source give every temporary limit;
/// otherwise the branch's own current ratings stand in, at the side 1 bus
/// voltage.
///
/// Returns the projected network and the number of branches it rated, or
/// `None` when no branch needs it.
pub(super) fn rate_branches_from_current_limits(
    net: &BalancedNetwork,
) -> Option<(BalancedNetwork, usize)> {
    let detailed = net.detailed_connectivity().as_deref();
    let bus_kv = net
        .buses()
        .iter()
        .map(|bus| (bus.id, bus.base_kv))
        .collect::<HashMap<BusId, f64>>();
    let mut rated = Vec::new();
    for (index, branch) in net.branches().iter().enumerate() {
        if branch.rate_a != 0.0
            || branch.rate_b != 0.0
            || branch.rate_c != 0.0
            || !branch.rating_sets.is_empty()
        {
            continue;
        }
        let side_kv = |side: u8| {
            let bus = if side == 1 { branch.from } else { branch.to };
            bus_kv.get(&bus).copied().unwrap_or(0.0)
        };
        let exact = detailed.and_then(|detailed| {
            let component = ComponentId::new("branch", branch.uid.as_deref()?).ok()?;
            (1..=2).find_map(|side| {
                detailed
                    .operational_limit_groups
                    .iter()
                    .filter(|group| {
                        group.equipment == component && group.terminal == side && group.selected
                    })
                    .find_map(|group| group.current_limits.as_ref())
                    .map(|limits| sorted_current_rates(limits, side_kv(side)))
                    .filter(|rates| !rates.is_empty())
            })
        });
        let rates = exact.or_else(|| {
            let ratings = branch.current_ratings?;
            let limits = LoadingLimits {
                permanent_limit: Some(ratings.c_rating_a),
                permanent_limit_name: None,
                temporary_limits: [ratings.c_rating_b, ratings.c_rating_c]
                    .into_iter()
                    .map(|value| crate::network::TemporaryLimit {
                        name: String::new(),
                        value,
                        acceptable_duration_seconds: 0,
                        fictitious: false,
                    })
                    .collect(),
            };
            Some(sorted_current_rates(&limits, side_kv(1))).filter(|rates| !rates.is_empty())
        });
        if let Some(rates) = rates {
            rated.push((index, rates));
        }
    }
    if rated.is_empty() {
        return None;
    }
    let count = rated.len();
    let mut projected = net.clone();
    let branches = projected.branches_mut();
    for (index, rates) in rated {
        let branch = &mut branches[index];
        let rate = |slot: usize| rates.get(slot).copied().unwrap_or(0.0);
        branch.rate_a = rate(0);
        branch.rate_b = rate(1);
        branch.rate_c = rate(2);
        branch.rating_sets = rates
            .iter()
            .enumerate()
            .skip(3)
            .map(|(slot, value)| BranchRatingSet::new(format!("RATE{}", slot + 1), *value))
            .collect();
        branch.current_ratings = None;
    }
    Some((projected, count))
}

/// Two tap values closer than this are one step (PowSybl's
/// `TransformerConverter.distance` tolerance).
const TAP_TOLERANCE: f64 = 1e-9;

/// One IIDM tap changer's control stated in PowerIO's transformer terms:
/// each step's tap ratio (a ratio tap changer, the branch `tap` convention)
/// or phase shift in degrees (a phase tap changer, the branch `shift`
/// convention), the present step, and its regulation.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TapControl {
    pub kind: TapChangerKind,
    pub steps: Vec<f64>,
    pub position: usize,
    /// Whether the present setting lies between the stated steps and was
    /// added as a step of its own.
    pub inserted_present: bool,
    pub regulation_mode: Option<TapChangerRegulationMode>,
    pub regulating: bool,
    pub regulation_value: Option<f64>,
    pub target_deadband: Option<f64>,
}

/// The IIDM tap changer PowSybl's PSS/E importer builds from a winding's
/// control (psse-converter `TransformerConverter.defineRawTapChanger`,
/// `defineTapPositionAndAdjustTapChangerToCurrentRatio`,
/// `defineVoltageControl`, and `defineActivePowerControl`):
///
/// - `NTP` steps spread evenly from `RMI` to `RMA`, a ratio tap changer for
///   `COD` 1 and 2 and a phase tap changer for `COD` 3. The present setting
///   is a step; when it lies between two, it is added as one.
/// - `COD` 1 regulates voltage at the controlled bus to `(VMA + VMI) / 2`
///   with deadband `VMA - VMI`, both scaled by that bus's nominal kV.
/// - `COD` 3 regulates active power to `(VMA + VMI) / 2` MW with deadband
///   `VMA - VMI`.
///
/// PowSybl drops `COD` 2 and `COD` 0. IIDM has a reactive power regulation
/// mode, so `COD` 2 becomes one, with the same target and deadband rule in
/// MVAr; `COD` 0 with several steps becomes a ratio tap changer that does
/// not regulate, so `RMA`, `RMI`, and `NTP` survive. A negative `COD`
/// writes the regulation with `regulating` false. `COD` 4 and 5, and a
/// control with one step, have no tap changer.
///
/// `present` is the winding's ratio or shift; `regulated_kv` is the nominal
/// voltage of the controlled bus.
pub(super) fn tap_control_from_psse(
    control: &TransformerControl,
    present: f64,
    regulated_kv: f64,
) -> Option<TapControl> {
    let kind = match control.mode {
        TransformerControlMode::Fixed
        | TransformerControlMode::Voltage
        | TransformerControlMode::ReactiveFlow => TapChangerKind::Ratio,
        TransformerControlMode::ActiveFlow => TapChangerKind::Phase,
        _ => return None,
    };
    if control.ntp <= 1 || !control.tap_min.is_finite() || !control.tap_max.is_finite() {
        return None;
    }
    let (low, high) = if control.tap_min <= control.tap_max {
        (control.tap_min, control.tap_max)
    } else {
        (control.tap_max, control.tap_min)
    };
    let increment = (high - low) / f64::from(control.ntp - 1);
    let mut steps = (0..control.ntp)
        .map(|step| low + increment * f64::from(step))
        .collect::<Vec<_>>();
    let on_step = steps
        .iter()
        .position(|value| (value - present).abs() <= TAP_TOLERANCE);
    let inserted_present = on_step.is_none();
    let position = on_step.unwrap_or_else(|| {
        let position = steps
            .iter()
            .position(|value| *value > present)
            .unwrap_or(steps.len());
        steps.insert(position, present);
        position
    });
    let middle = f64::midpoint(control.band_min, control.band_max);
    let width = control.band_max - control.band_min;
    let (regulation_mode, regulation_value, target_deadband) = match control.mode {
        TransformerControlMode::Voltage if regulated_kv > 0.0 => (
            Some(TapChangerRegulationMode::Voltage),
            Some(middle * regulated_kv),
            Some(width * regulated_kv),
        ),
        TransformerControlMode::ReactiveFlow => (
            Some(TapChangerRegulationMode::ReactivePower),
            Some(middle),
            Some(width),
        ),
        TransformerControlMode::ActiveFlow => (
            Some(TapChangerRegulationMode::ActivePower),
            Some(middle),
            Some(width),
        ),
        _ => (None, None, None),
    };
    let regulating = control.enabled
        && regulation_mode.is_some()
        && target_deadband.is_some_and(|deadband| deadband >= 0.0)
        && (regulation_mode != Some(TapChangerRegulationMode::Voltage)
            || regulation_value.is_some_and(|value| value > 0.0));
    Some(TapControl {
        kind,
        steps,
        position,
        inserted_present,
        regulation_mode,
        regulating,
        regulation_value,
        target_deadband,
    })
}

/// The winding control PowSybl's PSS/E exporter states for an IIDM tap
/// changer (psse-converter `TransformerConverter.findRatioData`): `NTP` is
/// the step count, `RMA` and `RMI` the largest and smallest step, and the
/// mode follows the regulation, the inverse of [`tap_control_from_psse`].
/// The exporter leaves `VMA` and `VMI` at their defaults; this restates them
/// from the target and deadband, so a control that crossed IIDM returns its
/// band. A tap changer with one step states no control.
pub(super) fn psse_control_from_tap(
    tap: &TapControl,
    controlled_bus: Option<BusId>,
    regulated_kv: f64,
) -> Option<TransformerControl> {
    if tap.steps.len() <= 1 {
        return None;
    }
    let mode = match (tap.kind, tap.regulation_mode) {
        (TapChangerKind::Ratio, Some(TapChangerRegulationMode::Voltage)) => {
            TransformerControlMode::Voltage
        }
        (TapChangerKind::Ratio, Some(TapChangerRegulationMode::ReactivePower)) => {
            TransformerControlMode::ReactiveFlow
        }
        (TapChangerKind::Phase, Some(TapChangerRegulationMode::ActivePower)) => {
            TransformerControlMode::ActiveFlow
        }
        _ => TransformerControlMode::Fixed,
    };
    let mut control = TransformerControl::new(mode);
    control.enabled = mode != TransformerControlMode::Fixed && tap.regulating;
    control.controlled_bus = controlled_bus;
    control.tap_min = tap.steps.iter().copied().fold(f64::INFINITY, f64::min);
    control.tap_max = tap.steps.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    control.ntp = u32::try_from(tap.steps.len()).unwrap_or(u32::MAX);
    if let Some(value) = tap.regulation_value {
        // IIDM before 1.12 states a target with no deadband.
        let deadband = tap.target_deadband.unwrap_or(0.0);
        let scale = match mode {
            TransformerControlMode::Voltage if regulated_kv > 0.0 => Some(regulated_kv),
            TransformerControlMode::ReactiveFlow | TransformerControlMode::ActiveFlow => Some(1.0),
            _ => None,
        };
        if let Some(scale) = scale {
            control.band_min = (value - deadband / 2.0) / scale;
            control.band_max = (value + deadband / 2.0) / scale;
        }
    }
    Some(control)
}

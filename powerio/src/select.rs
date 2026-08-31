//! Typed time and scenario inventory, selection, and export over
//! [`PioValue`].
//!
//! Selection returns the existing typed item and preserves the collection's
//! shared network and numerical owners: a network item borrows, and an
//! operating point item is the series' own small handle. Nothing here applies
//! an update map, serializes through `.pio.json`, or selects a base state
//! implicitly. Turning a selected item into an independent static module is
//! the separate explicit [`export_state`] operation. When the collection is
//! already in a module, [`export_module_state`] also carries its common
//! records through the export.

use powerio_core::{HistoryEntry, HistoryId, HistoryKind, PioModule};

use crate::BalancedNetwork;
use crate::codes;
use crate::value::PioValue;

type Error = powerio_core::Error;
type Result<T> = std::result::Result<T, Error>;

/// One entry of a value's time inventory.
#[derive(Clone, Debug, PartialEq)]
pub struct TimePointEntry {
    /// Zero based position, the selection key.
    pub position: usize,
    pub label: String,
    pub duration: Option<std::time::Duration>,
}

/// One entry of a value's scenario inventory.
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioEntry {
    /// Case sensitive scenario ID, the selection key.
    pub id: String,
    pub probability: Option<f64>,
}

/// The typed state inventory of one value: exactly what can be selected.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum StateInventory {
    TimePoints(Vec<TimePointEntry>),
    Scenarios(Vec<ScenarioEntry>),
}

/// One selection key: a time position or a case sensitive scenario ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StateSelector<'a> {
    TimePosition(usize),
    Scenario(&'a str),
}

impl std::fmt::Display for StateSelector<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimePosition(position) => write!(formatter, "time position {position}"),
            Self::Scenario(id) => write!(formatter, "scenario `{id}`"),
        }
    }
}

/// One selected item, borrowed from its collection.
#[derive(Debug)]
#[non_exhaustive]
pub enum SelectedState<'value> {
    /// A stored network item: a series entry or a scenario value.
    BalancedNetwork(&'value BalancedNetwork),
    /// An operating point item: the series' own handle over the shared
    /// network and columns. Cloning it copies no table and no column.
    BalancedOperatingPoint(&'value powerio_prob::OperatingPoint<BalancedNetwork>),
    /// A multiconductor operating point item, the same handle shape over the
    /// shared multiconductor network.
    MulticonductorOperatingPoint(
        &'value powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>,
    ),
}

/// The typed inventory of `value`, or a coded refusal for a static value.
///
/// # Errors
/// The value holds no time or scenario collection.
pub fn list_states(value: &PioValue) -> Result<StateInventory> {
    match value {
        PioValue::BalancedNetworkTimeSeries(series) => Ok(StateInventory::TimePoints(
            time_entries(series.time_points()),
        )),
        PioValue::BalancedOperatingPointTimeSeries(series) => Ok(StateInventory::TimePoints(
            time_entries(series.time_points()),
        )),
        PioValue::MulticonductorOperatingPointTimeSeries(series) => Ok(StateInventory::TimePoints(
            time_entries(series.time_points()),
        )),
        PioValue::BalancedNetworkScenarioSet(set) => Ok(StateInventory::Scenarios(
            set.iter()
                .map(|scenario| ScenarioEntry {
                    id: scenario.id().as_str().to_owned(),
                    probability: scenario.probability(),
                })
                .collect(),
        )),
        other => Err(not_a_collection(other)),
    }
}

/// Select one existing typed item. No clone, no serialization, no implicit
/// base state.
///
/// # Errors
/// A static value, a selector kind the value does not key by, a position
/// outside the time axis, or an unknown scenario ID.
pub fn select_state<'value>(
    value: &'value PioValue,
    selector: StateSelector<'_>,
) -> Result<SelectedState<'value>> {
    match (value, selector) {
        (PioValue::BalancedNetworkTimeSeries(series), StateSelector::TimePosition(position)) => {
            let item = series
                .value(position)
                .ok_or_else(|| out_of_range(position, series.len()))?;
            Ok(SelectedState::BalancedNetwork(item))
        }
        (
            PioValue::BalancedOperatingPointTimeSeries(series),
            StateSelector::TimePosition(position),
        ) => {
            let item = series
                .value(position)
                .ok_or_else(|| out_of_range(position, series.len()))?;
            Ok(SelectedState::BalancedOperatingPoint(item))
        }
        (
            PioValue::MulticonductorOperatingPointTimeSeries(series),
            StateSelector::TimePosition(position),
        ) => {
            let item = series
                .value(position)
                .ok_or_else(|| out_of_range(position, series.len()))?;
            Ok(SelectedState::MulticonductorOperatingPoint(item))
        }
        (PioValue::BalancedNetworkScenarioSet(set), StateSelector::Scenario(id)) => {
            let scenario = set.get(id).ok_or_else(|| unknown_scenario(id, set))?;
            Ok(SelectedState::BalancedNetwork(scenario.value()))
        }
        (
            PioValue::BalancedNetworkTimeSeries(_)
            | PioValue::BalancedOperatingPointTimeSeries(_)
            | PioValue::MulticonductorOperatingPointTimeSeries(_),
            StateSelector::Scenario(_),
        )
        | (PioValue::BalancedNetworkScenarioSet(_), StateSelector::TimePosition(_)) => {
            Err(Error::new(
                &codes::REQUEST_STATE_WRONG_SELECTOR,
                format!(
                    "a {} value keys by {}; the request named {selector}",
                    value.kind().as_str(),
                    match value {
                        PioValue::BalancedNetworkScenarioSet(_) => "scenario ID",
                        _ => "time position",
                    }
                ),
            ))
        }
        (other, _) => Err(not_a_collection(other)),
    }
}

/// Export one selected item as an independent static module. This is the
/// explicit materialization step: a stored network item shares its tables
/// through the cheap handle clone, and an operating point item writes its
/// stated quantities into the shared network typed, copying only the touched
/// tables. The module's history states the selection.
///
/// # Errors
/// Everything [`select_state`] refuses, plus an operating point quantity the
/// static tables cannot carry.
pub fn export_state(value: &PioValue, selector: StateSelector<'_>) -> Result<PioModule<PioValue>> {
    let exported = exported_value(value, selector)?;
    let mut module = PioModule::new(exported);
    append_selection_history(&mut module, value.kind().as_str(), selector)?;
    Ok(module)
}

/// Export one selected item from `module` as an independent static module.
///
/// Producer, source descriptors, diagnostics, history, and extensions carry
/// forward. The retained source is severed because its bytes describe the
/// collection, not the selected static value. The value kind changes, so
/// source map entries are cleared and diagnostic targets are severed; their
/// codes, messages, severities, and source spans remain. A Transform history
/// entry records the selection.
///
/// # Errors
/// Everything [`export_state`] refuses, or a common record that cannot be
/// copied or appended under the module record limits.
pub fn export_module_state(
    module: &PioModule<PioValue>,
    selector: StateSelector<'_>,
) -> Result<PioModule<PioValue>> {
    let source_kind = module.value().kind().as_str();
    let exported = exported_value(module.value(), selector)?;
    let mut out = PioModule::new(exported).with_producer(module.producer().clone());
    for descriptor in module.sources() {
        out.add_source_descriptor(descriptor.clone())?;
    }
    for entry in module.source_map() {
        out.add_source_map_entry(entry.clone())?;
    }
    for diagnostic in module.diagnostics() {
        out.add_diagnostic(diagnostic.clone())?;
    }
    for entry in module.history() {
        out.add_history_entry(entry.clone())?;
    }
    for (namespace, value) in module.extensions() {
        out.insert_extension(namespace.clone(), value.clone())?;
    }
    // Do not copy `module.source()`: the retained bytes describe the
    // collection. The selected value has a different kind, so no RFC 6901
    // target into the old value remains valid.
    out.sever_value_targets();
    append_selection_history(&mut out, source_kind, selector)?;
    Ok(out)
}

fn exported_value(value: &PioValue, selector: StateSelector<'_>) -> Result<PioValue> {
    let network = match select_state(value, selector)? {
        SelectedState::BalancedNetwork(network) => network.clone(),
        SelectedState::BalancedOperatingPoint(point) => point.materialize_network()?,
        SelectedState::MulticonductorOperatingPoint(_) => {
            return Err(Error::new(
                &codes::REQUEST_STATE_UNBOUND_EXPORT,
                "a multiconductor operating point selects and reads in place; its static \
                 materialization is not bound yet"
                    .to_string(),
            ));
        }
    };
    Ok(PioValue::BalancedNetwork(network))
}

fn append_selection_history(
    module: &mut PioModule<PioValue>,
    source_kind: &str,
    selector: StateSelector<'_>,
) -> Result<()> {
    let entry = HistoryEntry::new(
        unused_history_id(module, "export-selected-state"),
        HistoryKind::Transform,
        "export_selected_state",
    )
    .and_then(|entry| {
        entry.with_assumption(format!(
            "static export of {selector} from a {source_kind} value"
        ))
    })?;
    module.add_history_entry(entry)?;
    Ok(())
}

fn unused_history_id(module: &PioModule<PioValue>, base: &str) -> HistoryId {
    let taken: std::collections::BTreeSet<&str> = module
        .history()
        .iter()
        .map(|entry| entry.id().as_str())
        .collect();
    if !taken.contains(base) {
        return HistoryId::new(base).expect("static history ID is valid");
    }
    let mut counter = 2usize;
    loop {
        let candidate = format!("{base}-{counter}");
        if !taken.contains(candidate.as_str()) {
            return HistoryId::new(candidate).expect("numbered history ID is valid");
        }
        counter += 1;
    }
}

fn time_entries(points: &[powerio_core::TimePoint]) -> Vec<TimePointEntry> {
    points
        .iter()
        .enumerate()
        .map(|(position, point)| TimePointEntry {
            position,
            label: point.label().to_owned(),
            duration: point.duration(),
        })
        .collect()
}

fn not_a_collection(value: &PioValue) -> Error {
    Error::new(
        &codes::REQUEST_STATE_NOT_A_COLLECTION,
        format!(
            "a {} value carries no time or scenario collection to select from; \
             a static value is used directly, never selected implicitly",
            value.kind().as_str()
        ),
    )
}

fn out_of_range(position: usize, len: usize) -> Error {
    Error::new(
        &codes::REQUEST_STATE_OUT_OF_RANGE,
        format!("time position {position} is outside the {len} point axis"),
    )
}

fn unknown_scenario(id: &str, set: &powerio_core::ScenarioSet<BalancedNetwork>) -> Error {
    let mut known: Vec<&str> = set.iter().map(|scenario| scenario.id().as_str()).collect();
    known.sort_unstable();
    Error::new(
        &codes::REQUEST_STATE_UNKNOWN_SCENARIO,
        format!(
            "scenario `{id}` is not in the set; the set declares {}",
            known.join(", ")
        ),
    )
}

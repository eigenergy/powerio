//! Typed time and scenario inventory, selection, and export over
//! [`PioValue`].
//!
//! Selection returns the existing typed item and preserves the collection's
//! shared network and numerical owners: a network item borrows, and an
//! operating point item is the series' own small handle. Nothing here applies
//! an update map, serializes through `.pio.json`, or selects a base state
//! implicitly. Turning a selected item into an independent static module is
//! the separate explicit [`export_state`] operation.

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
}

/// The typed inventory of `value`, or a coded refusal for a static value.
///
/// # Errors
/// The value holds no time or scenario collection.
pub fn state_inventory(value: &PioValue) -> Result<StateInventory> {
    match value {
        PioValue::BalancedNetworkTimeSeries(series) => Ok(StateInventory::TimePoints(
            time_entries(series.time_points()),
        )),
        PioValue::BalancedOperatingPointTimeSeries(series) => Ok(StateInventory::TimePoints(
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
        (PioValue::BalancedNetworkScenarioSet(set), StateSelector::Scenario(id)) => {
            let scenario = set.get(id).ok_or_else(|| unknown_scenario(id, set))?;
            Ok(SelectedState::BalancedNetwork(scenario.value()))
        }
        (
            PioValue::BalancedNetworkTimeSeries(_) | PioValue::BalancedOperatingPointTimeSeries(_),
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
    let network = match select_state(value, selector)? {
        SelectedState::BalancedNetwork(network) => network.clone(),
        SelectedState::BalancedOperatingPoint(point) => point.materialize_network()?,
    };
    let mut module = PioModule::new(PioValue::BalancedNetwork(network));
    let entry = HistoryId::new("export-selected-state")
        .and_then(|id| HistoryEntry::new(id, HistoryKind::Transform, "export_selected_state"))
        .and_then(|entry| {
            entry.with_assumption(format!(
                "static export of {selector} from a {} value",
                value.kind().as_str()
            ))
        })?;
    module.add_history_entry(entry)?;
    Ok(module)
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

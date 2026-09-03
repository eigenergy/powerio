//! The one bridge between a runtime module and PowerIO 1.0 IR.

use std::collections::{BTreeMap, HashSet};

use powerio_core::{
    Diagnostic, DiagnosticCode, DiagnosticId, DiagnosticSeverity, Digest, HistoryEntry, HistoryId,
    HistoryKind, PioModule, Producer, SourceDescriptor, SourceId, SourceMapEntry, SourceRelation,
    SourceSpan, TimePoint, TimeSeries,
};

use super::dto::{self, StoredF64, StoredModule, StoredQuantity};
use crate::codes;
use crate::value::{PioScenarioSet, PioTimeSeries, PioValue};
use powerio_prob::{
    BalancedOperatingPointFlag, BalancedOperatingPointQuantity, MulticonductorOperatingPointFlag,
    MulticonductorOperatingPointQuantity,
};

type Result<T> = std::result::Result<T, powerio_core::Error>;

fn invalid(message: impl Into<String>) -> powerio_core::Error {
    powerio_core::Error::new(&codes::READ_MODULE_INVALID, message)
}

fn with_component_ids(mut network: crate::BalancedNetwork) -> crate::BalancedNetwork {
    network.assign_missing_component_ids();
    network
}

/// Serialize one runtime module to the PowerIO 1.0 IR document.
///
/// # Errors
/// A value whose stored form cannot be produced, or a serialization failure.
pub fn emit_module(module: &PioModule<PioValue>) -> Result<String> {
    let stored = StoredModule {
        schema: dto::SCHEMA_NAME.to_string(),
        version: dto::SCHEMA_VERSION,
        producer: dto::Producer {
            name: module.producer().name().to_string(),
            version: module.producer().version().to_string(),
        },
        value: encode_value(&module.value)?,
        sources: module.sources().iter().map(encode_source).collect(),
        source_map: module
            .source_map()
            .iter()
            .map(encode_map_entry)
            .collect::<Result<_>>()?,
        diagnostics: encode_diagnostics(&module.diagnostics),
        history: module
            .history()
            .iter()
            .map(encode_history)
            .collect::<Result<_>>()?,
        extensions: module.extensions().clone(),
    };
    dto::validate(&stored).map_err(invalid)?;
    serde_json::to_string_pretty(&stored).map_err(|error| invalid(error.to_string()))
}

/// Decode one PowerIO 1.0 IR document. Other schemas and versions are refused.
///
/// # Errors
/// An unsupported schema or version, or an invalid document.
pub fn read_module(text: &str) -> Result<PioModule<PioValue>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let header: dto::StoredHeader =
        serde_json::from_str(text).map_err(|error| invalid(error.to_string()))?;
    match (header.schema.as_deref(), header.version) {
        (Some(dto::SCHEMA_NAME), Some(dto::SCHEMA_VERSION)) => {
            let stored: StoredModule =
                serde_json::from_str(text).map_err(|error| invalid(error.to_string()))?;
            dto::validate(&stored).map_err(invalid)?;
            decode_stored(stored)
        }
        (Some(schema), version) => Err(powerio_core::Error::new(
            &codes::READ_MODULE_UNSUPPORTED,
            format!(
                "unsupported stored module `{schema}` version {}",
                version.map_or_else(|| "<none>".to_string(), |v| v.to_string())
            ),
        )),
        (None, _) => Err(powerio_core::Error::new(
            &codes::READ_MODULE_UNSUPPORTED,
            "the document is not PowerIO 1.0 IR",
        )),
    }
}

// ---- value encoding ---------------------------------------------------------

fn encode_value(value: &PioValue) -> Result<dto::StoredValue> {
    Ok(match value {
        PioValue::BalancedNetwork(network) => {
            dto::StoredValue::BalancedNetwork(Box::new(with_component_ids(network.clone())))
        }
        PioValue::MulticonductorNetwork(network) => {
            dto::StoredValue::MulticonductorNetwork(Box::new(network.clone()))
        }
        PioValue::GeoLayer(layer) => dto::StoredValue::GeoLayer(Box::new(layer.clone())),
        PioValue::BalancedOperatingPoint(point) => {
            dto::StoredValue::BalancedOperatingPoint(encode_balanced_point(point)?)
        }
        PioValue::MulticonductorOperatingPoint(point) => {
            dto::StoredValue::MulticonductorOperatingPoint(encode_mc_point(point))
        }
        PioValue::TimeSeries(series) => encode_time_series(series)?,
        PioValue::ScenarioSet(set) => encode_scenario_set(set)?,
        PioValue::DcPfInstance(instance) => {
            dto::StoredValue::DcPfInstance(encode_dc_pf_instance(instance)?)
        }
        PioValue::AcPfInstance(instance) => {
            dto::StoredValue::AcPfInstance(encode_ac_pf_instance(instance)?)
        }
        PioValue::DcOpfInstance(instance) => {
            dto::StoredValue::DcOpfInstance(encode_dc_opf_instance(instance)?)
        }
        PioValue::AcOpfInstance(instance) => {
            dto::StoredValue::AcOpfInstance(encode_ac_opf_instance(instance)?)
        }
        PioValue::McAcPfInstance(instance) => {
            dto::StoredValue::McAcPfInstance(encode_mc_ac_pf_instance(instance))
        }
        PioValue::McAcOpfInstance(instance) => {
            dto::StoredValue::McAcOpfInstance(encode_mc_ac_opf_instance(instance)?)
        }
        PioValue::AcScucInstance(instance) => {
            dto::StoredValue::AcScucInstance(dto::AcScucInstance {
                network: Box::new(with_component_ids(instance.network().clone())),
                inputs: Box::new(instance.inputs().clone()),
            })
        }
        PioValue::DcPfSolution(solution) => {
            dto::StoredValue::DcPfSolution(Box::new(encode_dc_pf_solution(solution)?))
        }
        PioValue::AcPfSolution(solution) => {
            dto::StoredValue::AcPfSolution(Box::new(encode_ac_pf_solution(solution)?))
        }
        PioValue::DcOpfSolution(solution) => {
            dto::StoredValue::DcOpfSolution(Box::new(encode_dc_opf_solution(solution)?))
        }
        PioValue::AcOpfSolution(solution) => {
            dto::StoredValue::AcOpfSolution(Box::new(encode_ac_opf_solution(solution)?))
        }
        PioValue::SocwrOpfSolution(solution) => {
            dto::StoredValue::SocwrOpfSolution(Box::new(encode_socwr_opf_solution(solution)?))
        }
        PioValue::McAcPfSolution(solution) => {
            dto::StoredValue::McAcPfSolution(Box::new(encode_mc_ac_pf_solution(solution)))
        }
        PioValue::McAcOpfSolution(solution) => {
            dto::StoredValue::McAcOpfSolution(Box::new(encode_mc_ac_opf_solution(solution)?))
        }
        PioValue::AcScucSolution(solution) => {
            dto::StoredValue::AcScucSolution(Box::new(encode_ac_scuc_solution(solution)))
        }
    })
}

const BALANCED_NETWORK_TYPE: &str = "powerio.BalancedNetwork";
const MULTICONDUCTOR_NETWORK_TYPE: &str = "powerio.MulticonductorNetwork";
const BALANCED_POINT_TYPE: &str = "powerio.OperatingPoint<powerio.BalancedNetwork>";
const MULTICONDUCTOR_POINT_TYPE: &str = "powerio.OperatingPoint<powerio.MulticonductorNetwork>";
const BALANCED_NETWORK_SERIES_TYPE: &str = "powerio.TimeSeries<powerio.BalancedNetwork>";
const MULTICONDUCTOR_NETWORK_SERIES_TYPE: &str =
    "powerio.TimeSeries<powerio.MulticonductorNetwork>";
const BALANCED_POINT_SERIES_TYPE: &str =
    "powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>";
const MULTICONDUCTOR_POINT_SERIES_TYPE: &str =
    "powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>";

fn encode_time_series(series: &PioTimeSeries) -> Result<dto::StoredValue> {
    Ok(match series.element_type() {
        BALANCED_NETWORK_TYPE => {
            dto::StoredValue::BalancedNetworkTimeSeries(encode_balanced_network_series(series)?)
        }
        MULTICONDUCTOR_NETWORK_TYPE => dto::StoredValue::MulticonductorNetworkTimeSeries(
            encode_multiconductor_network_series(series)?,
        ),
        BALANCED_POINT_TYPE => dto::StoredValue::BalancedOperatingPointTimeSeries(
            encode_balanced_point_series(series)?,
        ),
        MULTICONDUCTOR_POINT_TYPE => dto::StoredValue::MulticonductorOperatingPointTimeSeries(
            encode_multiconductor_point_series(series)?,
        ),
        other => {
            return Err(invalid(format!(
                "PowerIO IR cannot encode a time series of `{other}`"
            )));
        }
    })
}

fn encode_balanced_network_series(
    series: &PioTimeSeries,
) -> Result<dto::StoredTimeSeries<crate::BalancedNetwork>> {
    let values = series
        .values()
        .values()
        .iter()
        .map(|value| match value {
            PioValue::BalancedNetwork(network) => Ok(with_component_ids(network.clone())),
            other => Err(collection_element_mismatch(series.element_type(), other)),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(dto::StoredTimeSeries {
        time_points: series.time_points().iter().map(encode_time_point).collect(),
        values,
    })
}

fn encode_multiconductor_network_series(
    series: &PioTimeSeries,
) -> Result<dto::StoredTimeSeries<powerio_dist::MulticonductorNetwork>> {
    let values = series
        .values()
        .values()
        .iter()
        .map(|value| match value {
            PioValue::MulticonductorNetwork(network) => Ok(network.clone()),
            other => Err(collection_element_mismatch(series.element_type(), other)),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(dto::StoredTimeSeries {
        time_points: series.time_points().iter().map(encode_time_point).collect(),
        values,
    })
}

fn encode_balanced_point_series(
    series: &PioTimeSeries,
) -> Result<dto::StoredOperatingPointTimeSeries<crate::BalancedNetwork>> {
    let values = series
        .values()
        .values()
        .iter()
        .map(|value| match value {
            PioValue::BalancedOperatingPoint(point) => Ok(point.clone()),
            other => Err(collection_element_mismatch(series.element_type(), other)),
        })
        .collect::<Result<Vec<_>>>()?;
    let typed = TimeSeries::new(series.time_points().to_vec(), values)
        .map_err(|error| invalid(error.to_string()))?;
    encode_balanced_operating_point_series(&typed)
}

fn encode_multiconductor_point_series(
    series: &PioTimeSeries,
) -> Result<dto::StoredOperatingPointTimeSeries<powerio_dist::MulticonductorNetwork>> {
    let values = series
        .values()
        .values()
        .iter()
        .map(|value| match value {
            PioValue::MulticonductorOperatingPoint(point) => Ok(point.clone()),
            other => Err(collection_element_mismatch(series.element_type(), other)),
        })
        .collect::<Result<Vec<_>>>()?;
    let typed = TimeSeries::new(series.time_points().to_vec(), values)
        .map_err(|error| invalid(error.to_string()))?;
    encode_multiconductor_operating_point_series(&typed)
}

fn encode_scenario_entries<T>(
    set: &PioScenarioSet,
    mut encode: impl FnMut(&PioValue) -> Result<T>,
) -> Result<dto::StoredScenarioSet<T>> {
    let scenarios = set
        .values()
        .iter()
        .map(|scenario| {
            Ok(dto::StoredScenario {
                id: scenario.id().as_str().to_string(),
                probability: scenario.probability().map(StoredF64),
                value: encode(scenario.value())?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(dto::StoredScenarioSet { scenarios })
}

fn encode_scenario_set(set: &PioScenarioSet) -> Result<dto::StoredValue> {
    Ok(match set.element_type() {
        BALANCED_NETWORK_TYPE => {
            dto::StoredValue::BalancedNetworkScenarioSet(encode_scenario_entries(set, |value| {
                match value {
                    PioValue::BalancedNetwork(network) => Ok(with_component_ids(network.clone())),
                    other => Err(collection_element_mismatch(set.element_type(), other)),
                }
            })?)
        }
        MULTICONDUCTOR_NETWORK_TYPE => dto::StoredValue::MulticonductorNetworkScenarioSet(
            encode_scenario_entries(set, |value| match value {
                PioValue::MulticonductorNetwork(network) => Ok(network.clone()),
                other => Err(collection_element_mismatch(set.element_type(), other)),
            })?,
        ),
        BALANCED_POINT_TYPE => dto::StoredValue::BalancedOperatingPointScenarioSet(
            encode_balanced_point_scenarios(set)?,
        ),
        MULTICONDUCTOR_POINT_TYPE => dto::StoredValue::MulticonductorOperatingPointScenarioSet(
            encode_multiconductor_point_scenarios(set)?,
        ),
        BALANCED_NETWORK_SERIES_TYPE => dto::StoredValue::BalancedNetworkTimeSeriesScenarioSet(
            encode_scenario_entries(set, |value| {
                encode_balanced_network_series(expect_series(value, set.element_type())?)
            })?,
        ),
        MULTICONDUCTOR_NETWORK_SERIES_TYPE => {
            dto::StoredValue::MulticonductorNetworkTimeSeriesScenarioSet(encode_scenario_entries(
                set,
                |value| {
                    encode_multiconductor_network_series(expect_series(value, set.element_type())?)
                },
            )?)
        }
        BALANCED_POINT_SERIES_TYPE => {
            dto::StoredValue::BalancedOperatingPointTimeSeriesScenarioSet(encode_scenario_entries(
                set,
                |value| encode_balanced_point_series(expect_series(value, set.element_type())?),
            )?)
        }
        MULTICONDUCTOR_POINT_SERIES_TYPE => {
            dto::StoredValue::MulticonductorOperatingPointTimeSeriesScenarioSet(
                encode_scenario_entries(set, |value| {
                    encode_multiconductor_point_series(expect_series(value, set.element_type())?)
                })?,
            )
        }
        other => {
            return Err(invalid(format!(
                "PowerIO IR cannot encode a scenario set of `{other}`"
            )));
        }
    })
}

fn expect_series<'a>(value: &'a PioValue, declared: &str) -> Result<&'a PioTimeSeries> {
    match value {
        PioValue::TimeSeries(series) => Ok(series),
        other => Err(collection_element_mismatch(declared, other)),
    }
}

fn collection_element_mismatch(declared: &str, value: &PioValue) -> powerio_core::Error {
    invalid(format!(
        "collection declares `{declared}` elements but contains `{}`",
        value.type_name()
    ))
}

const BALANCED_NUMERIC_QUANTITIES: [BalancedOperatingPointQuantity; 11] = [
    BalancedOperatingPointQuantity::BusVoltageMagnitude,
    BalancedOperatingPointQuantity::BusVoltageAngle,
    BalancedOperatingPointQuantity::BusActiveInjection,
    BalancedOperatingPointQuantity::BusReactiveInjection,
    BalancedOperatingPointQuantity::GeneratorActivePower,
    BalancedOperatingPointQuantity::GeneratorReactivePower,
    BalancedOperatingPointQuantity::GeneratorVoltageSetpoint,
    BalancedOperatingPointQuantity::LoadActivePower,
    BalancedOperatingPointQuantity::LoadReactivePower,
    BalancedOperatingPointQuantity::BranchTapRatio,
    BalancedOperatingPointQuantity::BranchPhaseShift,
];

const BALANCED_FLAGS: [BalancedOperatingPointFlag; 3] = [
    BalancedOperatingPointFlag::GeneratorInService,
    BalancedOperatingPointFlag::BranchInService,
    BalancedOperatingPointFlag::SwitchClosed,
];

const MULTICONDUCTOR_NUMERIC_QUANTITIES: [MulticonductorOperatingPointQuantity; 6] = [
    MulticonductorOperatingPointQuantity::TerminalVoltageMagnitude,
    MulticonductorOperatingPointQuantity::TerminalVoltageAngle,
    MulticonductorOperatingPointQuantity::LoadActivePower,
    MulticonductorOperatingPointQuantity::LoadReactivePower,
    MulticonductorOperatingPointQuantity::TransformerTap,
    MulticonductorOperatingPointQuantity::CapacitorSteps,
];

const MULTICONDUCTOR_FLAGS: [MulticonductorOperatingPointFlag; 1] =
    [MulticonductorOperatingPointFlag::SwitchClosed];

/// The stable cross language DC formula names, spelled locally because this
/// branch precedes the shared helper.
fn dc_formula_name(formula: crate::BranchSusceptanceFormula) -> Result<&'static str> {
    Ok(match formula {
        crate::BranchSusceptanceFormula::TapAdjustedReactance => "tap_adjusted_reactance",
        crate::BranchSusceptanceFormula::ReactanceOnly => "reactance_only",
        crate::BranchSusceptanceFormula::SeriesSusceptance => "series_susceptance",
        // The runtime enum is non_exhaustive for additive growth; a new
        // formula must gain a stored spelling before it can be written.
        _ => return Err(invalid("unmapped branch susceptance formula")),
    })
}

fn dc_formula_from_name(name: &str) -> Result<crate::BranchSusceptanceFormula> {
    match name {
        "series_susceptance" => Ok(crate::BranchSusceptanceFormula::SeriesSusceptance),
        "tap_adjusted_reactance" => Ok(crate::BranchSusceptanceFormula::TapAdjustedReactance),
        "reactance_only" => Ok(crate::BranchSusceptanceFormula::ReactanceOnly),
        other => Err(invalid(format!(
            "unknown branch susceptance formula `{other}`"
        ))),
    }
}

fn stored_numbers<'a>(values: impl IntoIterator<Item = (&'a str, f64)>) -> StoredQuantity {
    let (identities, values) = values
        .into_iter()
        .map(|(identity, value)| (dto::StoredIdentity(identity.to_string()), StoredF64(value)))
        .unzip();
    StoredQuantity { identities, values }
}

fn stored_flags<'a>(values: impl IntoIterator<Item = (&'a str, bool)>) -> StoredQuantity {
    let (identities, values) = values
        .into_iter()
        .map(|(identity, value)| {
            (
                dto::StoredIdentity(identity.to_string()),
                StoredF64(f64::from(u8::from(value))),
            )
        })
        .unzip();
    StoredQuantity { identities, values }
}

fn encode_balanced_point_assignment(
    point: &powerio_prob::OperatingPoint<crate::BalancedNetwork>,
) -> dto::StoredOperatingPointAssignment {
    let mut quantities = BTreeMap::new();
    for quantity in BALANCED_NUMERIC_QUANTITIES {
        if let Some(values) = point.values(quantity) {
            quantities.insert(quantity.name().to_string(), stored_numbers(values));
        }
    }
    for flag in BALANCED_FLAGS {
        if let Some(values) = point.flags(flag) {
            quantities.insert(flag.name().to_string(), stored_flags(values));
        }
    }
    dto::StoredOperatingPointAssignment { quantities }
}

fn encode_mc_point_assignment(
    point: &powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>,
) -> dto::StoredOperatingPointAssignment {
    let mut quantities = BTreeMap::new();
    for quantity in MULTICONDUCTOR_NUMERIC_QUANTITIES {
        if let Some(values) = point.values(quantity) {
            quantities.insert(quantity.name().to_string(), stored_numbers(values));
        }
    }
    for flag in MULTICONDUCTOR_FLAGS {
        if let Some(values) = point.flags(flag) {
            quantities.insert(flag.name().to_string(), stored_flags(values));
        }
    }
    dto::StoredOperatingPointAssignment { quantities }
}

/// Assign the persistent identities written with a balanced network while
/// retaining the operating point's table ordered values.
fn align_balanced_quantity_identities(
    network: &crate::BalancedNetwork,
    mut quantities: BTreeMap<String, StoredQuantity>,
) -> Result<BTreeMap<String, StoredQuantity>> {
    for (name, quantity) in &mut quantities {
        let identities = balanced_identity_order(network, name)?;
        if quantity.identities.len() != identities.len() {
            return Err(invalid(format!(
                "quantity `{name}` has {} identities; the network resolves {}",
                quantity.identities.len(),
                identities.len()
            )));
        }
        quantity.identities = identities.into_iter().map(dto::StoredIdentity).collect();
    }
    Ok(quantities)
}

fn encode_balanced_point(
    point: &powerio_prob::OperatingPoint<crate::BalancedNetwork>,
) -> Result<dto::StoredOperatingPoint<crate::BalancedNetwork>> {
    let network = with_component_ids(point.network().clone());
    let quantities = align_balanced_quantity_identities(
        &network,
        encode_balanced_point_assignment(point).quantities,
    )?;
    Ok(dto::StoredOperatingPoint {
        network: Box::new(network),
        quantities,
    })
}

fn encode_mc_point(
    point: &powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>,
) -> dto::StoredOperatingPoint<powerio_dist::MulticonductorNetwork> {
    dto::StoredOperatingPoint {
        network: Box::new(point.network().clone()),
        quantities: encode_mc_point_assignment(point).quantities,
    }
}

fn ensure_same_network<'a, N: serde::Serialize + 'a>(
    expected: &N,
    networks: impl IntoIterator<Item = &'a N>,
) -> Result<()> {
    let expected = serde_json::to_vec(expected).map_err(|error| invalid(error.to_string()))?;
    for (position, network) in networks.into_iter().enumerate() {
        let candidate = serde_json::to_vec(network).map_err(|error| invalid(error.to_string()))?;
        if candidate != expected {
            return Err(invalid(format!(
                "operating point collection entry {position} uses a different base network"
            )));
        }
    }
    Ok(())
}

fn encode_balanced_operating_point_series(
    series: &TimeSeries<powerio_prob::OperatingPoint<crate::BalancedNetwork>>,
) -> Result<dto::StoredOperatingPointTimeSeries<crate::BalancedNetwork>> {
    let Some(first) = series.values().first() else {
        return Ok(dto::StoredOperatingPointTimeSeries {
            network: None,
            time_points: Vec::new(),
            values: Vec::new(),
        });
    };
    ensure_same_network(
        first.network(),
        series
            .values()
            .iter()
            .map(powerio_prob::OperatingPoint::network),
    )?;
    let network = with_component_ids(first.network().clone());
    let values = series
        .values()
        .iter()
        .map(|point| {
            align_balanced_quantity_identities(
                &network,
                encode_balanced_point_assignment(point).quantities,
            )
            .map(|quantities| dto::StoredOperatingPointAssignment { quantities })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(dto::StoredOperatingPointTimeSeries {
        network: Some(Box::new(network)),
        time_points: series.time_points().iter().map(encode_time_point).collect(),
        values,
    })
}

fn encode_multiconductor_operating_point_series(
    series: &TimeSeries<powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>>,
) -> Result<dto::StoredOperatingPointTimeSeries<powerio_dist::MulticonductorNetwork>> {
    let Some(first) = series.values().first() else {
        return Ok(dto::StoredOperatingPointTimeSeries {
            network: None,
            time_points: Vec::new(),
            values: Vec::new(),
        });
    };
    ensure_same_network(
        first.network(),
        series
            .values()
            .iter()
            .map(powerio_prob::OperatingPoint::network),
    )?;
    let values = series
        .values()
        .iter()
        .map(encode_mc_point_assignment)
        .collect();
    Ok(dto::StoredOperatingPointTimeSeries {
        network: Some(Box::new(first.network().clone())),
        time_points: series.time_points().iter().map(encode_time_point).collect(),
        values,
    })
}

fn encode_balanced_point_scenarios(
    set: &PioScenarioSet,
) -> Result<dto::StoredOperatingPointScenarioSet<crate::BalancedNetwork>> {
    let first = set.values().iter().next().map(|scenario| {
        let PioValue::BalancedOperatingPoint(point) = scenario.value() else {
            return Err(collection_element_mismatch(
                set.element_type(),
                scenario.value(),
            ));
        };
        Ok(point)
    });
    let Some(first) = first.transpose()? else {
        return Ok(dto::StoredOperatingPointScenarioSet {
            network: None,
            scenarios: Vec::new(),
        });
    };
    let points = set
        .values()
        .iter()
        .map(|scenario| match scenario.value() {
            PioValue::BalancedOperatingPoint(point) => Ok((scenario, point)),
            other => Err(collection_element_mismatch(set.element_type(), other)),
        })
        .collect::<Result<Vec<_>>>()?;
    ensure_same_network(
        first.network(),
        points.iter().map(|(_, point)| point.network()),
    )?;
    let network = with_component_ids(first.network().clone());
    let scenarios = points
        .into_iter()
        .map(|(scenario, point)| {
            let quantities = align_balanced_quantity_identities(
                &network,
                encode_balanced_point_assignment(point).quantities,
            )?;
            Ok(dto::StoredOperatingPointScenario {
                id: scenario.id().as_str().to_string(),
                probability: scenario.probability().map(StoredF64),
                quantities,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(dto::StoredOperatingPointScenarioSet {
        network: Some(Box::new(network)),
        scenarios,
    })
}

fn encode_multiconductor_point_scenarios(
    set: &PioScenarioSet,
) -> Result<dto::StoredOperatingPointScenarioSet<powerio_dist::MulticonductorNetwork>> {
    let first = set.values().iter().next().map(|scenario| {
        let PioValue::MulticonductorOperatingPoint(point) = scenario.value() else {
            return Err(collection_element_mismatch(
                set.element_type(),
                scenario.value(),
            ));
        };
        Ok(point)
    });
    let Some(first) = first.transpose()? else {
        return Ok(dto::StoredOperatingPointScenarioSet {
            network: None,
            scenarios: Vec::new(),
        });
    };
    let points = set
        .values()
        .iter()
        .map(|scenario| match scenario.value() {
            PioValue::MulticonductorOperatingPoint(point) => Ok((scenario, point)),
            other => Err(collection_element_mismatch(set.element_type(), other)),
        })
        .collect::<Result<Vec<_>>>()?;
    ensure_same_network(
        first.network(),
        points.iter().map(|(_, point)| point.network()),
    )?;
    let scenarios = points
        .into_iter()
        .map(|(scenario, point)| dto::StoredOperatingPointScenario {
            id: scenario.id().as_str().to_string(),
            probability: scenario.probability().map(StoredF64),
            quantities: encode_mc_point_assignment(point).quantities,
        })
        .collect();
    Ok(dto::StoredOperatingPointScenarioSet {
        network: Some(Box::new(first.network().clone())),
        scenarios,
    })
}

/// A stored quantity's identities must be exactly the order the network
/// resolves for it; a permutation or a different set is refused rather than
/// silently rebound to positions the document did not state.
fn check_identity_order(
    quantity: &str,
    stated: &[dto::StoredIdentity],
    resolved: &[String],
) -> Result<()> {
    if stated.len() != resolved.len() {
        return Err(invalid(format!(
            "quantity `{quantity}` states {} identities; the network resolves {}",
            stated.len(),
            resolved.len()
        )));
    }
    if let Some(position) = stated.iter().zip(resolved).position(|(a, b)| a.0 != *b) {
        return Err(invalid(format!(
            "quantity `{quantity}` identity {position} is `{}`; the network resolves `{}` at \
             that position",
            stated[position].0, resolved[position]
        )));
    }
    Ok(())
}

fn decode_balanced_point_assignment(
    network: &crate::BalancedNetwork,
    stored: dto::StoredOperatingPointAssignment,
) -> Result<powerio_prob::OperatingPoint<crate::BalancedNetwork>> {
    let time_points =
        vec![TimePoint::new("initial", None).map_err(|error| invalid(error.to_string()))?];
    let mut builder =
        powerio_prob::BalancedOperatingPointBuilder::new(network.clone(), time_points);
    for (name, quantity) in stored.quantities {
        let resolved = balanced_identity_order(network, &name)?;
        check_identity_order(&name, &quantity.identities, &resolved)?;
        let values: Vec<f64> = quantity.values.iter().map(|value| value.0).collect();
        builder = balanced_dense_by_name(builder, &name, values)?;
    }
    let series = builder
        .build()
        .map_err(|error| invalid(error.to_string()))?;
    series
        .values()
        .first()
        .cloned()
        .ok_or_else(|| invalid("a stored operating point decoded to no point"))
}

fn decode_mc_point_assignment(
    network: &powerio_dist::MulticonductorNetwork,
    stored: dto::StoredOperatingPointAssignment,
) -> Result<powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>> {
    let time_points =
        vec![TimePoint::new("initial", None).map_err(|error| invalid(error.to_string()))?];
    let mut builder =
        powerio_prob::MulticonductorOperatingPointBuilder::new(network.clone(), time_points);
    for (name, quantity) in stored.quantities {
        let resolved = multiconductor_identity_order(network, &name)?;
        check_identity_order(&name, &quantity.identities, &resolved)?;
        let values: Vec<f64> = quantity.values.iter().map(|value| value.0).collect();
        builder = mc_dense_by_name(builder, &name, values)?;
    }
    let series = builder
        .build()
        .map_err(|error| invalid(error.to_string()))?;
    series
        .values()
        .first()
        .cloned()
        .ok_or_else(|| invalid("a stored operating point decoded to no point"))
}

fn mc_dense_by_name(
    builder: powerio_prob::MulticonductorOperatingPointBuilder,
    name: &str,
    values: Vec<f64>,
) -> Result<powerio_prob::MulticonductorOperatingPointBuilder> {
    Ok(match name {
        "terminal_voltage_magnitude" => builder.terminal_voltage_magnitudes(values),
        "terminal_voltage_angle" => builder.terminal_voltage_angles(values),
        "load_active_power" => builder.load_active_powers(values),
        "load_reactive_power" => builder.load_reactive_powers(values),
        "switch_closed" => builder.switch_closed(decode_flags(name, values)?),
        "transformer_tap" => builder.transformer_taps(values),
        "capacitor_steps" => builder.capacitor_steps(values),
        other => {
            return Err(invalid(format!(
                "`{other}` is not a multiconductor operating point quantity"
            )));
        }
    })
}

fn balanced_dense_by_name(
    builder: powerio_prob::BalancedOperatingPointBuilder,
    name: &str,
    values: Vec<f64>,
) -> Result<powerio_prob::BalancedOperatingPointBuilder> {
    Ok(match name {
        "bus_voltage_magnitude" => builder.bus_voltage_magnitudes(values),
        "bus_voltage_angle" => builder.bus_voltage_angles(values),
        "bus_active_injection" => builder.bus_active_injections(values),
        "bus_reactive_injection" => builder.bus_reactive_injections(values),
        "generator_active_power" => builder.generator_active_powers(values),
        "generator_reactive_power" => builder.generator_reactive_powers(values),
        "generator_voltage_setpoint" => builder.generator_voltage_setpoints(values),
        "generator_in_service" => builder.generator_in_service(decode_flags(name, values)?),
        "load_active_power" => builder.load_active_powers(values),
        "load_reactive_power" => builder.load_reactive_powers(values),
        "branch_in_service" => builder.branch_in_service(decode_flags(name, values)?),
        "branch_tap_ratio" => builder.branch_tap_ratios(values),
        "branch_phase_shift" => builder.branch_phase_shifts(values),
        "switch_closed" => builder.switch_closed(decode_flags(name, values)?),
        other => {
            return Err(invalid(format!(
                "`{other}` is not a balanced operating point quantity"
            )));
        }
    })
}

fn decode_flags(name: &str, values: Vec<f64>) -> Result<Vec<bool>> {
    values
        .into_iter()
        .map(|value| match value {
            0.0 => Ok(false),
            1.0 => Ok(true),
            other => Err(invalid(format!(
                "boolean operating point quantity `{name}` contains {other}; expected 0 or 1"
            ))),
        })
        .collect()
}

fn row_identity(uid: Option<&str>, table: &str, row: usize) -> String {
    uid.map_or_else(|| format!("{table}:{row}"), str::to_string)
}

fn balanced_identity_order(network: &crate::BalancedNetwork, name: &str) -> Result<Vec<String>> {
    let identities = match name {
        "bus_voltage_magnitude"
        | "bus_voltage_angle"
        | "bus_active_injection"
        | "bus_reactive_injection" => network
            .buses()
            .iter()
            .map(|bus| bus.id.0.to_string())
            .collect(),
        "generator_active_power"
        | "generator_reactive_power"
        | "generator_voltage_setpoint"
        | "generator_in_service" => network
            .generators()
            .iter()
            .enumerate()
            .map(|(row, generator)| row_identity(generator.uid.as_deref(), "generators", row))
            .collect(),
        "load_active_power" | "load_reactive_power" => network
            .loads()
            .iter()
            .enumerate()
            .map(|(row, load)| row_identity(load.uid.as_deref(), "loads", row))
            .collect(),
        "branch_in_service" | "branch_tap_ratio" | "branch_phase_shift" => network
            .branches()
            .iter()
            .enumerate()
            .map(|(row, branch)| row_identity(branch.uid.as_deref(), "branches", row))
            .collect(),
        "switch_closed" => network
            .switches()
            .iter()
            .enumerate()
            .map(|(row, switch)| row_identity(switch.uid.as_deref(), "switches", row))
            .collect(),
        other => {
            return Err(invalid(format!(
                "`{other}` is not a balanced operating point quantity"
            )));
        }
    };
    Ok(identities)
}

/// The element identity order for multiconductor operating point quantities.
fn multiconductor_identity_order(
    network: &powerio_dist::MulticonductorNetwork,
    name: &str,
) -> Result<Vec<String>> {
    let identities = match name {
        "terminal_voltage_magnitude" | "terminal_voltage_angle" => network
            .buses()
            .iter()
            .flat_map(|bus| {
                bus.terminals
                    .iter()
                    .map(move |terminal| format!("{}/{terminal}", bus.id))
            })
            .collect(),
        "load_active_power" | "load_reactive_power" => network
            .loads()
            .iter()
            .flat_map(|load| {
                load.terminal_map
                    .iter()
                    .map(move |terminal| format!("{}/{terminal}", load.name))
            })
            .collect(),
        "switch_closed" => network
            .switches()
            .iter()
            .map(|value| value.name.clone())
            .collect(),
        "transformer_tap" => network
            .transformers()
            .iter()
            .map(|value| value.name.clone())
            .collect(),
        "capacitor_steps" => network
            .capacitors()
            .iter()
            .map(|value| value.name.clone())
            .collect(),
        other => {
            return Err(invalid(format!(
                "`{other}` is not a multiconductor operating point quantity"
            )));
        }
    };
    Ok(identities)
}

fn encode_dc_pf_instance(instance: &powerio_prob::DcPfInstance) -> Result<dto::DcPfInstance> {
    let network = with_component_ids(instance.network().clone());
    Ok(dto::DcPfInstance {
        initial_point: instance
            .initial_point()
            .map(|point| {
                align_balanced_quantity_identities(
                    &network,
                    encode_balanced_point_assignment(point).quantities,
                )
                .map(|quantities| dto::StoredOperatingPointAssignment { quantities })
            })
            .transpose()?,
        network: Box::new(network),
        approximation: dc_formula_name(instance.branch_susceptance_formula())?.to_string(),
    })
}

fn encode_ac_pf_instance(instance: &powerio_prob::AcPfInstance) -> Result<dto::AcPfInstance> {
    let network = with_component_ids(instance.network().clone());
    Ok(dto::AcPfInstance {
        initial_point: instance
            .initial_point()
            .map(|point| {
                align_balanced_quantity_identities(
                    &network,
                    encode_balanced_point_assignment(point).quantities,
                )
                .map(|quantities| dto::StoredOperatingPointAssignment { quantities })
            })
            .transpose()?,
        network: Box::new(network),
        specifications: instance.specifications().to_vec(),
    })
}

fn encode_dc_opf_instance(instance: &powerio_prob::DcOpfInstance) -> Result<dto::DcOpfInstance> {
    let network = with_component_ids(instance.network().clone());
    Ok(dto::DcOpfInstance {
        initial_point: instance
            .initial_point()
            .map(|point| {
                align_balanced_quantity_identities(
                    &network,
                    encode_balanced_point_assignment(point).quantities,
                )
                .map(|quantities| dto::StoredOperatingPointAssignment { quantities })
            })
            .transpose()?,
        network: Box::new(network),
        approximation: dc_formula_name(instance.branch_susceptance_formula())?.to_string(),
        objective: encode_objective(instance.objective())?,
        constraints: instance.constraints().clone(),
    })
}

fn encode_ac_opf_instance(instance: &powerio_prob::AcOpfInstance) -> Result<dto::AcOpfInstance> {
    let network = with_component_ids(instance.network().clone());
    Ok(dto::AcOpfInstance {
        initial_point: instance
            .initial_point()
            .map(|point| {
                align_balanced_quantity_identities(
                    &network,
                    encode_balanced_point_assignment(point).quantities,
                )
                .map(|quantities| dto::StoredOperatingPointAssignment { quantities })
            })
            .transpose()?,
        network: Box::new(network),
        objective: encode_objective(instance.objective())?,
        constraints: instance.constraints().clone(),
    })
}

fn encode_mc_ac_pf_instance(instance: &powerio_prob::McAcPfInstance) -> dto::McAcPfInstance {
    dto::McAcPfInstance {
        network: Box::new(instance.network().clone()),
        initial_point: instance.initial_point().map(encode_mc_point_assignment),
    }
}

fn encode_mc_ac_opf_instance(
    instance: &powerio_prob::McAcOpfInstance,
) -> Result<dto::McAcOpfInstance> {
    Ok(dto::McAcOpfInstance {
        network: Box::new(instance.network().clone()),
        objective: encode_objective(instance.objective())?,
        constraints: instance.constraints().clone(),
        initial_point: instance.initial_point().map(encode_mc_point_assignment),
    })
}

/// The typed objective, mirroring [`powerio_prob::Objective`] with each
/// term's own weight wrapped for the nonfinite spelling (see
/// [`dto::ObjectiveTerm`] for why this can't just be the runtime type).
fn encode_objective(objective: &powerio_prob::Objective) -> Result<dto::Objective> {
    Ok(dto::Objective {
        terms: objective
            .terms()
            .iter()
            .map(encode_objective_term)
            .collect::<Result<_>>()?,
    })
}

fn encode_objective_term(term: &powerio_prob::ObjectiveTerm) -> Result<dto::ObjectiveTerm> {
    Ok(match term {
        powerio_prob::ObjectiveTerm::NetworkGeneratorCost => {
            dto::ObjectiveTerm::NetworkGeneratorCost
        }
        powerio_prob::ObjectiveTerm::ActivePowerDispatchCost => {
            dto::ObjectiveTerm::ActivePowerDispatchCost
        }
        // The runtime enum is non_exhaustive for additive growth; a new term
        // must gain a stored spelling before it can be written.
        _ => return Err(invalid("unmapped objective term")),
    })
}

fn decode_objective(objective: dto::Objective) -> powerio_prob::Objective {
    let mut decoded = powerio_prob::Objective::default();
    for term in objective.terms {
        decoded = decoded.with_term(decode_objective_term(term));
    }
    decoded
}

fn decode_objective_term(term: dto::ObjectiveTerm) -> powerio_prob::ObjectiveTerm {
    match term {
        dto::ObjectiveTerm::NetworkGeneratorCost => {
            powerio_prob::ObjectiveTerm::NetworkGeneratorCost
        }
        dto::ObjectiveTerm::ActivePowerDispatchCost => {
            powerio_prob::ObjectiveTerm::ActivePowerDispatchCost
        }
    }
}

fn encode_time_point(point: &TimePoint) -> dto::TimePoint {
    dto::TimePoint {
        label: point.label().to_string(),
        duration: point.duration().map(|duration| dto::Duration {
            secs: duration.as_secs(),
            nanos: duration.subsec_nanos(),
        }),
    }
}

fn stored_row(values: &[f64]) -> Vec<StoredF64> {
    values.iter().copied().map(StoredF64).collect()
}

fn plain_row(values: &[StoredF64]) -> Vec<f64> {
    values.iter().map(|value| value.0).collect()
}

fn stored_grid(rows: &[Vec<f64>]) -> Vec<Vec<StoredF64>> {
    rows.iter().map(|row| stored_row(row)).collect()
}

fn plain_grid(rows: &[Vec<StoredF64>]) -> Vec<Vec<f64>> {
    rows.iter().map(|row| plain_row(row)).collect()
}

/// One balanced solution column per bus, read through the keyed accessor in
/// bus table order.
fn bus_column(
    network: &crate::BalancedNetwork,
    read: impl Fn(crate::BusId) -> Option<f64>,
) -> Vec<StoredF64> {
    network
        .buses()
        .iter()
        .map(|bus| StoredF64(read(bus.id).unwrap_or(f64::NAN)))
        .collect()
}

fn branch_identity(network: &crate::BalancedNetwork, row: usize) -> String {
    match network.branches()[row].uid.as_deref() {
        Some(uid) => uid.to_string(),
        None => format!("branches:{row}"),
    }
}

fn generator_identity(network: &crate::BalancedNetwork, row: usize) -> String {
    match network.generators()[row].uid.as_deref() {
        Some(uid) => uid.to_string(),
        None => format!("generators:{row}"),
    }
}

fn branch_column(
    network: &crate::BalancedNetwork,
    read: impl Fn(&str) -> Option<f64>,
) -> Vec<StoredF64> {
    (0..network.branches().len())
        .map(|row| StoredF64(read(&branch_identity(network, row)).unwrap_or(f64::NAN)))
        .collect()
}

fn generator_column(
    network: &crate::BalancedNetwork,
    read: impl Fn(&str) -> Option<f64>,
) -> Vec<StoredF64> {
    (0..network.generators().len())
        .map(|row| StoredF64(read(&generator_identity(network, row)).unwrap_or(f64::NAN)))
        .collect()
}

/// An optional solution column, already in the network's table order.
fn optional_column(values: Option<&[f64]>) -> Option<Vec<StoredF64>> {
    values.map(|column| column.iter().copied().map(StoredF64).collect())
}

fn encode_dispatch(
    dispatch: Option<&powerio_prob::GeneratorDispatch>,
) -> Option<dto::GeneratorDispatch> {
    dispatch.map(|dispatch| dto::GeneratorDispatch {
        p_mw: stored_row(&dispatch.p_mw),
        q_mvar: stored_row(&dispatch.q_mvar),
    })
}

fn encode_three_winding_transformer_terminal_powers(
    values: &[powerio_prob::ThreeWindingTransformerTerminalPower],
) -> Vec<dto::ThreeWindingTransformerTerminalPower> {
    values
        .iter()
        .map(|value| dto::ThreeWindingTransformerTerminalPower {
            p_mw: value.p_mw.map(StoredF64),
            q_mvar: value.q_mvar.map(StoredF64),
        })
        .collect()
}

fn decode_three_winding_transformer_terminal_powers(
    values: &[dto::ThreeWindingTransformerTerminalPower],
) -> Vec<powerio_prob::ThreeWindingTransformerTerminalPower> {
    values
        .iter()
        .map(|value| {
            powerio_prob::ThreeWindingTransformerTerminalPower::new(
                value.p_mw.map(|item| item.0),
                value.q_mvar.map(|item| item.0),
            )
        })
        .collect()
}

fn encode_three_winding_transformer_terminal_active_powers(
    values: &[powerio_prob::ThreeWindingTransformerTerminalActivePower],
) -> Vec<dto::ThreeWindingTransformerTerminalActivePower> {
    values
        .iter()
        .map(|value| dto::ThreeWindingTransformerTerminalActivePower {
            p_mw: value.p_mw.map(StoredF64),
        })
        .collect()
}

fn decode_three_winding_transformer_terminal_active_powers(
    values: &[dto::ThreeWindingTransformerTerminalActivePower],
) -> Vec<powerio_prob::ThreeWindingTransformerTerminalActivePower> {
    values
        .iter()
        .map(|value| {
            powerio_prob::ThreeWindingTransformerTerminalActivePower::new(
                value.p_mw.map(|item| item.0),
            )
        })
        .collect()
}

fn encode_dc_pf_solution(solution: &powerio_prob::DcPfSolution) -> Result<dto::DcPfSolution> {
    let network = solution.network();
    Ok(dto::DcPfSolution {
        instance: encode_dc_pf_instance(solution.instance())?,
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        bus_voltage_angle: bus_column(network, |bus| solution.bus_voltage_angle(bus)),
        bus_active_injection: bus_column(network, |bus| solution.bus_active_injection(bus)),
        branch_from_active_flow: branch_column(network, |id| solution.branch_from_active_flow(id)),
        branch_to_active_flow: branch_column(network, |id| solution.branch_to_active_flow(id)),
        three_winding_transformer_terminal_active_powers:
            encode_three_winding_transformer_terminal_active_powers(
                solution.three_winding_transformer_terminal_active_powers(),
            ),
        generator_dispatch: encode_dispatch(solution.generator_dispatch()),
    })
}

fn encode_ac_pf_solution(solution: &powerio_prob::AcPfSolution) -> Result<dto::AcPfSolution> {
    let network = solution.network();
    Ok(dto::AcPfSolution {
        instance: encode_ac_pf_instance(solution.instance())?,
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        bus_voltage_magnitude: bus_column(network, |bus| solution.bus_voltage_magnitude(bus)),
        bus_voltage_angle: bus_column(network, |bus| solution.bus_voltage_angle(bus)),
        bus_active_injection: bus_column(network, |bus| solution.bus_active_injection(bus)),
        bus_reactive_injection: bus_column(network, |bus| solution.bus_reactive_injection(bus)),
        branch_from_active_flow: branch_column(network, |id| solution.branch_from_active_flow(id)),
        branch_from_reactive_flow: branch_column(network, |id| {
            solution.branch_from_reactive_flow(id)
        }),
        branch_to_active_flow: branch_column(network, |id| solution.branch_to_active_flow(id)),
        branch_to_reactive_flow: branch_column(network, |id| solution.branch_to_reactive_flow(id)),
        three_winding_transformer_terminal_powers: encode_three_winding_transformer_terminal_powers(
            solution.three_winding_transformer_terminal_powers(),
        ),
        generator_dispatch: encode_dispatch(solution.generator_dispatch()),
    })
}

fn encode_dc_opf_solution(solution: &powerio_prob::DcOpfSolution) -> Result<dto::DcOpfSolution> {
    let network = solution.network();
    Ok(dto::DcOpfSolution {
        instance: encode_dc_opf_instance(solution.instance())?,
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        bus_voltage_angle: bus_column(network, |bus| solution.bus_voltage_angle(bus)),
        bus_active_injection: bus_column(network, |bus| solution.bus_active_injection(bus)),
        branch_from_active_flow: branch_column(network, |id| solution.branch_from_active_flow(id)),
        branch_to_active_flow: branch_column(network, |id| solution.branch_to_active_flow(id)),
        generator_active_power: generator_column(network, |id| solution.generator_active_power(id)),
        three_winding_transformer_terminal_active_powers:
            encode_three_winding_transformer_terminal_active_powers(
                solution.three_winding_transformer_terminal_active_powers(),
            ),
        objective: StoredF64(solution.objective()),
        bus_active_power_marginal: optional_column(solution.bus_active_power_marginals()),
        branch_from_limit_multiplier: optional_column(solution.branch_from_limit_multipliers()),
        branch_to_limit_multiplier: optional_column(solution.branch_to_limit_multipliers()),
    })
}

fn encode_ac_opf_solution(solution: &powerio_prob::AcOpfSolution) -> Result<dto::AcOpfSolution> {
    let network = solution.network();
    Ok(dto::AcOpfSolution {
        instance: encode_ac_opf_instance(solution.instance())?,
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        bus_voltage_magnitude: bus_column(network, |bus| solution.bus_voltage_magnitude(bus)),
        bus_voltage_angle: bus_column(network, |bus| solution.bus_voltage_angle(bus)),
        bus_active_injection: bus_column(network, |bus| solution.bus_active_injection(bus)),
        bus_reactive_injection: bus_column(network, |bus| solution.bus_reactive_injection(bus)),
        branch_from_active_flow: branch_column(network, |id| solution.branch_from_active_flow(id)),
        branch_from_reactive_flow: branch_column(network, |id| {
            solution.branch_from_reactive_flow(id)
        }),
        branch_to_active_flow: branch_column(network, |id| solution.branch_to_active_flow(id)),
        branch_to_reactive_flow: branch_column(network, |id| solution.branch_to_reactive_flow(id)),
        generator_active_power: generator_column(network, |id| solution.generator_active_power(id)),
        generator_reactive_power: generator_column(network, |id| {
            solution.generator_reactive_power(id)
        }),
        three_winding_transformer_terminal_powers: encode_three_winding_transformer_terminal_powers(
            solution.three_winding_transformer_terminal_powers(),
        ),
        objective: StoredF64(solution.objective()),
        bus_active_power_marginal: optional_column(solution.bus_active_power_marginals()),
        bus_reactive_power_marginal: optional_column(solution.bus_reactive_power_marginals()),
        branch_from_limit_multiplier: optional_column(solution.branch_from_limit_multipliers()),
        branch_to_limit_multiplier: optional_column(solution.branch_to_limit_multipliers()),
    })
}

fn encode_socwr_opf_solution(
    solution: &powerio_prob::solution::SocwrOpfSolution,
) -> Result<dto::SocwrOpfSolution> {
    let values = solution.values();
    let duals = solution.duals();
    Ok(dto::SocwrOpfSolution {
        instance: encode_ac_opf_instance(solution.instance())?,
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        values: dto::SocwrOpfValues {
            bus_voltage_magnitude_squared: stored_row(&values.bus_voltage_magnitude_squared),
            branch_voltage_product_real: stored_row(&values.branch_voltage_product_real),
            branch_voltage_product_imaginary: stored_row(&values.branch_voltage_product_imaginary),
            generator_active_power: stored_row(&values.generator_active_power),
            generator_reactive_power: stored_row(&values.generator_reactive_power),
            branch_from_active_power: stored_row(&values.branch_from_active_power),
            branch_from_reactive_power: stored_row(&values.branch_from_reactive_power),
            branch_to_active_power: stored_row(&values.branch_to_active_power),
            branch_to_reactive_power: stored_row(&values.branch_to_reactive_power),
            three_winding_transformer_terminal_powers:
                encode_three_winding_transformer_terminal_powers(
                    &values.three_winding_transformer_terminal_powers,
                ),
        },
        duals: dto::SocwrOpfDuals {
            bus_active_power_marginal: duals.bus_active_power_marginal.as_deref().map(stored_row),
            bus_reactive_power_marginal: duals
                .bus_reactive_power_marginal
                .as_deref()
                .map(stored_row),
            branch_from_thermal_limit_multiplier: duals
                .branch_from_thermal_limit_multiplier
                .as_deref()
                .map(stored_row),
            branch_to_thermal_limit_multiplier: duals
                .branch_to_thermal_limit_multiplier
                .as_deref()
                .map(stored_row),
        },
        objective_lower_bound: StoredF64(solution.objective_lower_bound()),
    })
}

/// Every terminal of every bus, in bus table order with each bus's stated
/// terminal order — the multiconductor solution column layout.
fn terminal_column(
    network: &powerio_dist::MulticonductorNetwork,
    read: impl Fn(&str, &str) -> Option<f64>,
) -> Vec<StoredF64> {
    let mut column = Vec::new();
    for bus in network.buses() {
        for terminal in &bus.terminals {
            column.push(StoredF64(read(&bus.id, terminal).unwrap_or(f64::NAN)));
        }
    }
    column
}

/// The optional terminal columns are present when any terminal answers.
fn optional_terminal_column(
    network: &powerio_dist::MulticonductorNetwork,
    read: impl Fn(&str, &str) -> Option<f64>,
) -> Option<Vec<StoredF64>> {
    let mut any = false;
    let mut column = Vec::new();
    for bus in network.buses() {
        for terminal in &bus.terminals {
            match read(&bus.id, terminal) {
                Some(value) => {
                    any = true;
                    column.push(StoredF64(value));
                }
                None => column.push(StoredF64(f64::NAN)),
            }
        }
    }
    any.then_some(column)
}

fn encode_mc_ac_pf_solution(solution: &powerio_prob::McAcPfSolution) -> dto::McAcPfSolution {
    let network = solution.network();
    dto::McAcPfSolution {
        instance: encode_mc_ac_pf_instance(solution.instance()),
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        terminal_voltage_magnitude: terminal_column(network, |bus, terminal| {
            solution.terminal_voltage_magnitude(bus, terminal)
        }),
        terminal_voltage_angle: terminal_column(network, |bus, terminal| {
            solution.terminal_voltage_angle(bus, terminal)
        }),
        terminal_current_magnitude: optional_terminal_column(network, |bus, terminal| {
            solution.terminal_current_magnitude(bus, terminal)
        }),
        terminal_active_power: optional_terminal_column(network, |bus, terminal| {
            solution.terminal_active_power(bus, terminal)
        }),
        source_active_injection: stored_row(solution.source_active_injections()),
    }
}

fn encode_mc_ac_opf_solution(
    solution: &powerio_prob::McAcOpfSolution,
) -> Result<dto::McAcOpfSolution> {
    let network = solution.network();
    Ok(dto::McAcOpfSolution {
        instance: encode_mc_ac_opf_instance(solution.instance())?,
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        terminal_voltage_magnitude: terminal_column(network, |bus, terminal| {
            solution.terminal_voltage_magnitude(bus, terminal)
        }),
        terminal_voltage_angle: terminal_column(network, |bus, terminal| {
            solution.terminal_voltage_angle(bus, terminal)
        }),
        terminal_current_magnitude: optional_terminal_column(network, |bus, terminal| {
            solution.terminal_current_magnitude(bus, terminal)
        }),
        terminal_active_power: optional_terminal_column(network, |bus, terminal| {
            solution.terminal_active_power(bus, terminal)
        }),
        source_active_injection: stored_row(solution.source_active_injections()),
        generator_active_power: stored_row(solution.generator_active_powers()),
        objective: StoredF64(solution.objective()),
    })
}

fn encode_ac_scuc_solution(solution: &powerio_prob::AcScucSolution) -> dto::AcScucSolution {
    let network_outputs = solution.network_outputs();
    let device_outputs = solution.device_outputs();
    dto::AcScucSolution {
        instance: dto::AcScucInstance {
            network: Box::new(with_component_ids(solution.instance().network().clone())),
            inputs: Box::new(solution.instance().inputs().clone()),
        },
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        network_outputs: dto::ScucNetworkOutputs {
            bus_vm: stored_grid(&network_outputs.bus_vm),
            bus_va: stored_grid(&network_outputs.bus_va),
            shunt_step: network_outputs.shunt_step.clone(),
            ac_line_on_status: network_outputs.ac_line_on_status.clone(),
            transformer_tm: stored_grid(&network_outputs.transformer_tm),
            transformer_ta: stored_grid(&network_outputs.transformer_ta),
            transformer_on_status: network_outputs.transformer_on_status.clone(),
            dc_line_pdc_fr: stored_grid(&network_outputs.dc_line_pdc_fr),
            dc_line_qdc_fr: stored_grid(&network_outputs.dc_line_qdc_fr),
            dc_line_qdc_to: stored_grid(&network_outputs.dc_line_qdc_to),
        },
        device_outputs: dto::ScucDeviceOutputs {
            on_status: device_outputs.on_status.clone(),
            startup_status: device_outputs.startup_status.clone(),
            shutdown_status: device_outputs.shutdown_status.clone(),
            p_on: stored_grid(&device_outputs.p_on),
            q: stored_grid(&device_outputs.q),
            p_reg_res_up: stored_grid(&device_outputs.p_reg_res_up),
            p_reg_res_down: stored_grid(&device_outputs.p_reg_res_down),
            p_syn_res: stored_grid(&device_outputs.p_syn_res),
            p_nsyn_res: stored_grid(&device_outputs.p_nsyn_res),
            p_ramp_res_up_online: stored_grid(&device_outputs.p_ramp_res_up_online),
            p_ramp_res_up_offline: stored_grid(&device_outputs.p_ramp_res_up_offline),
            p_ramp_res_down_online: stored_grid(&device_outputs.p_ramp_res_down_online),
            p_ramp_res_down_offline: stored_grid(&device_outputs.p_ramp_res_down_offline),
            q_res_up: stored_grid(&device_outputs.q_res_up),
            q_res_down: stored_grid(&device_outputs.q_res_down),
        },
        objective: solution.objective().map(StoredF64),
    }
}

// ---- value decoding ---------------------------------------------------------

/// Validate every network embedded in a decoded value.
fn validate_decoded_networks(value: &PioValue) -> Result<()> {
    let balanced = |network: &powerio_tx::BalancedNetwork| -> Result<()> {
        network
            .validate()
            .map_err(|error| invalid(format!("decoded network fails validation: {error}")))
    };
    let multiconductor = |_network: &powerio_dist::MulticonductorNetwork| -> Result<()> { Ok(()) };
    match value {
        PioValue::BalancedNetwork(network) => balanced(network),
        PioValue::MulticonductorNetwork(network) => multiconductor(network),
        // A layer carries no network to validate; its own rules run in the
        // stored document validator.
        PioValue::GeoLayer(_) => Ok(()),
        PioValue::BalancedOperatingPoint(point) => balanced(point.network()),
        PioValue::MulticonductorOperatingPoint(point) => multiconductor(point.network()),
        PioValue::TimeSeries(series) => series
            .iter()
            .try_for_each(|(_, value)| validate_decoded_networks(value)),
        PioValue::ScenarioSet(set) => set
            .iter()
            .try_for_each(|scenario| validate_decoded_networks(scenario.value())),
        PioValue::DcPfInstance(instance) => balanced(instance.network()),
        PioValue::AcPfInstance(instance) => balanced(instance.network()),
        PioValue::DcOpfInstance(instance) => balanced(instance.network()),
        PioValue::AcOpfInstance(instance) => balanced(instance.network()),
        PioValue::AcScucInstance(instance) => balanced(instance.network()),
        PioValue::McAcPfInstance(instance) => multiconductor(instance.network()),
        PioValue::McAcOpfInstance(instance) => multiconductor(instance.network()),
        PioValue::DcPfSolution(solution) => balanced(solution.network()),
        PioValue::AcPfSolution(solution) => balanced(solution.network()),
        PioValue::DcOpfSolution(solution) => balanced(solution.network()),
        PioValue::AcOpfSolution(solution) => balanced(solution.network()),
        PioValue::SocwrOpfSolution(solution) => balanced(solution.network()),
        PioValue::McAcPfSolution(solution) => multiconductor(solution.network()),
        PioValue::McAcOpfSolution(solution) => multiconductor(solution.network()),
        PioValue::AcScucSolution(solution) => balanced(solution.instance().network()),
    }
}

fn decode_stored(stored: StoredModule) -> Result<PioModule<PioValue>> {
    let value = decode_value(stored.value)?;
    validate_decoded_networks(&value)?;
    let mut module = PioModule::new(value).with_producer(
        Producer::new(stored.producer.name, stored.producer.version)
            .map_err(|error| invalid(error.to_string()))?,
    );
    for source in stored.sources {
        module
            .add_source_descriptor(decode_source(source)?)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for entry in stored.source_map {
        module
            .add_source_map_entry(decode_map_entry(entry)?)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for diagnostic in stored.diagnostics {
        module
            .add_diagnostic(decode_diagnostic(diagnostic)?)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for entry in stored.history {
        module
            .add_history_entry(decode_history(entry)?)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for (key, value) in stored.extensions {
        module
            .insert_extension(key, value)
            .map_err(|error| invalid(error.to_string()))?;
    }
    Ok(module)
}

fn decode_time_points(points: &[dto::TimePoint]) -> Result<Vec<TimePoint>> {
    points
        .iter()
        .map(|point| {
            TimePoint::new(
                point.label.clone(),
                point
                    .duration
                    .map(|duration| std::time::Duration::new(duration.secs, duration.nanos)),
            )
            .map_err(|error| invalid(error.to_string()))
        })
        .collect()
}

fn decode_balanced_point(
    stored: dto::StoredOperatingPoint<crate::BalancedNetwork>,
) -> Result<powerio_prob::OperatingPoint<crate::BalancedNetwork>> {
    let network = with_component_ids(*stored.network);
    decode_balanced_point_assignment(
        &network,
        dto::StoredOperatingPointAssignment {
            quantities: stored.quantities,
        },
    )
}

fn decode_mc_point(
    stored: dto::StoredOperatingPoint<powerio_dist::MulticonductorNetwork>,
) -> Result<powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>> {
    let network = *stored.network;
    decode_mc_point_assignment(
        &network,
        dto::StoredOperatingPointAssignment {
            quantities: stored.quantities,
        },
    )
}

fn decode_balanced_network_series(
    stored: dto::StoredTimeSeries<crate::BalancedNetwork>,
) -> Result<TimeSeries<crate::BalancedNetwork>> {
    TimeSeries::new(decode_time_points(&stored.time_points)?, stored.values)
        .map_err(|error| invalid(error.to_string()))
}

fn decode_multiconductor_network_series(
    stored: dto::StoredTimeSeries<powerio_dist::MulticonductorNetwork>,
) -> Result<TimeSeries<powerio_dist::MulticonductorNetwork>> {
    TimeSeries::new(decode_time_points(&stored.time_points)?, stored.values)
        .map_err(|error| invalid(error.to_string()))
}

fn decode_balanced_point_series(
    stored: dto::StoredOperatingPointTimeSeries<crate::BalancedNetwork>,
) -> Result<TimeSeries<powerio_prob::OperatingPoint<crate::BalancedNetwork>>> {
    let time_points = decode_time_points(&stored.time_points)?;
    let Some(network) = stored.network else {
        return TimeSeries::new(time_points, Vec::new())
            .map_err(|error| invalid(error.to_string()));
    };
    let network = with_component_ids(*network);
    let values = stored
        .values
        .into_iter()
        .map(|point| decode_balanced_point_assignment(&network, point))
        .collect::<Result<Vec<_>>>()?;
    TimeSeries::new(time_points, values).map_err(|error| invalid(error.to_string()))
}

fn decode_multiconductor_point_series(
    stored: dto::StoredOperatingPointTimeSeries<powerio_dist::MulticonductorNetwork>,
) -> Result<TimeSeries<powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>>> {
    let time_points = decode_time_points(&stored.time_points)?;
    let Some(network) = stored.network else {
        return TimeSeries::new(time_points, Vec::new())
            .map_err(|error| invalid(error.to_string()));
    };
    let network = *network;
    let values = stored
        .values
        .into_iter()
        .map(|point| decode_mc_point_assignment(&network, point))
        .collect::<Result<Vec<_>>>()?;
    TimeSeries::new(time_points, values).map_err(|error| invalid(error.to_string()))
}

fn decode_scenario_entries<T, U>(
    stored: dto::StoredScenarioSet<T>,
    mut decode: impl FnMut(T) -> Result<U>,
) -> Result<powerio_core::ScenarioSet<U>> {
    let scenarios = stored
        .scenarios
        .into_iter()
        .map(|scenario| {
            let id = powerio_core::ScenarioId::new(scenario.id)
                .map_err(|error| invalid(error.to_string()))?;
            Ok(powerio_core::Scenario::new(
                id,
                scenario.probability.map(|value| value.0),
                decode(scenario.value)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    powerio_core::ScenarioSet::new(scenarios).map_err(|error| invalid(error.to_string()))
}

fn decode_balanced_point_scenarios(
    stored: dto::StoredOperatingPointScenarioSet<crate::BalancedNetwork>,
) -> Result<powerio_core::ScenarioSet<powerio_prob::OperatingPoint<crate::BalancedNetwork>>> {
    let Some(network) = stored.network else {
        return powerio_core::ScenarioSet::new(Vec::new())
            .map_err(|error| invalid(error.to_string()));
    };
    let network = with_component_ids(*network);
    let scenarios = stored
        .scenarios
        .into_iter()
        .map(|scenario| {
            let id = powerio_core::ScenarioId::new(scenario.id)
                .map_err(|error| invalid(error.to_string()))?;
            let point = decode_balanced_point_assignment(
                &network,
                dto::StoredOperatingPointAssignment {
                    quantities: scenario.quantities,
                },
            )?;
            Ok(powerio_core::Scenario::new(
                id,
                scenario.probability.map(|value| value.0),
                point,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    powerio_core::ScenarioSet::new(scenarios).map_err(|error| invalid(error.to_string()))
}

fn decode_multiconductor_point_scenarios(
    stored: dto::StoredOperatingPointScenarioSet<powerio_dist::MulticonductorNetwork>,
) -> Result<
    powerio_core::ScenarioSet<powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>>,
> {
    let Some(network) = stored.network else {
        return powerio_core::ScenarioSet::new(Vec::new())
            .map_err(|error| invalid(error.to_string()));
    };
    let network = *network;
    let scenarios = stored
        .scenarios
        .into_iter()
        .map(|scenario| {
            let id = powerio_core::ScenarioId::new(scenario.id)
                .map_err(|error| invalid(error.to_string()))?;
            let point = decode_mc_point_assignment(
                &network,
                dto::StoredOperatingPointAssignment {
                    quantities: scenario.quantities,
                },
            )?;
            Ok(powerio_core::Scenario::new(
                id,
                scenario.probability.map(|value| value.0),
                point,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    powerio_core::ScenarioSet::new(scenarios).map_err(|error| invalid(error.to_string()))
}

fn decode_socwr_opf_solution(
    stored: dto::SocwrOpfSolution,
) -> Result<powerio_prob::solution::SocwrOpfSolution> {
    let instance = std::sync::Arc::new(decode_ac_opf_instance(stored.instance)?);
    let mut values = powerio_prob::solution::SocwrOpfValues::default();
    values.bus_voltage_magnitude_squared = plain_row(&stored.values.bus_voltage_magnitude_squared);
    values.branch_voltage_product_real = plain_row(&stored.values.branch_voltage_product_real);
    values.branch_voltage_product_imaginary =
        plain_row(&stored.values.branch_voltage_product_imaginary);
    values.generator_active_power = plain_row(&stored.values.generator_active_power);
    values.generator_reactive_power = plain_row(&stored.values.generator_reactive_power);
    values.branch_from_active_power = plain_row(&stored.values.branch_from_active_power);
    values.branch_from_reactive_power = plain_row(&stored.values.branch_from_reactive_power);
    values.branch_to_active_power = plain_row(&stored.values.branch_to_active_power);
    values.branch_to_reactive_power = plain_row(&stored.values.branch_to_reactive_power);
    values.three_winding_transformer_terminal_powers =
        decode_three_winding_transformer_terminal_powers(
            &stored.values.three_winding_transformer_terminal_powers,
        );
    let mut solution = powerio_prob::solution::SocwrOpfSolution::new(
        instance,
        stored.termination,
        values,
        stored.objective_lower_bound.0,
    )
    .map_err(|error| invalid(error.to_string()))?
    .with_residuals(stored.residuals);
    if let Some(producer) = stored.producer {
        solution = solution.with_producer(producer);
    }
    let mut duals = powerio_prob::solution::SocwrOpfDuals::default();
    duals.bus_active_power_marginal = stored
        .duals
        .bus_active_power_marginal
        .as_deref()
        .map(plain_row);
    duals.bus_reactive_power_marginal = stored
        .duals
        .bus_reactive_power_marginal
        .as_deref()
        .map(plain_row);
    duals.branch_from_thermal_limit_multiplier = stored
        .duals
        .branch_from_thermal_limit_multiplier
        .as_deref()
        .map(plain_row);
    duals.branch_to_thermal_limit_multiplier = stored
        .duals
        .branch_to_thermal_limit_multiplier
        .as_deref()
        .map(plain_row);
    solution
        .with_duals(duals)
        .map_err(|error| invalid(error.to_string()))
}

#[allow(clippy::too_many_lines)]
fn decode_value(value: dto::StoredValue) -> Result<PioValue> {
    Ok(match value {
        dto::StoredValue::BalancedNetwork(network) => PioValue::from(*network),
        dto::StoredValue::MulticonductorNetwork(network) => {
            PioValue::MulticonductorNetwork(*network)
        }
        dto::StoredValue::GeoLayer(layer) => PioValue::GeoLayer(*layer),
        dto::StoredValue::BalancedOperatingPoint(point) => {
            PioValue::BalancedOperatingPoint(decode_balanced_point(point)?)
        }
        dto::StoredValue::MulticonductorOperatingPoint(point) => {
            PioValue::MulticonductorOperatingPoint(decode_mc_point(point)?)
        }
        dto::StoredValue::BalancedNetworkTimeSeries(series) => {
            PioValue::from(decode_balanced_network_series(series)?)
        }
        dto::StoredValue::MulticonductorNetworkTimeSeries(series) => {
            PioValue::from(decode_multiconductor_network_series(series)?)
        }
        dto::StoredValue::BalancedOperatingPointTimeSeries(series) => {
            PioValue::from(decode_balanced_point_series(series)?)
        }
        dto::StoredValue::MulticonductorOperatingPointTimeSeries(series) => {
            PioValue::from(decode_multiconductor_point_series(series)?)
        }
        dto::StoredValue::BalancedNetworkScenarioSet(set) => {
            PioValue::from(decode_scenario_entries(set, Ok)?)
        }
        dto::StoredValue::MulticonductorNetworkScenarioSet(set) => {
            PioValue::from(decode_scenario_entries(set, Ok)?)
        }
        dto::StoredValue::BalancedOperatingPointScenarioSet(set) => {
            PioValue::from(decode_balanced_point_scenarios(set)?)
        }
        dto::StoredValue::MulticonductorOperatingPointScenarioSet(set) => {
            PioValue::from(decode_multiconductor_point_scenarios(set)?)
        }
        dto::StoredValue::BalancedNetworkTimeSeriesScenarioSet(set) => PioValue::from(
            decode_scenario_entries(set, decode_balanced_network_series)?,
        ),
        dto::StoredValue::MulticonductorNetworkTimeSeriesScenarioSet(set) => PioValue::from(
            decode_scenario_entries(set, decode_multiconductor_network_series)?,
        ),
        dto::StoredValue::BalancedOperatingPointTimeSeriesScenarioSet(set) => {
            PioValue::from(decode_scenario_entries(set, decode_balanced_point_series)?)
        }
        dto::StoredValue::MulticonductorOperatingPointTimeSeriesScenarioSet(set) => PioValue::from(
            decode_scenario_entries(set, decode_multiconductor_point_series)?,
        ),
        dto::StoredValue::DcPfInstance(instance) => {
            PioValue::DcPfInstance(decode_dc_pf_instance(instance)?)
        }
        dto::StoredValue::AcPfInstance(instance) => {
            PioValue::AcPfInstance(decode_ac_pf_instance(instance)?)
        }
        dto::StoredValue::DcOpfInstance(instance) => {
            PioValue::DcOpfInstance(decode_dc_opf_instance(instance)?)
        }
        dto::StoredValue::AcOpfInstance(instance) => {
            PioValue::AcOpfInstance(decode_ac_opf_instance(instance)?)
        }
        dto::StoredValue::McAcPfInstance(instance) => {
            PioValue::McAcPfInstance(decode_mc_ac_pf_instance(instance)?)
        }
        dto::StoredValue::McAcOpfInstance(instance) => {
            PioValue::McAcOpfInstance(decode_mc_ac_opf_instance(instance)?)
        }
        dto::StoredValue::AcScucInstance(instance) => {
            PioValue::AcScucInstance(decode_ac_scuc_instance(instance)?)
        }
        dto::StoredValue::DcPfSolution(solution) => {
            PioValue::DcPfSolution(decode_dc_pf_solution(*solution)?)
        }
        dto::StoredValue::AcPfSolution(solution) => {
            PioValue::AcPfSolution(decode_ac_pf_solution(*solution)?)
        }
        dto::StoredValue::DcOpfSolution(solution) => {
            PioValue::DcOpfSolution(decode_dc_opf_solution(*solution)?)
        }
        dto::StoredValue::AcOpfSolution(solution) => {
            PioValue::AcOpfSolution(decode_ac_opf_solution(*solution)?)
        }
        dto::StoredValue::SocwrOpfSolution(solution) => {
            PioValue::SocwrOpfSolution(decode_socwr_opf_solution(*solution)?)
        }
        dto::StoredValue::McAcPfSolution(solution) => {
            PioValue::McAcPfSolution(decode_mc_ac_pf_solution(*solution)?)
        }
        dto::StoredValue::McAcOpfSolution(solution) => {
            PioValue::McAcOpfSolution(decode_mc_ac_opf_solution(*solution)?)
        }
        dto::StoredValue::AcScucSolution(solution) => {
            PioValue::AcScucSolution(decode_ac_scuc_solution(*solution)?)
        }
    })
}

fn decode_ac_scuc_instance(instance: dto::AcScucInstance) -> Result<powerio_prob::AcScucInstance> {
    powerio_prob::AcScucInstance::new(with_component_ids(*instance.network), *instance.inputs)
        .map_err(|error| invalid(error.to_string()))
}

fn decode_dc_pf_solution(solution: dto::DcPfSolution) -> Result<powerio_prob::DcPfSolution> {
    let instance = std::sync::Arc::new(decode_dc_pf_instance(solution.instance)?);
    with_solution_records(
        powerio_prob::DcPfSolution::new(
            instance,
            solution.termination,
            plain_row(&solution.bus_voltage_angle),
            plain_row(&solution.bus_active_injection),
            plain_row(&solution.branch_from_active_flow),
            plain_row(&solution.branch_to_active_flow),
            decode_three_winding_transformer_terminal_active_powers(
                &solution.three_winding_transformer_terminal_active_powers,
            ),
        )
        .map_err(|error| invalid(error.to_string()))?,
        solution.residuals,
        solution.producer,
        powerio_prob::DcPfSolution::with_residuals,
        powerio_prob::DcPfSolution::with_producer,
        decode_dispatch(solution.generator_dispatch.as_ref()),
        |value, dispatch| {
            value
                .with_generator_dispatch(dispatch)
                .map_err(|error| invalid(error.to_string()))
        },
    )
}

fn decode_ac_pf_solution(solution: dto::AcPfSolution) -> Result<powerio_prob::AcPfSolution> {
    let instance = std::sync::Arc::new(decode_ac_pf_instance(solution.instance)?);
    with_solution_records(
        powerio_prob::AcPfSolution::new(
            instance,
            solution.termination,
            plain_row(&solution.bus_voltage_magnitude),
            plain_row(&solution.bus_voltage_angle),
            plain_row(&solution.bus_active_injection),
            plain_row(&solution.bus_reactive_injection),
            plain_row(&solution.branch_from_active_flow),
            plain_row(&solution.branch_from_reactive_flow),
            plain_row(&solution.branch_to_active_flow),
            plain_row(&solution.branch_to_reactive_flow),
            decode_three_winding_transformer_terminal_powers(
                &solution.three_winding_transformer_terminal_powers,
            ),
        )
        .map_err(|error| invalid(error.to_string()))?,
        solution.residuals,
        solution.producer,
        powerio_prob::AcPfSolution::with_residuals,
        powerio_prob::AcPfSolution::with_producer,
        decode_dispatch(solution.generator_dispatch.as_ref()),
        |value, dispatch| {
            value
                .with_generator_dispatch(dispatch)
                .map_err(|error| invalid(error.to_string()))
        },
    )
}

fn decode_dc_opf_solution(solution: dto::DcOpfSolution) -> Result<powerio_prob::DcOpfSolution> {
    let instance = std::sync::Arc::new(decode_dc_opf_instance(solution.instance)?);
    let mut value = powerio_prob::DcOpfSolution::new(
        instance,
        solution.termination,
        plain_row(&solution.bus_voltage_angle),
        plain_row(&solution.bus_active_injection),
        plain_row(&solution.branch_from_active_flow),
        plain_row(&solution.branch_to_active_flow),
        plain_row(&solution.generator_active_power),
        solution.objective.0,
        decode_three_winding_transformer_terminal_active_powers(
            &solution.three_winding_transformer_terminal_active_powers,
        ),
    )
    .map_err(|error| invalid(error.to_string()))?
    .with_residuals(solution.residuals);
    if let Some(producer) = solution.producer {
        value = value.with_producer(producer);
    }
    if let Some(marginals) = solution.bus_active_power_marginal {
        value = value
            .with_bus_active_power_marginals(plain_row(&marginals))
            .map_err(|error| invalid(error.to_string()))?;
    }
    match (
        solution.branch_from_limit_multiplier,
        solution.branch_to_limit_multiplier,
    ) {
        (Some(from), Some(to)) => {
            value = value
                .with_branch_thermal_limit_multipliers(plain_row(&from), plain_row(&to))
                .map_err(|error| invalid(error.to_string()))?;
        }
        (None, None) => {}
        _ => {
            return Err(invalid(
                "branch from and to thermal limit multipliers must appear together",
            ));
        }
    }
    Ok(value)
}

fn decode_ac_opf_solution(solution: dto::AcOpfSolution) -> Result<powerio_prob::AcOpfSolution> {
    let instance = std::sync::Arc::new(decode_ac_opf_instance(solution.instance)?);
    let mut value = powerio_prob::AcOpfSolution::new(
        instance,
        solution.termination,
        plain_row(&solution.bus_voltage_magnitude),
        plain_row(&solution.bus_voltage_angle),
        plain_row(&solution.bus_active_injection),
        plain_row(&solution.bus_reactive_injection),
        plain_row(&solution.branch_from_active_flow),
        plain_row(&solution.branch_from_reactive_flow),
        plain_row(&solution.branch_to_active_flow),
        plain_row(&solution.branch_to_reactive_flow),
        plain_row(&solution.generator_active_power),
        plain_row(&solution.generator_reactive_power),
        solution.objective.0,
        decode_three_winding_transformer_terminal_powers(
            &solution.three_winding_transformer_terminal_powers,
        ),
    )
    .map_err(|error| invalid(error.to_string()))?
    .with_residuals(solution.residuals);
    if let Some(producer) = solution.producer {
        value = value.with_producer(producer);
    }
    if let Some(marginals) = solution.bus_active_power_marginal {
        value = value
            .with_bus_active_power_marginals(plain_row(&marginals))
            .map_err(|error| invalid(error.to_string()))?;
    }
    if let Some(marginals) = solution.bus_reactive_power_marginal {
        value = value
            .with_bus_reactive_power_marginals(plain_row(&marginals))
            .map_err(|error| invalid(error.to_string()))?;
    }
    match (
        solution.branch_from_limit_multiplier,
        solution.branch_to_limit_multiplier,
    ) {
        (Some(from), Some(to)) => {
            value = value
                .with_branch_thermal_limit_multipliers(plain_row(&from), plain_row(&to))
                .map_err(|error| invalid(error.to_string()))?;
        }
        (None, None) => {}
        _ => {
            return Err(invalid(
                "branch from and to thermal limit multipliers must appear together",
            ));
        }
    }
    Ok(value)
}

fn decode_mc_ac_pf_solution(solution: dto::McAcPfSolution) -> Result<powerio_prob::McAcPfSolution> {
    let instance = std::sync::Arc::new(decode_mc_ac_pf_instance(solution.instance)?);
    let mut value = powerio_prob::McAcPfSolution::new(
        instance,
        solution.termination,
        plain_row(&solution.terminal_voltage_magnitude),
        plain_row(&solution.terminal_voltage_angle),
        plain_row(&solution.source_active_injection),
    )
    .map_err(|error| invalid(error.to_string()))?;
    if let Some(currents) = solution.terminal_current_magnitude {
        value = value
            .with_terminal_currents(plain_row(&currents))
            .map_err(|error| invalid(error.to_string()))?;
    }
    if let Some(powers) = solution.terminal_active_power {
        value = value
            .with_terminal_powers(plain_row(&powers))
            .map_err(|error| invalid(error.to_string()))?;
    }
    value = value.with_residuals(solution.residuals);
    if let Some(producer) = solution.producer {
        value = value.with_producer(producer);
    }
    Ok(value)
}

fn decode_mc_ac_opf_solution(
    solution: dto::McAcOpfSolution,
) -> Result<powerio_prob::McAcOpfSolution> {
    let instance = std::sync::Arc::new(decode_mc_ac_opf_instance(solution.instance)?);
    let mut value = powerio_prob::McAcOpfSolution::new(
        instance,
        solution.termination,
        plain_row(&solution.terminal_voltage_magnitude),
        plain_row(&solution.terminal_voltage_angle),
        plain_row(&solution.source_active_injection),
        plain_row(&solution.generator_active_power),
        solution.objective.0,
    )
    .map_err(|error| invalid(error.to_string()))?;
    if let Some(currents) = solution.terminal_current_magnitude {
        value = value
            .with_terminal_currents(plain_row(&currents))
            .map_err(|error| invalid(error.to_string()))?;
    }
    if let Some(powers) = solution.terminal_active_power {
        value = value
            .with_terminal_powers(plain_row(&powers))
            .map_err(|error| invalid(error.to_string()))?;
    }
    value = value.with_residuals(solution.residuals);
    if let Some(producer) = solution.producer {
        value = value.with_producer(producer);
    }
    Ok(value)
}

fn decode_ac_scuc_solution(solution: dto::AcScucSolution) -> Result<powerio_prob::AcScucSolution> {
    let instance = std::sync::Arc::new(decode_ac_scuc_instance(solution.instance)?);
    let mut network_outputs = powerio_prob::ScucNetworkOutputs::default();
    network_outputs.bus_vm = plain_grid(&solution.network_outputs.bus_vm);
    network_outputs.bus_va = plain_grid(&solution.network_outputs.bus_va);
    network_outputs.shunt_step = solution.network_outputs.shunt_step;
    network_outputs.ac_line_on_status = solution.network_outputs.ac_line_on_status;
    network_outputs.transformer_tm = plain_grid(&solution.network_outputs.transformer_tm);
    network_outputs.transformer_ta = plain_grid(&solution.network_outputs.transformer_ta);
    network_outputs.transformer_on_status = solution.network_outputs.transformer_on_status;
    network_outputs.dc_line_pdc_fr = plain_grid(&solution.network_outputs.dc_line_pdc_fr);
    network_outputs.dc_line_qdc_fr = plain_grid(&solution.network_outputs.dc_line_qdc_fr);
    network_outputs.dc_line_qdc_to = plain_grid(&solution.network_outputs.dc_line_qdc_to);
    let mut device_outputs = powerio_prob::ScucDeviceOutputs::default();
    device_outputs.on_status = solution.device_outputs.on_status;
    device_outputs.startup_status = solution.device_outputs.startup_status;
    device_outputs.shutdown_status = solution.device_outputs.shutdown_status;
    device_outputs.p_on = plain_grid(&solution.device_outputs.p_on);
    device_outputs.q = plain_grid(&solution.device_outputs.q);
    device_outputs.p_reg_res_up = plain_grid(&solution.device_outputs.p_reg_res_up);
    device_outputs.p_reg_res_down = plain_grid(&solution.device_outputs.p_reg_res_down);
    device_outputs.p_syn_res = plain_grid(&solution.device_outputs.p_syn_res);
    device_outputs.p_nsyn_res = plain_grid(&solution.device_outputs.p_nsyn_res);
    device_outputs.p_ramp_res_up_online = plain_grid(&solution.device_outputs.p_ramp_res_up_online);
    device_outputs.p_ramp_res_up_offline =
        plain_grid(&solution.device_outputs.p_ramp_res_up_offline);
    device_outputs.p_ramp_res_down_online =
        plain_grid(&solution.device_outputs.p_ramp_res_down_online);
    device_outputs.p_ramp_res_down_offline =
        plain_grid(&solution.device_outputs.p_ramp_res_down_offline);
    device_outputs.q_res_up = plain_grid(&solution.device_outputs.q_res_up);
    device_outputs.q_res_down = plain_grid(&solution.device_outputs.q_res_down);
    let mut value = powerio_prob::AcScucSolution::new(
        instance,
        solution.termination,
        network_outputs,
        device_outputs,
        solution.objective.map(|value| value.0),
    )
    .map_err(|error| invalid(error.to_string()))?
    .with_residuals(solution.residuals);
    if let Some(producer) = solution.producer {
        value = value.with_producer(producer);
    }
    Ok(value)
}

// One arm per stored value kind; splitting the match would scatter the
// kind-to-decoder table this function is.
#[allow(clippy::too_many_lines)]
fn decode_dispatch(
    dispatch: Option<&dto::GeneratorDispatch>,
) -> Option<powerio_prob::GeneratorDispatch> {
    dispatch.map(|dispatch| {
        let mut decoded = powerio_prob::GeneratorDispatch::default();
        decoded.p_mw = plain_row(&dispatch.p_mw);
        decoded.q_mvar = plain_row(&dispatch.q_mvar);
        decoded
    })
}

/// Apply the shared optional records to a freshly constructed solution.
#[allow(clippy::too_many_arguments)]
fn with_solution_records<S>(
    mut value: S,
    residuals: powerio_prob::Residuals,
    producer: Option<String>,
    with_residuals: impl FnOnce(S, powerio_prob::Residuals) -> S,
    with_producer: impl FnOnce(S, String) -> S,
    dispatch: Option<powerio_prob::GeneratorDispatch>,
    with_dispatch: impl FnOnce(S, powerio_prob::GeneratorDispatch) -> Result<S>,
) -> Result<S> {
    value = with_residuals(value, residuals);
    if let Some(producer) = producer {
        value = with_producer(value, producer);
    }
    if let Some(dispatch) = dispatch {
        value = with_dispatch(value, dispatch)?;
    }
    Ok(value)
}

fn decode_dc_pf_instance(instance: dto::DcPfInstance) -> Result<powerio_prob::DcPfInstance> {
    let network = with_component_ids(*instance.network);
    let mut decoded = powerio_prob::DcPfInstance::from_network(network)
        .map_err(|error| invalid(error.to_string()))?
        .with_branch_susceptance_formula(dc_formula_from_name(&instance.approximation)?);
    if let Some(stored) = instance.initial_point {
        let point = decode_balanced_point_assignment(decoded.network(), stored)?;
        decoded = decoded.with_initial_point(point);
    }
    Ok(decoded)
}

fn decode_ac_pf_instance(instance: dto::AcPfInstance) -> Result<powerio_prob::AcPfInstance> {
    let mut decoded = powerio_prob::AcPfInstance::new(
        with_component_ids(*instance.network),
        instance.specifications,
    )
    .map_err(|error| invalid(error.to_string()))?;
    if let Some(stored) = instance.initial_point {
        let point = decode_balanced_point_assignment(decoded.network(), stored)?;
        decoded = decoded.with_initial_point(point);
    }
    Ok(decoded)
}

fn decode_dc_opf_instance(instance: dto::DcOpfInstance) -> Result<powerio_prob::DcOpfInstance> {
    let mut decoded =
        powerio_prob::DcOpfInstance::from_network(with_component_ids(*instance.network))
            .map_err(|error| invalid(error.to_string()))?
            .with_branch_susceptance_formula(dc_formula_from_name(&instance.approximation)?)
            .with_objective(decode_objective(instance.objective))
            .with_constraints(instance.constraints);
    if let Some(stored) = instance.initial_point {
        let point = decode_balanced_point_assignment(decoded.network(), stored)?;
        decoded = decoded.with_initial_point(point);
    }
    Ok(decoded)
}

fn decode_ac_opf_instance(instance: dto::AcOpfInstance) -> Result<powerio_prob::AcOpfInstance> {
    let mut decoded =
        powerio_prob::AcOpfInstance::from_network(with_component_ids(*instance.network))
            .map_err(|error| invalid(error.to_string()))?
            .with_objective(decode_objective(instance.objective))
            .with_constraints(instance.constraints);
    if let Some(stored) = instance.initial_point {
        let point = decode_balanced_point_assignment(decoded.network(), stored)?;
        decoded = decoded.with_initial_point(point);
    }
    Ok(decoded)
}

fn decode_mc_ac_pf_instance(instance: dto::McAcPfInstance) -> Result<powerio_prob::McAcPfInstance> {
    let mut decoded = powerio_prob::McAcPfInstance::from_network(*instance.network)
        .map_err(|error| invalid(error.to_string()))?;
    if let Some(stored) = instance.initial_point {
        let point = decode_mc_point_assignment(decoded.network(), stored)?;
        decoded = decoded.with_initial_point(point);
    }
    Ok(decoded)
}

fn decode_mc_ac_opf_instance(
    instance: dto::McAcOpfInstance,
) -> Result<powerio_prob::McAcOpfInstance> {
    let mut decoded = powerio_prob::McAcOpfInstance::from_network(*instance.network)
        .map_err(|error| invalid(error.to_string()))?
        .with_objective(decode_objective(instance.objective))
        .with_constraints(instance.constraints);
    if let Some(stored) = instance.initial_point {
        let point = decode_mc_point_assignment(decoded.network(), stored)?;
        decoded = decoded.with_initial_point(point);
    }
    Ok(decoded)
}

// ---- record encoding / decoding --------------------------------------------

fn encode_source(source: &SourceDescriptor) -> dto::SourceDescriptor {
    dto::SourceDescriptor {
        id: dto::SourceId(source.id().as_str().to_string()),
        name: source.name().to_string(),
        byte_length: source.byte_length(),
        format: source.format().map(|format| format.as_str().to_string()),
        digest: source.digest().map(|digest| dto::Digest {
            algorithm: dto::DigestAlgorithm::Sha256,
            value: digest.value().to_string(),
        }),
    }
}

fn decode_source(source: dto::SourceDescriptor) -> Result<SourceDescriptor> {
    let id = SourceId::new(source.id.0).map_err(|error| invalid(error.to_string()))?;
    let mut decoded = SourceDescriptor::new(id, source.name, source.byte_length)
        .map_err(|error| invalid(error.to_string()))?;
    if let Some(format) = source.format {
        decoded = decoded.with_format(
            powerio_core::FormatId::new(format).map_err(|error| invalid(error.to_string()))?,
        );
    }
    if let Some(digest) = source.digest {
        decoded = decoded
            .with_digest(Digest::sha256(digest.value).map_err(|error| invalid(error.to_string()))?);
    }
    Ok(decoded)
}

fn encode_span(span: &SourceSpan) -> dto::SourceSpan {
    dto::SourceSpan {
        source: dto::SourceId(span.source().as_str().to_string()),
        byte_start: span.byte_start(),
        byte_end: span.byte_end(),
    }
}

fn decode_span(span: dto::SourceSpan) -> Result<SourceSpan> {
    let source = SourceId::new(span.source.0).map_err(|error| invalid(error.to_string()))?;
    SourceSpan::new(source, span.byte_start, span.byte_end)
        .map_err(|error| invalid(error.to_string()))
}

fn encode_relation(relation: SourceRelation) -> Result<dto::SourceRelation> {
    Ok(match relation {
        SourceRelation::Exact => dto::SourceRelation::Exact,
        SourceRelation::Defaulted => dto::SourceRelation::Defaulted,
        SourceRelation::Inferred => dto::SourceRelation::Inferred,
        SourceRelation::ConvertedUnits => dto::SourceRelation::ConvertedUnits,
        SourceRelation::Aggregated => dto::SourceRelation::Aggregated,
        SourceRelation::Split => dto::SourceRelation::Split,
        SourceRelation::Synthetic => dto::SourceRelation::Synthetic,
        SourceRelation::Transformed => dto::SourceRelation::Transformed,
        SourceRelation::RetainedExtra => dto::SourceRelation::RetainedExtra,
        // The runtime enum is non_exhaustive for additive growth; a new
        // relation must gain a stored spelling before it can be written.
        _ => return Err(invalid("unmapped source relation")),
    })
}

fn decode_relation(relation: dto::SourceRelation) -> SourceRelation {
    match relation {
        dto::SourceRelation::Exact => SourceRelation::Exact,
        dto::SourceRelation::Defaulted => SourceRelation::Defaulted,
        dto::SourceRelation::Inferred => SourceRelation::Inferred,
        dto::SourceRelation::ConvertedUnits => SourceRelation::ConvertedUnits,
        dto::SourceRelation::Aggregated => SourceRelation::Aggregated,
        dto::SourceRelation::Split => SourceRelation::Split,
        dto::SourceRelation::Synthetic => SourceRelation::Synthetic,
        dto::SourceRelation::Transformed => SourceRelation::Transformed,
        dto::SourceRelation::RetainedExtra => SourceRelation::RetainedExtra,
    }
}

fn encode_map_entry(entry: &SourceMapEntry) -> Result<dto::SourceMapEntry> {
    Ok(dto::SourceMapEntry {
        target: entry.target().to_string(),
        relation: encode_relation(entry.relation())?,
        spans: entry.spans().iter().map(encode_span).collect(),
    })
}

fn decode_map_entry(entry: dto::SourceMapEntry) -> Result<SourceMapEntry> {
    let spans = entry
        .spans
        .into_iter()
        .map(decode_span)
        .collect::<Result<Vec<_>>>()?;
    SourceMapEntry::new(entry.target, decode_relation(entry.relation), spans)
        .map_err(|error| invalid(error.to_string()))
}

fn encode_severity(severity: DiagnosticSeverity) -> dto::Severity {
    match severity {
        DiagnosticSeverity::Error => dto::Severity::Error,
        DiagnosticSeverity::Warning => dto::Severity::Warning,
        DiagnosticSeverity::Remark => dto::Severity::Remark,
        DiagnosticSeverity::Note => dto::Severity::Note,
    }
}

fn decode_severity(severity: dto::Severity) -> DiagnosticSeverity {
    match severity {
        dto::Severity::Error => DiagnosticSeverity::Error,
        dto::Severity::Warning => DiagnosticSeverity::Warning,
        dto::Severity::Remark => DiagnosticSeverity::Remark,
        dto::Severity::Note => DiagnosticSeverity::Note,
    }
}

/// Every diagnostic's stored form, each given an identifier: the one its
/// runtime record already carries, or the lowest `d{n}` not already claimed
/// by another diagnostic in this same list. Checked against every existing
/// id (explicit or synthesized earlier in this pass) rather than derived
/// from list position, so a diagnostic appended with no id of its own can
/// never collide with one an external document set explicitly, and
/// reordering the list can't make a collision appear later either.
pub(crate) fn encode_diagnostics(diagnostics: &[Diagnostic]) -> Vec<dto::Diagnostic> {
    let mut used: HashSet<String> = diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.id().map(|id| id.as_str().to_owned()))
        .collect();
    diagnostics
        .iter()
        .map(|diagnostic| {
            let id = if let Some(id) = diagnostic.id() {
                id.as_str().to_owned()
            } else {
                let minted = unused_id("d", &used);
                used.insert(minted.clone());
                minted
            };
            encode_diagnostic(id, diagnostic)
        })
        .collect()
}

/// The lowest `{prefix}{n}` (n = 0, 1, 2, ...) not already in `used`. Always
/// found within `used.len() + 1` tries: a finite set can only rule out that
/// many distinct candidates, so the search below always terminates.
#[allow(clippy::maybe_infinite_iter)]
fn unused_id(prefix: &str, used: &HashSet<String>) -> String {
    (0..)
        .map(|n| format!("{prefix}{n}"))
        .find(|candidate| !used.contains(candidate))
        .expect("a finite used set cannot rule out every candidate")
}

fn encode_diagnostic(id: String, diagnostic: &Diagnostic) -> dto::Diagnostic {
    dto::Diagnostic {
        id: dto::DiagnosticId(id),
        severity: encode_severity(diagnostic.severity()),
        code: diagnostic.code().to_string(),
        message: diagnostic.message().to_string(),
        target: diagnostic.target().map(str::to_string),
        spans: diagnostic.spans().iter().map(encode_span).collect(),
        related: diagnostic
            .related()
            .iter()
            .map(|id| dto::DiagnosticId(id.as_str().to_string()))
            .collect(),
        details: diagnostic.details().clone().into_iter().collect(),
        suggested_action: diagnostic.suggested_action().map(str::to_string),
    }
}

fn decode_diagnostic(diagnostic: dto::Diagnostic) -> Result<Diagnostic> {
    let code = DiagnosticCode::new(diagnostic.code).map_err(|error| invalid(error.to_string()))?;
    let mut decoded = Diagnostic::new(
        code,
        decode_severity(diagnostic.severity),
        diagnostic.message,
    )
    .with_id(DiagnosticId::new(diagnostic.id.0).map_err(|error| invalid(error.to_string()))?);
    if let Some(target) = diagnostic.target {
        decoded = decoded
            .with_target(target)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for span in diagnostic.spans {
        decoded = decoded
            .with_span(decode_span(span)?)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for related in diagnostic.related {
        decoded = decoded
            .with_related(DiagnosticId::new(related.0).map_err(|error| invalid(error.to_string()))?)
            .map_err(|error| invalid(error.to_string()))?;
    }
    if !diagnostic.details.is_empty() {
        decoded = decoded
            .with_details(diagnostic.details.into_iter().collect())
            .map_err(|error| invalid(error.to_string()))?;
    }
    if let Some(action) = diagnostic.suggested_action {
        decoded = decoded.with_suggested_action(action);
    }
    Ok(decoded)
}

fn encode_history_kind(kind: HistoryKind) -> Result<dto::HistoryKind> {
    Ok(match kind {
        HistoryKind::Parse => dto::HistoryKind::Parse,
        HistoryKind::Transform => dto::HistoryKind::Transform,
        HistoryKind::Edit => dto::HistoryKind::Edit,
        HistoryKind::Repair => dto::HistoryKind::Repair,
        HistoryKind::Solve => dto::HistoryKind::Solve,
        // As with relations: a new history kind gains a stored spelling first.
        _ => return Err(invalid("unmapped history kind")),
    })
}

fn decode_history_kind(kind: dto::HistoryKind) -> HistoryKind {
    match kind {
        dto::HistoryKind::Parse => HistoryKind::Parse,
        dto::HistoryKind::Transform => HistoryKind::Transform,
        dto::HistoryKind::Edit => HistoryKind::Edit,
        dto::HistoryKind::Repair => HistoryKind::Repair,
        dto::HistoryKind::Solve => HistoryKind::Solve,
    }
}

fn encode_history(entry: &HistoryEntry) -> Result<dto::HistoryEntry> {
    Ok(dto::HistoryEntry {
        id: dto::HistoryId(entry.id().as_str().to_string()),
        kind: encode_history_kind(entry.kind())?,
        name: entry.name().to_string(),
        input_type: entry.input_type().map(str::to_string),
        output_type: entry.output_type().map(str::to_string),
        parameters: entry.parameters().clone(),
        assumptions: entry.assumptions().to_vec(),
        losses: entry.losses().to_vec(),
    })
}

fn decode_history(entry: dto::HistoryEntry) -> Result<HistoryEntry> {
    let id = HistoryId::new(entry.id.0).map_err(|error| invalid(error.to_string()))?;
    let mut decoded = HistoryEntry::new(id, decode_history_kind(entry.kind), entry.name)
        .map_err(|error| invalid(error.to_string()))?;
    if let Some(input) = entry.input_type {
        decoded = decoded
            .with_input_type(input)
            .map_err(|error| invalid(error.to_string()))?;
    }
    if let Some(output) = entry.output_type {
        decoded = decoded
            .with_output_type(output)
            .map_err(|error| invalid(error.to_string()))?;
    }
    decode_history_records(decoded, entry.parameters, entry.assumptions, entry.losses)
}

fn decode_history_records(
    mut decoded: HistoryEntry,
    parameters: BTreeMap<String, serde_json::Value>,
    assumptions: Vec<String>,
    losses: Vec<String>,
) -> Result<HistoryEntry> {
    if !parameters.is_empty() {
        decoded = decoded
            .with_parameters(parameters)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for assumption in assumptions {
        decoded = decoded
            .with_assumption(assumption)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for loss in losses {
        decoded = decoded
            .with_loss(loss)
            .map_err(|error| invalid(error.to_string()))?;
    }
    Ok(decoded)
}

#[cfg(test)]
mod collection_ir_tests {
    use super::*;
    use powerio_core::{ComponentId, Scenario, ScenarioId, ScenarioSet};
    use powerio_tx::{Bus, BusId, BusType};
    use std::sync::Arc;

    fn network() -> crate::BalancedNetwork {
        crate::BalancedNetwork::in_memory(
            "stored-collection",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            Vec::new(),
        )
    }

    fn multiconductor_network() -> powerio_dist::MulticonductorNetwork {
        powerio_dist::MulticonductorNetwork::named("stored-multiconductor-collection")
    }

    fn time_points() -> Vec<TimePoint> {
        vec![TimePoint::new("t0", None).unwrap()]
    }

    fn balanced_network_series() -> TimeSeries<crate::BalancedNetwork> {
        TimeSeries::new(time_points(), vec![network()]).unwrap()
    }

    fn multiconductor_network_series() -> TimeSeries<powerio_dist::MulticonductorNetwork> {
        TimeSeries::new(time_points(), vec![multiconductor_network()]).unwrap()
    }

    fn balanced_point_series() -> TimeSeries<powerio_prob::OperatingPoint<crate::BalancedNetwork>> {
        powerio_prob::BalancedOperatingPointBuilder::new(network(), time_points())
            .bus_voltage_magnitudes(vec![1.0])
            .build()
            .unwrap()
    }

    fn multiconductor_point_series()
    -> TimeSeries<powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>> {
        powerio_prob::MulticonductorOperatingPointBuilder::new(
            multiconductor_network(),
            time_points(),
        )
        .build()
        .unwrap()
    }

    fn one_scenario<T>(value: T) -> ScenarioSet<T> {
        ScenarioSet::new(vec![Scenario::new(
            ScenarioId::new("base").unwrap(),
            None,
            value,
        )])
        .unwrap()
    }

    fn assert_ir_round_trip(value: PioValue, expected_type: &str) {
        assert_eq!(value.type_name(), expected_type);
        let text = emit_module(&PioModule::new(value)).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(raw["version"], dto::SCHEMA_VERSION);
        assert_eq!(raw["value"]["type"], expected_type);
        let decoded = read_module(&text).unwrap();
        assert_eq!(decoded.value.type_name(), expected_type);
        assert_eq!(emit_module(&decoded).unwrap(), text);
    }

    #[test]
    fn ir_round_trips_every_registered_operating_point_composition() {
        let balanced_series = balanced_point_series();
        let multiconductor_series = multiconductor_point_series();
        let balanced_point = balanced_series.values()[0].clone();
        let multiconductor_point = multiconductor_series.values()[0].clone();
        let balanced_network_series = balanced_network_series();
        let multiconductor_network_series = multiconductor_network_series();

        for (value, expected_type) in [
            (PioValue::from(balanced_point.clone()), BALANCED_POINT_TYPE),
            (
                PioValue::from(multiconductor_point.clone()),
                MULTICONDUCTOR_POINT_TYPE,
            ),
            (
                PioValue::from(balanced_network_series.clone()),
                BALANCED_NETWORK_SERIES_TYPE,
            ),
            (
                PioValue::from(multiconductor_network_series.clone()),
                MULTICONDUCTOR_NETWORK_SERIES_TYPE,
            ),
            (
                PioValue::from(balanced_series.clone()),
                BALANCED_POINT_SERIES_TYPE,
            ),
            (
                PioValue::from(multiconductor_series.clone()),
                MULTICONDUCTOR_POINT_SERIES_TYPE,
            ),
            (
                PioValue::from(one_scenario(network())),
                "powerio.ScenarioSet<powerio.BalancedNetwork>",
            ),
            (
                PioValue::from(one_scenario(multiconductor_network())),
                "powerio.ScenarioSet<powerio.MulticonductorNetwork>",
            ),
            (
                PioValue::from(one_scenario(balanced_point)),
                "powerio.ScenarioSet<powerio.OperatingPoint<powerio.BalancedNetwork>>",
            ),
            (
                PioValue::from(one_scenario(multiconductor_point)),
                "powerio.ScenarioSet<powerio.OperatingPoint<powerio.MulticonductorNetwork>>",
            ),
            (
                PioValue::from(one_scenario(balanced_network_series)),
                "powerio.ScenarioSet<powerio.TimeSeries<powerio.BalancedNetwork>>",
            ),
            (
                PioValue::from(one_scenario(multiconductor_network_series)),
                "powerio.ScenarioSet<powerio.TimeSeries<powerio.MulticonductorNetwork>>",
            ),
            (
                PioValue::from(one_scenario(balanced_series)),
                "powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>>",
            ),
            (
                PioValue::from(one_scenario(multiconductor_series)),
                "powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>>",
            ),
        ] {
            assert_ir_round_trip(value, expected_type);
        }
    }

    #[test]
    fn ir_preserves_types_for_empty_nested_collections() {
        let empty_points = TimeSeries::<powerio_prob::OperatingPoint<crate::BalancedNetwork>>::new(
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        assert_ir_round_trip(PioValue::from(empty_points), BALANCED_POINT_SERIES_TYPE);

        let empty_scenarios = ScenarioSet::<
            TimeSeries<powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>>,
        >::new(Vec::new())
        .unwrap();
        assert_ir_round_trip(
            PioValue::from(empty_scenarios),
            "powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>>",
        );
    }

    #[test]
    fn ir_preserves_a_new_override_on_one_time_entry() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
        let network = powerio_tx::parse(powerio_core::Source::open(path).unwrap())
            .unwrap()
            .into_value();
        let points = vec![
            TimePoint::new("t0", None).unwrap(),
            TimePoint::new("t1", None).unwrap(),
        ];
        let mut series = powerio_prob::BalancedOperatingPointBuilder::new(network, points)
            .build()
            .unwrap();
        let load_id = series.values()[1].network().loads()[0].uid.clone().unwrap();
        powerio_prob::apply_updates(
            series.get_mut(1).unwrap(),
            &[powerio_prob::OperatingPointUpdate::LoadActivePower {
                load: ComponentId::new("load", load_id.clone()).unwrap(),
                terminal: None,
                p: powerio_prob::ActivePower::from_megawatts(91.5),
            }],
        )
        .unwrap();
        assert_eq!(series.values()[0].load_active_power(&load_id), None);
        assert_eq!(series.values()[1].load_active_power(&load_id), Some(91.5));

        let text = emit_module(&PioModule::new(PioValue::from(series))).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(
            raw["value"]["data"]["values"][0]["quantities"]
                .get("load_active_power")
                .is_none()
        );
        assert_eq!(
            raw["value"]["data"]["values"][1]["quantities"]["load_active_power"]["values"][0],
            91.5
        );

        let decoded = read_module(&text).unwrap();
        let PioValue::TimeSeries(series) = &decoded.value else {
            panic!("expected an operating point time series");
        };
        let PioValue::BalancedOperatingPoint(first) = series.get(0).unwrap() else {
            panic!("expected a balanced operating point");
        };
        let PioValue::BalancedOperatingPoint(second) = series.get(1).unwrap() else {
            panic!("expected a balanced operating point");
        };
        assert_eq!(first.load_active_power(&load_id), None);
        assert_eq!(second.load_active_power(&load_id), Some(91.5));
    }

    #[test]
    fn ir_round_trips_socwr_solution() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
        let network = powerio_tx::parse(powerio_core::Source::open(path).unwrap())
            .unwrap()
            .into_value();
        let instance = Arc::new(powerio_prob::AcOpfInstance::from_network(network).unwrap());
        let buses = instance.network().buses().len();
        let branches = instance.network().branches().len();
        let generators = instance.network().generators().len();
        let mut values = powerio_prob::solution::SocwrOpfValues::default();
        values.bus_voltage_magnitude_squared = vec![1.0; buses];
        values.branch_voltage_product_real = vec![0.99; branches];
        values.branch_voltage_product_imaginary = vec![0.01; branches];
        values.generator_active_power = vec![10.0; generators];
        values.generator_reactive_power = vec![2.0; generators];
        values.branch_from_active_power = vec![3.0; branches];
        values.branch_from_reactive_power = vec![0.5; branches];
        values.branch_to_active_power = vec![-2.9; branches];
        values.branch_to_reactive_power = vec![-0.4; branches];
        let solution = powerio_prob::solution::SocwrOpfSolution::new(
            instance,
            powerio_prob::Termination::Converged,
            values,
            5_000.0,
        )
        .unwrap()
        .with_producer("stored-ir-test");

        assert_ir_round_trip(PioValue::from(solution), "powerio.SocwrOpfSolution");
    }

    #[test]
    fn ir_calls_a_calculation_seed_an_initial_point() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
        let network = powerio_tx::parse(powerio_core::Source::open(path).unwrap())
            .unwrap()
            .into_value();
        let initial_point = powerio_prob::BalancedOperatingPointBuilder::for_point(network.clone())
            .bus_voltage_magnitudes(vec![1.0; network.buses().len()])
            .build_point()
            .unwrap();
        let instance = powerio_prob::AcOpfInstance::from_network(network)
            .unwrap()
            .with_initial_point(initial_point);
        let text = emit_module(&PioModule::new(PioValue::from(instance))).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&text).unwrap();

        assert!(raw["value"]["data"].get("initial_point").is_some());
        let decoded = read_module(&text).unwrap();
        assert_eq!(emit_module(&decoded).unwrap(), text);
    }

    #[test]
    fn ir_history_names_structural_types() {
        let history = HistoryEntry::new(
            HistoryId::new("parse-1").unwrap(),
            HistoryKind::Parse,
            "parse",
        )
        .unwrap()
        .with_input_type("powerio.Source")
        .unwrap()
        .with_output_type(BALANCED_NETWORK_TYPE)
        .unwrap();
        let mut module = PioModule::new(PioValue::from(network()));
        module.add_history_entry(history).unwrap();
        let text = emit_module(&module).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&text).unwrap();

        assert_eq!(raw["history"][0]["input_type"], "powerio.Source");
        assert_eq!(raw["history"][0]["output_type"], BALANCED_NETWORK_TYPE);
        let decoded = read_module(&text).unwrap();
        assert_eq!(decoded.history()[0].input_type(), Some("powerio.Source"));
        assert_eq!(
            decoded.history()[0].output_type(),
            Some(BALANCED_NETWORK_TYPE)
        );
    }
}

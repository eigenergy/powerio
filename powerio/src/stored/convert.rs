//! The one bridge between the runtime module and the stored version 1 wire:
//! encode a `PioModule<PioValue>` to `.pio.json`, decode version 1 back, and
//! upgrade released 0.9.x `NetworkPackage` documents one way.

use std::collections::BTreeMap;

use powerio_core::{
    Diagnostic, DiagnosticCode, DiagnosticId, DiagnosticSeverity, Digest, HistoryEntry, HistoryId,
    HistoryKind, PioModule, Producer, SourceDescriptor, SourceId, SourceMapEntry, SourceRelation,
    SourceSpan, TimePoint, TimeSeries,
};

use super::dto::{
    self, DiagnosticIdV1, DiagnosticV1, DigestAlgorithmV1, DigestV1, DurationV1, HistoryEntryV1,
    HistoryIdV1, HistoryKindV1, ProducerV1, SeverityV1, SourceDescriptorV1, SourceIdV1,
    SourceMapEntryV1, SourceRelationV1, SourceSpanV1, StoredF64, StoredModuleV1, StoredQuantityV1,
    StoredValueV1, TimePointV1,
};
use crate::codes;
use crate::value::PioValue;

type Result<T> = std::result::Result<T, powerio_core::Error>;

fn invalid(message: impl Into<String>) -> powerio_core::Error {
    powerio_core::Error::new(&codes::READ_MODULE_INVALID, message)
}

/// The balanced instantaneous quantity vocabulary is
/// [`powerio_prob::BALANCED_STATE_QUANTITIES`]: the set the writer emits is
/// exactly the set the reader accepts, from one definition.
use powerio_prob::BALANCED_STATE_QUANTITIES;

/// Serialize one runtime module to the `.pio.json` version 1 document.
///
/// # Errors
/// A value whose stored form cannot be produced, or a serialization failure.
pub fn write_module(module: &PioModule<PioValue>) -> Result<String> {
    let stored = StoredModuleV1 {
        schema: dto::SCHEMA_NAME.to_string(),
        version: dto::SCHEMA_VERSION,
        producer: ProducerV1 {
            name: module.producer().name().to_string(),
            version: module.producer().version().to_string(),
        },
        value: encode_value(module.value())?,
        sources: module.sources().iter().map(encode_source).collect(),
        source_map: module.source_map().iter().map(encode_map_entry).collect(),
        diagnostics: module
            .diagnostics()
            .iter()
            .enumerate()
            .map(|(index, diagnostic)| encode_diagnostic(index, diagnostic))
            .collect(),
        history: module.history().iter().map(encode_history).collect(),
        extensions: module.extensions().clone(),
    };
    dto::validate(&stored).map_err(invalid)?;
    serde_json::to_string_pretty(&stored).map_err(|error| invalid(error.to_string()))
}

/// Decode `.pio.json` text into the runtime module: version 1 directly, a
/// released 0.9.x `NetworkPackage` through the one way upgrade, anything else
/// refused with its stated identity.
///
/// # Errors
/// An unsupported schema or version, an invalid document, or a legacy
/// document the upgrade must refuse (a nonempty `study`).
pub fn read_module(text: &str) -> Result<PioModule<PioValue>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let header: dto::StoredHeader =
        serde_json::from_str(text).map_err(|error| invalid(error.to_string()))?;
    match (header.schema.as_deref(), header.version) {
        (Some(dto::SCHEMA_NAME), Some(dto::SCHEMA_VERSION)) => {
            let stored: StoredModuleV1 =
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
        (None, _) => super::upgrade::upgrade_legacy(text, header.powerio_version.as_deref()),
    }
}

// ---- value encoding ---------------------------------------------------------

fn encode_value(value: &PioValue) -> Result<StoredValueV1> {
    Ok(match value {
        PioValue::BalancedNetwork(network) => {
            StoredValueV1::BalancedNetwork(Box::new(network.clone()))
        }
        PioValue::MulticonductorNetwork(network) => {
            StoredValueV1::MulticonductorNetwork(Box::new(network.clone()))
        }
        PioValue::BalancedNetworkTimeSeries(series) => {
            StoredValueV1::BalancedNetworkTimeSeries(dto::BalancedNetworkTimeSeriesV1 {
                time_points: series.time_points().iter().map(encode_time_point).collect(),
                values: series.values().to_vec(),
            })
        }
        PioValue::BalancedOperatingPointTimeSeries(series) => {
            StoredValueV1::BalancedOperatingPointTimeSeries(encode_operating_points(series)?)
        }
        PioValue::BalancedNetworkScenarioSet(set) => {
            StoredValueV1::BalancedNetworkScenarioSet(dto::BalancedNetworkScenarioSetV1 {
                scenarios: set
                    .iter()
                    .map(|scenario| dto::BalancedNetworkScenarioV1 {
                        id: scenario.id().as_str().to_string(),
                        probability: scenario.probability().map(StoredF64),
                        value: scenario.value().clone(),
                    })
                    .collect(),
            })
        }
        PioValue::MulticonductorOperatingPointTimeSeries(series) => {
            StoredValueV1::MulticonductorOperatingPointTimeSeries(encode_mc_operating_points(
                series,
            )?)
        }
        PioValue::DcPfInstance(instance) => {
            StoredValueV1::DcPfInstance(encode_dc_pf_instance(instance))
        }
        PioValue::AcPfInstance(instance) => {
            StoredValueV1::AcPfInstance(encode_ac_pf_instance(instance))
        }
        PioValue::DcOpfInstance(instance) => {
            StoredValueV1::DcOpfInstance(encode_dc_opf_instance(instance))
        }
        PioValue::AcOpfInstance(instance) => {
            StoredValueV1::AcOpfInstance(encode_ac_opf_instance(instance))
        }
        PioValue::McAcPfInstance(instance) => {
            StoredValueV1::McAcPfInstance(encode_mc_ac_pf_instance(instance))
        }
        PioValue::McAcOpfInstance(instance) => {
            StoredValueV1::McAcOpfInstance(encode_mc_ac_opf_instance(instance))
        }
        PioValue::AcScucInstance(instance) => {
            StoredValueV1::AcScucInstance(dto::AcScucInstanceV1 {
                network: Box::new(instance.network().clone()),
                inputs: Box::new(instance.inputs().clone()),
            })
        }
        PioValue::DcPfSolution(solution) => {
            StoredValueV1::DcPfSolution(Box::new(encode_dc_pf_solution(solution)))
        }
        PioValue::AcPfSolution(solution) => {
            StoredValueV1::AcPfSolution(Box::new(encode_ac_pf_solution(solution)))
        }
        PioValue::DcOpfSolution(solution) => {
            StoredValueV1::DcOpfSolution(Box::new(encode_dc_opf_solution(solution)))
        }
        PioValue::AcOpfSolution(solution) => {
            StoredValueV1::AcOpfSolution(Box::new(encode_ac_opf_solution(solution)))
        }
        PioValue::McAcPfSolution(solution) => {
            StoredValueV1::McAcPfSolution(Box::new(encode_mc_ac_pf_solution(solution)))
        }
        PioValue::McAcOpfSolution(solution) => {
            StoredValueV1::McAcOpfSolution(Box::new(encode_mc_ac_opf_solution(solution)))
        }
        PioValue::AcScucSolution(solution) => {
            StoredValueV1::AcScucSolution(Box::new(encode_ac_scuc_solution(solution)))
        }
    })
}

/// The multiconductor instantaneous vocabulary, mirroring the state module's
/// registered quantity names.
const MULTICONDUCTOR_QUANTITIES: [&str; 7] = [
    "terminal_voltage_magnitude",
    "terminal_voltage_angle",
    "load_active_power",
    "load_reactive_power",
    "switch_closed",
    "transformer_tap",
    "capacitor_steps",
];

/// The stable cross language DC formula names, spelled locally because this
/// branch precedes the shared helper.
fn dc_formula_name(convention: crate::DcConvention) -> &'static str {
    match convention {
        crate::DcConvention::TapAdjustedReactance => "tap_adjusted_reactance",
        crate::DcConvention::ReactanceOnly => "reactance_only",
        // The default series formula, and the spelling any future variant
        // must replace deliberately.
        _ => "series_susceptance",
    }
}

fn dc_formula_from_name(name: &str) -> Result<crate::DcConvention> {
    match name {
        "series_susceptance" => Ok(crate::DcConvention::SeriesSusceptance),
        "tap_adjusted_reactance" => Ok(crate::DcConvention::TapAdjustedReactance),
        "reactance_only" => Ok(crate::DcConvention::ReactanceOnly),
        other => Err(invalid(format!(
            "unknown branch susceptance formula `{other}`"
        ))),
    }
}

fn encode_point<N>(
    point: &powerio_prob::OperatingPoint<N>,
    vocabulary: &[&str],
) -> dto::StoredOperatingPointV1 {
    let mut quantities = BTreeMap::new();
    for name in vocabulary {
        let Some(order) = point.identity_order(name) else {
            continue;
        };
        let identities: Vec<String> = order.map(str::to_string).collect();
        let Some(row) = point.quantity_values(name) else {
            continue;
        };
        quantities.insert(
            (*name).to_string(),
            StoredQuantityV1 {
                identities,
                values: row.into_iter().map(StoredF64).collect(),
            },
        );
    }
    dto::StoredOperatingPointV1 { quantities }
}

/// A stored quantity's identities must be exactly the order the network
/// resolves for it; a permutation or a different set is refused rather than
/// silently rebound to positions the document did not state.
fn check_identity_order(quantity: &str, stated: &[String], resolved: &[String]) -> Result<()> {
    if stated.len() != resolved.len() {
        return Err(invalid(format!(
            "quantity `{quantity}` states {} identities; the network resolves {}",
            stated.len(),
            resolved.len()
        )));
    }
    if let Some(position) = stated.iter().zip(resolved).position(|(a, b)| a != b) {
        return Err(invalid(format!(
            "quantity `{quantity}` identity {position} is `{}`; the network resolves `{}` at \
             that position",
            stated[position], resolved[position]
        )));
    }
    Ok(())
}

fn decode_balanced_point(
    network: &crate::BalancedNetwork,
    stored: dto::StoredOperatingPointV1,
) -> Result<powerio_prob::OperatingPoint<crate::BalancedNetwork>> {
    let time_points =
        vec![TimePoint::new("initial", None).map_err(|error| invalid(error.to_string()))?];
    let mut builder = powerio_prob::BalancedStateBuilder::new(network.clone(), time_points);
    for (name, quantity) in stored.quantities {
        let resolved = builder
            .identity_order(&name)
            .map_err(|error| invalid(error.to_string()))?;
        check_identity_order(&name, &quantity.identities, &resolved)?;
        let values: Vec<f64> = quantity.values.iter().map(|value| value.0).collect();
        builder = builder
            .dense_by_name(&name, values)
            .map_err(|error| invalid(error.to_string()))?;
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

fn decode_mc_point(
    network: &powerio_dist::MulticonductorNetwork,
    stored: dto::StoredOperatingPointV1,
) -> Result<powerio_prob::OperatingPoint<powerio_dist::MulticonductorNetwork>> {
    let time_points =
        vec![TimePoint::new("initial", None).map_err(|error| invalid(error.to_string()))?];
    let mut builder = powerio_prob::MulticonductorStateBuilder::new(network.clone(), time_points);
    for (name, quantity) in stored.quantities {
        let resolved = builder
            .identity_order(&name)
            .map_err(|error| invalid(error.to_string()))?;
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
    builder: powerio_prob::MulticonductorStateBuilder,
    name: &str,
    values: Vec<f64>,
) -> Result<powerio_prob::MulticonductorStateBuilder> {
    Ok(match name {
        "terminal_voltage_magnitude" => builder.terminal_voltage_magnitudes(values),
        "terminal_voltage_angle" => builder.terminal_voltage_angles(values),
        "load_active_power" => builder.load_active_powers(values),
        "load_reactive_power" => builder.load_reactive_powers(values),
        "switch_closed" => builder.switch_closed(values),
        "transformer_tap" => builder.transformer_taps(values),
        "capacitor_steps" => builder.capacitor_steps(values),
        other => {
            return Err(invalid(format!(
                "`{other}` is not a multiconductor state quantity"
            )));
        }
    })
}

fn encode_mc_operating_points(
    series: &powerio_prob::MulticonductorOperatingPoints,
) -> Result<dto::MulticonductorOperatingPointTimeSeriesV1> {
    let first = series
        .values()
        .first()
        .ok_or_else(|| invalid("an operating point series needs at least one point"))?;
    let mut quantities = BTreeMap::new();
    for name in MULTICONDUCTOR_QUANTITIES {
        let Some(order) = first.identity_order(name) else {
            continue;
        };
        let identities: Vec<String> = order.map(str::to_string).collect();
        let mut values = Vec::with_capacity(series.len() * identities.len());
        for point in series.values() {
            let row = point
                .quantity_values(name)
                .ok_or_else(|| invalid(format!("quantity `{name}` vanished mid series")))?;
            values.extend(row.into_iter().map(StoredF64));
        }
        quantities.insert(name.to_string(), StoredQuantityV1 { identities, values });
    }
    Ok(dto::MulticonductorOperatingPointTimeSeriesV1 {
        network: Box::new(first.network().clone()),
        time_points: series.time_points().iter().map(encode_time_point).collect(),
        quantities,
    })
}

fn encode_dc_pf_instance(instance: &powerio_prob::DcPfInstance) -> dto::DcPfInstanceV1 {
    dto::DcPfInstanceV1 {
        network: Box::new(instance.network().clone()),
        approximation: dc_formula_name(instance.approximation()).to_string(),
        initial_state: instance
            .initial_state()
            .map(|point| encode_point(point, &BALANCED_STATE_QUANTITIES)),
    }
}

fn encode_ac_pf_instance(instance: &powerio_prob::AcPfInstance) -> dto::AcPfInstanceV1 {
    dto::AcPfInstanceV1 {
        network: Box::new(instance.network().clone()),
        initial_state: instance
            .initial_state()
            .map(|point| encode_point(point, &BALANCED_STATE_QUANTITIES)),
    }
}

fn encode_dc_opf_instance(instance: &powerio_prob::DcOpfInstance) -> dto::DcOpfInstanceV1 {
    dto::DcOpfInstanceV1 {
        network: Box::new(instance.network().clone()),
        approximation: dc_formula_name(instance.approximation()).to_string(),
        objective: instance.objective().clone(),
        constraints: instance.constraints().clone(),
        initial_state: instance
            .initial_state()
            .map(|point| encode_point(point, &BALANCED_STATE_QUANTITIES)),
    }
}

fn encode_ac_opf_instance(instance: &powerio_prob::AcOpfInstance) -> dto::AcOpfInstanceV1 {
    dto::AcOpfInstanceV1 {
        network: Box::new(instance.network().clone()),
        objective: instance.objective().clone(),
        constraints: instance.constraints().clone(),
        initial_state: instance
            .initial_state()
            .map(|point| encode_point(point, &BALANCED_STATE_QUANTITIES)),
    }
}

fn encode_mc_ac_pf_instance(instance: &powerio_prob::McAcPfInstance) -> dto::McAcPfInstanceV1 {
    dto::McAcPfInstanceV1 {
        network: Box::new(instance.network().clone()),
        initial_state: instance
            .initial_state()
            .map(|point| encode_point(point, &MULTICONDUCTOR_QUANTITIES)),
    }
}

fn encode_mc_ac_opf_instance(instance: &powerio_prob::McAcOpfInstance) -> dto::McAcOpfInstanceV1 {
    dto::McAcOpfInstanceV1 {
        network: Box::new(instance.network().clone()),
        objective: instance.objective().clone(),
        constraints: instance.constraints().clone(),
        initial_state: instance
            .initial_state()
            .map(|point| encode_point(point, &MULTICONDUCTOR_QUANTITIES)),
    }
}

fn encode_operating_points(
    series: &powerio_prob::BalancedOperatingPoints,
) -> Result<dto::BalancedOperatingPointTimeSeriesV1> {
    let first = series
        .values()
        .first()
        .ok_or_else(|| invalid("an operating point series needs at least one point"))?;
    let mut quantities = BTreeMap::new();
    for name in BALANCED_STATE_QUANTITIES {
        let Some(order) = first.identity_order(name) else {
            continue;
        };
        let identities: Vec<String> = order.map(str::to_string).collect();
        let mut values = Vec::with_capacity(series.len() * identities.len());
        for point in series.values() {
            let row = point
                .quantity_values(name)
                .ok_or_else(|| invalid(format!("quantity `{name}` vanished mid series")))?;
            values.extend(row.into_iter().map(StoredF64));
        }
        quantities.insert(name.to_string(), StoredQuantityV1 { identities, values });
    }
    Ok(dto::BalancedOperatingPointTimeSeriesV1 {
        network: Box::new(first.network().clone()),
        time_points: series.time_points().iter().map(encode_time_point).collect(),
        quantities,
    })
}

fn encode_time_point(point: &TimePoint) -> TimePointV1 {
    TimePointV1 {
        label: point.label().to_string(),
        duration: point.duration().map(|duration| DurationV1 {
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

fn encode_dispatch(
    dispatch: Option<&powerio_prob::GeneratorDispatch>,
) -> Option<dto::GeneratorDispatchV1> {
    dispatch.map(|dispatch| dto::GeneratorDispatchV1 {
        p_mw: stored_row(&dispatch.p_mw),
        q_mvar: stored_row(&dispatch.q_mvar),
    })
}

fn encode_dc_pf_solution(solution: &powerio_prob::DcPfSolution) -> dto::DcPfSolutionV1 {
    let network = solution.network();
    dto::DcPfSolutionV1 {
        instance: encode_dc_pf_instance(solution.instance()),
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        bus_voltage_angle: bus_column(network, |bus| solution.bus_voltage_angle(bus)),
        bus_active_injection: bus_column(network, |bus| solution.bus_active_injection(bus)),
        branch_from_active_flow: branch_column(network, |id| solution.branch_from_active_flow(id)),
        branch_to_active_flow: branch_column(network, |id| solution.branch_to_active_flow(id)),
        generator_dispatch: encode_dispatch(solution.generator_dispatch()),
    }
}

fn encode_ac_pf_solution(solution: &powerio_prob::AcPfSolution) -> dto::AcPfSolutionV1 {
    let network = solution.network();
    dto::AcPfSolutionV1 {
        instance: encode_ac_pf_instance(solution.instance()),
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
        generator_dispatch: encode_dispatch(solution.generator_dispatch()),
    }
}

fn encode_dc_opf_solution(solution: &powerio_prob::DcOpfSolution) -> dto::DcOpfSolutionV1 {
    let network = solution.network();
    dto::DcOpfSolutionV1 {
        instance: encode_dc_opf_instance(solution.instance()),
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        bus_voltage_angle: bus_column(network, |bus| solution.bus_voltage_angle(bus)),
        bus_active_injection: bus_column(network, |bus| solution.bus_active_injection(bus)),
        branch_from_active_flow: branch_column(network, |id| solution.branch_from_active_flow(id)),
        branch_to_active_flow: branch_column(network, |id| solution.branch_to_active_flow(id)),
        generator_active_power: generator_column(network, |id| solution.generator_active_power(id)),
        objective: StoredF64(solution.objective()),
    }
}

fn encode_ac_opf_solution(solution: &powerio_prob::AcOpfSolution) -> dto::AcOpfSolutionV1 {
    let network = solution.network();
    dto::AcOpfSolutionV1 {
        instance: encode_ac_opf_instance(solution.instance()),
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
        objective: StoredF64(solution.objective()),
    }
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

fn encode_mc_ac_pf_solution(solution: &powerio_prob::McAcPfSolution) -> dto::McAcPfSolutionV1 {
    let network = solution.network();
    dto::McAcPfSolutionV1 {
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

fn encode_mc_ac_opf_solution(solution: &powerio_prob::McAcOpfSolution) -> dto::McAcOpfSolutionV1 {
    let network = solution.network();
    dto::McAcOpfSolutionV1 {
        instance: encode_mc_ac_opf_instance(solution.instance()),
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
    }
}

fn encode_ac_scuc_solution(solution: &powerio_prob::AcScucSolution) -> dto::AcScucSolutionV1 {
    let network_outputs = solution.network_outputs();
    let device_outputs = solution.device_outputs();
    dto::AcScucSolutionV1 {
        instance: dto::AcScucInstanceV1 {
            network: Box::new(solution.instance().network().clone()),
            inputs: Box::new(solution.instance().inputs().clone()),
        },
        termination: solution.termination().clone(),
        residuals: *solution.residuals(),
        producer: solution.producer().map(str::to_string),
        network_outputs: dto::ScucNetworkOutputsV1 {
            bus_vm: stored_grid(&network_outputs.bus_vm),
            bus_va: stored_grid(&network_outputs.bus_va),
            shunt_step: stored_grid(&network_outputs.shunt_step),
            ac_line_on_status: stored_grid(&network_outputs.ac_line_on_status),
            transformer_tm: stored_grid(&network_outputs.transformer_tm),
            transformer_ta: stored_grid(&network_outputs.transformer_ta),
            transformer_on_status: stored_grid(&network_outputs.transformer_on_status),
            dc_line_pdc_fr: stored_grid(&network_outputs.dc_line_pdc_fr),
            dc_line_qdc_fr: stored_grid(&network_outputs.dc_line_qdc_fr),
            dc_line_qdc_to: stored_grid(&network_outputs.dc_line_qdc_to),
        },
        device_outputs: dto::ScucDeviceOutputsV1 {
            on_status: stored_grid(&device_outputs.on_status),
            p_on: stored_grid(&device_outputs.p_on),
            q: stored_grid(&device_outputs.q),
            p_reg_res_up: stored_grid(&device_outputs.p_reg_res_up),
            p_reg_res_down: stored_grid(&device_outputs.p_reg_res_down),
            p_syn_res: stored_grid(&device_outputs.p_syn_res),
            p_nsyn_res: stored_grid(&device_outputs.p_nsyn_res),
            p_ramp_res_up_online: stored_grid(&device_outputs.p_ramp_res_up_online),
            p_ramp_res_down_online: stored_grid(&device_outputs.p_ramp_res_down_online),
            q_res_up: stored_grid(&device_outputs.q_res_up),
            q_res_down: stored_grid(&device_outputs.q_res_down),
        },
        objective: solution.objective().map(StoredF64),
    }
}

// ---- value decoding ---------------------------------------------------------

fn decode_stored(stored: StoredModuleV1) -> Result<PioModule<PioValue>> {
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

// One arm per stored kind: the length is the enum's, and splitting the arms
// into twenty single-use functions would hide the exhaustiveness this match
// enforces.
#[allow(clippy::too_many_lines)]
/// Every network a decoded value embeds is held to its own model's rule: a
/// balanced network passes the structural validation its readers enforce,
/// so a stored document cannot smuggle a network they would refuse. The
/// multiconductor unresolved reference walk is warning level in its reader
/// (a refused include legitimately leaves dangling references beside the
/// recorded findings), so it stays out of the decode gate.
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
        PioValue::BalancedNetworkTimeSeries(series) => {
            series.values().iter().try_for_each(balanced)
        }
        PioValue::BalancedOperatingPointTimeSeries(series) => series
            .values()
            .first()
            .map_or(Ok(()), |point| balanced(point.network())),
        PioValue::MulticonductorOperatingPointTimeSeries(series) => series
            .values()
            .first()
            .map_or(Ok(()), |point| multiconductor(point.network())),
        PioValue::BalancedNetworkScenarioSet(set) => set
            .iter()
            .try_for_each(|scenario| balanced(scenario.value())),
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
        PioValue::McAcPfSolution(solution) => multiconductor(solution.network()),
        PioValue::McAcOpfSolution(solution) => multiconductor(solution.network()),
        _ => Ok(()),
    }
}

// One arm per stored value kind; splitting the match would scatter the
// kind-to-decoder table this function is.
#[allow(clippy::too_many_lines)]
fn decode_value(value: StoredValueV1) -> Result<PioValue> {
    Ok(match value {
        StoredValueV1::BalancedNetwork(network) => PioValue::BalancedNetwork(*network),
        StoredValueV1::MulticonductorNetwork(network) => PioValue::MulticonductorNetwork(*network),
        StoredValueV1::BalancedNetworkTimeSeries(series) => {
            let time_points = decode_time_points(&series.time_points)?;
            PioValue::BalancedNetworkTimeSeries(
                TimeSeries::new(time_points, series.values)
                    .map_err(|error| invalid(error.to_string()))?,
            )
        }
        StoredValueV1::BalancedOperatingPointTimeSeries(series) => {
            let time_points = decode_time_points(&series.time_points)?;
            let mut builder = powerio_prob::BalancedStateBuilder::new(*series.network, time_points);
            for (name, quantity) in series.quantities {
                let resolved = builder
                    .identity_order(&name)
                    .map_err(|error| invalid(error.to_string()))?;
                check_identity_order(&name, &quantity.identities, &resolved)?;
                let values: Vec<f64> = quantity.values.iter().map(|value| value.0).collect();
                builder = builder
                    .dense_by_name(&name, values)
                    .map_err(|error| invalid(error.to_string()))?;
            }
            PioValue::BalancedOperatingPointTimeSeries(
                builder
                    .build()
                    .map_err(|error| invalid(error.to_string()))?,
            )
        }
        StoredValueV1::BalancedNetworkScenarioSet(set) => {
            let scenarios = set
                .scenarios
                .into_iter()
                .map(|scenario| {
                    powerio_core::ScenarioId::new(scenario.id)
                        .map(|id| {
                            powerio_core::Scenario::new(
                                id,
                                scenario.probability.map(|p| p.0),
                                scenario.value,
                            )
                        })
                        .map_err(|error| invalid(error.to_string()))
                })
                .collect::<Result<Vec<_>>>()?;
            PioValue::BalancedNetworkScenarioSet(
                powerio_core::ScenarioSet::new(scenarios)
                    .map_err(|error| invalid(error.to_string()))?,
            )
        }
        StoredValueV1::MulticonductorOperatingPointTimeSeries(series) => {
            let time_points = decode_time_points(&series.time_points)?;
            let mut builder =
                powerio_prob::MulticonductorStateBuilder::new(*series.network, time_points);
            for (name, quantity) in series.quantities {
                let resolved = builder
                    .identity_order(&name)
                    .map_err(|error| invalid(error.to_string()))?;
                check_identity_order(&name, &quantity.identities, &resolved)?;
                let values: Vec<f64> = quantity.values.iter().map(|value| value.0).collect();
                builder = mc_dense_by_name(builder, &name, values)?;
            }
            PioValue::MulticonductorOperatingPointTimeSeries(
                builder
                    .build()
                    .map_err(|error| invalid(error.to_string()))?,
            )
        }
        StoredValueV1::DcPfInstance(instance) => {
            PioValue::DcPfInstance(decode_dc_pf_instance(instance)?)
        }
        StoredValueV1::AcPfInstance(instance) => {
            PioValue::AcPfInstance(decode_ac_pf_instance(instance)?)
        }
        StoredValueV1::DcOpfInstance(instance) => {
            PioValue::DcOpfInstance(decode_dc_opf_instance(instance)?)
        }
        StoredValueV1::AcOpfInstance(instance) => {
            PioValue::AcOpfInstance(decode_ac_opf_instance(instance)?)
        }
        StoredValueV1::McAcPfInstance(instance) => {
            PioValue::McAcPfInstance(decode_mc_ac_pf_instance(instance)?)
        }
        StoredValueV1::McAcOpfInstance(instance) => {
            PioValue::McAcOpfInstance(decode_mc_ac_opf_instance(instance)?)
        }
        StoredValueV1::AcScucInstance(instance) => PioValue::AcScucInstance(
            powerio_prob::AcScucInstance::new(*instance.network, *instance.inputs)
                .map_err(|error| invalid(error.to_string()))?,
        ),
        StoredValueV1::DcPfSolution(solution) => {
            let instance = std::sync::Arc::new(decode_dc_pf_instance(solution.instance.clone())?);
            PioValue::DcPfSolution(with_solution_records(
                powerio_prob::DcPfSolution::new(
                    instance,
                    solution.termination.clone(),
                    plain_row(&solution.bus_voltage_angle),
                    plain_row(&solution.bus_active_injection),
                    plain_row(&solution.branch_from_active_flow),
                    plain_row(&solution.branch_to_active_flow),
                )
                .map_err(|error| invalid(error.to_string()))?,
                solution.residuals,
                solution.producer.clone(),
                powerio_prob::DcPfSolution::with_residuals,
                powerio_prob::DcPfSolution::with_producer,
                decode_dispatch(solution.generator_dispatch.as_ref()),
                |value, dispatch| {
                    value
                        .with_generator_dispatch(dispatch)
                        .map_err(|error| invalid(error.to_string()))
                },
            )?)
        }
        StoredValueV1::AcPfSolution(solution) => {
            let instance = std::sync::Arc::new(decode_ac_pf_instance(solution.instance.clone())?);
            PioValue::AcPfSolution(with_solution_records(
                powerio_prob::AcPfSolution::new(
                    instance,
                    solution.termination.clone(),
                    plain_row(&solution.bus_voltage_magnitude),
                    plain_row(&solution.bus_voltage_angle),
                    plain_row(&solution.bus_active_injection),
                    plain_row(&solution.bus_reactive_injection),
                    plain_row(&solution.branch_from_active_flow),
                    plain_row(&solution.branch_from_reactive_flow),
                    plain_row(&solution.branch_to_active_flow),
                    plain_row(&solution.branch_to_reactive_flow),
                )
                .map_err(|error| invalid(error.to_string()))?,
                solution.residuals,
                solution.producer.clone(),
                powerio_prob::AcPfSolution::with_residuals,
                powerio_prob::AcPfSolution::with_producer,
                decode_dispatch(solution.generator_dispatch.as_ref()),
                |value, dispatch| {
                    value
                        .with_generator_dispatch(dispatch)
                        .map_err(|error| invalid(error.to_string()))
                },
            )?)
        }
        StoredValueV1::DcOpfSolution(solution) => {
            let instance = std::sync::Arc::new(decode_dc_opf_instance(solution.instance.clone())?);
            let mut value = powerio_prob::DcOpfSolution::new(
                instance,
                solution.termination.clone(),
                plain_row(&solution.bus_voltage_angle),
                plain_row(&solution.bus_active_injection),
                plain_row(&solution.branch_from_active_flow),
                plain_row(&solution.branch_to_active_flow),
                plain_row(&solution.generator_active_power),
                solution.objective.0,
            )
            .map_err(|error| invalid(error.to_string()))?;
            value = value.with_residuals(solution.residuals);
            if let Some(producer) = solution.producer.clone() {
                value = value.with_producer(producer);
            }
            PioValue::DcOpfSolution(value)
        }
        StoredValueV1::AcOpfSolution(solution) => {
            let instance = std::sync::Arc::new(decode_ac_opf_instance(solution.instance.clone())?);
            let mut value = powerio_prob::AcOpfSolution::new(
                instance,
                solution.termination.clone(),
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
            )
            .map_err(|error| invalid(error.to_string()))?;
            value = value.with_residuals(solution.residuals);
            if let Some(producer) = solution.producer.clone() {
                value = value.with_producer(producer);
            }
            PioValue::AcOpfSolution(value)
        }
        StoredValueV1::McAcPfSolution(solution) => {
            let instance =
                std::sync::Arc::new(decode_mc_ac_pf_instance(solution.instance.clone())?);
            let mut value = powerio_prob::McAcPfSolution::new(
                instance,
                solution.termination.clone(),
                plain_row(&solution.terminal_voltage_magnitude),
                plain_row(&solution.terminal_voltage_angle),
                plain_row(&solution.source_active_injection),
            )
            .map_err(|error| invalid(error.to_string()))?;
            if let Some(currents) = &solution.terminal_current_magnitude {
                value = value
                    .with_terminal_currents(plain_row(currents))
                    .map_err(|error| invalid(error.to_string()))?;
            }
            if let Some(powers) = &solution.terminal_active_power {
                value = value
                    .with_terminal_powers(plain_row(powers))
                    .map_err(|error| invalid(error.to_string()))?;
            }
            value = value.with_residuals(solution.residuals);
            if let Some(producer) = solution.producer.clone() {
                value = value.with_producer(producer);
            }
            PioValue::McAcPfSolution(value)
        }
        StoredValueV1::McAcOpfSolution(solution) => {
            let instance =
                std::sync::Arc::new(decode_mc_ac_opf_instance(solution.instance.clone())?);
            let mut value = powerio_prob::McAcOpfSolution::new(
                instance,
                solution.termination.clone(),
                plain_row(&solution.terminal_voltage_magnitude),
                plain_row(&solution.terminal_voltage_angle),
                plain_row(&solution.source_active_injection),
                plain_row(&solution.generator_active_power),
                solution.objective.0,
            )
            .map_err(|error| invalid(error.to_string()))?;
            if let Some(currents) = &solution.terminal_current_magnitude {
                value = value
                    .with_terminal_currents(plain_row(currents))
                    .map_err(|error| invalid(error.to_string()))?;
            }
            if let Some(powers) = &solution.terminal_active_power {
                value = value
                    .with_terminal_powers(plain_row(powers))
                    .map_err(|error| invalid(error.to_string()))?;
            }
            value = value.with_residuals(solution.residuals);
            if let Some(producer) = solution.producer.clone() {
                value = value.with_producer(producer);
            }
            PioValue::McAcOpfSolution(value)
        }
        StoredValueV1::AcScucSolution(solution) => {
            let instance = std::sync::Arc::new(
                powerio_prob::AcScucInstance::new(
                    *solution.instance.network.clone(),
                    (*solution.instance.inputs).clone(),
                )
                .map_err(|error| invalid(error.to_string()))?,
            );
            let mut network_outputs = powerio_prob::ScucNetworkOutputs::default();
            network_outputs.bus_vm = plain_grid(&solution.network_outputs.bus_vm);
            network_outputs.bus_va = plain_grid(&solution.network_outputs.bus_va);
            network_outputs.shunt_step = plain_grid(&solution.network_outputs.shunt_step);
            network_outputs.ac_line_on_status =
                plain_grid(&solution.network_outputs.ac_line_on_status);
            network_outputs.transformer_tm = plain_grid(&solution.network_outputs.transformer_tm);
            network_outputs.transformer_ta = plain_grid(&solution.network_outputs.transformer_ta);
            network_outputs.transformer_on_status =
                plain_grid(&solution.network_outputs.transformer_on_status);
            network_outputs.dc_line_pdc_fr = plain_grid(&solution.network_outputs.dc_line_pdc_fr);
            network_outputs.dc_line_qdc_fr = plain_grid(&solution.network_outputs.dc_line_qdc_fr);
            network_outputs.dc_line_qdc_to = plain_grid(&solution.network_outputs.dc_line_qdc_to);
            let mut device_outputs = powerio_prob::ScucDeviceOutputs::default();
            device_outputs.on_status = plain_grid(&solution.device_outputs.on_status);
            device_outputs.p_on = plain_grid(&solution.device_outputs.p_on);
            device_outputs.q = plain_grid(&solution.device_outputs.q);
            device_outputs.p_reg_res_up = plain_grid(&solution.device_outputs.p_reg_res_up);
            device_outputs.p_reg_res_down = plain_grid(&solution.device_outputs.p_reg_res_down);
            device_outputs.p_syn_res = plain_grid(&solution.device_outputs.p_syn_res);
            device_outputs.p_nsyn_res = plain_grid(&solution.device_outputs.p_nsyn_res);
            device_outputs.p_ramp_res_up_online =
                plain_grid(&solution.device_outputs.p_ramp_res_up_online);
            device_outputs.p_ramp_res_down_online =
                plain_grid(&solution.device_outputs.p_ramp_res_down_online);
            device_outputs.q_res_up = plain_grid(&solution.device_outputs.q_res_up);
            device_outputs.q_res_down = plain_grid(&solution.device_outputs.q_res_down);
            let mut value = powerio_prob::AcScucSolution::new(
                instance,
                solution.termination.clone(),
                network_outputs,
                device_outputs,
                solution.objective.map(|value| value.0),
            )
            .map_err(|error| invalid(error.to_string()))?;
            value = value.with_residuals(solution.residuals);
            if let Some(producer) = solution.producer.clone() {
                value = value.with_producer(producer);
            }
            PioValue::AcScucSolution(value)
        }
    })
}

fn decode_dispatch(
    dispatch: Option<&dto::GeneratorDispatchV1>,
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

fn decode_dc_pf_instance(instance: dto::DcPfInstanceV1) -> Result<powerio_prob::DcPfInstance> {
    let network = *instance.network;
    let mut decoded = powerio_prob::DcPfInstance::from_network(network)
        .map_err(|error| invalid(error.to_string()))?
        .with_approximation(dc_formula_from_name(&instance.approximation)?);
    if let Some(stored) = instance.initial_state {
        let point = decode_balanced_point(decoded.network(), stored)?;
        decoded = decoded.with_initial_state(point);
    }
    Ok(decoded)
}

fn decode_ac_pf_instance(instance: dto::AcPfInstanceV1) -> Result<powerio_prob::AcPfInstance> {
    let mut decoded = powerio_prob::AcPfInstance::from_network(*instance.network)
        .map_err(|error| invalid(error.to_string()))?;
    if let Some(stored) = instance.initial_state {
        let point = decode_balanced_point(decoded.network(), stored)?;
        decoded = decoded.with_initial_state(point);
    }
    Ok(decoded)
}

fn decode_dc_opf_instance(instance: dto::DcOpfInstanceV1) -> Result<powerio_prob::DcOpfInstance> {
    let mut decoded = powerio_prob::DcOpfInstance::from_network(*instance.network)
        .map_err(|error| invalid(error.to_string()))?
        .with_approximation(dc_formula_from_name(&instance.approximation)?)
        .with_objective(instance.objective)
        .with_constraints(instance.constraints);
    if let Some(stored) = instance.initial_state {
        let point = decode_balanced_point(decoded.network(), stored)?;
        decoded = decoded.with_initial_state(point);
    }
    Ok(decoded)
}

fn decode_ac_opf_instance(instance: dto::AcOpfInstanceV1) -> Result<powerio_prob::AcOpfInstance> {
    let mut decoded = powerio_prob::AcOpfInstance::from_network(*instance.network)
        .map_err(|error| invalid(error.to_string()))?
        .with_objective(instance.objective)
        .with_constraints(instance.constraints);
    if let Some(stored) = instance.initial_state {
        let point = decode_balanced_point(decoded.network(), stored)?;
        decoded = decoded.with_initial_state(point);
    }
    Ok(decoded)
}

fn decode_mc_ac_pf_instance(
    instance: dto::McAcPfInstanceV1,
) -> Result<powerio_prob::McAcPfInstance> {
    let mut decoded = powerio_prob::McAcPfInstance::from_network(*instance.network)
        .map_err(|error| invalid(error.to_string()))?;
    if let Some(stored) = instance.initial_state {
        let point = decode_mc_point(decoded.network(), stored)?;
        decoded = decoded.with_initial_state(point);
    }
    Ok(decoded)
}

fn decode_mc_ac_opf_instance(
    instance: dto::McAcOpfInstanceV1,
) -> Result<powerio_prob::McAcOpfInstance> {
    let mut decoded = powerio_prob::McAcOpfInstance::from_network(*instance.network)
        .map_err(|error| invalid(error.to_string()))?
        .with_objective(instance.objective)
        .with_constraints(instance.constraints);
    if let Some(stored) = instance.initial_state {
        let point = decode_mc_point(decoded.network(), stored)?;
        decoded = decoded.with_initial_state(point);
    }
    Ok(decoded)
}

fn decode_time_points(points: &[TimePointV1]) -> Result<Vec<TimePoint>> {
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

// ---- record encoding / decoding --------------------------------------------

fn encode_source(source: &SourceDescriptor) -> SourceDescriptorV1 {
    SourceDescriptorV1 {
        id: SourceIdV1(source.id().as_str().to_string()),
        name: source.name().to_string(),
        byte_length: source.byte_length(),
        format: source.format().map(|format| format.as_str().to_string()),
        digest: source.digest().map(|digest| DigestV1 {
            algorithm: DigestAlgorithmV1::Sha256,
            value: digest.value().to_string(),
        }),
    }
}

fn decode_source(source: SourceDescriptorV1) -> Result<SourceDescriptor> {
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

fn encode_span(span: &SourceSpan) -> SourceSpanV1 {
    SourceSpanV1 {
        source: SourceIdV1(span.source().as_str().to_string()),
        byte_start: span.byte_start(),
        byte_end: span.byte_end(),
    }
}

fn decode_span(span: SourceSpanV1) -> Result<SourceSpan> {
    let source = SourceId::new(span.source.0).map_err(|error| invalid(error.to_string()))?;
    SourceSpan::new(source, span.byte_start, span.byte_end)
        .map_err(|error| invalid(error.to_string()))
}

fn encode_relation(relation: SourceRelation) -> SourceRelationV1 {
    match relation {
        SourceRelation::Exact => SourceRelationV1::Exact,
        SourceRelation::Defaulted => SourceRelationV1::Defaulted,
        SourceRelation::Inferred => SourceRelationV1::Inferred,
        SourceRelation::ConvertedUnits => SourceRelationV1::ConvertedUnits,
        SourceRelation::Aggregated => SourceRelationV1::Aggregated,
        SourceRelation::Split => SourceRelationV1::Split,
        SourceRelation::Synthetic => SourceRelationV1::Synthetic,
        SourceRelation::Transformed => SourceRelationV1::Transformed,
        SourceRelation::RetainedExtra => SourceRelationV1::RetainedExtra,
        // The runtime enum is non_exhaustive for additive growth; a new
        // relation must gain a stored spelling before it can be written.
        _ => unreachable!("unmapped source relation"),
    }
}

fn decode_relation(relation: SourceRelationV1) -> SourceRelation {
    match relation {
        SourceRelationV1::Exact => SourceRelation::Exact,
        SourceRelationV1::Defaulted => SourceRelation::Defaulted,
        SourceRelationV1::Inferred => SourceRelation::Inferred,
        SourceRelationV1::ConvertedUnits => SourceRelation::ConvertedUnits,
        SourceRelationV1::Aggregated => SourceRelation::Aggregated,
        SourceRelationV1::Split => SourceRelation::Split,
        SourceRelationV1::Synthetic => SourceRelation::Synthetic,
        SourceRelationV1::Transformed => SourceRelation::Transformed,
        SourceRelationV1::RetainedExtra => SourceRelation::RetainedExtra,
    }
}

fn encode_map_entry(entry: &SourceMapEntry) -> SourceMapEntryV1 {
    SourceMapEntryV1 {
        target: entry.target().to_string(),
        relation: encode_relation(entry.relation()),
        spans: entry.spans().iter().map(encode_span).collect(),
    }
}

fn decode_map_entry(entry: SourceMapEntryV1) -> Result<SourceMapEntry> {
    let spans = entry
        .spans
        .into_iter()
        .map(decode_span)
        .collect::<Result<Vec<_>>>()?;
    SourceMapEntry::new(entry.target, decode_relation(entry.relation), spans)
        .map_err(|error| invalid(error.to_string()))
}

fn encode_severity(severity: DiagnosticSeverity) -> SeverityV1 {
    match severity {
        DiagnosticSeverity::Error => SeverityV1::Error,
        DiagnosticSeverity::Warning => SeverityV1::Warning,
        DiagnosticSeverity::Remark => SeverityV1::Remark,
        DiagnosticSeverity::Note => SeverityV1::Note,
    }
}

fn decode_severity(severity: SeverityV1) -> DiagnosticSeverity {
    match severity {
        SeverityV1::Error => DiagnosticSeverity::Error,
        SeverityV1::Warning => DiagnosticSeverity::Warning,
        SeverityV1::Remark => DiagnosticSeverity::Remark,
        SeverityV1::Note => DiagnosticSeverity::Note,
    }
}

fn encode_diagnostic(index: usize, diagnostic: &Diagnostic) -> DiagnosticV1 {
    DiagnosticV1 {
        id: DiagnosticIdV1(
            diagnostic
                .id()
                .map_or_else(|| format!("d{index}"), |id| id.as_str().to_string()),
        ),
        severity: encode_severity(diagnostic.severity()),
        code: diagnostic.code().to_string(),
        message: diagnostic.message().to_string(),
        target: diagnostic.target().map(str::to_string),
        spans: diagnostic.spans().iter().map(encode_span).collect(),
        related: diagnostic
            .related()
            .iter()
            .map(|id| DiagnosticIdV1(id.as_str().to_string()))
            .collect(),
        details: diagnostic.details().clone().into_iter().collect(),
    }
}

fn decode_diagnostic(diagnostic: DiagnosticV1) -> Result<Diagnostic> {
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
    Ok(decoded)
}

fn encode_history_kind(kind: HistoryKind) -> HistoryKindV1 {
    match kind {
        HistoryKind::Parse => HistoryKindV1::Parse,
        HistoryKind::Upgrade => HistoryKindV1::Upgrade,
        HistoryKind::Transform => HistoryKindV1::Transform,
        HistoryKind::Edit => HistoryKindV1::Edit,
        HistoryKind::Repair => HistoryKindV1::Repair,
        // As with relations: a new history kind gains a stored spelling first.
        _ => unreachable!("unmapped history kind"),
    }
}

fn decode_history_kind(kind: HistoryKindV1) -> HistoryKind {
    match kind {
        HistoryKindV1::Parse => HistoryKind::Parse,
        HistoryKindV1::Upgrade => HistoryKind::Upgrade,
        HistoryKindV1::Transform => HistoryKind::Transform,
        HistoryKindV1::Edit => HistoryKind::Edit,
        HistoryKindV1::Repair => HistoryKind::Repair,
    }
}

fn encode_history(entry: &HistoryEntry) -> HistoryEntryV1 {
    HistoryEntryV1 {
        id: HistoryIdV1(entry.id().as_str().to_string()),
        kind: encode_history_kind(entry.kind()),
        name: entry.name().to_string(),
        input_kind: entry.input_kind().map(str::to_string),
        output_kind: entry.output_kind().map(str::to_string),
        parameters: entry.parameters().clone(),
        assumptions: entry.assumptions().to_vec(),
        losses: entry.losses().to_vec(),
    }
}

fn decode_history(entry: HistoryEntryV1) -> Result<HistoryEntry> {
    let id = HistoryId::new(entry.id.0).map_err(|error| invalid(error.to_string()))?;
    let mut decoded = HistoryEntry::new(id, decode_history_kind(entry.kind), entry.name)
        .map_err(|error| invalid(error.to_string()))?;
    if let Some(input) = entry.input_kind {
        decoded = decoded
            .with_input_kind(input)
            .map_err(|error| invalid(error.to_string()))?;
    }
    if let Some(output) = entry.output_kind {
        decoded = decoded
            .with_output_kind(output)
            .map_err(|error| invalid(error.to_string()))?;
    }
    if !entry.parameters.is_empty() {
        decoded = decoded
            .with_parameters(entry.parameters)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for assumption in entry.assumptions {
        decoded = decoded
            .with_assumption(assumption)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for loss in entry.losses {
        decoded = decoded
            .with_loss(loss)
            .map_err(|error| invalid(error.to_string()))?;
    }
    Ok(decoded)
}

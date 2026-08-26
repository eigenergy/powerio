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
use crate::package::diagnostics::codes;
use crate::value::PioValue;

type Result<T> = std::result::Result<T, powerio_core::Error>;

fn invalid(message: impl Into<String>) -> powerio_core::Error {
    powerio_core::Error::new(&codes::READ_MODULE_INVALID, message)
}

/// The balanced instantaneous quantity vocabulary, the accessor spellings the
/// state module resolves. Stored quantities outside this list are refused.
const BALANCED_QUANTITIES: [&str; 13] = [
    "bus_voltage_magnitude",
    "bus_voltage_angle",
    "bus_active_injection",
    "bus_reactive_injection",
    "generator_active_power",
    "generator_reactive_power",
    "generator_voltage_setpoint",
    "generator_in_service",
    "load_active_power",
    "load_reactive_power",
    "branch_in_service",
    "branch_tap_ratio",
    "branch_phase_shift",
];

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
    })
}

fn encode_operating_points(
    series: &powerio_prob::BalancedOperatingPoints,
) -> Result<dto::BalancedOperatingPointTimeSeriesV1> {
    let first = series
        .values()
        .first()
        .ok_or_else(|| invalid("an operating point series needs at least one point"))?;
    let mut quantities = BTreeMap::new();
    for name in BALANCED_QUANTITIES {
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

// ---- value decoding ---------------------------------------------------------

fn decode_stored(stored: StoredModuleV1) -> Result<PioModule<PioValue>> {
    let value = decode_value(stored.value)?;
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
    })
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

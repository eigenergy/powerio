//! [`BalancedNetwork`] to a CGMES instance file set: EQ, TP, SSH, and SV
//! documents tied together by `md:FullModel` dependency headers.
//!
//! The writer is the reader's inverse. Bus-branch synthesis: one
//! `TopologicalNode` per bus (in TP, contained by a per-bus `VoltageLevel`
//! inside a minimal region/substation hierarchy in EQ), terminals linked to
//! nodes in TP, the operating point in SSH, and solution quantities (plus the
//! island with its angle reference) in SV. Per-unit values leave through the
//! bus voltage bases onto ohms/siemens; CGMES carries no system MVA base, so
//! a write from a base other than 100 MVA warns that a reparse re-bases.
//!
//! mRIDs are deterministic. A valid imported UUID passes through; other
//! identifiers derive a UUIDv5 value from the component kind and stable
//! identity.
//! Header timestamps are a fixed sentinel for the same reason.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use powerio_core::ComponentId;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

use super::{CGMES_CLASS_PROPERTY, CgmesDiagnostics, CgmesVersion};
use crate::diagnostics::codes;
use crate::network::{
    AcDcConverterControlMode, ActivePowerControl, BalancedNetwork, BusId, BusType, CaseMetadata,
    ComponentMetadata, CurveStyle, DcConverterOperatingMode, DcPolarity, DcSwitchKind, DcTerminal,
    DetailedConnectivity, GeneratorEnergySource, LineCommutatedConverter,
    LineCommutatedConverterOperatingMode, LoadVoltageModel, LoadingLimits, OmittedFieldName,
    ReactiveCapabilityCurve, ReactiveLimits, Shunt, StaticVarCompensatorRegulationMode, SwitchKind,
    SwitchedShuntMode, TapChanger, TapChangerKind, TapChangerRegulationMode, Terminal,
    TerminalReference, TopologyEndpoint, TopologyKind, VoltageSourceConverter,
    calc_reactive_limits_at_active_power,
};
use crate::{Error, Result};

/// The emitted profile documents, `(file_name, xml)` in EQ/TP/SSH/SV order,
/// plus every fidelity loss the writer took.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CgmesFiles {
    pub files: Vec<(String, String)>,
    pub warnings: CgmesDiagnostics,
}

/// Deterministic output needs a fixed header timestamp; consumers read it as
/// provenance metadata, not data.
const STAMP: &str = "2000-01-01T00:00:00Z";

/// A UUID-shaped identifier derived from an object's role and name, so writes
/// are reproducible; imported mRIDs (element `uid`s) take precedence at the
/// call sites.
fn det_mrid(kind: &str, name: &str) -> String {
    let namespace = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"https://powerio.dev/cgmes");
    uuid::Uuid::new_v5(&namespace, format!("{kind}:{name}").as_bytes()).to_string()
}

/// The imported mRID when the element carries one, else deterministic.
fn mrid_or(kind: &str, name: &str, uid: Option<&str>) -> String {
    uid.filter(|value| uuid::Uuid::parse_str(value).is_ok())
        .map_or_else(|| det_mrid(kind, uid.unwrap_or(name)), str::to_owned)
}

fn metadata<'a>(
    detailed: &'a DetailedConnectivity,
    component: &ComponentId,
) -> Option<&'a ComponentMetadata> {
    detailed
        .component_metadata
        .iter()
        .find(|value| value.component == *component)
}

fn component_mrid(detailed: &DetailedConnectivity, component: &ComponentId) -> String {
    let imported = metadata(detailed, component).and_then(|value| {
        value
            .external_identifiers
            .iter()
            .find(|identifier| {
                identifier
                    .authority
                    .as_deref()
                    .is_some_and(|authority| authority.eq_ignore_ascii_case("CGMES"))
                    && uuid::Uuid::parse_str(&identifier.value).is_ok()
            })
            .map(|identifier| identifier.value.as_str())
    });
    imported.map_or_else(
        || det_mrid(component.component_type(), &component.to_string()),
        str::to_owned,
    )
}

fn transformer_end_mrid(
    detailed: Option<&DetailedConnectivity>,
    transformer_type: &str,
    transformer_local_id: &str,
    emitted_transformer_mrid: &str,
    winding: usize,
) -> Result<String> {
    let fallback = || det_mrid("xfend", &format!("{emitted_transformer_mrid}:{winding}"));
    let Some(detailed) = detailed else {
        return Ok(fallback());
    };
    let mut source_transformer_ids = HashSet::from([transformer_local_id.to_owned()]);
    if let Some(metadata) =
        mapped_component_metadata(Some(detailed), transformer_type, transformer_local_id)
    {
        source_transformer_ids.extend(
            metadata
                .external_identifiers
                .iter()
                .filter(|identifier| {
                    identifier
                        .authority
                        .as_deref()
                        .is_some_and(|authority| authority.eq_ignore_ascii_case("CGMES"))
                })
                .map(|identifier| identifier.value.clone()),
        );
    }
    source_transformer_ids.insert(emitted_transformer_mrid.to_owned());

    let mut matches = detailed.component_metadata.iter().filter(|metadata| {
        metadata
            .properties
            .get(CGMES_CLASS_PROPERTY)
            .is_some_and(|class| class == "PowerTransformerEnd")
            && metadata
                .properties
                .get("PowerTransformerEnd.PowerTransformer")
                .is_some_and(|transformer| source_transformer_ids.contains(transformer))
            && metadata
                .properties
                .get("TransformerEnd.endNumber")
                .and_then(|value| value.parse::<f64>().ok())
                .is_some_and(|value| {
                    value.is_finite() && value.fract().eq(&0.0) && value.eq(&(winding as f64))
                })
    });
    let Some(retained) = matches.next() else {
        return Ok(fallback());
    };
    if matches.next().is_some() {
        return Err(emission_error(format!(
            "PowerTransformer `{transformer_local_id}` has more than one retained PowerTransformerEnd identity for winding {winding}"
        )));
    }
    Ok(component_mrid(detailed, &retained.component))
}

#[derive(Debug, Clone)]
struct RetainedIdentifiedMetadata {
    name: Option<String>,
    short_name: Option<String>,
    fictitious: bool,
}

fn retained_identified_metadata(
    detailed: Option<&DetailedConnectivity>,
    warnings: &mut CgmesDiagnostics,
) -> Arc<HashMap<String, RetainedIdentifiedMetadata>> {
    let Some(detailed) = detailed else {
        return Arc::new(HashMap::new());
    };
    let mut retained = HashMap::new();
    for metadata in &detailed.component_metadata {
        let mut short_names = metadata
            .aliases
            .iter()
            .filter(|alias| alias.alias_type.as_deref() == Some("short_name"));
        let short_name = short_names.next().map(|alias| alias.value.clone());
        for alias in short_names {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "component `{}` has more than one CGMES short name; emitted `{}` and omitted `{}`",
                metadata.component,
                short_name.as_deref().unwrap_or_default(),
                alias.value
            ));
        }
        for alias in metadata
            .aliases
            .iter()
            .filter(|alias| alias.alias_type.as_deref() != Some("short_name"))
        {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "component `{}` alias `{}` of type `{}` has no CGMES IdentifiedObject.shortName mapping",
                metadata.component,
                alias.value,
                alias.alias_type.as_deref().unwrap_or("unspecified")
            ));
        }
        for identifier in &metadata.external_identifiers {
            if identifier
                .authority
                .as_deref()
                .is_some_and(|authority| authority.eq_ignore_ascii_case("CGMES"))
            {
                if uuid::Uuid::parse_str(&identifier.value).is_err() {
                    warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                        "component `{}` has non-UUID CGMES identifier `{}`; fresh CGMES uses a deterministic UUID",
                        metadata.component, identifier.value
                    ));
                }
            } else {
                warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                    "component `{}` external identifier `{}` from authority `{}` has no CGMES IdentifiedObject field",
                    metadata.component,
                    identifier.value,
                    identifier.authority.as_deref().unwrap_or("unspecified")
                ));
            }
        }
        retained.insert(
            component_mrid(detailed, &metadata.component),
            RetainedIdentifiedMetadata {
                name: metadata.name.clone(),
                short_name,
                fictitious: metadata.fictitious,
            },
        );
    }
    Arc::new(retained)
}

fn component_name<'a>(
    detailed: &'a DetailedConnectivity,
    component: &ComponentId,
    fallback: &'a str,
) -> &'a str {
    metadata(detailed, component)
        .and_then(|value| value.name.as_deref())
        .unwrap_or(fallback)
}

fn mapped_component_name<'a>(
    detailed: Option<&'a DetailedConnectivity>,
    component_type: &str,
    local_id: &str,
    fallback: &'a str,
) -> &'a str {
    detailed
        .and_then(|detailed| {
            detailed.component_metadata.iter().find(|metadata| {
                metadata.component.component_type() == component_type
                    && metadata.component.local_id() == local_id
            })
        })
        .and_then(|metadata| metadata.name.as_deref())
        .unwrap_or(fallback)
}

fn mapped_component_metadata<'a>(
    detailed: Option<&'a DetailedConnectivity>,
    component_type: &str,
    local_id: &str,
) -> Option<&'a ComponentMetadata> {
    detailed?.component_metadata.iter().find(|metadata| {
        metadata.component.component_type() == component_type
            && metadata.component.local_id() == local_id
    })
}

fn cgmes_metadata_by_external_id<'a>(
    detailed: &'a DetailedConnectivity,
    external_id: &str,
) -> Option<&'a ComponentMetadata> {
    detailed.component_metadata.iter().find(|metadata| {
        metadata.external_identifiers.iter().any(|identifier| {
            identifier.value == external_id
                && identifier
                    .authority
                    .as_deref()
                    .is_some_and(|authority| authority.eq_ignore_ascii_case("CGMES"))
        })
    })
}

fn has_cgmes_external_identifier(metadata: &ComponentMetadata) -> bool {
    metadata.external_identifiers.iter().any(|identifier| {
        identifier
            .authority
            .as_deref()
            .is_some_and(|authority| authority.eq_ignore_ascii_case("CGMES"))
    })
}

fn mapped_generating_unit_metadata<'a>(
    detailed: Option<&'a DetailedConnectivity>,
    generator: &str,
) -> Option<&'a ComponentMetadata> {
    let detailed = detailed?;
    let unit = mapped_component_metadata(Some(detailed), "generator", generator)?
        .properties
        .get(super::CGMES_GENERATING_UNIT_PROPERTY)?;
    cgmes_metadata_by_external_id(detailed, unit)
}

const fn generating_unit_class(source: GeneratorEnergySource) -> &'static str {
    match source {
        GeneratorEnergySource::Hydro => "HydroGeneratingUnit",
        GeneratorEnergySource::Nuclear => "NuclearGeneratingUnit",
        GeneratorEnergySource::Wind => "WindGeneratingUnit",
        GeneratorEnergySource::Thermal => "ThermalGeneratingUnit",
        GeneratorEnergySource::Solar => "SolarGeneratingUnit",
        GeneratorEnergySource::Other => "GeneratingUnit",
    }
}

fn mapped_regulating_control_metadata<'a>(
    detailed: Option<&'a DetailedConnectivity>,
    equipment_type: &str,
    equipment: &str,
) -> Option<&'a ComponentMetadata> {
    let detailed = detailed?;
    let control = mapped_component_metadata(Some(detailed), equipment_type, equipment)?
        .properties
        .get(super::CGMES_REGULATING_CONTROL_PROPERTY)?;
    cgmes_metadata_by_external_id(detailed, control)
}

fn retained_bool(metadata: Option<&ComponentMetadata>, property: &str) -> Option<bool> {
    metadata?.properties.get(property)?.parse().ok()
}

fn unit_multiplier_scale_to_kv(multiplier: &str) -> Option<f64> {
    match multiplier
        .strip_prefix("UnitMultiplier.")
        .unwrap_or(multiplier)
    {
        "none" => Some(1e-3),
        "m" => Some(1e-6),
        "k" => Some(1.0),
        "M" => Some(1e3),
        "G" => Some(1e6),
        _ => None,
    }
}

fn retained_control_target(
    metadata: Option<&ComponentMetadata>,
    target_kv: f64,
    source_control_is_effective: bool,
    typed_control_is_effective: bool,
    warnings: &mut CgmesDiagnostics,
    equipment: &str,
) -> (String, String) {
    let multiplier = metadata
        .and_then(|metadata| {
            metadata
                .properties
                .get("RegulatingControl.targetValueUnitMultiplier")
        })
        .map_or("UnitMultiplier.k", String::as_str);
    let raw_target =
        metadata.and_then(|metadata| metadata.properties.get("RegulatingControl.targetValue"));
    let Some(scale_to_kv) = unit_multiplier_scale_to_kv(multiplier) else {
        warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
            "equipment `{equipment}` has unsupported RegulatingControl.targetValueUnitMultiplier `{multiplier}`; fresh CGMES output uses UnitMultiplier.k"
        ));
        return (target_kv.to_string(), "UnitMultiplier.k".into());
    };
    let source_target_kv = raw_target
        .and_then(|value| value.parse::<f64>().ok())
        .map(|value| value * scale_to_kv);
    let unchanged = source_target_kv.is_some_and(|value| {
        let tolerance = 1e-9 * value.abs().max(target_kv.abs()).max(1.0);
        (value - target_kv).abs() <= tolerance
    }) || (!source_control_is_effective && !typed_control_is_effective);
    if unchanged && let Some(raw_target) = raw_target {
        return (raw_target.clone(), multiplier.into());
    }
    ((target_kv / scale_to_kv).to_string(), multiplier.into())
}

fn mapped_equipment_container_mrid(
    detailed: Option<&DetailedConnectivity>,
    component_type: &str,
    local_id: &str,
    unsupported_fallback: Option<String>,
    warnings: &mut CgmesDiagnostics,
) -> Option<String> {
    let detailed = detailed?;
    let container = mapped_component_metadata(Some(detailed), component_type, local_id)?
        .equipment_container
        .as_ref()?;
    if container.component_type() == "cgmes_object" {
        let component = format!("{component_type}/{local_id}");
        return match unsupported_fallback {
            None => {
                warnings.push_as(
                    &codes::EMIT_CGMES.field_dropped,
                    format!(
                        "component `{component}` references source EquipmentContainer \
                         `{container}`, whose CGMES class has no typed fresh-emission mapping; \
                         the EquipmentContainer reference was omitted"
                    ),
                );
                None
            }
            Some(fallback) => {
                warnings.push_as(
                    &codes::EMIT_CGMES.value_substituted,
                    format!(
                        "component `{component}` references source EquipmentContainer \
                         `{container}`, whose CGMES class has no typed fresh-emission mapping; \
                         fresh CGMES uses the equipment terminal's VoltageLevel instead"
                    ),
                );
                Some(fallback)
            }
        };
    }
    Some(component_mrid(detailed, container))
}

fn field_was_omitted(
    detailed: Option<&DetailedConnectivity>,
    component_type: &str,
    local_id: &str,
    field: OmittedFieldName,
) -> bool {
    detailed.is_some_and(|detailed| {
        detailed.omitted_fields.iter().any(|omitted| {
            omitted.field == field
                && omitted.component.component_type() == component_type
                && omitted.component.local_id() == local_id
        })
    })
}

fn metadata_property_is_used(property: &str) -> bool {
    [
        CGMES_CLASS_PROPERTY,
        super::CGMES_SV_STATUS_PROPERTY,
        super::CGMES_GENERATING_UNIT_PROPERTY,
        super::CGMES_REGULATING_CONTROL_PROPERTY,
        super::CGMES_SV_VOLTAGE_AUTHORITY_MISMATCH_PROPERTY,
        "PowerTransformerEnd.PowerTransformer",
        "TransformerEnd.endNumber",
        "SeriesCompensator.r0",
        "SeriesCompensator.x0",
        "SeriesCompensator.varistorPresent",
        "SeriesCompensator.varistorRatedCurrent",
        "SeriesCompensator.varistorVoltageThreshold",
        "RegulatingCondEq.controlEnabled",
        "RegulatingControl.discrete",
        "RegulatingControl.enabled",
        "RegulatingControl.mode",
        "RegulatingControl.targetDeadband",
        "RegulatingControl.targetValue",
        "RegulatingControl.targetValueUnitMultiplier",
        "IdentifiedObject.description",
        "Equipment.aggregate",
        "GeneratingUnit.genControlSource",
        "GeneratingUnit.initialP",
        "GeneratingUnit.nominalP",
        "SynchronousMachine.type",
        "SynchronousMachine.operatingMode",
        "SynchronousMachine.referencePriority",
    ]
    .contains(&property)
}

fn case_metadata_fields(metadata: &CaseMetadata, include_date: bool) -> Vec<String> {
    let mut fields = Vec::new();
    if include_date && let Some(value) = &metadata.case_date {
        fields.push(format!("case_date=`{value}`"));
    }
    if let Some(value) = metadata.forecast_distance {
        fields.push(format!("forecast_distance={value}"));
    }
    if let Some(value) = &metadata.source_model_format {
        fields.push(format!("source_model_format=`{value}`"));
    }
    if let Some(value) = &metadata.minimum_validation_level {
        fields.push(format!("minimum_validation_level=`{value}`"));
    }
    fields
}

fn component_matches_omission_record(
    network: &BalancedNetwork,
    component: &ComponentId,
    field: OmittedFieldName,
) -> bool {
    let local_id = component.local_id();
    match (component.component_type(), field) {
        ("load", OmittedFieldName::ActivePower | OmittedFieldName::ReactivePower) => {
            network.loads().iter().enumerate().any(|(index, load)| {
                load.uid
                    .clone()
                    .unwrap_or_else(|| format!("{}-{index}", load.bus))
                    == local_id
            })
        }
        (
            "generator",
            OmittedFieldName::ActivePower
            | OmittedFieldName::ReactivePower
            | OmittedFieldName::VoltageSetpoint
            | OmittedFieldName::RatedApparentPower,
        ) => network
            .generators()
            .iter()
            .enumerate()
            .any(|(index, generator)| {
                generator
                    .uid
                    .clone()
                    .unwrap_or_else(|| format!("{}-{index}", generator.bus))
                    == local_id
            }),
        ("shunt", OmittedFieldName::ShuntConductancePerSection) => {
            network.shunts().iter().enumerate().any(|(index, shunt)| {
                shunt
                    .uid
                    .clone()
                    .unwrap_or_else(|| format!("{}-{index}", shunt.bus))
                    == local_id
            })
        }
        _ => false,
    }
}

const fn omitted_field_label(field: OmittedFieldName) -> &'static str {
    match field {
        OmittedFieldName::ActivePower => "active_power",
        OmittedFieldName::ReactivePower => "reactive_power",
        OmittedFieldName::VoltageSetpoint => "voltage_setpoint",
        OmittedFieldName::RatedApparentPower => "rated_apparent_power",
        OmittedFieldName::ShuntConductancePerSection => "shunt_conductance_per_section",
    }
}

const fn dc_polarity_label(polarity: DcPolarity) -> &'static str {
    match polarity {
        DcPolarity::Positive => "positive",
        DcPolarity::Middle => "middle",
        DcPolarity::Negative => "negative",
    }
}

#[allow(clippy::too_many_lines)] // one audit pass names every unsupported detailed-connectivity field
pub(super) fn warn_unemitted_detailed_fields(
    network: &BalancedNetwork,
    detailed: &DetailedConnectivity,
    version: CgmesVersion,
    warnings: &mut CgmesDiagnostics,
) {
    for boundary in &detailed.boundary_lines {
        let load = boundary
            .calculation_load
            .as_ref()
            .map_or("none".into(), ToString::to_string);
        let generator = boundary
            .calculation_generator
            .as_ref()
            .map_or("none".into(), ToString::to_string);
        warnings.push_as(&codes::EMIT_CGMES.record_dropped, format!(
            "BoundaryLine `{}` is retained in PowerIO detailed connectivity but fresh CGMES does not emit EQBD or TPBD boundary records; its balanced calculation projections are load `{load}` and generator `{generator}`",
            boundary.component
        ));
    }
    for tie in &detailed.tie_lines {
        let branch = tie
            .calculation_branch
            .as_ref()
            .map_or("none".into(), ToString::to_string);
        warnings.push_as(&codes::EMIT_CGMES.record_dropped, format!(
            "TieLine `{}` joining BoundaryLines `{}` and `{}` is retained in PowerIO detailed connectivity but fresh CGMES emits neither the TieLine nor its boundary records; its balanced calculation projection is branch `{branch}`",
            tie.component, tie.boundary_line1, tie.boundary_line2
        ));
    }

    for node in &detailed.connectivity_nodes {
        if let Some(number) = node.node_number {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "ConnectivityNode `{}` has source node number {number}; CGMES identifies connectivity nodes by mRID and has no node number field",
                node.component
            ));
        }
    }
    for node in &detailed.dc_nodes {
        if let Some(voltage) = node.nominal_voltage_kv {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "DCNode `{}` has nominal_voltage_kv={voltage}; CGMES DCNode has no nominal voltage field",
                node.component
            ));
        }
        if let Some(voltage) = node.voltage_kv {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "DCNode `{}` has voltage_kv={voltage}; the emitted EQ, TP, SSH, and SV profiles have no DCNode voltage field",
                node.component
            ));
        }
    }

    let mut warn_nonconverter_polarity =
        |class: &str, equipment: &ComponentId, fallback_sequence: usize, terminal: &DcTerminal| {
            if let Some(polarity) = terminal.polarity {
                let sequence = terminal
                    .sequence_number
                    .map_or(fallback_sequence as u32, |value| value);
                let identity = terminal
                    .component
                    .as_ref()
                    .map_or_else(|| format!("{equipment}:{sequence}"), ToString::to_string);
                warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                    "{class} `{equipment}` DC terminal `{identity}` has polarity `{}`; CGMES defines ACDCConverterDCTerminal.polarity only for converter terminals",
                    dc_polarity_label(polarity)
                ));
            }
        };
    for ground in &detailed.dc_grounds {
        warn_nonconverter_polarity("DCGround", &ground.component, 1, &ground.dc_terminal);
    }
    for busbar in &detailed.dc_busbars {
        warn_nonconverter_polarity("DCBusbar", &busbar.component, 1, &busbar.dc_terminal);
    }
    for line in &detailed.dc_lines {
        warn_nonconverter_polarity("DCLineSegment", &line.component, 1, &line.dc_terminal1);
        warn_nonconverter_polarity("DCLineSegment", &line.component, 2, &line.dc_terminal2);
    }
    for device in &detailed.dc_series_devices {
        warn_nonconverter_polarity("DCSeriesDevice", &device.component, 1, &device.dc_terminal1);
        warn_nonconverter_polarity("DCSeriesDevice", &device.component, 2, &device.dc_terminal2);
    }
    for switch in &detailed.dc_switches {
        warn_nonconverter_polarity("DCSwitch", &switch.component, 1, &switch.dc_terminal1);
        warn_nonconverter_polarity("DCSwitch", &switch.component, 2, &switch.dc_terminal2);
    }

    for converter in &detailed.voltage_source_converters {
        if let (Some(uf), Some(uv)) = (converter.uf_kv, converter.uv_kv) {
            let tolerance = 1e-9 * uf.abs().max(uv.abs()).max(1.0);
            if (uf - uv).abs() > tolerance {
                let (emitted_property, emitted_value, omitted_property, omitted_value) =
                    if version == CgmesVersion::V3_0 {
                        ("VsConverter.uv", uv, "VsConverter.uf", uf)
                    } else {
                        ("VsConverter.uf", uf, "VsConverter.uv", uv)
                    };
                warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                    "VsConverter `{}` has conflicting valve voltages uf={uf} kV and uv={uv} kV; {} writes {emitted_property}={emitted_value} kV and does not emit {omitted_property}={omitted_value} kV",
                    converter.component,
                    version.label()
                ));
            }
        }
    }

    for metadata in &detailed.component_metadata {
        for property in metadata
            .properties
            .keys()
            .filter(|property| !metadata_property_is_used(property))
        {
            warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "component `{}` metadata property `{property}` has no fresh CGMES mapping",
                    metadata.component
                ),
            );
        }
    }
    for group in &detailed.operational_limit_groups {
        for property in group.properties.keys() {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "operational limit group `{}` on equipment `{}` metadata property `{property}` has no fresh CGMES mapping",
                group.id, group.equipment
            ));
        }
    }
    for omitted in &detailed.omitted_fields {
        if !component_matches_omission_record(network, &omitted.component, omitted.field) {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "omitted field record for component `{}` field `{}` does not match a CGMES assignment the writer can suppress and has no effect on fresh output",
                omitted.component,
                omitted_field_label(omitted.field)
            ));
        }
    }

    for subnetwork in &detailed.subnetworks {
        let fields = case_metadata_fields(&subnetwork.case_metadata, true);
        let metadata = if fields.is_empty() {
            "no subnetwork case metadata".into()
        } else {
            format!("subnetwork case metadata [{}]", fields.join(", "))
        };
        warnings.push_as(&codes::EMIT_CGMES.value_collapsed, format!(
            "subnetwork `{}` with parent `{}` and {} component reference(s) is flattened into the single fresh CGMES model set; its component grouping and {metadata} are not emitted separately",
            subnetwork.component,
            subnetwork.parent,
            subnetwork.components.len()
        ));
    }
    for field in case_metadata_fields(network.case_metadata(), false) {
        warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
            "network case metadata `{field}` has no field in the fresh CGMES EQ, TP, SSH, or SV FullModel headers"
        ));
    }
}

fn detailed_terminal<'a>(
    detailed: &'a DetailedConnectivity,
    component_type: &str,
    local_id: &str,
    terminal: usize,
) -> Option<&'a Terminal> {
    detailed.terminals.iter().find(|value| {
        value.equipment.component_type() == component_type
            && value.equipment.local_id() == local_id
            && usize::from(value.terminal) == terminal
    })
}

fn equipment_mrid(
    network: &BalancedNetwork,
    detailed: Option<&DetailedConnectivity>,
    component: &ComponentId,
) -> Option<String> {
    let local_id = component.local_id();
    match component.component_type() {
        "load" => network
            .loads()
            .iter()
            .enumerate()
            .find_map(|(index, value)| {
                (value.uid.as_deref() == Some(local_id)).then(|| {
                    mrid_or(
                        "load",
                        &format!("{}-{index}", value.bus),
                        value.uid.as_deref(),
                    )
                })
            }),
        "generator" => network
            .generators()
            .iter()
            .enumerate()
            .find_map(|(index, value)| {
                (value.uid.as_deref() == Some(local_id)).then(|| {
                    mrid_or(
                        "generator",
                        &format!("{}-{index}", value.bus),
                        value.uid.as_deref(),
                    )
                })
            }),
        "shunt" => network
            .shunts()
            .iter()
            .enumerate()
            .find_map(|(index, value)| {
                (value.uid.as_deref() == Some(local_id)).then(|| {
                    mrid_or(
                        "shunt",
                        &format!("{}-{index}", value.bus),
                        value.uid.as_deref(),
                    )
                })
            }),
        "static_var_compensator" => network
            .static_var_compensators()
            .iter()
            .enumerate()
            .find_map(|(index, value)| {
                (value.uid.as_deref() == Some(local_id)).then(|| {
                    mrid_or(
                        "static_var_compensator",
                        &format!("{}-{index}", value.bus),
                        value.uid.as_deref(),
                    )
                })
            }),
        "branch" => network
            .branches()
            .iter()
            .enumerate()
            .find_map(|(index, value)| {
                (value.uid.as_deref() == Some(local_id)).then(|| {
                    mrid_or(
                        "branch",
                        &format!("{}-{}-{index}", value.from, value.to),
                        value.uid.as_deref(),
                    )
                })
            })
            .or_else(|| {
                network
                    .transformers_3w()
                    .iter()
                    .enumerate()
                    .find_map(|(index, value)| {
                        (value.uid.as_deref() == Some(local_id)).then(|| {
                            mrid_or(
                                "transformer_3w",
                                &format!("transformer3w-{index}"),
                                value.uid.as_deref(),
                            )
                        })
                    })
            }),
        "switch" => network
            .switches()
            .iter()
            .enumerate()
            .find_map(|(index, value)| {
                (value.uid.as_deref() == Some(local_id)).then(|| {
                    mrid_or(
                        "switch",
                        &format!("{}-{}-{index}", value.from, value.to),
                        value.uid.as_deref(),
                    )
                })
            })
            .or_else(|| detailed.map(|value| component_mrid(value, component))),
        _ => detailed.map(|value| component_mrid(value, component)),
    }
}

fn terminal_reference_mrid(
    network: &BalancedNetwork,
    detailed: Option<&DetailedConnectivity>,
    reference: &TerminalReference,
) -> Option<String> {
    let equipment = equipment_mrid(network, detailed, &reference.equipment)?;
    let record = detailed.and_then(|details| {
        details.terminals.iter().find(|terminal| {
            terminal.equipment == reference.equipment && terminal.terminal == reference.terminal
        })
    });
    Some(terminal_mrid(
        detailed,
        record,
        &equipment,
        usize::from(reference.terminal),
    ))
}

fn configured_bus_mrid(detailed: &DetailedConnectivity, component: &ComponentId) -> String {
    component_mrid(detailed, component)
}

fn generated_voltage_level_mrid(container: &ComponentId) -> String {
    det_mrid(
        "voltagelevel",
        &format!("generated-for-connectivity-node-container:{container}"),
    )
}

fn topology_voltage_level_mrid(detailed: &DetailedConnectivity, container: &ComponentId) -> String {
    if container.component_type() == "line" {
        return component_mrid(detailed, container);
    }
    detailed
        .voltage_levels
        .iter()
        .find(|level| level.component == *container)
        .map_or_else(
            || generated_voltage_level_mrid(container),
            |level| component_mrid(detailed, &level.component),
        )
}

fn connectivity_node_mrid(
    detailed: Option<&DetailedConnectivity>,
    terminal: Option<&Terminal>,
    bus: BusId,
    project_mixed_topology: bool,
) -> String {
    if let Some(detailed) = detailed {
        let projected_bus_breaker_terminal = terminal.is_some_and(|terminal| {
            project_mixed_topology
                && terminal_voltage_level_topology(detailed, terminal)
                    == Some(TopologyKind::BusBreaker)
        });
        if !projected_bus_breaker_terminal
            && let Some(node) = terminal.and_then(|value| value.node.as_ref())
        {
            return component_mrid(detailed, node);
        }
        if let Some(configured) =
            terminal.and_then(|value| value.connectable_bus.as_ref().or(value.bus.as_ref()))
        {
            return det_mrid("connectivity_node", &configured.to_string());
        }
        if let Some(configured) = detailed
            .bus_breaker_buses
            .iter()
            .find(|value| value.calculated_bus == Some(bus))
        {
            return det_mrid("connectivity_node", &configured.component.to_string());
        }
    }
    det_mrid("connectivity_node", &format!("bus:{bus}"))
}

fn terminal_voltage_level_topology(
    detailed: &DetailedConnectivity,
    terminal: &Terminal,
) -> Option<TopologyKind> {
    detailed
        .voltage_levels
        .iter()
        .find(|level| level.component == terminal.voltage_level)
        .map(|level| level.topology_kind)
}

fn project_mixed_topology(detailed: &DetailedConnectivity) -> bool {
    let has_node_breaker = detailed
        .voltage_levels
        .iter()
        .any(|level| level.topology_kind == TopologyKind::NodeBreaker);
    let has_bus_breaker = detailed
        .voltage_levels
        .iter()
        .any(|level| level.topology_kind == TopologyKind::BusBreaker);
    has_node_breaker && has_bus_breaker
}

fn dc_terminal_nodes(terminal: &DcTerminal) -> impl Iterator<Item = &ComponentId> {
    [
        terminal.dc_node.as_ref(),
        terminal.dc_topological_node.as_ref(),
    ]
    .into_iter()
    .flatten()
}

fn extend_connected_dc_nodes(
    affected: &mut HashSet<ComponentId>,
    first: &DcTerminal,
    second: &DcTerminal,
) {
    let nodes = dc_terminal_nodes(first)
        .chain(dc_terminal_nodes(second))
        .cloned()
        .collect::<Vec<_>>();
    if nodes.iter().any(|node| affected.contains(node)) {
        affected.extend(nodes);
    }
}

fn converters_in_dc_series_device_islands(detailed: &DetailedConnectivity) -> HashSet<ComponentId> {
    let mut affected = detailed
        .dc_series_devices
        .iter()
        .flat_map(|device| {
            dc_terminal_nodes(&device.dc_terminal1).chain(dc_terminal_nodes(&device.dc_terminal2))
        })
        .cloned()
        .collect::<HashSet<_>>();
    loop {
        let previous_len = affected.len();
        for line in &detailed.dc_lines {
            extend_connected_dc_nodes(&mut affected, &line.dc_terminal1, &line.dc_terminal2);
        }
        for device in &detailed.dc_series_devices {
            extend_connected_dc_nodes(&mut affected, &device.dc_terminal1, &device.dc_terminal2);
        }
        for switch in &detailed.dc_switches {
            extend_connected_dc_nodes(&mut affected, &switch.dc_terminal1, &switch.dc_terminal2);
        }
        if affected.len() == previous_len {
            break;
        }
    }

    detailed
        .voltage_source_converters
        .iter()
        .filter(|converter| {
            dc_terminal_nodes(&converter.dc_terminal1)
                .chain(dc_terminal_nodes(&converter.dc_terminal2))
                .any(|node| affected.contains(node))
        })
        .map(|converter| converter.component.clone())
        .chain(
            detailed
                .line_commutated_converters
                .iter()
                .filter(|converter| {
                    dc_terminal_nodes(&converter.dc_terminal1)
                        .chain(dc_terminal_nodes(&converter.dc_terminal2))
                        .any(|node| affected.contains(node))
                })
                .map(|converter| converter.component.clone()),
        )
        .collect()
}

fn configured_terminal_bus(terminal: &Terminal) -> Option<&ComponentId> {
    terminal.connectable_bus.as_ref().or(terminal.bus.as_ref())
}

pub(super) fn warn_pow_sybl_projected_transformer_connections(
    network: &BalancedNetwork,
    detailed: &DetailedConnectivity,
    warnings: &mut CgmesDiagnostics,
) {
    let affected_converters = converters_in_dc_series_device_islands(detailed);
    if affected_converters.is_empty() {
        return;
    }
    for terminal in &detailed.terminals {
        if terminal.equipment.component_type() != "branch"
            || terminal_voltage_level_topology(detailed, terminal) != Some(TopologyKind::BusBreaker)
            || !network.branches().iter().any(|branch| {
                branch.uid.as_deref() == Some(terminal.equipment.local_id())
                    && (branch.is_transformer()
                        || source_branch_is_power_transformer(
                            Some(detailed),
                            terminal.equipment.local_id(),
                        ))
            })
        {
            continue;
        }
        let Some(bus) = configured_terminal_bus(terminal) else {
            continue;
        };
        let mut converters = detailed
            .terminals
            .iter()
            .filter(|candidate| configured_terminal_bus(candidate) == Some(bus))
            .map(|candidate| &candidate.equipment)
            .filter(|equipment| affected_converters.contains(*equipment))
            .cloned()
            .collect::<Vec<_>>();
        converters.sort();
        converters.dedup();
        if converters.is_empty() {
            continue;
        }
        let has_other_anchor = detailed.terminals.iter().any(|candidate| {
            configured_terminal_bus(candidate) == Some(bus)
                && candidate.equipment != terminal.equipment
                && !converters.contains(&candidate.equipment)
                && candidate.equipment.component_type() != "cgmes_object"
        });
        if has_other_anchor {
            continue;
        }
        let converters = converters
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "mixed topology projection preserves transformer `{}` terminal {} as connected at configured bus `{bus}`, but its projected ConnectivityNode is shared only with converter(s) [{converters}] in a DC island containing DCSeriesDevice; PowSybl 7.3.0 reports that source island as unsupported BACK_TO_BACK, drops the converter(s), and reloads this transformer terminal as disconnected; PowerIO retains the source connection",
            terminal.equipment, terminal.terminal
        ));
    }
}

fn terminal_uses_connectivity_node(
    detailed: &DetailedConnectivity,
    terminal: &Terminal,
    project_mixed_topology: bool,
) -> bool {
    terminal_voltage_level_topology(detailed, terminal).map_or_else(
        || {
            terminal.node.is_some()
                || (project_mixed_topology
                    && terminal
                        .connectable_bus
                        .as_ref()
                        .or(terminal.bus.as_ref())
                        .is_some())
        },
        |topology| {
            topology == TopologyKind::NodeBreaker
                || (project_mixed_topology && topology == TopologyKind::BusBreaker)
        },
    )
}

fn terminal_topological_node_mrid(
    network: &BalancedNetwork,
    detailed: Option<&DetailedConnectivity>,
    terminal: Option<&Terminal>,
    bus: BusId,
) -> String {
    if let Some(detailed) = detailed {
        if let Some(configured) = terminal.and_then(|value| value.bus.as_ref()) {
            return configured_bus_mrid(detailed, configured);
        }
        if let Some(node) = terminal.and_then(|value| value.node.as_ref())
            && let Some(calculated_bus) = calculated_bus_for_node(detailed, node)
        {
            return bus_mrid(network, calculated_bus);
        }
    }
    bus_mrid(network, bus)
}

fn terminal_voltage_level_mrid(
    detailed: Option<&DetailedConnectivity>,
    terminal: Option<&Terminal>,
    bus: BusId,
) -> String {
    if let Some(detailed) = detailed {
        if let Some(level) = terminal.map(|value| &value.voltage_level) {
            return topology_voltage_level_mrid(detailed, level);
        }
        if let Some(level) = detailed
            .voltage_levels
            .iter()
            .find(|value| value.buses.contains(&bus))
        {
            return component_mrid(detailed, &level.component);
        }
    }
    det_mrid("voltagelevel", &bus.to_string())
}

fn detailed_voltage_level_for_bus(
    detailed: &DetailedConnectivity,
    bus: BusId,
) -> Option<&ComponentId> {
    detailed
        .bus_breaker_buses
        .iter()
        .find(|configured| configured.calculated_bus == Some(bus))
        .map(|configured| &configured.voltage_level)
        .or_else(|| {
            detailed
                .calculated_buses
                .iter()
                .find(|calculated| calculated.calculated_bus == bus)
                .map(|calculated| &calculated.voltage_level)
        })
        .or_else(|| {
            detailed
                .connectivity_nodes
                .iter()
                .find(|node| calculated_bus_for_node(detailed, &node.component) == Some(bus))
                .map(|node| &node.voltage_level)
        })
        .or_else(|| {
            detailed
                .voltage_levels
                .iter()
                .find(|level| level.buses.contains(&bus))
                .map(|level| &level.component)
        })
}

fn missing_detailed_terminal_buses(
    network: &BalancedNetwork,
    detailed: &DetailedConnectivity,
) -> HashSet<BusId> {
    let mut buses = HashSet::new();
    let mut check = |component_type: &str, local_id: &str, terminal: usize, bus: BusId| {
        if detailed_terminal(detailed, component_type, local_id, terminal).is_none() {
            buses.insert(bus);
        }
    };

    for (index, load) in network.loads().iter().enumerate() {
        let fallback = format!("{}-{index}", load.bus);
        check(
            "load",
            load.uid.as_deref().unwrap_or(&fallback),
            1,
            load.bus,
        );
    }
    for (index, generator) in network.generators().iter().enumerate() {
        let fallback = format!("{}-{index}", generator.bus);
        check(
            "generator",
            generator.uid.as_deref().unwrap_or(&fallback),
            1,
            generator.bus,
        );
    }
    for (index, shunt) in network.shunts().iter().enumerate() {
        let fallback = format!("{}-{index}", shunt.bus);
        check(
            "shunt",
            shunt.uid.as_deref().unwrap_or(&fallback),
            1,
            shunt.bus,
        );
    }
    for (index, svc) in network.static_var_compensators().iter().enumerate() {
        let fallback = format!("{}-{index}", svc.bus);
        check(
            "static_var_compensator",
            svc.uid.as_deref().unwrap_or(&fallback),
            1,
            svc.bus,
        );
    }
    if detailed.switches.is_empty() {
        for (index, switch) in network.switches().iter().enumerate() {
            let fallback = format!("{}-{}-{index}", switch.from, switch.to);
            let local_id = switch.uid.as_deref().unwrap_or(&fallback);
            check("switch", local_id, 1, switch.from);
            check("switch", local_id, 2, switch.to);
        }
    }
    for (index, branch) in network.branches().iter().enumerate() {
        let fallback = format!("{}-{}-{index}", branch.from, branch.to);
        let local_id = branch.uid.as_deref().unwrap_or(&fallback);
        check("branch", local_id, 1, branch.from);
        check("branch", local_id, 2, branch.to);
    }
    for (index, transformer) in network.transformers_3w().iter().enumerate() {
        let fallback = format!("transformer3w-{index}");
        let local_id = transformer.uid.as_deref().unwrap_or(&fallback);
        for (winding, data) in transformer.windings.iter().enumerate() {
            check("branch", local_id, winding + 1, data.bus);
        }
    }
    buses
}

fn switch_class(kind: SwitchKind) -> &'static str {
    match kind {
        SwitchKind::Breaker => "Breaker",
        SwitchKind::Disconnector => "Disconnector",
        SwitchKind::LoadBreakSwitch => "LoadBreakSwitch",
    }
}

fn endpoint_bus(detailed: &DetailedConnectivity, endpoint: &TopologyEndpoint) -> Option<BusId> {
    match endpoint {
        TopologyEndpoint::Bus(component) => detailed
            .bus_breaker_buses
            .iter()
            .find(|value| value.component == *component)
            .and_then(|value| value.calculated_bus),
        TopologyEndpoint::Node(component) => calculated_bus_for_node(detailed, component),
    }
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One profile document under construction.
struct Doc {
    body: String,
    retained_metadata: Arc<HashMap<String, RetainedIdentifiedMetadata>>,
}

impl Doc {
    fn new(retained_metadata: Arc<HashMap<String, RetainedIdentifiedMetadata>>) -> Doc {
        Doc {
            body: String::new(),
            retained_metadata,
        }
    }

    /// Open an object element (`rdf:ID` definition or `rdf:about` extension).
    fn open(&mut self, class: &str, id: &str, about: bool) {
        let attr = if about {
            format!("rdf:about=\"#_{id}\"")
        } else {
            format!("rdf:ID=\"_{id}\"")
        };
        let _ = writeln!(self.body, "  <cim:{class} {attr}>");
        if !about && let Some(metadata) = self.retained_metadata.get(id).cloned() {
            if let Some(name) = metadata.name {
                self.text("IdentifiedObject.name", name);
            }
            if let Some(short_name) = metadata.short_name {
                self.text("IdentifiedObject.shortName", short_name);
            }
            if metadata.fictitious {
                self.text("IdentifiedObject.isFictitious", true);
            }
        }
    }

    fn close(&mut self, class: &str) {
        let _ = writeln!(self.body, "  </cim:{class}>");
    }

    fn text(&mut self, prop: &str, value: impl std::fmt::Display) {
        let _ = writeln!(
            self.body,
            "    <cim:{prop}>{}</cim:{prop}>",
            esc(&value.to_string())
        );
    }

    /// A property in a non-`cim` namespace (`entsoe:`/`eu:` extensions).
    fn ext_ref(&mut self, prefix: &str, prop: &str, uri: &str) {
        let _ = writeln!(self.body, "    <{prefix}:{prop} rdf:resource=\"{uri}\"/>");
    }

    fn reference(&mut self, prop: &str, target: &str) {
        let _ = writeln!(self.body, "    <cim:{prop} rdf:resource=\"#_{target}\"/>");
    }

    fn enumeration(&mut self, prop: &str, cim_ns: &str, value: &str) {
        let _ = writeln!(
            self.body,
            "    <cim:{prop} rdf:resource=\"{cim_ns}{value}\"/>"
        );
    }

    fn named(&mut self, class: &str, id: &str, name: &str) {
        self.open(class, id, false);
        if self
            .retained_metadata
            .get(id)
            .is_none_or(|metadata| metadata.name.is_none())
        {
            self.text("IdentifiedObject.name", name);
        }
    }
}

fn write_sv_status(sv: &mut Doc, equipment: &str, in_service: bool) {
    let status = det_mrid("svstatus", equipment);
    sv.open("SvStatus", &status, false);
    sv.reference("SvStatus.ConductingEquipment", equipment);
    sv.text("SvStatus.inService", in_service);
    sv.close("SvStatus");
}

fn write_sv_power_flow(
    sv: &mut Doc,
    terminal: &str,
    active_power_mw: Option<f64>,
    reactive_power_mvar: Option<f64>,
    warnings: &mut CgmesDiagnostics,
) {
    let (active_power_mw, reactive_power_mvar) = match (active_power_mw, reactive_power_mvar) {
        (Some(active), Some(reactive)) => (active, reactive),
        (None, None) => return,
        (Some(active), None) => {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "terminal `{terminal}`: retained active power field {active} MW without reactive power; CGMES SvPowerFlow requires both p and q, so the partial observation was not emitted"
            ));
            return;
        }
        (None, Some(reactive)) => {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "terminal `{terminal}`: retained reactive power field {reactive} MVAr without active power; CGMES SvPowerFlow requires both p and q, so the partial observation was not emitted"
            ));
            return;
        }
    };
    let flow = det_mrid("svpowerflow", terminal);
    sv.open("SvPowerFlow", &flow, false);
    sv.reference("SvPowerFlow.Terminal", terminal);
    sv.text("SvPowerFlow.p", active_power_mw);
    sv.text("SvPowerFlow.q", reactive_power_mvar);
    sv.close("SvPowerFlow");
}

fn write_sv_voltage(
    sv: &mut Doc,
    topological_node: &str,
    voltage_kv: Option<f64>,
    angle_degrees: Option<f64>,
    authority_mismatch: bool,
    warnings: &mut CgmesDiagnostics,
) {
    if authority_mismatch {
        if voltage_kv.is_some() || angle_degrees.is_some() {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "TopologicalNode `{topological_node}`: retained SvVoltage belongs to a different modeling authority than its source equipment; the observation remains in PowerIO data but was not emitted into the single authority CGMES profile set"
            ));
        }
        return;
    }
    let (voltage_kv, angle_degrees) = match (voltage_kv, angle_degrees) {
        (Some(voltage), Some(angle)) => (voltage, angle),
        (None, None) => return,
        (Some(voltage), None) => {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "TopologicalNode `{topological_node}`: retained voltage field {voltage} kV without an angle; CGMES SvVoltage requires both v and angle, so the partial observation was not emitted"
            ));
            return;
        }
        (None, Some(angle)) => {
            warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "TopologicalNode `{topological_node}`: retained angle field {angle} degrees without voltage; CGMES SvVoltage requires both v and angle, so the partial observation was not emitted"
            ));
            return;
        }
    };
    let sv_voltage = det_mrid("svvoltage", topological_node);
    sv.open("SvVoltage", &sv_voltage, false);
    sv.reference("SvVoltage.TopologicalNode", topological_node);
    sv.text("SvVoltage.v", voltage_kv);
    sv.text("SvVoltage.angle", angle_degrees);
    sv.close("SvVoltage");
}

fn retained_sv_status(detailed: &DetailedConnectivity, component: &ComponentId) -> Option<bool> {
    metadata(detailed, component)?
        .properties
        .get(super::CGMES_SV_STATUS_PROPERTY)?
        .parse()
        .ok()
}

struct Profiles {
    cim_ns: &'static str,
    ext: (&'static str, &'static str), // (prefix, namespace)
    eq: &'static str,
    tp: &'static str,
    ssh: &'static str,
    sv: &'static str,
}

fn profiles(version: CgmesVersion) -> Profiles {
    match version {
        CgmesVersion::V2_4_15 => Profiles {
            cim_ns: "http://iec.ch/TC57/2013/CIM-schema-cim16#",
            ext: ("entsoe", "http://entsoe.eu/CIM/SchemaExtension/3/1#"),
            eq: "http://entsoe.eu/CIM/EquipmentCore/3/1",
            tp: "http://entsoe.eu/CIM/Topology/4/1",
            ssh: "http://entsoe.eu/CIM/SteadyStateHypothesis/1/1",
            sv: "http://entsoe.eu/CIM/StateVariables/4/1",
        },
        CgmesVersion::V3_0 => Profiles {
            cim_ns: "http://iec.ch/TC57/CIM100#",
            ext: ("eu", "http://iec.ch/TC57/CIM100-European#"),
            eq: "http://iec.ch/TC57/ns/CIM/CoreEquipment-EU/3.0",
            tp: "http://iec.ch/TC57/ns/CIM/Topology-EU/3.0",
            ssh: "http://iec.ch/TC57/ns/CIM/SteadyStateHypothesis-EU/3.0",
            sv: "http://iec.ch/TC57/ns/CIM/StateVariables-EU/3.0",
        },
    }
}

/// Build the `rdf:RDF` document with its `md:FullModel` header.
fn document(
    p: &Profiles,
    profile: &str,
    model_id: &str,
    description: &str,
    case_date: &str,
    depends: &[&str],
    body: &str,
) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n         \
         xmlns:cim=\"{}\"\n         xmlns:{}=\"{}\"\n         \
         xmlns:md=\"http://iec.ch/TC57/61970-552/ModelDescription/1#\">",
        p.cim_ns, p.ext.0, p.ext.1
    );
    let _ = writeln!(out, "  <md:FullModel rdf:about=\"urn:uuid:{model_id}\">");
    let _ = writeln!(
        out,
        "    <md:Model.scenarioTime>{}</md:Model.scenarioTime>",
        esc(case_date)
    );
    let _ = writeln!(
        out,
        "    <md:Model.created>{}</md:Model.created>",
        esc(case_date)
    );
    let _ = writeln!(
        out,
        "    <md:Model.description>{}</md:Model.description>",
        esc(description)
    );
    let _ = writeln!(out, "    <md:Model.version>1</md:Model.version>");
    let _ = writeln!(out, "    <md:Model.profile>{profile}</md:Model.profile>");
    for dep in depends {
        let _ = writeln!(
            out,
            "    <md:Model.DependentOn rdf:resource=\"urn:uuid:{dep}\"/>"
        );
    }
    let _ = writeln!(
        out,
        "    <md:Model.modelingAuthoritySet>http://powerio.dev/cgmes</md:Model.modelingAuthoritySet>"
    );
    out.push_str("  </md:FullModel>\n");
    out.push_str(body);
    out.push_str("</rdf:RDF>\n");
    out
}

/// The per-element naming and unit conversions shared by the four profiles.
struct Writer<'a> {
    net: &'a BalancedNetwork,
    p: Profiles,
    warnings: CgmesDiagnostics,
}

fn emission_error(message: impl Into<String>) -> Error {
    Error::Emit {
        format: "CGMES",
        message: message.into(),
    }
}

fn validate_active_power_control(
    equipment: &str,
    control: &ActivePowerControl,
    equipment_minimum: f64,
    equipment_maximum: f64,
) -> Result<()> {
    for (field, value) in [
        ("droop_percent", control.droop_percent),
        ("participation_factor", control.participation_factor),
        (
            "minimum_target_active_power_mw",
            control.minimum_target_active_power_mw,
        ),
        (
            "maximum_target_active_power_mw",
            control.maximum_target_active_power_mw,
        ),
    ] {
        if value.is_some_and(|value| !value.is_finite()) {
            return Err(emission_error(format!(
                "active power control `{field}` on `{equipment}` is not finite"
            )));
        }
    }
    if control
        .participation_factor
        .is_some_and(|value| value < 0.0)
    {
        return Err(emission_error(format!(
            "active power control participation_factor on `{equipment}` is negative"
        )));
    }
    for (field, value) in [
        (
            "minimum_target_active_power_mw",
            control.minimum_target_active_power_mw,
        ),
        (
            "maximum_target_active_power_mw",
            control.maximum_target_active_power_mw,
        ),
    ] {
        if value.is_some_and(|value| value < equipment_minimum || value > equipment_maximum) {
            return Err(emission_error(format!(
                "active power control `{field}` on `{equipment}` is outside equipment active power limits [{equipment_minimum}, {equipment_maximum}] MW"
            )));
        }
    }
    if control
        .minimum_target_active_power_mw
        .zip(control.maximum_target_active_power_mw)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(emission_error(format!(
            "active power control minimum target on `{equipment}` exceeds its maximum target"
        )));
    }
    Ok(())
}

#[derive(Clone)]
struct SourceLimitType {
    id: String,
    kind: &'static str,
    acceptable_duration_seconds: u64,
    infinite: bool,
}

fn source_limit_type(
    types: &mut Vec<SourceLimitType>,
    kind: &'static str,
    acceptable_duration_seconds: u64,
    infinite: bool,
) -> String {
    if let Some(value) = types.iter().find(|value| {
        value.kind == kind
            && value.acceptable_duration_seconds == acceptable_duration_seconds
            && value.infinite == infinite
    }) {
        return value.id.clone();
    }
    let id = det_mrid(
        "source_limit_type",
        &format!("{kind}:{acceptable_duration_seconds}:{infinite}"),
    );
    types.push(SourceLimitType {
        id: id.clone(),
        kind,
        acceptable_duration_seconds,
        infinite,
    });
    id
}

#[allow(clippy::float_cmp)]
fn tap_step(tap: &TapChanger) -> Option<&crate::network::TapChangerStep> {
    tap.tap_position
        .and_then(|position| tap.steps.iter().find(|value| value.position == position))
        .or_else(|| {
            tap.tap_position.is_none().then(|| {
                tap.steps
                    .iter()
                    .find(|step| match tap.kind {
                        TapChangerKind::Ratio => step.rho == 1.0,
                        TapChangerKind::Phase => step.rho == 1.0 && step.alpha_degrees == 0.0,
                    })
                    .or_else(|| tap.steps.first())
            })?
        })
}

fn source_tap_changer<'a>(
    detailed: Option<&'a DetailedConnectivity>,
    transformer: &str,
    winding: usize,
    kind: TapChangerKind,
) -> Option<&'a TapChanger> {
    detailed?.tap_changers.iter().find(|value| {
        value.transformer.component_type() == "branch"
            && value.transformer.local_id() == transformer
            && usize::from(value.winding) == winding
            && value.kind == kind
    })
}

fn source_branch_is_power_transformer(
    detailed: Option<&DetailedConnectivity>,
    branch_local_id: &str,
) -> bool {
    mapped_component_metadata(detailed, "branch", branch_local_id).is_some_and(|metadata| {
        metadata
            .properties
            .get(CGMES_CLASS_PROPERTY)
            .is_some_and(|class| class == "PowerTransformer")
    }) || detailed.is_some_and(|detailed| {
        detailed.tap_changers.iter().any(|tap| {
            tap.transformer.component_type() == "branch"
                && tap.transformer.local_id() == branch_local_id
        })
    })
}

fn tap_neutral_position(tap: &TapChanger) -> i32 {
    if let Some(position) = tap.neutral_tap_position {
        return position;
    }
    tap.steps
        .iter()
        .min_by(|left, right| {
            let left_distance = (left.rho - 1.0).abs() + left.alpha_degrees.abs();
            let right_distance = (right.rho - 1.0).abs() + right.alpha_degrees.abs();
            left_distance.total_cmp(&right_distance)
        })
        .map_or(tap.low_tap_position, |value| value.position)
}

fn tap_high_position(tap: &TapChanger) -> i32 {
    tap.steps
        .iter()
        .map(|value| value.position)
        .max()
        .unwrap_or(tap.low_tap_position)
}

fn tap_normal_position(tap: &TapChanger) -> i32 {
    tap.normal_tap_position
        .or(tap.tap_position)
        .unwrap_or_else(|| tap_neutral_position(tap))
}

fn ratio_voltage_step_increment_percent(tap: &TapChanger) -> f64 {
    if let Some(increment) = tap.voltage_step_increment_percent {
        return increment;
    }
    let mut increments = tap.steps.windows(2).filter_map(|steps| {
        let positions = steps[1].position - steps[0].position;
        (positions != 0).then(|| (steps[1].rho - steps[0].rho) * 100.0 / f64::from(positions))
    });
    let Some(first) = increments.next() else {
        return 0.0;
    };
    if increments.all(|increment| {
        (increment - first).abs() <= 1e-12 * increment.abs().max(first.abs()).max(1.0)
    }) {
        first
    } else {
        0.0
    }
}

fn tap_rated_kv(network: &BalancedNetwork, tap: &TapChanger) -> f64 {
    if let Some(transformer) = network
        .transformers_3w()
        .iter()
        .find(|value| value.uid.as_deref() == Some(tap.transformer.local_id()))
        && let Some(winding) = transformer
            .windings
            .get(usize::from(tap.winding.saturating_sub(1)))
        && winding.nominal_kv > 0.0
    {
        return winding.nominal_kv;
    }
    let bus = if let Some(branch) = network
        .branches()
        .iter()
        .find(|value| value.uid.as_deref() == Some(tap.transformer.local_id()))
    {
        match tap.winding {
            1 => Some(branch.from),
            2 => Some(branch.to),
            _ => None,
        }
    } else {
        network
            .transformers_3w()
            .iter()
            .find(|value| value.uid.as_deref() == Some(tap.transformer.local_id()))
            .and_then(|value| {
                value
                    .windings
                    .get(usize::from(tap.winding.saturating_sub(1)))
            })
            .map(|value| value.bus)
    };
    bus.and_then(|bus| {
        network
            .buses()
            .iter()
            .find(|value| value.id == bus)
            .map(|value| value.base_kv)
    })
    .unwrap_or(0.0)
}

#[derive(Clone, Copy)]
struct TapWriteContext<'a> {
    network: &'a BalancedNetwork,
    detailed: &'a DetailedConnectivity,
    cim_namespace: &'a str,
    tap: &'a TapChanger,
}

// This writes one tap changer consistently across EQ, SSH, and SV; splitting it
// would separate the shared identifiers and references it validates together.
#[allow(clippy::too_many_lines)]
fn write_source_tap_changer(
    eq: &mut Doc,
    ssh: &mut Doc,
    sv: &mut Doc,
    context: TapWriteContext<'_>,
) -> std::result::Result<(), String> {
    let tap = context.tap;
    let owner = equipment_mrid(context.network, Some(context.detailed), &tap.transformer)
        .ok_or_else(|| format!("unknown transformer `{}`", tap.transformer))?;
    if tap.winding == 0 {
        return Err(format!(
            "transformer `{}` has a tap changer on winding 0",
            tap.transformer
        ));
    }
    if tap.steps.is_empty() {
        return Err(format!(
            "transformer `{}` winding {} has a tap changer with no steps",
            tap.transformer, tap.winding
        ));
    }
    let class = match tap.kind {
        TapChangerKind::Ratio => "RatioTapChanger",
        TapChangerKind::Phase => "PhaseTapChangerTabular",
    };
    let kind = match tap.kind {
        TapChangerKind::Ratio => "ratio",
        TapChangerKind::Phase => "phase",
    };
    let id = tap.component.as_ref().map_or_else(
        || {
            det_mrid(
                "source_tap_changer",
                &format!("{}:{}:{kind}", tap.transformer, tap.winding),
            )
        },
        |component| component_mrid(context.detailed, component),
    );
    let table_class = match tap.kind {
        TapChangerKind::Ratio => "RatioTapChangerTable",
        TapChangerKind::Phase => "PhaseTapChangerTable",
    };
    let point_class = match tap.kind {
        TapChangerKind::Ratio => "RatioTapChangerTablePoint",
        TapChangerKind::Phase => "PhaseTapChangerTablePoint",
    };
    let table = det_mrid("source_tap_table", &id);
    let control = (tap.regulating
        || tap.regulation_mode.is_some()
        || tap.regulation_value.is_some()
        || tap.regulation_terminal.is_some())
    .then(|| det_mrid("source_tap_control", &id));
    let control_terminal = control
        .as_ref()
        .map(|_| match tap.regulation_terminal.as_ref() {
            Some(reference) => terminal_reference_mrid(
                context.network,
                Some(context.detailed),
                reference,
            )
            .ok_or_else(|| {
                format!(
                    "regulation terminal `{}` terminal {} does not resolve to an emitted CGMES terminal",
                    reference.equipment, reference.terminal
                )
            }),
            None => Ok(term_id(&owner, usize::from(tap.winding))),
        })
        .transpose()?;

    eq.named(class, &id, &format!("{kind} tap changer"));
    eq.text("TapChanger.lowStep", tap.low_tap_position);
    eq.text("TapChanger.highStep", tap_high_position(tap));
    eq.text("TapChanger.neutralStep", tap_neutral_position(tap));
    eq.text("TapChanger.normalStep", tap_normal_position(tap));
    eq.text("TapChanger.neutralU", tap_rated_kv(context.network, tap));
    eq.text("TapChanger.ltcFlag", tap.load_tap_changing_capabilities);
    if let Some(control) = &control {
        eq.reference("TapChanger.TapChangerControl", control);
    }
    eq.reference(
        match tap.kind {
            TapChangerKind::Ratio => "RatioTapChanger.TransformerEnd",
            TapChangerKind::Phase => "PhaseTapChanger.TransformerEnd",
        },
        &transformer_end_mrid(
            Some(context.detailed),
            tap.transformer.component_type(),
            tap.transformer.local_id(),
            &owner,
            usize::from(tap.winding),
        )
        .map_err(|error| error.to_string())?,
    );
    eq.reference(
        match tap.kind {
            TapChangerKind::Ratio => "RatioTapChanger.RatioTapChangerTable",
            TapChangerKind::Phase => "PhaseTapChangerTabular.PhaseTapChangerTable",
        },
        &table,
    );
    if tap.kind == TapChangerKind::Ratio {
        eq.text(
            "RatioTapChanger.stepVoltageIncrement",
            ratio_voltage_step_increment_percent(tap),
        );
    }
    eq.close(class);

    write_source_tap_table(eq, context, &id, &table, table_class, point_class, kind);

    if let (Some(control_id), Some(control_terminal)) = (control, control_terminal) {
        write_source_tap_control(eq, ssh, context, &control_id, &control_terminal);
    }
    ssh.open(class, &id, true);
    if let Some(position) = tap.tap_position {
        ssh.text("TapChanger.step", position);
    }
    ssh.text("TapChanger.controlEnabled", tap.regulating);
    ssh.close(class);
    if let Some(position) = tap.solved_tap_position {
        sv.open("SvTapStep", &det_mrid("svtap", &id), false);
        sv.reference("SvTapStep.TapChanger", &id);
        sv.text("SvTapStep.position", position);
        sv.close("SvTapStep");
    }
    Ok(())
}

fn write_source_tap_table(
    eq: &mut Doc,
    context: TapWriteContext<'_>,
    id: &str,
    table: &str,
    table_class: &str,
    point_class: &str,
    kind: &str,
) {
    eq.named(table_class, table, &format!("{kind} tap changer table"));
    eq.close(table_class);
    for step in &context.tap.steps {
        let point = det_mrid("source_tap_point", &format!("{id}:{}", step.position));
        eq.open(point_class, &point, false);
        eq.reference(&format!("{point_class}.{table_class}"), table);
        eq.text("TapChangerTablePoint.step", step.position);
        eq.text("TapChangerTablePoint.ratio", step.rho);
        if context.tap.kind == TapChangerKind::Phase {
            eq.text("PhaseTapChangerTablePoint.angle", step.alpha_degrees);
        }
        eq.text("TapChangerTablePoint.r", step.resistance_deviation_percent);
        eq.text("TapChangerTablePoint.x", step.reactance_deviation_percent);
        eq.text("TapChangerTablePoint.g", step.conductance_deviation_percent);
        eq.text("TapChangerTablePoint.b", step.susceptance_deviation_percent);
        eq.close(point_class);
    }
}

fn write_source_tap_control(
    eq: &mut Doc,
    ssh: &mut Doc,
    context: TapWriteContext<'_>,
    control_id: &str,
    terminal: &str,
) {
    let tap = context.tap;
    eq.named("TapChangerControl", control_id, "tap changer control");
    let mode = tap.regulation_mode.unwrap_or(match tap.kind {
        TapChangerKind::Ratio => TapChangerRegulationMode::Voltage,
        TapChangerKind::Phase => TapChangerRegulationMode::ActivePower,
    });
    eq.enumeration(
        "RegulatingControl.mode",
        context.cim_namespace,
        match mode {
            TapChangerRegulationMode::Voltage => "RegulatingControlModeKind.voltage",
            TapChangerRegulationMode::ReactivePower => "RegulatingControlModeKind.reactivePower",
            TapChangerRegulationMode::ActivePower => "RegulatingControlModeKind.activePower",
            TapChangerRegulationMode::Current => "RegulatingControlModeKind.currentFlow",
        },
    );
    eq.reference("RegulatingControl.Terminal", terminal);
    eq.close("TapChangerControl");

    ssh.open("TapChangerControl", control_id, true);
    ssh.text("RegulatingControl.discrete", true);
    ssh.text("RegulatingControl.enabled", tap.regulating);
    if let Some(value) = tap.regulation_value {
        ssh.text("RegulatingControl.targetValue", value);
        ssh.enumeration(
            "RegulatingControl.targetValueUnitMultiplier",
            context.cim_namespace,
            match mode {
                TapChangerRegulationMode::Voltage => "UnitMultiplier.k",
                TapChangerRegulationMode::ReactivePower | TapChangerRegulationMode::ActivePower => {
                    "UnitMultiplier.M"
                }
                TapChangerRegulationMode::Current => "UnitMultiplier.none",
            },
        );
    }
    if let Some(value) = tap.target_deadband {
        ssh.text("RegulatingControl.targetDeadband", value);
    }
    ssh.close("TapChangerControl");
}

#[derive(Clone, Copy)]
struct LimitWriteContext<'a> {
    version: CgmesVersion,
    group_id: &'a str,
    class: &'static str,
    limits: &'a LoadingLimits,
}

fn write_source_loading_limits(
    body: &mut Doc,
    types: &mut Vec<SourceLimitType>,
    context: LimitWriteContext<'_>,
    warnings: &mut CgmesDiagnostics,
) {
    let value_property = if context.version == CgmesVersion::V3_0 {
        format!("{}.normalValue", context.class)
    } else {
        format!("{}.value", context.class)
    };
    if let Some(value) = context.limits.permanent_limit {
        if value.is_finite() && value > 0.0 {
            let type_id = source_limit_type(types, "patl", 0, true);
            let id = det_mrid(
                "source_limit",
                &format!("{}:{}:permanent", context.group_id, context.class),
            );
            body.named(
                context.class,
                &id,
                context
                    .limits
                    .permanent_limit_name
                    .as_deref()
                    .unwrap_or("PATL"),
            );
            body.reference("OperationalLimit.OperationalLimitSet", context.group_id);
            body.reference("OperationalLimit.OperationalLimitType", &type_id);
            body.text(&value_property, value);
            body.close(context.class);
        } else {
            warnings.push_as(&codes::EMIT_CGMES.record_dropped, format!(
                "operational limit set `{}` {} permanent limit `{value}` was not emitted because a CGMES limit must be positive and finite",
                context.group_id, context.class
            ));
        }
    }
    for (index, limit) in context.limits.temporary_limits.iter().enumerate() {
        if !limit.value.is_finite() || limit.value <= 0.0 {
            warnings.push_as(&codes::EMIT_CGMES.record_dropped, format!(
                "operational limit set `{}` {} temporary limit `{}` with value `{}` and duration {} seconds was not emitted because a CGMES limit must be positive and finite",
                context.group_id,
                context.class,
                limit.name,
                limit.value,
                limit.acceptable_duration_seconds
            ));
            continue;
        }
        let type_id = source_limit_type(types, "tatl", limit.acceptable_duration_seconds, false);
        let id = det_mrid(
            "source_limit",
            &format!(
                "{}:{}:temporary:{}:{index}",
                context.group_id, context.class, limit.acceptable_duration_seconds
            ),
        );
        body.named(context.class, &id, &limit.name);
        if limit.fictitious {
            body.text("IdentifiedObject.isFictitious", true);
        }
        body.reference("OperationalLimit.OperationalLimitSet", context.group_id);
        body.reference("OperationalLimit.OperationalLimitType", &type_id);
        body.text(&value_property, limit.value);
        body.close(context.class);
    }
}

impl Writer<'_> {
    fn kv(&self, bus: BusId) -> Result<f64> {
        let bus = self
            .net
            .buses()
            .iter()
            .find(|b| b.id == bus)
            .ok_or_else(|| emission_error(format!("bus {bus} does not exist")))?;
        if !bus.base_kv.is_finite() || bus.base_kv <= 0.0 {
            return Err(emission_error(format!(
                "bus {} has nonpositive or nonfinite base_kv {}",
                bus.id, bus.base_kv
            )));
        }
        Ok(bus.base_kv)
    }
}

/// A bus's TopologicalNode id: the imported uid, else deterministic.
fn bus_mrid(net: &BalancedNetwork, bus: BusId) -> String {
    if let Some(detailed) = net.detailed_connectivity().as_deref()
        && let Some(configured) = detailed
            .bus_breaker_buses
            .iter()
            .find(|value| value.calculated_bus == Some(bus))
    {
        return component_mrid(detailed, &configured.component);
    }
    net.buses()
        .iter()
        .find(|b| b.id == bus)
        .map(|b| mrid_or("bus", &b.id.to_string(), b.uid.as_deref()))
        .unwrap_or_default()
}

/// Terminal id for equipment `eq` at sequence `seq`.
fn term_id(eq: &str, seq: usize) -> String {
    det_mrid("terminal", &format!("{eq}:{seq}"))
}

fn terminal_mrid(
    detailed: Option<&DetailedConnectivity>,
    terminal: Option<&Terminal>,
    equipment_mrid: &str,
    sequence: usize,
) -> String {
    match (
        detailed,
        terminal.and_then(|terminal| terminal.component.as_ref()),
    ) {
        (Some(detailed), Some(component)) => component_mrid(detailed, component),
        _ => term_id(equipment_mrid, sequence),
    }
}

fn expanded_shunt_sections(shunt: &Shunt) -> Vec<(f64, f64)> {
    let Some(control) = shunt.control.as_ref() else {
        return vec![(shunt.g, shunt.b)];
    };
    control
        .blocks
        .iter()
        .flat_map(|block| std::iter::repeat_n((block.g, block.b), block.steps as usize))
        .collect()
}

fn shunt_section_count(shunt: &Shunt, sections: &[(f64, f64)]) -> (usize, f64) {
    let mut running_g = 0.0;
    let mut running_b = 0.0;
    let mut best = 0usize;
    let mut best_error = (shunt.g - running_g).abs() + (shunt.b - running_b).abs();
    for (index, (g, b)) in sections.iter().copied().enumerate() {
        running_g += g;
        running_b += b;
        let error = (shunt.g - running_g).abs() + (shunt.b - running_b).abs();
        if error < best_error {
            best = index + 1;
            best_error = error;
        }
    }
    (best, best_error)
}

fn linear_shunt_sections(sections: &[(f64, f64)]) -> bool {
    let Some(&(first_g, first_b)) = sections.first() else {
        return true;
    };
    sections.iter().skip(1).all(|&(g, b)| {
        let scale = 1.0 + first_g.abs() + first_b.abs() + g.abs() + b.abs();
        (g - first_g).abs() + (b - first_b).abs() <= 1e-12 * scale
    })
}

fn dc_terminal_mrid(owner: &str, sequence: u32) -> String {
    det_mrid("dc_terminal", &format!("{owner}:{sequence}"))
}

fn dc_unit_mrid(
    detailed: &DetailedConnectivity,
    component: Option<&ComponentId>,
    owner: &ComponentId,
) -> Result<String> {
    let component = component
        .ok_or_else(|| emission_error(format!("{owner} has no DCConverterUnit containment")))?;
    if !detailed
        .dc_converter_units
        .iter()
        .any(|unit| unit.component == *component)
    {
        return Err(emission_error(format!(
            "{owner} references unknown DCConverterUnit {component}"
        )));
    }
    Ok(component_mrid(detailed, component))
}

fn dc_topological_node_mrid(
    detailed: &DetailedConnectivity,
    component: Option<&ComponentId>,
    owner: &ComponentId,
) -> Result<String> {
    let component = component
        .ok_or_else(|| emission_error(format!("{owner} has no DCTopologicalNode reference")))?;
    if !detailed
        .dc_topological_nodes
        .iter()
        .any(|node| node.component == *component)
    {
        return Err(emission_error(format!(
            "{owner} references unknown DCTopologicalNode {component}"
        )));
    }
    Ok(component_mrid(detailed, component))
}

fn write_equipment_container(
    eq: &mut Doc,
    detailed: &DetailedConnectivity,
    container: Option<&ComponentId>,
) {
    if let Some(container) = container {
        eq.reference(
            "Equipment.EquipmentContainer",
            &component_mrid(detailed, container),
        );
    }
}

fn write_required_equipment_container(
    eq: &mut Doc,
    detailed: &DetailedConnectivity,
    container: Option<&ComponentId>,
    owner: &ComponentId,
) -> Result<()> {
    let container = container.ok_or_else(|| {
        emission_error(format!(
            "{owner} has no required Equipment.EquipmentContainer"
        ))
    })?;
    eq.reference(
        "Equipment.EquipmentContainer",
        &component_mrid(detailed, container),
    );
    Ok(())
}

fn write_optional_number(doc: &mut Doc, property: &str, value: Option<f64>) {
    if let Some(value) = value {
        doc.text(property, value);
    }
}

fn required_number(value: Option<f64>, component: &ComponentId, property: &str) -> Result<f64> {
    value.ok_or_else(|| {
        emission_error(format!(
            "{component} has no value for required CGMES property {property}"
        ))
    })
}

#[allow(clippy::too_many_arguments)]
fn write_dc_terminal(
    eq: &mut Doc,
    tp: &mut Doc,
    ssh: &mut Doc,
    detailed: &DetailedConnectivity,
    owner: &str,
    fallback_sequence: u32,
    terminal: &DcTerminal,
    converter: bool,
    version: CgmesVersion,
) -> Result<()> {
    let class = if converter {
        "ACDCConverterDCTerminal"
    } else {
        "DCTerminal"
    };
    let equipment_property = if converter {
        "ACDCConverterDCTerminal.DCConductingEquipment"
    } else {
        "DCTerminal.DCConductingEquipment"
    };
    let sequence = terminal.sequence_number.ok_or_else(|| {
        emission_error(format!(
            "DC terminal {owner}:{fallback_sequence} has no sequence number"
        ))
    })?;
    let id = terminal.component.as_ref().map_or_else(
        || dc_terminal_mrid(owner, sequence),
        |component| component_mrid(detailed, component),
    );
    let node = terminal.dc_node.as_ref().ok_or_else(|| {
        emission_error(format!("DC terminal {id} has no physical DCNode reference"))
    })?;
    let topological = dc_topological_node_mrid(
        detailed,
        terminal.dc_topological_node.as_ref(),
        terminal.component.as_ref().unwrap_or(node),
    )?;
    let connected = terminal
        .connected
        .ok_or_else(|| emission_error(format!("DC terminal {id} has no connected value")))?;
    eq.named(class, &id, &format!("DC terminal {sequence}"));
    eq.reference(equipment_property, owner);
    eq.reference("DCBaseTerminal.DCNode", &component_mrid(detailed, node));
    eq.text("ACDCTerminal.sequenceNumber", sequence);
    if converter {
        if let Some(polarity) = terminal.polarity {
            eq.enumeration(
                "ACDCConverterDCTerminal.polarity",
                profiles(version).cim_ns,
                match polarity {
                    DcPolarity::Positive => "DCPolarityKind.positive",
                    DcPolarity::Middle => "DCPolarityKind.middle",
                    DcPolarity::Negative => "DCPolarityKind.negative",
                },
            );
        } else if version == CgmesVersion::V3_0 {
            return Err(emission_error(format!(
                "converter DC terminal {id} has no required CGMES 3 polarity"
            )));
        }
    }
    eq.close(class);
    tp.open(class, &id, true);
    tp.reference("DCBaseTerminal.DCTopologicalNode", &topological);
    tp.close(class);
    ssh.open(class, &id, true);
    ssh.text("ACDCTerminal.connected", connected);
    ssh.close(class);
    Ok(())
}

fn calculated_bus_for_terminal(
    detailed: &DetailedConnectivity,
    terminal: &Terminal,
) -> Option<BusId> {
    terminal
        .bus
        .as_ref()
        .or(terminal.connectable_bus.as_ref())
        .and_then(|bus| {
            detailed
                .bus_breaker_buses
                .iter()
                .find(|value| value.component == *bus)
                .and_then(|value| value.calculated_bus)
        })
        .or_else(|| {
            terminal
                .node
                .as_ref()
                .and_then(|node| calculated_bus_for_node(detailed, node))
        })
}

fn calculated_bus_for_node(detailed: &DetailedConnectivity, node: &ComponentId) -> Option<BusId> {
    detailed
        .connectivity_nodes
        .iter()
        .find(|value| value.component == *node)
        .and_then(|value| value.calculated_bus)
        .or_else(|| {
            detailed
                .calculated_buses
                .iter()
                .find(|calculated| calculated.nodes.contains(node))
                .map(|calculated| calculated.calculated_bus)
        })
}

#[allow(clippy::too_many_arguments)]
fn write_converter_ac_terminals(
    network: &BalancedNetwork,
    detailed: &DetailedConnectivity,
    eq: &mut Doc,
    tp: &mut Doc,
    ssh: &mut Doc,
    sv: &mut Doc,
    converter: &ComponentId,
    owner: &str,
    warnings: &mut CgmesDiagnostics,
) {
    let project_mixed_topology = project_mixed_topology(detailed);
    let records = detailed
        .terminals
        .iter()
        .filter(|terminal| terminal.equipment == *converter)
        .collect::<Vec<_>>();
    if records.is_empty() {
        warnings.push_as(&codes::EMIT_CGMES.reference_missing, format!(
            "converter `{converter}` has no AC Terminal record; its DC equipment was emitted without an AC connection"
        ));
    }
    for record in records {
        let Some(bus) = calculated_bus_for_terminal(detailed, record) else {
            warnings.push(format!(
                "converter `{converter}` terminal {} has no calculated AC bus and was not emitted",
                record.terminal
            ));
            continue;
        };
        let id = terminal_mrid(
            Some(detailed),
            Some(record),
            owner,
            usize::from(record.terminal),
        );
        eq.named("Terminal", &id, &format!("AC terminal {}", record.terminal));
        eq.reference("Terminal.ConductingEquipment", owner);
        eq.text("ACDCTerminal.sequenceNumber", record.terminal);
        if terminal_uses_connectivity_node(detailed, record, project_mixed_topology) {
            eq.reference(
                "Terminal.ConnectivityNode",
                &connectivity_node_mrid(Some(detailed), Some(record), bus, project_mixed_topology),
            );
        }
        eq.close("Terminal");
        if record.connected {
            tp.open("Terminal", &id, true);
            tp.reference(
                "Terminal.TopologicalNode",
                &terminal_topological_node_mrid(network, Some(detailed), Some(record), bus),
            );
            tp.close("Terminal");
        }
        ssh.open("Terminal", &id, true);
        ssh.text("ACDCTerminal.connected", record.connected);
        ssh.close("Terminal");
        write_sv_power_flow(
            sv,
            &id,
            record.active_power_mw,
            record.reactive_power_mvar,
            warnings,
        );
    }
}

#[derive(Clone, Copy)]
struct CapabilityCurveWriteContext<'a> {
    detailed: &'a DetailedConnectivity,
    component: &'a ComponentId,
    owner_mrid: &'a str,
    class: &'static str,
    curve_id_kind: &'static str,
    point_id_kind: &'static str,
    fallback_name: &'static str,
    curve: &'a ReactiveCapabilityCurve,
    cim_ns: &'a str,
}

fn write_reactive_capability_curve(
    eq: &mut Doc,
    context: CapabilityCurveWriteContext<'_>,
    warnings: &mut CgmesDiagnostics,
) -> Option<String> {
    let CapabilityCurveWriteContext {
        detailed,
        component,
        owner_mrid,
        class,
        curve_id_kind,
        point_id_kind,
        fallback_name,
        curve,
        cim_ns,
    } = context;
    if curve.points.is_empty() {
        warnings.push_as(
            &codes::EMIT_CGMES.record_dropped,
            format!(
                "{class} `{component}` has an empty reactive capability curve and was not emitted"
            ),
        );
        return None;
    }
    if !curve.properties.is_empty()
        || curve
            .points
            .iter()
            .any(|point| !point.properties.is_empty())
    {
        warnings.push_as(
            &codes::EMIT_CGMES.field_dropped,
            format!(
                "{class} `{component}`: reactive capability curve properties have no CGMES field"
            ),
        );
    }
    let curve_id = det_mrid(curve_id_kind, owner_mrid);
    eq.named(
        class,
        &curve_id,
        component_name(detailed, component, fallback_name),
    );
    eq.enumeration(
        "Curve.curveStyle",
        cim_ns,
        match curve.curve_style {
            CurveStyle::ConstantYValue => "CurveStyle.constantYValue",
            CurveStyle::StraightLineYValues => "CurveStyle.straightLineYValues",
        },
    );
    eq.enumeration("Curve.xUnit", cim_ns, "UnitSymbol.W");
    eq.enumeration("Curve.y1Unit", cim_ns, "UnitSymbol.VAr");
    eq.enumeration("Curve.y2Unit", cim_ns, "UnitSymbol.VAr");
    eq.close(class);
    for (index, point) in curve.points.iter().enumerate() {
        let id = det_mrid(point_id_kind, &format!("{curve_id}:{index}"));
        eq.open("CurveData", &id, false);
        eq.reference("CurveData.Curve", &curve_id);
        eq.text("CurveData.xvalue", point.active_power_mw);
        eq.text("CurveData.y1value", point.minimum_reactive_power_mvar);
        eq.text("CurveData.y2value", point.maximum_reactive_power_mvar);
        eq.close("CurveData");
    }
    Some(curve_id)
}

fn retained_equipment_reactive_limits<'a>(
    detailed: &'a DetailedConnectivity,
    component: &ComponentId,
) -> Result<Option<&'a ReactiveLimits>> {
    let mut matches = detailed
        .equipment_reactive_limits
        .iter()
        .filter(|record| record.equipment == *component);
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(emission_error(format!(
            "equipment `{component}` has more than one reactive limits record"
        )));
    }
    Ok(Some(&first.limits))
}

fn warn_min_max_reactive_limit_properties(
    component: &ComponentId,
    limits: &crate::network::MinMaxReactiveLimits,
    warnings: &mut CgmesDiagnostics,
) {
    if !limits.properties.is_empty() {
        warnings.push_as(
            &codes::EMIT_CGMES.field_dropped,
            format!(
                "equipment `{component}`: min/max reactive limits properties have no CGMES field"
            ),
        );
    }
}

fn write_vsc_capability_curve(
    eq: &mut Doc,
    detailed: &DetailedConnectivity,
    converter: &VoltageSourceConverter,
    converter_mrid: &str,
    cim_ns: &str,
    warnings: &mut CgmesDiagnostics,
) -> Result<Option<String>> {
    let retained = retained_equipment_reactive_limits(detailed, &converter.component)?;
    let limits = match (converter.reactive_limits.as_ref(), retained) {
        (Some(direct), Some(generic)) if direct != generic => {
            return Err(emission_error(format!(
                "VsConverter `{}` has conflicting direct and equipment reactive limits records",
                converter.component
            )));
        }
        (Some(direct), _) => Some(direct),
        (None, generic) => generic,
    };
    let Some(limits) = limits else {
        return Ok(None);
    };
    let synthesized;
    let curve = match limits {
        ReactiveLimits::CapabilityCurve(value) => value,
        ReactiveLimits::MinMax(value) => {
            warn_min_max_reactive_limit_properties(&converter.component, value, warnings);
            synthesized = ReactiveCapabilityCurve {
                curve_style: CurveStyle::ConstantYValue,
                properties: std::collections::BTreeMap::default(),
                points: vec![crate::network::ReactiveCapabilityCurvePoint {
                    active_power_mw: 0.0,
                    minimum_reactive_power_mvar: value.minimum_reactive_power_mvar,
                    maximum_reactive_power_mvar: value.maximum_reactive_power_mvar,
                    properties: std::collections::BTreeMap::default(),
                }],
            };
            &synthesized
        }
    };
    Ok(write_reactive_capability_curve(
        eq,
        CapabilityCurveWriteContext {
            detailed,
            component: &converter.component,
            owner_mrid: converter_mrid,
            class: "VsCapabilityCurve",
            curve_id_kind: "vsc_capability_curve",
            point_id_kind: "vsc_capability_point",
            fallback_name: "VSC capability curve",
            curve,
            cim_ns,
        },
        warnings,
    ))
}

#[allow(clippy::too_many_arguments)]
fn write_synchronous_machine_reactive_limits(
    eq: &mut Doc,
    detailed: &DetailedConnectivity,
    component: &ComponentId,
    machine_mrid: &str,
    cim_ns: &str,
    active_power_mw: f64,
    row_minimum_mvar: f64,
    row_maximum_mvar: f64,
    warnings: &mut CgmesDiagnostics,
) -> Result<(f64, f64, Option<String>)> {
    let Some(limits) = retained_equipment_reactive_limits(detailed, component)? else {
        return Ok((row_minimum_mvar, row_maximum_mvar, None));
    };
    let (minimum_mvar, maximum_mvar) = calc_reactive_limits_at_active_power(
        &format!("generator `{component}`"),
        limits,
        active_power_mw,
    )
    .map_err(emission_error)?;
    let tolerance = 1e-9
        * minimum_mvar
            .abs()
            .max(maximum_mvar.abs())
            .max(row_minimum_mvar.abs())
            .max(row_maximum_mvar.abs())
            .max(1.0);
    if (minimum_mvar - row_minimum_mvar).abs() > tolerance
        || (maximum_mvar - row_maximum_mvar).abs() > tolerance
    {
        warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
            "generator `{component}`: typed reactive limits evaluate to minQ={minimum_mvar} MVAr and maxQ={maximum_mvar} MVAr at p={active_power_mw} MW, while the balanced generator row contains minQ={row_minimum_mvar} MVAr and maxQ={row_maximum_mvar} MVAr; fresh CGMES uses the typed limits"
        ));
    }
    let curve = match limits {
        ReactiveLimits::MinMax(limits) => {
            warn_min_max_reactive_limit_properties(component, limits, warnings);
            None
        }
        ReactiveLimits::CapabilityCurve(curve) => write_reactive_capability_curve(
            eq,
            CapabilityCurveWriteContext {
                detailed,
                component,
                owner_mrid: machine_mrid,
                class: "ReactiveCapabilityCurve",
                curve_id_kind: "reactive_capability_curve",
                point_id_kind: "reactive_capability_point",
                fallback_name: "generator reactive capability curve",
                curve,
                cim_ns,
            },
            warnings,
        ),
    };
    Ok((minimum_mvar, maximum_mvar, curve))
}

fn write_vsc_sv(
    writer: &mut Writer<'_>,
    sv: &mut Doc,
    id: &str,
    converter: &VoltageSourceConverter,
) {
    let valve_voltage = if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
        converter.uv_kv.or(converter.uf_kv)
    } else {
        converter.uf_kv.or(converter.uv_kv)
    };
    let missing = [
        (
            "ACDCConverter.poleLossP",
            converter.pole_loss_active_power_mw.is_none(),
        ),
        ("ACDCConverter.idc", converter.dc_current_a.is_none()),
        ("ACDCConverter.uc", converter.ac_voltage_kv.is_none()),
        ("ACDCConverter.udc", converter.dc_voltage_kv.is_none()),
        ("VsConverter.delta", converter.delta_degrees.is_none()),
        (
            if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
                "VsConverter.uv"
            } else {
                "VsConverter.uf"
            },
            valve_voltage.is_none(),
        ),
    ]
    .into_iter()
    .filter_map(|(name, absent)| absent.then_some(name))
    .collect::<Vec<_>>();
    if !missing.is_empty() {
        writer.warnings.push(format!(
            "VsConverter `{}`: converter SV object omitted because {} is absent",
            converter.component,
            missing.join(", ")
        ));
        return;
    }
    sv.open("VsConverter", id, true);
    sv.text(
        "ACDCConverter.poleLossP",
        converter.pole_loss_active_power_mw.unwrap(),
    );
    sv.text("ACDCConverter.idc", converter.dc_current_a.unwrap());
    sv.text("ACDCConverter.uc", converter.ac_voltage_kv.unwrap());
    sv.text("ACDCConverter.udc", converter.dc_voltage_kv.unwrap());
    sv.text("VsConverter.delta", converter.delta_degrees.unwrap());
    sv.text(
        if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
            "VsConverter.uv"
        } else {
            "VsConverter.uf"
        },
        valve_voltage.unwrap(),
    );
    sv.close("VsConverter");
}

fn write_lcc_sv(
    writer: &mut Writer<'_>,
    sv: &mut Doc,
    id: &str,
    converter: &LineCommutatedConverter,
) {
    let missing = [
        (
            "ACDCConverter.poleLossP",
            converter.pole_loss_active_power_mw.is_none(),
        ),
        ("ACDCConverter.idc", converter.dc_current_a.is_none()),
        ("ACDCConverter.uc", converter.ac_voltage_kv.is_none()),
        ("ACDCConverter.udc", converter.dc_voltage_kv.is_none()),
        ("CsConverter.alpha", converter.alpha_degrees.is_none()),
        ("CsConverter.gamma", converter.gamma_degrees.is_none()),
    ]
    .into_iter()
    .filter_map(|(name, absent)| absent.then_some(name))
    .collect::<Vec<_>>();
    if !missing.is_empty() {
        writer.warnings.push(format!(
            "CsConverter `{}`: converter SV object omitted because {} is absent",
            converter.component,
            missing.join(", ")
        ));
        return;
    }
    sv.open("CsConverter", id, true);
    sv.text(
        "ACDCConverter.poleLossP",
        converter.pole_loss_active_power_mw.unwrap(),
    );
    sv.text("ACDCConverter.idc", converter.dc_current_a.unwrap());
    sv.text("ACDCConverter.uc", converter.ac_voltage_kv.unwrap());
    sv.text("ACDCConverter.udc", converter.dc_voltage_kv.unwrap());
    sv.text("CsConverter.alpha", converter.alpha_degrees.unwrap());
    sv.text("CsConverter.gamma", converter.gamma_degrees.unwrap());
    sv.close("CsConverter");
}

#[allow(clippy::too_many_arguments)]
fn write_common_converter_eq(
    eq: &mut Doc,
    component: &ComponentId,
    base_s: Option<f64>,
    minimum_p: Option<f64>,
    maximum_p: Option<f64>,
    minimum_udc: Option<f64>,
    maximum_udc: Option<f64>,
    rated_udc: Option<f64>,
    valve_u0: Option<f64>,
    number_of_valves: Option<u32>,
    idle_loss: Option<f64>,
    switching_loss: Option<f64>,
    resistive_loss: Option<f64>,
) -> Result<()> {
    write_optional_number(eq, "ACDCConverter.baseS", base_s);
    write_optional_number(eq, "ACDCConverter.minP", minimum_p);
    write_optional_number(eq, "ACDCConverter.maxP", maximum_p);
    write_optional_number(eq, "ACDCConverter.minUdc", minimum_udc);
    write_optional_number(eq, "ACDCConverter.maxUdc", maximum_udc);
    eq.text(
        "ACDCConverter.ratedUdc",
        required_number(rated_udc, component, "ACDCConverter.ratedUdc")?,
    );
    write_optional_number(eq, "ACDCConverter.valveU0", valve_u0);
    if let Some(number_of_valves) = number_of_valves {
        eq.text("ACDCConverter.numberOfValves", number_of_valves);
    }
    write_optional_number(eq, "ACDCConverter.idleLoss", idle_loss);
    write_optional_number(eq, "ACDCConverter.switchingLoss", switching_loss);
    write_optional_number(eq, "ACDCConverter.resistiveLoss", resistive_loss);
    Ok(())
}

fn write_common_converter_ssh(
    ssh: &mut Doc,
    version: CgmesVersion,
    component: &ComponentId,
    active_power_at_pcc_mw: Option<f64>,
    reactive_power_at_pcc_mvar: Option<f64>,
    target_active_power_mw: Option<f64>,
    target_dc_voltage_kv: Option<f64>,
) -> Result<()> {
    ssh.text(
        "ACDCConverter.p",
        required_number(active_power_at_pcc_mw, component, "ACDCConverter.p")?,
    );
    ssh.text(
        "ACDCConverter.q",
        required_number(reactive_power_at_pcc_mvar, component, "ACDCConverter.q")?,
    );
    if version == CgmesVersion::V2_4_15 {
        ssh.text(
            "ACDCConverter.targetPpcc",
            required_number(
                target_active_power_mw,
                component,
                "ACDCConverter.targetPpcc",
            )?,
        );
        ssh.text(
            "ACDCConverter.targetUdc",
            required_number(target_dc_voltage_kv, component, "ACDCConverter.targetUdc")?,
        );
    } else {
        write_optional_number(ssh, "ACDCConverter.targetPpcc", target_active_power_mw);
        write_optional_number(ssh, "ACDCConverter.targetUdc", target_dc_voltage_kv);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn write_dc_equipment(
    writer: &mut Writer<'_>,
    detailed: &DetailedConnectivity,
    eq: &mut Doc,
    tp: &mut Doc,
    ssh: &mut Doc,
    sv: &mut Doc,
) -> Result<()> {
    let count = detailed.dc_converter_units.len()
        + detailed.dc_topological_nodes.len()
        + detailed.dc_nodes.len()
        + detailed.dc_grounds.len()
        + detailed.dc_busbars.len()
        + detailed.dc_lines.len()
        + detailed.dc_series_devices.len()
        + detailed.dc_switches.len()
        + detailed.voltage_source_converters.len()
        + detailed.line_commutated_converters.len();
    if count == 0 {
        return Ok(());
    }

    for unit in &detailed.dc_converter_units {
        let id = component_mrid(detailed, &unit.component);
        eq.named(
            "DCConverterUnit",
            &id,
            component_name(detailed, &unit.component, unit.component.local_id()),
        );
        eq.enumeration(
            "DCConverterUnit.operationMode",
            writer.p.cim_ns,
            match unit.operation_mode {
                DcConverterOperatingMode::Bipolar => "DCConverterOperatingModeKind.bipolar",
                DcConverterOperatingMode::MonopolarGroundReturn => {
                    "DCConverterOperatingModeKind.monopolarGroundReturn"
                }
                DcConverterOperatingMode::MonopolarMetallicReturn => {
                    "DCConverterOperatingModeKind.monopolarMetallicReturn"
                }
            },
        );
        if let Some(substation) = &unit.substation {
            eq.reference(
                "DCConverterUnit.Substation",
                &component_mrid(detailed, substation),
            );
        }
        eq.close("DCConverterUnit");
    }

    for node in &detailed.dc_topological_nodes {
        let id = component_mrid(detailed, &node.component);
        let unit = dc_unit_mrid(detailed, node.dc_converter_unit.as_ref(), &node.component)?;
        tp.named(
            "DCTopologicalNode",
            &id,
            component_name(detailed, &node.component, node.component.local_id()),
        );
        tp.reference("DCTopologicalNode.DCEquipmentContainer", &unit);
        tp.close("DCTopologicalNode");
    }

    for node in &detailed.dc_nodes {
        let id = component_mrid(detailed, &node.component);
        let unit = dc_unit_mrid(detailed, node.dc_converter_unit.as_ref(), &node.component)?;
        let topological =
            dc_topological_node_mrid(detailed, node.dc_topological_node.as_ref(), &node.component)?;
        eq.named(
            "DCNode",
            &id,
            component_name(detailed, &node.component, node.component.local_id()),
        );
        eq.reference("DCNode.DCEquipmentContainer", &unit);
        eq.close("DCNode");
        tp.open("DCNode", &id, true);
        tp.reference("DCNode.DCTopologicalNode", &topological);
        tp.close("DCNode");
    }

    let mut dc_line_containers = Vec::<&ComponentId>::new();
    for line in &detailed.dc_lines {
        if let Some(container) = line.equipment_container.as_ref()
            && container.component_type() == "dc_line_container"
            && !dc_line_containers.contains(&container)
        {
            dc_line_containers.push(container);
        }
    }
    for container in dc_line_containers {
        let id = component_mrid(detailed, container);
        eq.named(
            "DCLine",
            &id,
            component_name(detailed, container, container.local_id()),
        );
        eq.close("DCLine");
    }

    for ground in &detailed.dc_grounds {
        let id = component_mrid(detailed, &ground.component);
        eq.named(
            "DCGround",
            &id,
            component_name(detailed, &ground.component, ground.component.local_id()),
        );
        write_equipment_container(eq, detailed, ground.equipment_container.as_ref());
        if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
            eq.text(
                "DCConductingEquipment.ratedUdc",
                required_number(
                    ground.rated_dc_voltage_kv,
                    &ground.component,
                    "DCConductingEquipment.ratedUdc",
                )?,
            );
        }
        write_optional_number(eq, "DCGround.r", ground.resistance_ohm);
        write_optional_number(eq, "DCGround.inductance", ground.inductance_h);
        eq.close("DCGround");
        write_dc_terminal(
            eq,
            tp,
            ssh,
            detailed,
            &id,
            1,
            &ground.dc_terminal,
            false,
            if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
                CgmesVersion::V3_0
            } else {
                CgmesVersion::V2_4_15
            },
        )?;
        if ground.dc_terminal.active_power_mw.is_some() || ground.dc_terminal.current_a.is_some() {
            writer.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "DCGround `{}`: CGMES DCTerminal has no active power or current field",
                    ground.component
                ),
            );
        }
    }
    for busbar in &detailed.dc_busbars {
        let id = component_mrid(detailed, &busbar.component);
        eq.named(
            "DCBusbar",
            &id,
            component_name(detailed, &busbar.component, busbar.component.local_id()),
        );
        write_required_equipment_container(
            eq,
            detailed,
            busbar.equipment_container.as_ref(),
            &busbar.component,
        )?;
        if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
            eq.text(
                "DCConductingEquipment.ratedUdc",
                required_number(
                    busbar.rated_dc_voltage_kv,
                    &busbar.component,
                    "DCConductingEquipment.ratedUdc",
                )?,
            );
        } else if busbar.rated_dc_voltage_kv.is_some() {
            writer.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "DCBusbar `{}`: rated DC voltage has no CGMES 2.4.15 field",
                    busbar.component
                ),
            );
        }
        eq.close("DCBusbar");
        write_dc_terminal(
            eq,
            tp,
            ssh,
            detailed,
            &id,
            1,
            &busbar.dc_terminal,
            false,
            if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
                CgmesVersion::V3_0
            } else {
                CgmesVersion::V2_4_15
            },
        )?;
        if busbar.dc_terminal.active_power_mw.is_some() || busbar.dc_terminal.current_a.is_some() {
            writer.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "DCBusbar `{}`: CGMES DCTerminal has no active power or current field",
                    busbar.component
                ),
            );
        }
    }
    for line in &detailed.dc_lines {
        let id = component_mrid(detailed, &line.component);
        eq.named(
            "DCLineSegment",
            &id,
            component_name(detailed, &line.component, line.component.local_id()),
        );
        write_required_equipment_container(
            eq,
            detailed,
            line.equipment_container.as_ref(),
            &line.component,
        )?;
        if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
            eq.text(
                "DCConductingEquipment.ratedUdc",
                required_number(
                    line.rated_dc_voltage_kv,
                    &line.component,
                    "DCConductingEquipment.ratedUdc",
                )?,
            );
            eq.text(
                "DCLineSegment.resistance",
                required_number(
                    line.resistance_ohm,
                    &line.component,
                    "DCLineSegment.resistance",
                )?,
            );
            eq.text(
                "DCLineSegment.inductance",
                required_number(
                    line.inductance_h,
                    &line.component,
                    "DCLineSegment.inductance",
                )?,
            );
            eq.text(
                "DCLineSegment.capacitance",
                required_number(
                    line.capacitance_f,
                    &line.component,
                    "DCLineSegment.capacitance",
                )?,
            );
        } else {
            write_optional_number(eq, "DCLineSegment.resistance", line.resistance_ohm);
            write_optional_number(eq, "DCLineSegment.inductance", line.inductance_h);
            write_optional_number(eq, "DCLineSegment.capacitance", line.capacitance_f);
        }
        write_optional_number(eq, "DCLineSegment.length", line.length_km);
        eq.close("DCLineSegment");
        let version = if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
            CgmesVersion::V3_0
        } else {
            CgmesVersion::V2_4_15
        };
        write_dc_terminal(
            eq,
            tp,
            ssh,
            detailed,
            &id,
            1,
            &line.dc_terminal1,
            false,
            version,
        )?;
        write_dc_terminal(
            eq,
            tp,
            ssh,
            detailed,
            &id,
            2,
            &line.dc_terminal2,
            false,
            version,
        )?;
        if line.dc_terminal1.active_power_mw.is_some()
            || line.dc_terminal2.active_power_mw.is_some()
            || line.dc_terminal1.current_a.is_some()
            || line.dc_terminal2.current_a.is_some()
        {
            writer.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "DCLineSegment `{}`: CGMES DCTerminal has no active power or current field",
                    line.component
                ),
            );
        }
    }
    for device in &detailed.dc_series_devices {
        let id = component_mrid(detailed, &device.component);
        eq.named(
            "DCSeriesDevice",
            &id,
            component_name(detailed, &device.component, device.component.local_id()),
        );
        write_required_equipment_container(
            eq,
            detailed,
            device.equipment_container.as_ref(),
            &device.component,
        )?;
        eq.text(
            if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
                "DCConductingEquipment.ratedUdc"
            } else {
                "DCSeriesDevice.ratedUdc"
            },
            required_number(
                device.rated_dc_voltage_kv,
                &device.component,
                if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
                    "DCConductingEquipment.ratedUdc"
                } else {
                    "DCSeriesDevice.ratedUdc"
                },
            )?,
        );
        eq.text(
            "DCSeriesDevice.resistance",
            required_number(
                device.resistance_ohm,
                &device.component,
                "DCSeriesDevice.resistance",
            )?,
        );
        eq.text(
            "DCSeriesDevice.inductance",
            required_number(
                device.inductance_h,
                &device.component,
                "DCSeriesDevice.inductance",
            )?,
        );
        eq.close("DCSeriesDevice");
        let version = if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
            CgmesVersion::V3_0
        } else {
            CgmesVersion::V2_4_15
        };
        write_dc_terminal(
            eq,
            tp,
            ssh,
            detailed,
            &id,
            1,
            &device.dc_terminal1,
            false,
            version,
        )?;
        write_dc_terminal(
            eq,
            tp,
            ssh,
            detailed,
            &id,
            2,
            &device.dc_terminal2,
            false,
            version,
        )?;
        if device.dc_terminal1.active_power_mw.is_some()
            || device.dc_terminal2.active_power_mw.is_some()
            || device.dc_terminal1.current_a.is_some()
            || device.dc_terminal2.current_a.is_some()
        {
            writer.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "DCSeriesDevice `{}`: CGMES DCTerminal has no active power or current field",
                    device.component
                ),
            );
        }
    }
    for switch in &detailed.dc_switches {
        let id = component_mrid(detailed, &switch.component);
        let class = match switch.kind {
            DcSwitchKind::Switch => "DCSwitch",
            DcSwitchKind::Breaker => "DCBreaker",
            DcSwitchKind::Disconnector => "DCDisconnector",
        };
        eq.named(
            class,
            &id,
            component_name(detailed, &switch.component, switch.component.local_id()),
        );
        write_equipment_container(eq, detailed, switch.equipment_container.as_ref());
        if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
            eq.text(
                "DCConductingEquipment.ratedUdc",
                required_number(
                    switch.rated_dc_voltage_kv,
                    &switch.component,
                    "DCConductingEquipment.ratedUdc",
                )?,
            );
        }
        eq.close(class);
        let version = if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
            CgmesVersion::V3_0
        } else {
            CgmesVersion::V2_4_15
        };
        write_dc_terminal(
            eq,
            tp,
            ssh,
            detailed,
            &id,
            1,
            &switch.dc_terminal1,
            false,
            version,
        )?;
        write_dc_terminal(
            eq,
            tp,
            ssh,
            detailed,
            &id,
            2,
            &switch.dc_terminal2,
            false,
            version,
        )?;
        if let Some(open) = switch.open {
            ssh.open(class, &id, true);
            ssh.text("Switch.open", open);
            ssh.close(class);
        }
        if switch.resistance_ohm.is_some_and(|value| value != 0.0) {
            writer.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "DC switch `{}`: resistance {} ohm has no CGMES DC switch field",
                    switch.component,
                    switch.resistance_ohm.unwrap()
                ),
            );
        }
    }

    for converter in &detailed.voltage_source_converters {
        let id = component_mrid(detailed, &converter.component);
        let curve = write_vsc_capability_curve(
            eq,
            detailed,
            converter,
            &id,
            writer.p.cim_ns,
            &mut writer.warnings,
        )?;
        eq.named(
            "VsConverter",
            &id,
            component_name(
                detailed,
                &converter.component,
                converter.component.local_id(),
            ),
        );
        eq.reference(
            "Equipment.EquipmentContainer",
            &dc_unit_mrid(
                detailed,
                converter.dc_converter_unit.as_ref(),
                &converter.component,
            )?,
        );
        write_common_converter_eq(
            eq,
            &converter.component,
            converter.base_apparent_power_mva,
            converter.minimum_active_power_mw,
            converter.maximum_active_power_mw,
            converter.minimum_dc_voltage_kv,
            converter.maximum_dc_voltage_kv,
            converter.rated_dc_voltage_kv,
            converter.valve_u0_kv,
            converter.number_of_valves,
            converter.idle_loss_mw,
            converter.switching_loss_mw_per_ampere,
            converter.resistive_loss_ohm,
        )?;
        write_optional_number(
            eq,
            "VsConverter.maxModulationIndex",
            converter.maximum_modulation_index,
        );
        write_optional_number(
            eq,
            "VsConverter.maxValveCurrent",
            converter.maximum_valve_current_a,
        );
        if let Some(curve) = curve {
            eq.reference("VsConverter.CapabilityCurve", &curve);
        }
        if let Some(reference) = converter.pcc_terminal.as_ref()
            && let Some(pcc) = terminal_reference_mrid(writer.net, Some(detailed), reference)
        {
            eq.reference("ACDCConverter.PccTerminal", &pcc);
        }
        eq.close("VsConverter");
        write_converter_ac_terminals(
            writer.net,
            detailed,
            eq,
            tp,
            ssh,
            sv,
            &converter.component,
            &id,
            &mut writer.warnings,
        );
        let version = if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
            CgmesVersion::V3_0
        } else {
            CgmesVersion::V2_4_15
        };
        write_dc_terminal(
            eq,
            tp,
            ssh,
            detailed,
            &id,
            1,
            &converter.dc_terminal1,
            true,
            version,
        )?;
        write_dc_terminal(
            eq,
            tp,
            ssh,
            detailed,
            &id,
            2,
            &converter.dc_terminal2,
            true,
            version,
        )?;
        let control_mode = converter.control_mode.ok_or_else(|| {
            emission_error(format!(
                "VsConverter {} has no pPcc control mode",
                converter.component
            ))
        })?;
        match control_mode {
            AcDcConverterControlMode::ActivePowerAtPcc => {
                required_number(
                    converter.target_active_power_mw,
                    &converter.component,
                    "ACDCConverter.targetPpcc",
                )?;
            }
            AcDcConverterControlMode::DcVoltage => {
                required_number(
                    converter.target_dc_voltage_kv,
                    &converter.component,
                    "ACDCConverter.targetUdc",
                )?;
            }
            AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroop
            | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation
            | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopPilot => {
                required_number(
                    converter.target_active_power_mw,
                    &converter.component,
                    "ACDCConverter.targetPpcc",
                )?;
                required_number(
                    converter.target_dc_voltage_kv,
                    &converter.component,
                    "ACDCConverter.targetUdc",
                )?;
                required_number(converter.droop, &converter.component, "VsConverter.droop")?;
                if control_mode
                    == AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation
                {
                    required_number(
                        converter.droop_compensation,
                        &converter.component,
                        "VsConverter.droopCompensation",
                    )?;
                }
            }
            AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopCurve => {
                return Err(emission_error(format!(
                    "VsConverter {} uses the XIIDM piecewise droop curve control, which has no CGMES VsPpccControlKind value",
                    converter.component
                )));
            }
            AcDcConverterControlMode::DcCurrent => {
                return Err(emission_error(format!(
                    "VsConverter {} cannot use the CsConverter dcCurrent control",
                    converter.component
                )));
            }
        }
        ssh.open("VsConverter", &id, true);
        write_common_converter_ssh(
            ssh,
            version,
            &converter.component,
            converter.active_power_at_pcc_mw,
            converter.reactive_power_at_pcc_mvar,
            converter.target_active_power_mw,
            converter.target_dc_voltage_kv,
        )?;
        ssh.enumeration(
            "VsConverter.pPccControl",
            writer.p.cim_ns,
            match control_mode {
                AcDcConverterControlMode::DcVoltage => "VsPpccControlKind.udc",
                AcDcConverterControlMode::ActivePowerAtPcc => "VsPpccControlKind.pPcc",
                AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroop => {
                    "VsPpccControlKind.pPccAndUdcDroop"
                }
                AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation => {
                    "VsPpccControlKind.pPccAndUdcDroopWithCompensation"
                }
                AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopPilot => {
                    "VsPpccControlKind.pPccAndUdcDroopPilot"
                }
                AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopCurve => {
                    unreachable!()
                }
                AcDcConverterControlMode::DcCurrent => unreachable!(),
            },
        );
        let voltage_regulator_on = converter.voltage_regulator_on.ok_or_else(|| {
            emission_error(format!(
                "VsConverter {} has no qPcc control mode",
                converter.component
            ))
        })?;
        if voltage_regulator_on {
            required_number(
                converter.voltage_setpoint_kv,
                &converter.component,
                "VsConverter.targetUpcc",
            )?;
        } else {
            required_number(
                converter.reactive_power_setpoint_mvar,
                &converter.component,
                "VsConverter.targetQpcc",
            )?;
        }
        ssh.enumeration(
            "VsConverter.qPccControl",
            writer.p.cim_ns,
            if voltage_regulator_on {
                "VsQpccControlKind.voltagePcc"
            } else {
                "VsQpccControlKind.reactivePcc"
            },
        );
        if version == CgmesVersion::V2_4_15 {
            ssh.text(
                "VsConverter.targetQpcc",
                required_number(
                    converter.reactive_power_setpoint_mvar,
                    &converter.component,
                    "VsConverter.targetQpcc",
                )?,
            );
            ssh.text(
                "VsConverter.targetUpcc",
                required_number(
                    converter.voltage_setpoint_kv,
                    &converter.component,
                    "VsConverter.targetUpcc",
                )?,
            );
            ssh.text(
                "VsConverter.droop",
                required_number(converter.droop, &converter.component, "VsConverter.droop")?,
            );
            ssh.text(
                "VsConverter.droopCompensation",
                required_number(
                    converter.droop_compensation,
                    &converter.component,
                    "VsConverter.droopCompensation",
                )?,
            );
            ssh.text(
                "VsConverter.qShare",
                required_number(
                    converter.q_share,
                    &converter.component,
                    "VsConverter.qShare",
                )?,
            );
        } else {
            write_optional_number(
                ssh,
                "VsConverter.targetQpcc",
                converter.reactive_power_setpoint_mvar,
            );
            write_optional_number(ssh, "VsConverter.targetUpcc", converter.voltage_setpoint_kv);
            write_optional_number(ssh, "VsConverter.droop", converter.droop);
            write_optional_number(
                ssh,
                "VsConverter.droopCompensation",
                converter.droop_compensation,
            );
            write_optional_number(ssh, "VsConverter.qShare", converter.q_share);
        }
        ssh.close("VsConverter");
        if converter.droop_curve.is_some() {
            writer.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "VsConverter `{}`: PowerIO's segmented droop curve is not the CGMES scalar droop and was not emitted",
                converter.component
            ));
        }
        write_vsc_sv(writer, sv, &id, converter);
        if converter.dc_terminal1.current_a.is_some()
            || converter.dc_terminal2.current_a.is_some()
            || converter.dc_terminal1.active_power_mw.is_some()
            || converter.dc_terminal2.active_power_mw.is_some()
        {
            writer.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "VsConverter `{}`: XIIDM DC terminal current and active power have no CGMES DCTerminal fields; converter dc_current_a owns ACDCConverter.idc",
                converter.component
            ));
        }
    }

    for converter in &detailed.line_commutated_converters {
        write_line_commutated_converter(writer, detailed, eq, tp, ssh, sv, converter)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn write_line_commutated_converter(
    writer: &mut Writer<'_>,
    detailed: &DetailedConnectivity,
    eq: &mut Doc,
    tp: &mut Doc,
    ssh: &mut Doc,
    sv: &mut Doc,
    converter: &LineCommutatedConverter,
) -> Result<()> {
    let id = component_mrid(detailed, &converter.component);
    eq.named(
        "CsConverter",
        &id,
        component_name(
            detailed,
            &converter.component,
            converter.component.local_id(),
        ),
    );
    eq.reference(
        "Equipment.EquipmentContainer",
        &dc_unit_mrid(
            detailed,
            converter.dc_converter_unit.as_ref(),
            &converter.component,
        )?,
    );
    write_common_converter_eq(
        eq,
        &converter.component,
        converter.base_apparent_power_mva,
        converter.minimum_active_power_mw,
        converter.maximum_active_power_mw,
        converter.minimum_dc_voltage_kv,
        converter.maximum_dc_voltage_kv,
        converter.rated_dc_voltage_kv,
        converter.valve_u0_kv,
        converter.number_of_valves,
        converter.idle_loss_mw,
        converter.switching_loss_mw_per_ampere,
        converter.resistive_loss_ohm,
    )?;
    write_optional_number(eq, "CsConverter.ratedIdc", converter.rated_dc_current_a);
    write_optional_number(eq, "CsConverter.minAlpha", converter.minimum_alpha_degrees);
    write_optional_number(eq, "CsConverter.maxAlpha", converter.maximum_alpha_degrees);
    write_optional_number(eq, "CsConverter.minGamma", converter.minimum_gamma_degrees);
    write_optional_number(eq, "CsConverter.maxGamma", converter.maximum_gamma_degrees);
    if let Some(reference) = converter.pcc_terminal.as_ref()
        && let Some(pcc) = terminal_reference_mrid(writer.net, Some(detailed), reference)
    {
        eq.reference("ACDCConverter.PccTerminal", &pcc);
    }
    eq.close("CsConverter");
    write_converter_ac_terminals(
        writer.net,
        detailed,
        eq,
        tp,
        ssh,
        sv,
        &converter.component,
        &id,
        &mut writer.warnings,
    );
    let version = if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
        CgmesVersion::V3_0
    } else {
        CgmesVersion::V2_4_15
    };
    write_dc_terminal(
        eq,
        tp,
        ssh,
        detailed,
        &id,
        1,
        &converter.dc_terminal1,
        true,
        version,
    )?;
    write_dc_terminal(
        eq,
        tp,
        ssh,
        detailed,
        &id,
        2,
        &converter.dc_terminal2,
        true,
        version,
    )?;
    let control_mode = converter.control_mode.ok_or_else(|| {
        emission_error(format!(
            "CsConverter {} has no pPcc control mode",
            converter.component
        ))
    })?;
    match control_mode {
        AcDcConverterControlMode::ActivePowerAtPcc => {
            required_number(
                converter.target_active_power_mw,
                &converter.component,
                "ACDCConverter.targetPpcc",
            )?;
        }
        AcDcConverterControlMode::DcVoltage => {
            required_number(
                converter.target_dc_voltage_kv,
                &converter.component,
                "ACDCConverter.targetUdc",
            )?;
        }
        AcDcConverterControlMode::DcCurrent => {
            required_number(
                converter.target_dc_current_a,
                &converter.component,
                "CsConverter.targetIdc",
            )?;
        }
        AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopCurve
        | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroop
        | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation
        | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopPilot => {
            return Err(emission_error(format!(
                "CsConverter {} has no CGMES droop control kind",
                converter.component
            )));
        }
    }
    ssh.open("CsConverter", &id, true);
    write_common_converter_ssh(
        ssh,
        version,
        &converter.component,
        converter.active_power_at_pcc_mw,
        converter.reactive_power_at_pcc_mvar,
        converter.target_active_power_mw,
        converter.target_dc_voltage_kv,
    )?;
    ssh.enumeration(
        "CsConverter.pPccControl",
        writer.p.cim_ns,
        match control_mode {
            AcDcConverterControlMode::DcVoltage => "CsPpccControlKind.dcVoltage",
            AcDcConverterControlMode::DcCurrent => "CsPpccControlKind.dcCurrent",
            AcDcConverterControlMode::ActivePowerAtPcc => "CsPpccControlKind.activePower",
            AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopCurve
            | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroop
            | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation
            | AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopPilot => unreachable!(),
        },
    );
    let operating_mode = converter.operating_mode.ok_or_else(|| {
        emission_error(format!(
            "CsConverter {} has no rectifier/inverter operating mode",
            converter.component
        ))
    })?;
    ssh.enumeration(
        "CsConverter.operatingMode",
        writer.p.cim_ns,
        match operating_mode {
            LineCommutatedConverterOperatingMode::Rectifier => "CsOperatingModeKind.rectifier",
            LineCommutatedConverterOperatingMode::Inverter => "CsOperatingModeKind.inverter",
        },
    );
    if version == CgmesVersion::V2_4_15 {
        ssh.text(
            "CsConverter.targetAlpha",
            required_number(
                converter.target_alpha_degrees,
                &converter.component,
                "CsConverter.targetAlpha",
            )?,
        );
        ssh.text(
            "CsConverter.targetGamma",
            required_number(
                converter.target_gamma_degrees,
                &converter.component,
                "CsConverter.targetGamma",
            )?,
        );
        ssh.text(
            "CsConverter.targetIdc",
            required_number(
                converter.target_dc_current_a,
                &converter.component,
                "CsConverter.targetIdc",
            )?,
        );
    } else {
        write_optional_number(
            ssh,
            "CsConverter.targetAlpha",
            converter.target_alpha_degrees,
        );
        write_optional_number(
            ssh,
            "CsConverter.targetGamma",
            converter.target_gamma_degrees,
        );
        write_optional_number(ssh, "CsConverter.targetIdc", converter.target_dc_current_a);
    }
    ssh.close("CsConverter");
    if converter.reactive_model.is_some() || converter.power_factor.is_some() {
        writer.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
            "CsConverter `{}`: reactive model and power factor have no direct CGMES field; PCC p/q carries the available operating assignment",
            converter.component
        ));
    }
    if converter.droop_curve.is_some() {
        writer.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
            "CsConverter `{}`: PowerIO's segmented droop curve has no CGMES CsConverter field and was not emitted",
            converter.component
        ));
    }
    write_lcc_sv(writer, sv, &id, converter);
    if converter.dc_terminal1.current_a.is_some()
        || converter.dc_terminal2.current_a.is_some()
        || converter.dc_terminal1.active_power_mw.is_some()
        || converter.dc_terminal2.active_power_mw.is_some()
    {
        writer.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
            "CsConverter `{}`: XIIDM DC terminal current and active power have no CGMES DCTerminal fields; converter dc_current_a owns ACDCConverter.idc",
            converter.component
        ));
    }
    Ok(())
}

/// Serialize `net` as a CGMES EQ/TP/SSH/SV file set at `version`.
///
/// Every model field the profile set cannot carry is reported in the
/// warnings; nothing drops silently.
#[allow(clippy::too_many_lines)] // the four profiles emit in one ordered pass
#[allow(clippy::if_not_else)] // the line arm reads first, matching the reader
pub fn write_cgmes(net: &BalancedNetwork, version: CgmesVersion) -> Result<CgmesFiles> {
    net.validate().map_err(|error| {
        emission_error(format!(
            "network validation failed before CGMES emission: {error}"
        ))
    })?;
    if let Some(bus) = net
        .buses()
        .iter()
        .find(|bus| !bus.base_kv.is_finite() || bus.base_kv <= 0.0)
    {
        return Err(emission_error(format!(
            "bus {} has nonpositive or nonfinite base_kv {}; CGMES emission requires an exact positive voltage base",
            bus.id, bus.base_kv
        )));
    }
    if let Some(level) = net.detailed_connectivity().as_deref().and_then(|detailed| {
        detailed
            .voltage_levels
            .iter()
            .find(|level| !level.nominal_kv.is_finite() || level.nominal_kv <= 0.0)
    }) {
        return Err(emission_error(format!(
            "VoltageLevel {} has nonpositive or nonfinite nominal_kv {}; CGMES emission requires an exact positive voltage base",
            level.component, level.nominal_kv
        )));
    }
    let p = profiles(version);
    let v3 = version == CgmesVersion::V3_0;
    let mut w = Writer {
        net,
        p,
        warnings: CgmesDiagnostics::new(&codes::EMIT_CGMES.record_dropped),
    };
    let detailed = net.detailed_connectivity().as_deref();
    let retained_metadata = retained_identified_metadata(detailed, &mut w.warnings);
    if let Some(detailed) = detailed {
        warn_unemitted_detailed_fields(net, detailed, version, &mut w.warnings);
    } else {
        for field in case_metadata_fields(net.case_metadata(), false) {
            w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                "network case metadata `{field}` has no field in the fresh CGMES EQ, TP, SSH, or SV FullModel headers"
            ));
        }
    }
    let mut eq = Doc::new(Arc::clone(&retained_metadata));
    let mut tp = Doc::new(Arc::clone(&retained_metadata));
    let mut ssh = Doc::new(Arc::clone(&retained_metadata));
    let mut sv = Doc::new(Arc::clone(&retained_metadata));

    if (net.base_mva() - 100.0).abs() > 1e-9 {
        w.warnings.push_as(
            &codes::EMIT_CGMES.value_collapsed,
            format!(
                "system base {} MVA: CGMES carries no MVA base, so a reparse lands \
             per-unit values on 100 MVA",
                net.base_mva()
            ),
        );
    }
    let project_mixed_topology = detailed.is_some_and(project_mixed_topology);
    let use_detailed_topology = detailed.is_some_and(|value| {
        !value.substations.is_empty()
            || !value.voltage_levels.is_empty()
            || !value.connectivity_nodes.is_empty()
            || !value.bus_breaker_buses.is_empty()
    });
    let mut active_connectivity_nodes = Vec::new();
    let mut active_bus_breaker_buses = Vec::new();
    let mut active_calculated_buses = Vec::new();
    let mut active_source_lines = Vec::<ComponentId>::new();
    let mut terminal_generated_connectivity_nodes = HashSet::<ComponentId>::new();
    let mut represented_calculated_buses = HashSet::<BusId>::new();
    if let Some(detailed) = detailed {
        if project_mixed_topology {
            let projected_levels = detailed
                .voltage_levels
                .iter()
                .filter(|level| level.topology_kind == TopologyKind::BusBreaker)
                .count();
            w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                "source detailed connectivity contains both node breaker and bus breaker VoltageLevels; fresh CGMES emission promotes {projected_levels} bus breaker VoltageLevel(s) to node breaker connectivity by adding one ConnectivityNode per TopologicalNode because PowSybl imports one topology mode per CGMES profile set; PowerIO's typed voltage level topology is unchanged"
            ));
            warn_pow_sybl_projected_transformer_connections(net, detailed, &mut w.warnings);
        }
        for container in detailed
            .component_metadata
            .iter()
            .filter_map(|metadata| metadata.equipment_container.as_ref())
            .filter(|container| container.component_type() == "line")
        {
            if !active_source_lines.contains(container) {
                active_source_lines.push(container.clone());
            }
        }
        let retained_terminals = detailed
            .terminals
            .iter()
            .filter(|terminal| terminal.equipment.component_type() != "cgmes_object")
            .collect::<Vec<_>>();
        terminal_generated_connectivity_nodes.extend(
            retained_terminals
                .iter()
                .filter(|terminal| {
                    let topology = terminal_voltage_level_topology(detailed, terminal);
                    let projected_bus_breaker =
                        project_mixed_topology && topology == Some(TopologyKind::BusBreaker);
                    (terminal.node.is_none() || projected_bus_breaker)
                        && terminal_uses_connectivity_node(
                            detailed,
                            terminal,
                            project_mixed_topology,
                        )
                })
                .filter_map(|terminal| terminal.connectable_bus.as_ref().or(terminal.bus.as_ref()))
                .cloned(),
        );
        let retained_terminal_nodes = retained_terminals
            .iter()
            .filter_map(|terminal| terminal.node.as_ref())
            .collect::<HashSet<_>>();
        let retained_terminal_buses = retained_terminals
            .iter()
            .flat_map(|terminal| [terminal.bus.as_ref(), terminal.connectable_bus.as_ref()])
            .flatten()
            .collect::<HashSet<_>>();
        let retained_switch_nodes = detailed
            .switches
            .iter()
            .flat_map(|switch| [&switch.endpoint1, &switch.endpoint2])
            .filter_map(|endpoint| match endpoint {
                TopologyEndpoint::Node(node) => Some(node),
                TopologyEndpoint::Bus(_) => None,
            })
            .collect::<HashSet<_>>();
        let retained_busbar_nodes = detailed
            .busbar_sections
            .iter()
            .map(|busbar| &busbar.node)
            .collect::<HashSet<_>>();
        let retained_calculated_nodes = detailed
            .calculated_buses
            .iter()
            .flat_map(|bus| &bus.nodes)
            .collect::<HashSet<_>>();

        for node in &detailed.connectivity_nodes {
            let bus_breaker_level = detailed.voltage_levels.iter().any(|level| {
                level.component == node.voltage_level
                    && level.topology_kind == TopologyKind::BusBreaker
            });
            if bus_breaker_level {
                if !project_mixed_topology
                    && metadata(detailed, &node.component)
                        .is_some_and(has_cgmes_external_identifier)
                {
                    w.warnings.push_as(&codes::EMIT_CGMES.record_dropped, format!(
                    "source CGMES ConnectivityNode `{}` belongs to typed bus breaker VoltageLevel `{}` and was omitted from fresh emission so the voltage level remains bus breaker",
                        node.component, node.voltage_level
                    ));
                }
                continue;
            }
            let active = calculated_bus_for_node(detailed, &node.component).is_some()
                || retained_terminal_nodes.contains(&node.component)
                || retained_switch_nodes.contains(&node.component)
                || retained_busbar_nodes.contains(&node.component)
                || retained_calculated_nodes.contains(&node.component);
            if active {
                active_connectivity_nodes.push(node);
                if node.voltage_level.component_type() == "line"
                    && !active_source_lines.contains(&node.voltage_level)
                {
                    active_source_lines.push(node.voltage_level.clone());
                }
            } else {
                w.warnings.push_as(&codes::EMIT_CGMES.record_dropped, format!(
                    "source CGMES ConnectivityNode `{}` in ConnectivityNodeContainer `{}` is not connected to a calculated bus or retained equipment and was omitted from fresh CGMES emission; no VoltageLevel or BaseVoltage was generated for that container",
                    node.component, node.voltage_level
                ));
            }
        }
        for configured in &detailed.bus_breaker_buses {
            let active = configured.calculated_bus.is_some()
                || retained_terminal_buses.contains(&configured.component);
            if active {
                active_bus_breaker_buses.push(configured);
                if project_mixed_topology
                    && detailed.voltage_levels.iter().any(|level| {
                        level.component == configured.voltage_level
                            && level.topology_kind == TopologyKind::BusBreaker
                    })
                {
                    terminal_generated_connectivity_nodes.insert(configured.component.clone());
                }
                if configured.voltage_level.component_type() == "line"
                    && !active_source_lines.contains(&configured.voltage_level)
                {
                    active_source_lines.push(configured.voltage_level.clone());
                }
            } else {
                w.warnings.push_as(&codes::EMIT_CGMES.record_dropped, format!(
                    "source CGMES TopologicalNode `{}` in ConnectivityNodeContainer `{}` is not connected to a calculated bus or retained equipment and was omitted from fresh CGMES emission; no VoltageLevel or BaseVoltage was generated for that container",
                    configured.component, configured.voltage_level
                ));
            }
        }
        let configured_calculated_buses = active_bus_breaker_buses
            .iter()
            .filter_map(|configured| configured.calculated_bus)
            .collect::<HashSet<_>>();
        represented_calculated_buses.extend(configured_calculated_buses);
        for calculated in &detailed.calculated_buses {
            if represented_calculated_buses.insert(calculated.calculated_bus) {
                active_calculated_buses.push((
                    calculated.calculated_bus,
                    &calculated.voltage_level,
                    calculated.voltage_kv,
                    calculated.angle_degrees,
                ));
            }
        }
        for node in &active_connectivity_nodes {
            let Some(calculated_bus) = calculated_bus_for_node(detailed, &node.component) else {
                continue;
            };
            if represented_calculated_buses.insert(calculated_bus) {
                active_calculated_buses.push((calculated_bus, &node.voltage_level, None, None));
            }
        }
        for level in &detailed.voltage_levels {
            for bus in &level.buses {
                if represented_calculated_buses.insert(*bus) {
                    active_calculated_buses.push((*bus, &level.component, None, None));
                }
            }
        }
    }
    let fallback_calculated_buses = if use_detailed_topology {
        net.buses()
            .iter()
            .filter(|bus| !represented_calculated_buses.contains(&bus.id))
            .map(|bus| bus.id)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let fallback_connectivity_buses = if use_detailed_topology {
        let detailed = detailed.expect("detailed topology has detailed connectivity");
        let terminal_buses = missing_detailed_terminal_buses(net, detailed);
        net.buses()
            .iter()
            .filter(|bus| {
                terminal_buses.contains(&bus.id) || fallback_calculated_buses.contains(&bus.id)
            })
            .map(|bus| bus.id)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut generated_voltage_levels = Vec::<(ComponentId, f64)>::new();
    if let Some(detailed) = detailed {
        let mut record_container = |container: &ComponentId,
                                    bus: Option<BusId>,
                                    affected_class: &str,
                                    affected: &ComponentId|
         -> Result<()> {
            if detailed
                .voltage_levels
                .iter()
                .any(|level| level.component == *container)
            {
                return Ok(());
            }
            let source_line = container.component_type() == "line";
            if source_line && affected_class == "ConnectivityNode" {
                return Ok(());
            }
            let bus = bus.ok_or_else(|| {
                emission_error(format!(
                    "source ConnectivityNodeContainer `{container}` for {affected_class} `{affected}` has no calculated bus from which to preserve its required voltage base"
                ))
            })?;
            let nominal_kv = net
                .buses()
                .iter()
                .find(|candidate| candidate.id == bus)
                .ok_or_else(|| {
                    emission_error(format!(
                        "source ConnectivityNodeContainer `{container}` for {affected_class} `{affected}` references unknown calculated bus {bus}"
                    ))
                })?
                .base_kv;
            if !nominal_kv.is_finite() || nominal_kv <= 0.0 {
                return Err(emission_error(format!(
                    "source ConnectivityNodeContainer `{container}` for {affected_class} `{affected}` has calculated bus {bus} with nonpositive or nonfinite base_kv {nominal_kv}"
                )));
            }
            if source_line {
                return Ok(());
            }
            if let Some((_, existing_kv)) = generated_voltage_levels
                .iter()
                .find(|(existing, _)| existing == container)
            {
                if existing_kv.to_bits() != nominal_kv.to_bits() {
                    return Err(emission_error(format!(
                        "source ConnectivityNodeContainer `{container}` gives {affected_class} `{affected}` voltage base {nominal_kv} kV, inconsistent with its other retained topology at {existing_kv} kV"
                    )));
                }
                return Ok(());
            }
            generated_voltage_levels.push((container.clone(), nominal_kv));
            Ok(())
        };
        for configured in &active_bus_breaker_buses {
            record_container(
                &configured.voltage_level,
                configured.calculated_bus,
                "TopologicalNode",
                &configured.component,
            )?;
        }
        for node in &active_connectivity_nodes {
            record_container(
                &node.voltage_level,
                calculated_bus_for_node(detailed, &node.component),
                "ConnectivityNode",
                &node.component,
            )?;
        }
        for (bus, container, _, _) in &active_calculated_buses {
            let affected = ComponentId::new("calculated_bus", bus.to_string())
                .expect("calculated bus identity is valid");
            record_container(container, Some(*bus), "TopologicalNode", &affected)?;
        }
    }

    // --- containment + bases (EQ) ---------------------------------------
    let region = det_mrid("region", "GR");
    eq.named("GeographicalRegion", &region, "GR");
    eq.close("GeographicalRegion");
    let subregion = det_mrid("region", "SGR");
    eq.named("SubGeographicalRegion", &subregion, "SGR");
    eq.reference("SubGeographicalRegion.Region", &region);
    eq.close("SubGeographicalRegion");
    let mut nominal_voltages = net
        .buses()
        .iter()
        .map(|bus| bus.base_kv)
        .collect::<Vec<_>>();
    if let Some(detailed) = detailed {
        nominal_voltages.extend(detailed.voltage_levels.iter().map(|level| level.nominal_kv));
    }
    nominal_voltages.extend(
        generated_voltage_levels
            .iter()
            .map(|(_, nominal_kv)| *nominal_kv),
    );
    let mut base_ids: Vec<(f64, String)> = Vec::new();
    for nominal_kv in nominal_voltages {
        if !base_ids
            .iter()
            .any(|(kv, _)| kv.to_bits() == nominal_kv.to_bits())
        {
            let id = det_mrid("basevoltage", &format!("{nominal_kv}"));
            eq.named("BaseVoltage", &id, &format!("{nominal_kv} kV"));
            eq.text("BaseVoltage.nominalVoltage", nominal_kv);
            eq.close("BaseVoltage");
            base_ids.push((nominal_kv, id));
        }
    }
    let base_of = |kv: f64| -> String {
        base_ids
            .iter()
            .find(|(k, _)| k.to_bits() == kv.to_bits())
            .map(|(_, id)| id.clone())
            .expect("validated voltage base was registered")
    };
    let freq = det_mrid("frequency", "base");
    eq.named(
        "BaseFrequency",
        &freq,
        &format!("{} Hz", net.base_frequency()),
    );
    eq.text("BaseFrequency.frequency", net.base_frequency());
    eq.close("BaseFrequency");

    let fallback_substation = det_mrid("substation", "powerio");
    if use_detailed_topology {
        let detailed = detailed.expect("detailed topology has detailed connectivity");
        let voltage_level_missing_substation = detailed.voltage_levels.iter().any(|level| {
            level.substation.as_ref().is_none_or(|substation| {
                !detailed
                    .substations
                    .iter()
                    .any(|value| value.component == *substation)
            })
        });
        let needs_fallback_substation = voltage_level_missing_substation
            || !generated_voltage_levels.is_empty()
            || !fallback_calculated_buses.is_empty();
        for substation in &detailed.substations {
            let id = component_mrid(detailed, &substation.component);
            eq.named(
                "Substation",
                &id,
                component_name(
                    detailed,
                    &substation.component,
                    substation.component.local_id(),
                ),
            );
            eq.reference("Substation.Region", &subregion);
            eq.close("Substation");
            if substation.country.is_some()
                || substation.operator.is_some()
                || !substation.geographical_tags.is_empty()
            {
                w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                    "substation `{}`: country, operator, and geographical tags are not emitted by the CGMES core equipment profile",
                    substation.component.local_id()
                ));
            }
        }
        if needs_fallback_substation {
            eq.named("Substation", &fallback_substation, "PowerIO");
            eq.reference("Substation.Region", &subregion);
            eq.close("Substation");
            if voltage_level_missing_substation {
                w.warnings.push_as(
                    &codes::EMIT_CGMES.value_defaulted,
                    "voltage levels without a declared substation were placed in the PowerIO substation",
                );
            }
        }
        for level in &detailed.voltage_levels {
            let id = component_mrid(detailed, &level.component);
            eq.named(
                "VoltageLevel",
                &id,
                component_name(detailed, &level.component, level.component.local_id()),
            );
            let substation = level
                .substation
                .as_ref()
                .filter(|component| {
                    detailed
                        .substations
                        .iter()
                        .any(|value| value.component == **component)
                })
                .map_or_else(
                    || fallback_substation.clone(),
                    |component| component_mrid(detailed, component),
                );
            eq.reference("VoltageLevel.Substation", &substation);
            eq.reference("VoltageLevel.BaseVoltage", &base_of(level.nominal_kv));
            if let Some(value) = level.low_voltage_limit_kv {
                eq.text("VoltageLevel.lowVoltageLimit", value);
            }
            if let Some(value) = level.high_voltage_limit_kv {
                eq.text("VoltageLevel.highVoltageLimit", value);
            }
            eq.close("VoltageLevel");
        }
        for (container, nominal_kv) in &generated_voltage_levels {
            let id = generated_voltage_level_mrid(container);
            eq.named(
                "VoltageLevel",
                &id,
                &format!("Generated voltage level for {container}"),
            );
            eq.reference("VoltageLevel.Substation", &fallback_substation);
            eq.reference("VoltageLevel.BaseVoltage", &base_of(*nominal_kv));
            eq.close("VoltageLevel");
        }
        for bus_id in &fallback_calculated_buses {
            let bus = net
                .buses()
                .iter()
                .find(|bus| bus.id == *bus_id)
                .expect("fallback bus exists in the balanced network");
            let id = det_mrid("voltagelevel", &bus.id.to_string());
            eq.named("VoltageLevel", &id, &format!("VL{}", bus.id));
            eq.reference("VoltageLevel.Substation", &fallback_substation);
            eq.reference("VoltageLevel.BaseVoltage", &base_of(bus.base_kv));
            eq.close("VoltageLevel");
            w.warnings.push_as(&codes::EMIT_CGMES.value_defaulted, format!(
                "balanced bus {} is absent from detailed connectivity; fresh CGMES emitted its TopologicalNode and a generated VoltageLevel and ConnectivityNode",
                bus.id
            ));
        }
        for line in &active_source_lines {
            let id = component_mrid(detailed, line);
            eq.named("Line", &id, component_name(detailed, line, line.local_id()));
            eq.reference("Line.Region", &subregion);
            eq.close("Line");
        }

        let mut defined_connectivity_node_ids = HashSet::<String>::new();
        for node in &active_connectivity_nodes {
            let id = component_mrid(detailed, &node.component);
            defined_connectivity_node_ids.insert(id.clone());
            eq.named(
                "ConnectivityNode",
                &id,
                component_name(detailed, &node.component, node.component.local_id()),
            );
            eq.reference(
                "ConnectivityNode.ConnectivityNodeContainer",
                &topology_voltage_level_mrid(detailed, &node.voltage_level),
            );
            eq.close("ConnectivityNode");
            if let Some(bus) = calculated_bus_for_node(detailed, &node.component) {
                tp.open("ConnectivityNode", &id, true);
                tp.reference("ConnectivityNode.TopologicalNode", &bus_mrid(net, bus));
                tp.close("ConnectivityNode");
            }
            if node.voltage_level.component_type() != "line"
                && !detailed
                    .voltage_levels
                    .iter()
                    .any(|level| level.component == node.voltage_level)
            {
                let affected = calculated_bus_for_node(detailed, &node.component).map_or_else(
                    || {
                        format!(
                            "ConnectivityNode `{}` with no calculated bus",
                            node.component
                        )
                    },
                    |bus| format!("bus {bus} through ConnectivityNode `{}`", node.component),
                );
                w.warnings.push_as(&codes::EMIT_CGMES.value_defaulted, format!(
                    "source ConnectivityNodeContainer `{}` for {affected} was not a typed VoltageLevel; during fresh CGMES emission, that topology was placed in generated VoltageLevel `{}`",
                    node.voltage_level,
                    topology_voltage_level_mrid(detailed, &node.voltage_level)
                ));
            }
        }
        for (bus, voltage_level, voltage_kv, angle_degrees) in &active_calculated_buses {
            let balanced_bus = net
                .buses()
                .iter()
                .find(|candidate| candidate.id == *bus)
                .expect("active calculated bus exists in the balanced network");
            let id = bus_mrid(net, *bus);
            tp.named(
                "TopologicalNode",
                &id,
                balanced_bus
                    .name
                    .as_deref()
                    .unwrap_or_else(|| balanced_bus.uid.as_deref().unwrap_or("calculated bus")),
            );
            tp.reference(
                "TopologicalNode.BaseVoltage",
                &base_of(balanced_bus.base_kv),
            );
            tp.reference(
                "TopologicalNode.ConnectivityNodeContainer",
                &topology_voltage_level_mrid(detailed, voltage_level),
            );
            tp.close("TopologicalNode");
            write_sv_voltage(
                &mut sv,
                &id,
                *voltage_kv,
                *angle_degrees,
                false,
                &mut w.warnings,
            );
        }
        for bus_id in &fallback_calculated_buses {
            let bus = net
                .buses()
                .iter()
                .find(|bus| bus.id == *bus_id)
                .expect("fallback bus exists in the balanced network");
            let id = bus_mrid(net, bus.id);
            let voltage_level = det_mrid("voltagelevel", &bus.id.to_string());
            tp.named(
                "TopologicalNode",
                &id,
                bus.name.as_deref().unwrap_or(&bus.id.to_string()),
            );
            tp.reference("TopologicalNode.BaseVoltage", &base_of(bus.base_kv));
            tp.reference("TopologicalNode.ConnectivityNodeContainer", &voltage_level);
            tp.close("TopologicalNode");
            write_sv_voltage(
                &mut sv,
                &id,
                Some(bus.vm * bus.base_kv),
                Some(bus.va),
                false,
                &mut w.warnings,
            );
        }
        for configured in &active_bus_breaker_buses {
            if terminal_generated_connectivity_nodes.contains(&configured.component) {
                let id = det_mrid("connectivity_node", &configured.component.to_string());
                if !defined_connectivity_node_ids.insert(id.clone()) {
                    continue;
                }
                eq.named(
                    "ConnectivityNode",
                    &id,
                    component_name(
                        detailed,
                        &configured.component,
                        configured.component.local_id(),
                    ),
                );
                eq.reference(
                    "ConnectivityNode.ConnectivityNodeContainer",
                    &topology_voltage_level_mrid(detailed, &configured.voltage_level),
                );
                eq.close("ConnectivityNode");
                tp.open("ConnectivityNode", &id, true);
                tp.reference(
                    "ConnectivityNode.TopologicalNode",
                    &configured_bus_mrid(detailed, &configured.component),
                );
                tp.close("ConnectivityNode");
            }
        }
        for bus_id in &fallback_connectivity_buses {
            let id = connectivity_node_mrid(Some(detailed), None, *bus_id, project_mixed_topology);
            if !defined_connectivity_node_ids.insert(id.clone()) {
                continue;
            }
            let container = detailed_voltage_level_for_bus(detailed, *bus_id).map_or_else(
                || det_mrid("voltagelevel", &bus_id.to_string()),
                |component| topology_voltage_level_mrid(detailed, component),
            );
            eq.named(
                "ConnectivityNode",
                &id,
                &format!("Generated connectivity node for bus {bus_id}"),
            );
            eq.reference("ConnectivityNode.ConnectivityNodeContainer", &container);
            eq.close("ConnectivityNode");
            tp.open("ConnectivityNode", &id, true);
            tp.reference("ConnectivityNode.TopologicalNode", &bus_mrid(net, *bus_id));
            tp.close("ConnectivityNode");
            if !fallback_calculated_buses.contains(bus_id) {
                w.warnings.push_as(&codes::EMIT_CGMES.value_defaulted, format!(
                    "at least one balanced equipment terminal at bus {bus_id} is absent from detailed connectivity; fresh CGMES emitted a generated ConnectivityNode for that terminal"
                ));
            }
        }

        for configured in &active_bus_breaker_buses {
            let id = configured_bus_mrid(detailed, &configured.component);
            let level = detailed
                .voltage_levels
                .iter()
                .find(|level| level.component == configured.voltage_level);
            let nominal_kv = level.map_or_else(
                || {
                    if configured.voltage_level.component_type() == "line" {
                        return configured
                            .calculated_bus
                            .and_then(|id| net.buses().iter().find(|bus| bus.id == id))
                            .map(|bus| bus.base_kv)
                            .expect("active source Line TopologicalNode has a calculated bus");
                    }
                    generated_voltage_levels
                        .iter()
                        .find(|(container, _)| container == &configured.voltage_level)
                        .map(|(_, nominal_kv)| *nominal_kv)
                        .expect("generated voltage levels were validated above")
                },
                |level| level.nominal_kv,
            );
            tp.named(
                "TopologicalNode",
                &id,
                component_name(
                    detailed,
                    &configured.component,
                    configured.component.local_id(),
                ),
            );
            tp.reference("TopologicalNode.BaseVoltage", &base_of(nominal_kv));
            tp.reference(
                "TopologicalNode.ConnectivityNodeContainer",
                &topology_voltage_level_mrid(detailed, &configured.voltage_level),
            );
            tp.close("TopologicalNode");
            if level.is_none() && configured.voltage_level.component_type() != "line" {
                w.warnings.push_as(&codes::EMIT_CGMES.value_defaulted, format!(
                    "source ConnectivityNodeContainer `{}` for TopologicalNode `{}` was not a typed VoltageLevel; during fresh CGMES emission, that topology was placed in generated VoltageLevel `{}`",
                    configured.voltage_level,
                    configured.component,
                    topology_voltage_level_mrid(detailed, &configured.voltage_level)
                ));
            }
            let authority_mismatch =
                metadata(detailed, &configured.component).is_some_and(|metadata| {
                    metadata
                        .properties
                        .get(super::CGMES_SV_VOLTAGE_AUTHORITY_MISMATCH_PROPERTY)
                        .is_some_and(|value| value == "true")
                });
            write_sv_voltage(
                &mut sv,
                &id,
                configured.voltage_kv,
                configured.angle_degrees,
                authority_mismatch,
                &mut w.warnings,
            );
        }
    } else {
        // A flat case has one synthetic hierarchy and connectivity node per bus.
        for bus in net.buses() {
            let sub = det_mrid("substation", &bus.id.to_string());
            eq.named("Substation", &sub, &format!("S{}", bus.id));
            eq.reference("Substation.Region", &subregion);
            eq.close("Substation");
            let vl = det_mrid("voltagelevel", &bus.id.to_string());
            eq.named("VoltageLevel", &vl, &format!("VL{}", bus.id));
            eq.reference("VoltageLevel.Substation", &sub);
            eq.reference("VoltageLevel.BaseVoltage", &base_of(bus.base_kv));
            eq.text("VoltageLevel.lowVoltageLimit", bus.vmin * bus.base_kv);
            eq.text("VoltageLevel.highVoltageLimit", bus.vmax * bus.base_kv);
            eq.close("VoltageLevel");

            let tn = bus_mrid(net, bus.id);
            tp.named(
                "TopologicalNode",
                &tn,
                bus.name.as_deref().unwrap_or(&bus.id.to_string()),
            );
            tp.reference("TopologicalNode.BaseVoltage", &base_of(bus.base_kv));
            tp.reference("TopologicalNode.ConnectivityNodeContainer", &vl);
            tp.close("TopologicalNode");

            let cn = det_mrid("connectivity_node", &format!("bus:{}", bus.id));
            eq.named("ConnectivityNode", &cn, &format!("CN{}", bus.id));
            eq.reference("ConnectivityNode.ConnectivityNodeContainer", &vl);
            eq.close("ConnectivityNode");
            tp.open("ConnectivityNode", &cn, true);
            tp.reference("ConnectivityNode.TopologicalNode", &tn);
            tp.close("ConnectivityNode");

            let svv = det_mrid("svvoltage", &bus.id.to_string());
            sv.open("SvVoltage", &svv, false);
            sv.reference("SvVoltage.TopologicalNode", &tn);
            sv.text("SvVoltage.v", bus.vm * bus.base_kv);
            sv.text("SvVoltage.angle", bus.va);
            sv.close("SvVoltage");
        }
    }

    for bus in net.buses() {
        if bus.evhi.is_some() || bus.evlo.is_some() {
            w.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "bus {}: emergency voltage band (evhi/evlo) has no CGMES slot",
                    bus.id
                ),
            );
        }
    }

    // A terminal: EQ definition and connectivity, TP topology, SSH connection.
    let terminal = |eq: &mut Doc,
                    tp: &mut Doc,
                    ssh: &mut Doc,
                    sv: &mut Doc,
                    owner: &str,
                    component_type: &str,
                    local_id: &str,
                    record_override: Option<&Terminal>,
                    seq: usize,
                    bus: BusId,
                    connected: bool,
                    warnings: &mut CgmesDiagnostics| {
        let record = record_override.or_else(|| {
            detailed.and_then(|detailed| detailed_terminal(detailed, component_type, local_id, seq))
        });
        let id = terminal_mrid(detailed, record, owner, seq);
        eq.open("Terminal", &id, false);
        eq.reference("Terminal.ConductingEquipment", owner);
        eq.text("ACDCTerminal.sequenceNumber", seq);
        if record.is_none_or(|record| {
            detailed.is_none_or(|detailed| {
                terminal_uses_connectivity_node(detailed, record, project_mixed_topology)
            })
        }) {
            eq.reference(
                "Terminal.ConnectivityNode",
                &connectivity_node_mrid(detailed, record, bus, project_mixed_topology),
            );
        }
        eq.close("Terminal");
        let terminal_connected = record.map_or(connected, |value| value.connected);
        if terminal_connected {
            tp.open("Terminal", &id, true);
            tp.reference(
                "Terminal.TopologicalNode",
                &terminal_topological_node_mrid(net, detailed, record, bus),
            );
            tp.close("Terminal");
        }
        ssh.open("Terminal", &id, true);
        ssh.text("ACDCTerminal.connected", terminal_connected);
        ssh.close("Terminal");
        if let Some(record) = record
            && !matches!(
                record.equipment.component_type(),
                "voltage_source_converter" | "line_commutated_converter"
            )
        {
            write_sv_power_flow(
                sv,
                &id,
                record.active_power_mw,
                record.reactive_power_mvar,
                warnings,
            );
        }
        id
    };

    let disconnected_terminal = |eq: &mut Doc,
                                 ssh: &mut Doc,
                                 sv: &mut Doc,
                                 owner: &str,
                                 record: &Terminal,
                                 warnings: &mut CgmesDiagnostics| {
        let id = terminal_mrid(detailed, Some(record), owner, usize::from(record.terminal));
        eq.open("Terminal", &id, false);
        eq.reference("Terminal.ConductingEquipment", owner);
        eq.text("ACDCTerminal.sequenceNumber", record.terminal);
        if terminal_uses_connectivity_node(
            detailed.expect("detailed terminal has detailed connectivity"),
            record,
            project_mixed_topology,
        ) {
            if let Some(node) = record.node.as_ref() {
                eq.reference(
                    "Terminal.ConnectivityNode",
                    &component_mrid(
                        detailed.expect("detailed terminal has detailed connectivity"),
                        node,
                    ),
                );
            } else if let Some(bus) = record.connectable_bus.as_ref().or(record.bus.as_ref()) {
                eq.reference(
                    "Terminal.ConnectivityNode",
                    &det_mrid("connectivity_node", &bus.to_string()),
                );
            }
        }
        eq.close("Terminal");
        ssh.open("Terminal", &id, true);
        ssh.text("ACDCTerminal.connected", false);
        ssh.close("Terminal");
        if !matches!(
            record.equipment.component_type(),
            "voltage_source_converter" | "line_commutated_converter"
        ) {
            write_sv_power_flow(
                sv,
                &id,
                record.active_power_mw,
                record.reactive_power_mvar,
                warnings,
            );
        }
        id
    };

    if let Some(detailed) = detailed {
        for busbar in &detailed.busbar_sections {
            let level = detailed
                .voltage_levels
                .iter()
                .find(|level| level.component == busbar.voltage_level);
            if level.is_some_and(|level| level.topology_kind == TopologyKind::BusBreaker)
                && !project_mixed_topology
            {
                w.warnings.push_as(&codes::EMIT_CGMES.record_dropped, format!(
                    "BusbarSection `{}` belongs to bus breaker VoltageLevel `{}`; it remains in PowerIO detailed connectivity but was omitted from fresh CGMES because CIM100 represents that bus branch topology through TopologicalNode terminals",
                    busbar.component, busbar.voltage_level
                ));
                continue;
            }
            let bus = calculated_bus_for_node(detailed, &busbar.node);
            let id = component_mrid(detailed, &busbar.component);
            eq.named(
                "BusbarSection",
                &id,
                component_name(detailed, &busbar.component, busbar.component.local_id()),
            );
            let fallback_container = component_mrid(detailed, &busbar.voltage_level);
            let container = mapped_equipment_container_mrid(
                Some(detailed),
                "busbar_section",
                busbar.component.local_id(),
                Some(fallback_container.clone()),
                &mut w.warnings,
            )
            .unwrap_or(fallback_container);
            eq.reference("Equipment.EquipmentContainer", &container);
            if let Some(level) = level {
                eq.reference(
                    "ConductingEquipment.BaseVoltage",
                    &base_of(level.nominal_kv),
                );
            }
            eq.close("BusbarSection");
            let fallback_busbar_terminal = Terminal {
                component: None,
                equipment: busbar.component.clone(),
                terminal: 1,
                voltage_level: busbar.voltage_level.clone(),
                bus: None,
                connectable_bus: None,
                node: Some(busbar.node.clone()),
                connected: bus.is_some(),
                active_power_mw: None,
                reactive_power_mvar: None,
            };
            let busbar_terminal =
                detailed_terminal(detailed, "busbar_section", busbar.component.local_id(), 1)
                    .unwrap_or(&fallback_busbar_terminal);
            if let Some(bus) = bus {
                terminal(
                    &mut eq,
                    &mut tp,
                    &mut ssh,
                    &mut sv,
                    &id,
                    "busbar_section",
                    busbar.component.local_id(),
                    Some(busbar_terminal),
                    1,
                    bus,
                    busbar_terminal.connected,
                    &mut w.warnings,
                );
            } else if busbar_terminal.connected {
                return Err(emission_error(format!(
                    "BusbarSection `{}` terminal 1 is connected but its ConnectivityNode `{}` has no calculated bus",
                    busbar.component, busbar.node
                )));
            } else {
                disconnected_terminal(
                    &mut eq,
                    &mut ssh,
                    &mut sv,
                    &id,
                    busbar_terminal,
                    &mut w.warnings,
                );
            }
            if let Some(in_service) = retained_sv_status(detailed, &busbar.component) {
                write_sv_status(&mut sv, &id, in_service);
            }
        }
        for junction in &detailed.junctions {
            let id = component_mrid(detailed, &junction.component);
            eq.named(
                "Junction",
                &id,
                component_name(detailed, &junction.component, junction.component.local_id()),
            );
            let fallback_container = detailed
                .terminals
                .iter()
                .find(|terminal| terminal.equipment == junction.component)
                .map(|terminal| topology_voltage_level_mrid(detailed, &terminal.voltage_level));
            if let Some(container) = mapped_equipment_container_mrid(
                Some(detailed),
                "junction",
                junction.component.local_id(),
                fallback_container,
                &mut w.warnings,
            ) {
                eq.reference("Equipment.EquipmentContainer", &container);
            }
            eq.close("Junction");
            for record in detailed
                .terminals
                .iter()
                .filter(|record| record.equipment == junction.component)
            {
                if let Some(bus) = calculated_bus_for_terminal(detailed, record) {
                    terminal(
                        &mut eq,
                        &mut tp,
                        &mut ssh,
                        &mut sv,
                        &id,
                        "junction",
                        junction.component.local_id(),
                        Some(record),
                        usize::from(record.terminal),
                        bus,
                        record.connected,
                        &mut w.warnings,
                    );
                } else if record.connected {
                    return Err(emission_error(format!(
                        "Junction `{}` terminal {} is connected but has no calculated bus",
                        junction.component, record.terminal
                    )));
                } else {
                    disconnected_terminal(&mut eq, &mut ssh, &mut sv, &id, record, &mut w.warnings);
                }
            }
            if let Some(in_service) = retained_sv_status(detailed, &junction.component) {
                write_sv_status(&mut sv, &id, in_service);
            }
        }
        for switch in &detailed.switches {
            let first = endpoint_bus(detailed, &switch.endpoint1);
            let second = endpoint_bus(detailed, &switch.endpoint2);
            let id = component_mrid(detailed, &switch.component);
            let balanced_switch = net
                .switches()
                .iter()
                .find(|value| value.uid.as_deref() == Some(switch.component.local_id()));
            let class =
                mapped_component_metadata(Some(detailed), "switch", switch.component.local_id())
                    .and_then(|metadata| metadata.properties.get(CGMES_CLASS_PROPERTY))
                    .filter(|class| {
                        matches!(
                            class.as_str(),
                            "Breaker"
                                | "Disconnector"
                                | "LoadBreakSwitch"
                                | "Switch"
                                | "Fuse"
                                | "Jumper"
                                | "GroundDisconnector"
                                | "DisconnectingCircuitBreaker"
                        )
                    })
                    .map_or_else(|| switch_class(switch.kind), String::as_str);
            eq.named(
                class,
                &id,
                component_name(detailed, &switch.component, switch.component.local_id()),
            );
            let fallback_container = component_mrid(detailed, &switch.voltage_level);
            let container = mapped_equipment_container_mrid(
                Some(detailed),
                "switch",
                switch.component.local_id(),
                Some(fallback_container.clone()),
                &mut w.warnings,
            )
            .unwrap_or(fallback_container);
            eq.reference("Equipment.EquipmentContainer", &container);
            eq.text("Switch.normalOpen", switch.open);
            eq.text("Switch.retained", switch.retained);
            if let Some(current_rating) = balanced_switch.and_then(|value| value.current_rating) {
                eq.text("Switch.ratedCurrent", current_rating);
            }
            eq.close(class);
            if let Some(thermal_rating) = balanced_switch.and_then(|value| value.thermal_rating) {
                w.warnings.push_as(
                    &codes::EMIT_CGMES.field_dropped,
                    format!(
                        "switch `{}` thermal rating {thermal_rating} MVA was not emitted: CGMES Switch carries rated current, not an apparent power limit",
                        switch.component.local_id()
                    ),
                );
            }
            let fallback_switch_terminal = |number: u8,
                                            endpoint: &TopologyEndpoint,
                                            bus: Option<BusId>,
                                            connected: bool|
             -> Terminal {
                let balanced_flow = balanced_switch.map(|value| {
                    if bus == Some(value.to) || (bus.is_none() && number == 2) {
                        (value.pt, value.qt)
                    } else {
                        (value.pf, value.qf)
                    }
                });
                Terminal {
                    component: None,
                    equipment: switch.component.clone(),
                    terminal: number,
                    voltage_level: switch.voltage_level.clone(),
                    bus: match endpoint {
                        TopologyEndpoint::Bus(bus) => Some(bus.clone()),
                        TopologyEndpoint::Node(_) => None,
                    },
                    connectable_bus: match endpoint {
                        TopologyEndpoint::Bus(bus) => Some(bus.clone()),
                        TopologyEndpoint::Node(_) => None,
                    },
                    node: match endpoint {
                        TopologyEndpoint::Node(node) => Some(node.clone()),
                        TopologyEndpoint::Bus(_) => None,
                    },
                    connected,
                    active_power_mw: balanced_flow.and_then(|value| value.0),
                    reactive_power_mvar: balanced_flow.and_then(|value| value.1),
                }
            };
            let merged_terminal =
                |number: u8, endpoint: &TopologyEndpoint, bus: Option<BusId>, connected: bool| {
                    let mut record = detailed_terminal(
                        detailed,
                        "switch",
                        switch.component.local_id(),
                        usize::from(number),
                    )
                    .cloned()
                    .unwrap_or_else(|| fallback_switch_terminal(number, endpoint, bus, connected));
                    if let Some(value) = balanced_switch {
                        let (active, reactive) =
                            if bus == Some(value.to) || (bus.is_none() && number == 2) {
                                (value.pt, value.qt)
                            } else {
                                (value.pf, value.qf)
                            };
                        record.active_power_mw = record.active_power_mw.or(active);
                        record.reactive_power_mvar = record.reactive_power_mvar.or(reactive);
                    }
                    record
                };
            let first_terminal = merged_terminal(1, &switch.endpoint1, first, first.is_some());
            let second_terminal = merged_terminal(2, &switch.endpoint2, second, second.is_some());
            for (record, bus) in [(&first_terminal, first), (&second_terminal, second)] {
                if let Some(bus) = bus {
                    terminal(
                        &mut eq,
                        &mut tp,
                        &mut ssh,
                        &mut sv,
                        &id,
                        "switch",
                        switch.component.local_id(),
                        Some(record),
                        usize::from(record.terminal),
                        bus,
                        record.connected,
                        &mut w.warnings,
                    );
                } else if record.connected {
                    return Err(emission_error(format!(
                        "switch `{}` terminal {} is connected but its endpoint has no calculated bus",
                        switch.component, record.terminal
                    )));
                } else {
                    disconnected_terminal(&mut eq, &mut ssh, &mut sv, &id, record, &mut w.warnings);
                }
            }
            ssh.open(class, &id, true);
            ssh.text("Switch.open", switch.open);
            ssh.close(class);
            if let Some(in_service) = retained_sv_status(detailed, &switch.component) {
                write_sv_status(&mut sv, &id, in_service);
            }
        }
        if !detailed.internal_connections.is_empty() {
            w.warnings.push_as(&codes::EMIT_CGMES.value_collapsed, format!(
                "{} internal connection(s) have no distinct CGMES equipment record; their calculated topology is retained through TopologicalNode assignments",
                detailed.internal_connections.len()
            ));
        }

        write_dc_equipment(&mut w, detailed, &mut eq, &mut tp, &mut ssh, &mut sv)?;
    }

    // --- operational limit plumbing --------------------------------------
    let ext_ns = w.p.ext.1;
    let limit_kind_uri = move |kind: &str| -> String {
        if v3 {
            format!("{ext_ns}LimitKind.{kind}")
        } else {
            format!("{ext_ns}LimitTypeKind.{kind}")
        }
    };
    let mut limit_types_used: Vec<&'static str> = Vec::new();
    let mut limit_doc = Doc::new(Arc::clone(&retained_metadata));

    // --- loads ------------------------------------------------------------
    for (i, load) in net.loads().iter().enumerate() {
        let fallback = format!("{}-{i}", load.bus);
        let local_id = load.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("load", &fallback, load.uid.as_deref());
        let record = detailed.and_then(|value| detailed_terminal(value, "load", local_id, 1));
        let fallback_name = format!("load{}-{i}", load.bus);
        let class = mapped_component_metadata(detailed, "load", local_id)
            .and_then(|metadata| metadata.properties.get(CGMES_CLASS_PROPERTY))
            .filter(|class| {
                matches!(
                    class.as_str(),
                    "EnergyConsumer" | "ConformLoad" | "NonConformLoad" | "StationSupply"
                )
            })
            .map_or("EnergyConsumer", String::as_str);
        eq.named(
            class,
            &id,
            mapped_component_name(detailed, "load", local_id, &fallback_name),
        );
        let fallback_container = terminal_voltage_level_mrid(detailed, record, load.bus);
        let container = mapped_equipment_container_mrid(
            detailed,
            "load",
            local_id,
            Some(fallback_container.clone()),
            &mut w.warnings,
        )
        .unwrap_or(fallback_container);
        eq.reference("Equipment.EquipmentContainer", &container);
        let response = load
            .voltage_model
            .as_ref()
            .map(|_| det_mrid("load_response", &id));
        if let Some(response) = &response {
            eq.reference("EnergyConsumer.LoadResponse", response);
        }
        eq.close(class);
        if let (Some(model), Some(response)) = (&load.voltage_model, &response) {
            eq.named(
                "LoadResponseCharacteristic",
                response,
                &format!("load{}-{i}-response", load.bus),
            );
            let fractions = |values: [f64; 3], total: f64| {
                if total.abs() > f64::EPSILON {
                    values.map(|value| value / total)
                } else {
                    [1.0, 0.0, 0.0]
                }
            };
            let (exponent, gamma_p, gamma_q, p_coefficients, q_coefficients) = match model {
                LoadVoltageModel::ConstantPower => {
                    (false, 0.0, 0.0, [1.0, 0.0, 0.0], [1.0, 0.0, 0.0])
                }
                LoadVoltageModel::Zip {
                    p_constant_power,
                    q_constant_power,
                    p_constant_current,
                    q_constant_current,
                    p_constant_impedance,
                    q_constant_impedance,
                    v_nom,
                    load_type,
                    scaling,
                } => {
                    if v_nom.is_some() || load_type.is_some() || scaling.is_some() {
                        w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                            "load at bus {}: nominal voltage, source load type, and scaling metadata have no LoadResponseCharacteristic fields and were omitted",
                            load.bus
                        ));
                    }
                    (
                        false,
                        0.0,
                        0.0,
                        fractions(
                            [
                                *p_constant_power,
                                *p_constant_current,
                                *p_constant_impedance,
                            ],
                            load.p,
                        ),
                        fractions(
                            [
                                *q_constant_power,
                                *q_constant_current,
                                *q_constant_impedance,
                            ],
                            load.q,
                        ),
                    )
                }
                LoadVoltageModel::Exponential {
                    v_nom,
                    gamma_p,
                    gamma_q,
                    ..
                } => {
                    if v_nom.is_some() {
                        w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                            "load at bus {}: nominal voltage has no LoadResponseCharacteristic field and was omitted",
                            load.bus
                        ));
                    }
                    (true, *gamma_p, *gamma_q, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0])
                }
            };
            eq.text("LoadResponseCharacteristic.exponentModel", exponent);
            eq.text("LoadResponseCharacteristic.pVoltageExponent", gamma_p);
            eq.text("LoadResponseCharacteristic.qVoltageExponent", gamma_q);
            for (name, value) in [
                ("pConstantPower", p_coefficients[0]),
                ("pConstantCurrent", p_coefficients[1]),
                ("pConstantImpedance", p_coefficients[2]),
                ("qConstantPower", q_coefficients[0]),
                ("qConstantCurrent", q_coefficients[1]),
                ("qConstantImpedance", q_coefficients[2]),
            ] {
                eq.text(&format!("LoadResponseCharacteristic.{name}"), value);
            }
            eq.close("LoadResponseCharacteristic");
        }
        terminal(
            &mut eq,
            &mut tp,
            &mut ssh,
            &mut sv,
            &id,
            "load",
            local_id,
            None,
            1,
            load.bus,
            load.in_service,
            &mut w.warnings,
        );
        ssh.open(class, &id, true);
        if !field_was_omitted(detailed, "load", local_id, OmittedFieldName::ActivePower) {
            ssh.text("EnergyConsumer.p", load.p);
        }
        if !field_was_omitted(detailed, "load", local_id, OmittedFieldName::ReactivePower) {
            ssh.text("EnergyConsumer.q", load.q);
        }
        if v3 {
            ssh.text("Equipment.inService", load.in_service);
        }
        ssh.close(class);
        write_sv_status(&mut sv, &id, load.in_service);
    }

    // --- generators --------------------------------------------------------
    for (i, machine) in net.generators().iter().enumerate() {
        let fallback = format!("{}-{i}", machine.bus);
        let local_id = machine.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("generator", &fallback, machine.uid.as_deref());
        let record = detailed.and_then(|value| detailed_terminal(value, "generator", local_id, 1));
        let machine_metadata = mapped_component_metadata(detailed, "generator", local_id);
        let source_is_cgmes = machine_metadata.is_some_and(has_cgmes_external_identifier);
        let source_control_id = machine_metadata.and_then(|metadata| {
            metadata
                .properties
                .get(super::CGMES_REGULATING_CONTROL_PROPERTY)
        });
        let source_control_mode = machine_metadata
            .and_then(|metadata| metadata.properties.get("RegulatingControl.mode"))
            .map_or("RegulatingControlModeKind.voltage", String::as_str);
        let source_machine_control_enabled =
            retained_bool(machine_metadata, "RegulatingCondEq.controlEnabled");
        let source_control_enabled = retained_bool(machine_metadata, "RegulatingControl.enabled");
        let source_control_is_effective = source_control_mode
            == "RegulatingControlModeKind.voltage"
            && source_control_enabled.unwrap_or(false)
            && source_machine_control_enabled.unwrap_or(true);
        let control_was_edited = source_is_cgmes
            && source_control_id.is_some()
            && source_control_is_effective != machine.voltage_regulation_on;
        if control_was_edited {
            w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                "generator `{local_id}` source RegulatingControl.enabled={} and RegulatingCondEq.controlEnabled={} represented voltage regulation {}; the typed value is {}, so fresh CGMES output sets both enable flags to the typed value",
                source_control_enabled.map_or("absent".into(), |value| value.to_string()),
                source_machine_control_enabled
                    .map_or("absent".into(), |value| value.to_string()),
                source_control_is_effective,
                machine.voltage_regulation_on
            ));
        }
        let machine_control_enabled = if control_was_edited {
            machine.voltage_regulation_on
        } else {
            source_machine_control_enabled.unwrap_or(machine.voltage_regulation_on)
        };
        let control_enabled = if control_was_edited {
            machine.voltage_regulation_on
        } else {
            source_control_enabled.unwrap_or(machine.voltage_regulation_on)
        };
        let source_control = mapped_regulating_control_metadata(detailed, "generator", local_id);
        let emit_control = source_control_id.is_some()
            || !source_is_cgmes
            || machine.voltage_regulation_on
            || machine.regulating_terminal.is_some();
        let control = source_control.map_or_else(
            || {
                source_control_id
                    .filter(|value| uuid::Uuid::parse_str(value).is_ok())
                    .cloned()
                    .unwrap_or_else(|| det_mrid("regcontrol", &id))
            },
            |metadata| {
                component_mrid(
                    detailed
                        .expect("source RegulatingControl metadata requires detailed connectivity"),
                    &metadata.component,
                )
            },
        );
        let source_unit = mapped_generating_unit_metadata(detailed, local_id);
        let unit = source_unit.map_or_else(
            || det_mrid("genunit", &id),
            |metadata| {
                component_mrid(
                    detailed
                        .expect("source GeneratingUnit metadata requires detailed connectivity"),
                    &metadata.component,
                )
            },
        );
        let unit_fallback_name = format!("gen{}-{i}-unit", machine.bus);
        let unit_class = generating_unit_class(machine.energy_source);
        eq.named(
            unit_class,
            &unit,
            source_unit
                .and_then(|metadata| metadata.name.as_deref())
                .unwrap_or(&unit_fallback_name),
        );
        if let Some(description) =
            source_unit.and_then(|metadata| metadata.properties.get("IdentifiedObject.description"))
        {
            eq.text("IdentifiedObject.description", description);
        }
        if let Some(aggregate) =
            source_unit.and_then(|metadata| metadata.properties.get("Equipment.aggregate"))
        {
            eq.text("Equipment.aggregate", aggregate);
        }
        if let Some(source_unit) = source_unit {
            let fallback_container = terminal_voltage_level_mrid(detailed, record, machine.bus);
            if let Some(container) = mapped_equipment_container_mrid(
                detailed,
                source_unit.component.component_type(),
                source_unit.component.local_id(),
                Some(fallback_container),
                &mut w.warnings,
            ) {
                eq.reference("Equipment.EquipmentContainer", &container);
            }
        }
        if let Some(control_source) = source_unit
            .and_then(|metadata| metadata.properties.get("GeneratingUnit.genControlSource"))
        {
            eq.enumeration(
                "GeneratingUnit.genControlSource",
                w.p.cim_ns,
                control_source,
            );
        }
        let source_initial_p =
            source_unit.and_then(|metadata| metadata.properties.get("GeneratingUnit.initialP"));
        if v3 {
            if let Some(initial_p) = source_initial_p {
                w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                    "GeneratingUnit `{unit}` source initialP `{initial_p}` has no CGMES 3.0 property"
                ));
            }
        } else {
            eq.text(
                "GeneratingUnit.initialP",
                source_initial_p.map_or_else(|| machine.pg.to_string(), Clone::clone),
            );
        }
        eq.text("GeneratingUnit.minOperatingP", machine.pmin);
        eq.text("GeneratingUnit.maxOperatingP", machine.pmax);
        if let Some(nominal_p) =
            source_unit.and_then(|metadata| metadata.properties.get("GeneratingUnit.nominalP"))
        {
            eq.text("GeneratingUnit.nominalP", nominal_p);
        }
        eq.close(unit_class);
        let mut unit_ssh_written = false;
        if v3 {
            ssh.open(unit_class, &unit, true);
            ssh.text("Equipment.inService", machine.in_service);
            unit_ssh_written = true;
        }
        if let Some(active_power_control) = &machine.active_power_control {
            validate_active_power_control(
                local_id,
                active_power_control,
                machine.pmin,
                machine.pmax,
            )?;
            if active_power_control.participate
                && let Some(participation_factor) = active_power_control.participation_factor
            {
                if !unit_ssh_written {
                    ssh.open(unit_class, &unit, true);
                    unit_ssh_written = true;
                }
                ssh.text("GeneratingUnit.normalPF", participation_factor);
            }
            if !active_power_control.participate {
                w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                    "generator `{local_id}`: participate=false{} cannot be represented by CGMES GeneratingUnit.normalPF",
                    active_power_control
                        .participation_factor
                        .map_or("", |_| " with a participation factor")
                ));
            } else if active_power_control.participation_factor.is_none() {
                w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                    "generator `{local_id}`: participate=true without a participation factor cannot be represented by CGMES GeneratingUnit.normalPF"
                ));
            }
            if active_power_control.droop_percent.is_some() {
                w.warnings.push_as(
                    &codes::EMIT_CGMES.field_dropped,
                    format!(
                        "generator `{local_id}`: active power control droop has no CGMES property"
                    ),
                );
            }
            if active_power_control
                .minimum_target_active_power_mw
                .is_some()
                || active_power_control
                    .maximum_target_active_power_mw
                    .is_some()
            {
                w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                    "generator `{local_id}`: active power control target limits have no CGMES property"
                ));
            }
        }
        if unit_ssh_written {
            ssh.close(unit_class);
        }
        let generator_component = ComponentId::new("generator", local_id)
            .map_err(|error| emission_error(error.to_string()))?;
        let (minimum_reactive_power_mvar, maximum_reactive_power_mvar, capability_curve) =
            if let Some(detailed) = detailed {
                write_synchronous_machine_reactive_limits(
                    &mut eq,
                    detailed,
                    &generator_component,
                    &id,
                    w.p.cim_ns,
                    machine.pg,
                    machine.qmin,
                    machine.qmax,
                    &mut w.warnings,
                )?
            } else {
                (machine.qmin, machine.qmax, None)
            };
        let fallback_name = format!("gen{}-{i}", machine.bus);
        eq.named(
            "SynchronousMachine",
            &id,
            mapped_component_name(detailed, "generator", local_id, &fallback_name),
        );
        eq.enumeration(
            "SynchronousMachine.type",
            w.p.cim_ns,
            machine_metadata
                .and_then(|metadata| metadata.properties.get("SynchronousMachine.type"))
                .map_or("SynchronousMachineKind.generator", String::as_str),
        );
        eq.text("SynchronousMachine.maxQ", maximum_reactive_power_mvar);
        eq.text("SynchronousMachine.minQ", minimum_reactive_power_mvar);
        if machine.mbase > 0.0 {
            eq.text("RotatingMachine.ratedS", machine.mbase);
        }
        eq.reference("RotatingMachine.GeneratingUnit", &unit);
        if emit_control {
            eq.reference(super::CGMES_REGULATING_CONTROL_PROPERTY, &control);
        }
        if let Some(curve) = &capability_curve {
            eq.reference("SynchronousMachine.InitialReactiveCapabilityCurve", curve);
        }
        let fallback_container = terminal_voltage_level_mrid(detailed, record, machine.bus);
        let container = mapped_equipment_container_mrid(
            detailed,
            "generator",
            local_id,
            Some(fallback_container.clone()),
            &mut w.warnings,
        )
        .unwrap_or(fallback_container);
        eq.reference("Equipment.EquipmentContainer", &container);
        eq.close("SynchronousMachine");
        let term = terminal(
            &mut eq,
            &mut tp,
            &mut ssh,
            &mut sv,
            &id,
            "generator",
            local_id,
            record,
            1,
            machine.bus,
            machine.in_service,
            &mut w.warnings,
        );
        if emit_control {
            let regulating_terminal = machine
                .regulating_terminal
                .as_ref()
                .map(|reference| {
                    terminal_reference_mrid(net, detailed, reference).ok_or_else(|| {
                        emission_error(format!(
                            "generator `{local_id}` references terminal {} number {}, which cannot be identified in CGMES output",
                            reference.equipment, reference.terminal
                        ))
                    })
                })
                .transpose()?
                .unwrap_or_else(|| term.clone());
            let control_fallback_name = format!("gen{}-{i}-avr", machine.bus);
            eq.named(
                "RegulatingControl",
                &control,
                source_control
                    .and_then(|metadata| metadata.name.as_deref())
                    .unwrap_or(&control_fallback_name),
            );
            eq.enumeration("RegulatingControl.mode", w.p.cim_ns, source_control_mode);
            eq.reference("RegulatingControl.Terminal", &regulating_terminal);
            eq.close("RegulatingControl");
        }
        ssh.open("SynchronousMachine", &id, true);
        ssh.text("RegulatingCondEq.controlEnabled", machine_control_enabled);
        if !field_was_omitted(
            detailed,
            "generator",
            local_id,
            OmittedFieldName::ActivePower,
        ) {
            ssh.text("RotatingMachine.p", -machine.pg);
        }
        if !field_was_omitted(
            detailed,
            "generator",
            local_id,
            OmittedFieldName::ReactivePower,
        ) {
            ssh.text("RotatingMachine.q", -machine.qg);
        }
        let generated_reference_priority = i32::from(
            net.buses()
                .iter()
                .any(|b| b.id == machine.bus && b.kind == BusType::Ref),
        );
        ssh.text(
            "SynchronousMachine.referencePriority",
            machine_metadata
                .and_then(|metadata| {
                    metadata
                        .properties
                        .get("SynchronousMachine.referencePriority")
                })
                .map_or_else(|| generated_reference_priority.to_string(), Clone::clone),
        );
        ssh.enumeration(
            "SynchronousMachine.operatingMode",
            w.p.cim_ns,
            machine_metadata
                .and_then(|metadata| metadata.properties.get("SynchronousMachine.operatingMode"))
                .map_or("SynchronousMachineOperatingMode.generator", String::as_str),
        );
        if v3 {
            ssh.text("Equipment.inService", machine.in_service);
        }
        ssh.close("SynchronousMachine");
        if emit_control {
            let target_kv = machine.vg * w.kv(machine.regulated_bus.unwrap_or(machine.bus))?;
            let (target_value, target_multiplier) = retained_control_target(
                machine_metadata,
                target_kv,
                source_control_is_effective,
                machine.voltage_regulation_on,
                &mut w.warnings,
                local_id,
            );
            ssh.open("RegulatingControl", &control, true);
            ssh.text(
                "RegulatingControl.discrete",
                machine_metadata
                    .and_then(|metadata| metadata.properties.get("RegulatingControl.discrete"))
                    .map_or("false", String::as_str),
            );
            ssh.text("RegulatingControl.enabled", control_enabled);
            if !field_was_omitted(
                detailed,
                "generator",
                local_id,
                OmittedFieldName::VoltageSetpoint,
            ) {
                ssh.text("RegulatingControl.targetValue", target_value);
                ssh.enumeration(
                    "RegulatingControl.targetValueUnitMultiplier",
                    w.p.cim_ns,
                    &target_multiplier,
                );
            }
            if let Some(deadband) = machine_metadata
                .and_then(|metadata| metadata.properties.get("RegulatingControl.targetDeadband"))
            {
                ssh.text("RegulatingControl.targetDeadband", deadband);
            }
            ssh.close("RegulatingControl");
        }
        write_sv_status(&mut sv, &id, machine.in_service);
        if machine.regulating_terminal.is_none()
            && machine.regulated_bus.is_some_and(|b| b != machine.bus)
        {
            w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                "generator `{local_id}` names remote regulated bus {} without an exact regulating terminal; CGMES output uses the generator terminal",
                machine.regulated_bus.expect("checked present")
            ));
        }
        if machine.cost.is_some() {
            w.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "generator at bus {}: cost curves have no CGMES slot",
                    machine.bus
                ),
            );
        }
        if machine.has_caps() {
            w.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "generator at bus {}: capability/ramp columns have no CGMES slot",
                    machine.bus
                ),
            );
        }
    }
    for (index, storage) in net.storage().iter().enumerate() {
        if let Some(active_power_control) = &storage.active_power_control {
            let fallback = format!("{}-{index}", storage.bus);
            let id = storage.uid.as_deref().unwrap_or(&fallback);
            validate_active_power_control(
                id,
                active_power_control,
                -storage.charge_rating,
                storage.discharge_rating,
            )?;
            w.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!("storage `{id}`: active power control has no CGMES battery mapping"),
            );
        }
    }

    // --- shunts -------------------------------------------------------------
    for (i, shunt) in net.shunts().iter().enumerate() {
        let fallback = format!("{}-{i}", shunt.bus);
        let local_id = shunt.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("shunt", &fallback, shunt.uid.as_deref());
        let shunt_metadata = mapped_component_metadata(detailed, "shunt", local_id);
        let source_is_cgmes = shunt_metadata.is_some_and(has_cgmes_external_identifier);
        let source_control_id = shunt_metadata.and_then(|metadata| {
            metadata
                .properties
                .get(super::CGMES_REGULATING_CONTROL_PROPERTY)
        });
        let source_control = mapped_regulating_control_metadata(detailed, "shunt", local_id);
        let source_control_mode = shunt_metadata
            .and_then(|metadata| metadata.properties.get("RegulatingControl.mode"))
            .map_or("RegulatingControlModeKind.voltage", String::as_str);
        let source_equipment_control_enabled =
            retained_bool(shunt_metadata, "RegulatingCondEq.controlEnabled");
        let source_control_enabled = retained_bool(shunt_metadata, "RegulatingControl.enabled");
        let source_control_is_effective = source_control_id.is_some()
            && source_control_enabled.unwrap_or(false)
            && source_equipment_control_enabled.unwrap_or(true);
        let typed_control_is_effective = shunt
            .control
            .as_ref()
            .is_some_and(|control| control.mode != SwitchedShuntMode::Locked);
        let control_was_edited = source_is_cgmes
            && source_control_id.is_some()
            && source_control_is_effective != typed_control_is_effective;
        if control_was_edited {
            w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                "shunt `{local_id}` source RegulatingControl.enabled={} and RegulatingCondEq.controlEnabled={} represented automatic voltage control {}; the typed value is {}, so fresh CGMES output sets both enable flags to the typed value",
                source_control_enabled.map_or_else(|| "absent".into(), |value| value.to_string()),
                source_equipment_control_enabled
                    .map_or_else(|| "absent".into(), |value| value.to_string()),
                source_control_is_effective,
                typed_control_is_effective
            ));
        }
        let equipment_control_enabled = if control_was_edited {
            typed_control_is_effective
        } else {
            source_equipment_control_enabled.unwrap_or(typed_control_is_effective)
        };
        let control_enabled = if control_was_edited {
            typed_control_is_effective
        } else {
            source_control_enabled.unwrap_or(typed_control_is_effective)
        };
        let kv = w.kv(shunt.bus)?;
        let sections = expanded_shunt_sections(shunt);
        let sections = if sections.is_empty() {
            vec![(shunt.g, shunt.b)]
        } else {
            sections
        };
        let (calculated_section_count, section_error) = shunt_section_count(shunt, &sections);
        let section_count = match shunt.section_count {
            Some(section_count) => {
                let section_count = section_count as usize;
                if section_count > sections.len() {
                    return Err(emission_error(format!(
                        "shunt `{local_id}` assigned section count {section_count} exceeds its maximum section count {}",
                        sections.len()
                    )));
                }
                if section_count != calculated_section_count {
                    w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                        "shunt `{local_id}` assigned section count {section_count} does not match the section count {calculated_section_count} calculated from its conductance and susceptance; fresh CGMES uses the explicit assigned section count"
                    ));
                }
                section_count
            }
            None => calculated_section_count,
        };
        let section_scale = 1.0 + shunt.g.abs() + shunt.b.abs();
        if shunt.section_count.is_none() && section_error > 1e-9 * section_scale {
            w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                "shunt at bus {}: its assigned conductance and susceptance do not equal a prefix of its control blocks; CGMES uses the closest section count {}",
                shunt.bus, section_count
            ));
        }
        let linear = linear_shunt_sections(&sections);
        let class = if linear {
            "LinearShuntCompensator"
        } else {
            "NonlinearShuntCompensator"
        };
        let control_id = shunt.control.as_ref().map(|_| {
            source_control.map_or_else(
                || {
                    source_control_id
                        .filter(|value| uuid::Uuid::parse_str(value).is_ok())
                        .cloned()
                        .unwrap_or_else(|| det_mrid("regcontrol", &id))
                },
                |metadata| {
                    component_mrid(
                        detailed.expect(
                            "source RegulatingControl metadata requires detailed connectivity",
                        ),
                        &metadata.component,
                    )
                },
            )
        });
        let fallback_name = format!("shunt{}-{i}", shunt.bus);
        eq.named(
            class,
            &id,
            mapped_component_name(detailed, "shunt", local_id, &fallback_name),
        );
        if linear {
            eq.text(
                "LinearShuntCompensator.bPerSection",
                sections[0].1 / (kv * kv),
            );
            if !field_was_omitted(
                detailed,
                "shunt",
                local_id,
                OmittedFieldName::ShuntConductancePerSection,
            ) {
                eq.text(
                    "LinearShuntCompensator.gPerSection",
                    sections[0].0 / (kv * kv),
                );
            }
        }
        eq.text("ShuntCompensator.maximumSections", sections.len());
        eq.text("ShuntCompensator.normalSections", section_count);
        eq.text("ShuntCompensator.nomU", kv);
        if let Some(control) = &control_id {
            eq.reference("RegulatingCondEq.RegulatingControl", control);
        }
        let record = detailed.and_then(|value| detailed_terminal(value, "shunt", local_id, 1));
        let fallback_container = terminal_voltage_level_mrid(detailed, record, shunt.bus);
        let container = mapped_equipment_container_mrid(
            detailed,
            "shunt",
            local_id,
            Some(fallback_container.clone()),
            &mut w.warnings,
        )
        .unwrap_or(fallback_container);
        eq.reference("Equipment.EquipmentContainer", &container);
        eq.close(class);
        if !linear {
            for (section, (g, b)) in sections.iter().copied().enumerate() {
                let point = det_mrid("shunt_section", &format!("{id}:{}", section + 1));
                eq.open("NonlinearShuntCompensatorPoint", &point, false);
                eq.reference(
                    "NonlinearShuntCompensatorPoint.NonlinearShuntCompensator",
                    &id,
                );
                eq.text("NonlinearShuntCompensatorPoint.sectionNumber", section + 1);
                eq.text("NonlinearShuntCompensatorPoint.b", b / (kv * kv));
                if !field_was_omitted(
                    detailed,
                    "shunt",
                    local_id,
                    OmittedFieldName::ShuntConductancePerSection,
                ) {
                    eq.text("NonlinearShuntCompensatorPoint.g", g / (kv * kv));
                }
                eq.close("NonlinearShuntCompensatorPoint");
            }
        }
        let term = terminal(
            &mut eq,
            &mut tp,
            &mut ssh,
            &mut sv,
            &id,
            "shunt",
            local_id,
            record,
            1,
            shunt.bus,
            shunt.in_service,
            &mut w.warnings,
        );
        ssh.open(class, &id, true);
        ssh.text("ShuntCompensator.sections", section_count);
        ssh.text("RegulatingCondEq.controlEnabled", equipment_control_enabled);
        if v3 {
            ssh.text("Equipment.inService", shunt.in_service);
        }
        ssh.close(class);
        if let (Some(control), Some(control_id)) = (&shunt.control, control_id) {
            let control_fallback_name = format!("shunt{}-{i}-control", shunt.bus);
            eq.named(
                "RegulatingControl",
                &control_id,
                source_control
                    .and_then(|metadata| metadata.name.as_deref())
                    .unwrap_or(&control_fallback_name),
            );
            eq.enumeration("RegulatingControl.mode", w.p.cim_ns, source_control_mode);
            let regulating_terminal = control
                .regulating_terminal
                .as_ref()
                .map(|reference| {
                    terminal_reference_mrid(net, detailed, reference).ok_or_else(|| {
                        emission_error(format!(
                            "shunt `{local_id}` references terminal {} number {}, which cannot be identified in CGMES output",
                            reference.equipment, reference.terminal
                        ))
                    })
                })
                .transpose()?
                .unwrap_or_else(|| term.clone());
            eq.reference("RegulatingControl.Terminal", &regulating_terminal);
            eq.close("RegulatingControl");
            let controlled_bus = control.control_bus.unwrap_or(shunt.bus);
            let controlled_kv = w.kv(controlled_bus)?;
            let target_kv = (control.vhigh + control.vlow) * controlled_kv / 2.0;
            let deadband_kv = (control.vhigh - control.vlow).abs() * controlled_kv;
            let (target_value, target_multiplier) = retained_control_target(
                shunt_metadata,
                target_kv,
                source_control_is_effective,
                typed_control_is_effective,
                &mut w.warnings,
                local_id,
            );
            let scale_to_kv = unit_multiplier_scale_to_kv(&target_multiplier)
                .expect("retained_control_target returns a supported multiplier");
            let source_deadband = shunt_metadata
                .and_then(|metadata| metadata.properties.get("RegulatingControl.targetDeadband"));
            let source_deadband_kv = source_deadband
                .and_then(|value| value.parse::<f64>().ok())
                .map(|value| value * scale_to_kv);
            let deadband_unchanged = source_deadband_kv.is_some_and(|value| {
                let tolerance = 1e-9 * value.abs().max(deadband_kv.abs()).max(1.0);
                (value - deadband_kv).abs() <= tolerance
            }) || (!source_control_is_effective
                && !typed_control_is_effective);
            let target_deadband = if deadband_unchanged {
                source_deadband.cloned()
            } else {
                Some((deadband_kv / scale_to_kv).to_string())
            };
            ssh.open("RegulatingControl", &control_id, true);
            ssh.text(
                "RegulatingControl.discrete",
                retained_bool(shunt_metadata, "RegulatingControl.discrete")
                    .unwrap_or(control.mode == SwitchedShuntMode::Discrete),
            );
            ssh.text("RegulatingControl.enabled", control_enabled);
            ssh.text("RegulatingControl.targetValue", target_value);
            ssh.enumeration(
                "RegulatingControl.targetValueUnitMultiplier",
                w.p.cim_ns,
                &target_multiplier,
            );
            if let Some(target_deadband) = target_deadband {
                ssh.text("RegulatingControl.targetDeadband", target_deadband);
            }
            ssh.close("RegulatingControl");
            if control.regulating_terminal.is_none()
                && control.control_bus.is_some_and(|bus| bus != shunt.bus)
            {
                w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                    "shunt at bus {}: remote regulated bus {} is written as local regulation because the balanced model does not identify a terminal on the regulated equipment",
                    shunt.bus, controlled_bus
                ));
            }
        }
        write_sv_status(&mut sv, &id, shunt.in_service);
        let solved_sections = det_mrid("svshuntsections", &id);
        sv.open("SvShuntCompensatorSections", &solved_sections, false);
        sv.reference("SvShuntCompensatorSections.ShuntCompensator", &id);
        sv.text("SvShuntCompensatorSections.sections", section_count);
        sv.close("SvShuntCompensatorSections");
    }

    // --- static VAR compensators ---------------------------------------------
    for (i, svc) in net.static_var_compensators().iter().enumerate() {
        let fallback = format!("{}-{i}", svc.bus);
        let local_id = svc.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("static_var_compensator", &fallback, svc.uid.as_deref());
        let svc_metadata = mapped_component_metadata(detailed, "static_var_compensator", local_id);
        let source_is_cgmes = svc_metadata.is_some_and(has_cgmes_external_identifier);
        let source_control_id = svc_metadata.and_then(|metadata| {
            metadata
                .properties
                .get(super::CGMES_REGULATING_CONTROL_PROPERTY)
        });
        let source_control =
            mapped_regulating_control_metadata(detailed, "static_var_compensator", local_id);
        let source_control_mode = svc_metadata
            .and_then(|metadata| metadata.properties.get("RegulatingControl.mode"))
            .map_or_else(
                || match svc.regulation_mode {
                    StaticVarCompensatorRegulationMode::Voltage => {
                        "RegulatingControlModeKind.voltage"
                    }
                    StaticVarCompensatorRegulationMode::ReactivePower => {
                        "RegulatingControlModeKind.reactivePower"
                    }
                },
                String::as_str,
            );
        let source_equipment_control_enabled =
            retained_bool(svc_metadata, "RegulatingCondEq.controlEnabled");
        let source_control_enabled = retained_bool(svc_metadata, "RegulatingControl.enabled");
        let source_control_is_effective = source_control_id.is_some()
            && source_control_enabled.unwrap_or(false)
            && source_equipment_control_enabled.unwrap_or(true);
        let control_was_edited = source_is_cgmes
            && source_control_id.is_some()
            && source_control_is_effective != svc.regulating;
        if control_was_edited {
            w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                "static var compensator `{local_id}` source RegulatingControl.enabled={} and RegulatingCondEq.controlEnabled={} represented regulation {}; the typed value is {}, so fresh CGMES output sets both enable flags to the typed value",
                source_control_enabled.map_or_else(|| "absent".into(), |value| value.to_string()),
                source_equipment_control_enabled
                    .map_or_else(|| "absent".into(), |value| value.to_string()),
                source_control_is_effective,
                svc.regulating
            ));
        }
        let equipment_control_enabled = if control_was_edited {
            svc.regulating
        } else {
            source_equipment_control_enabled.unwrap_or(svc.regulating)
        };
        let control_enabled = if control_was_edited {
            svc.regulating
        } else {
            source_control_enabled.unwrap_or(svc.regulating)
        };
        let control = source_control.map_or_else(
            || {
                source_control_id
                    .filter(|value| uuid::Uuid::parse_str(value).is_ok())
                    .cloned()
                    .unwrap_or_else(|| det_mrid("regcontrol", &id))
            },
            |metadata| {
                component_mrid(
                    detailed
                        .expect("source RegulatingControl metadata requires detailed connectivity"),
                    &metadata.component,
                )
            },
        );
        let record = detailed
            .and_then(|value| detailed_terminal(value, "static_var_compensator", local_id, 1));
        let fallback_name = format!("svc{}-{i}", svc.bus);
        eq.named(
            "StaticVarCompensator",
            &id,
            mapped_component_name(detailed, "static_var_compensator", local_id, &fallback_name),
        );
        let fallback_container = terminal_voltage_level_mrid(detailed, record, svc.bus);
        let container = mapped_equipment_container_mrid(
            detailed,
            "static_var_compensator",
            local_id,
            Some(fallback_container.clone()),
            &mut w.warnings,
        )
        .unwrap_or(fallback_container);
        eq.reference("Equipment.EquipmentContainer", &container);
        eq.reference("RegulatingCondEq.RegulatingControl", &control);
        eq.text(
            "StaticVarCompensator.inductiveRating",
            if svc.b_min_siemens == 0.0 {
                0.0
            } else {
                1.0 / svc.b_min_siemens
            },
        );
        eq.text(
            "StaticVarCompensator.capacitiveRating",
            if svc.b_max_siemens == 0.0 {
                0.0
            } else {
                1.0 / svc.b_max_siemens
            },
        );
        eq.text("StaticVarCompensator.slope", 0.0);
        eq.enumeration(
            "StaticVarCompensator.sVCControlMode",
            w.p.cim_ns,
            match svc.regulation_mode {
                StaticVarCompensatorRegulationMode::Voltage => "SVCControlMode.voltage",
                StaticVarCompensatorRegulationMode::ReactivePower => "SVCControlMode.reactivePower",
            },
        );
        eq.text(
            "StaticVarCompensator.voltageSetPoint",
            svc.voltage_setpoint_kv,
        );
        eq.close("StaticVarCompensator");
        let local_terminal = terminal(
            &mut eq,
            &mut tp,
            &mut ssh,
            &mut sv,
            &id,
            "static_var_compensator",
            local_id,
            record,
            1,
            svc.bus,
            svc.in_service,
            &mut w.warnings,
        );
        eq.named(
            "RegulatingControl",
            &control,
            source_control
                .and_then(|metadata| metadata.name.as_deref())
                .unwrap_or(&fallback_name),
        );
        eq.enumeration("RegulatingControl.mode", w.p.cim_ns, source_control_mode);
        let regulation_terminal = svc
            .regulating_terminal
            .as_ref()
            .and_then(|value| terminal_reference_mrid(net, detailed, value))
            .unwrap_or_else(|| local_terminal.clone());
        eq.reference("RegulatingControl.Terminal", &regulation_terminal);
        eq.close("RegulatingControl");

        ssh.open("StaticVarCompensator", &id, true);
        ssh.text("StaticVarCompensator.q", svc.q);
        ssh.text("RegulatingCondEq.controlEnabled", equipment_control_enabled);
        if v3 {
            ssh.text("Equipment.inService", svc.in_service);
        }
        ssh.close("StaticVarCompensator");
        ssh.open("RegulatingControl", &control, true);
        ssh.text(
            "RegulatingControl.discrete",
            retained_bool(svc_metadata, "RegulatingControl.discrete").unwrap_or(false),
        );
        ssh.text("RegulatingControl.enabled", control_enabled);
        let typed_target = match svc.regulation_mode {
            StaticVarCompensatorRegulationMode::Voltage => svc.voltage_setpoint_kv,
            StaticVarCompensatorRegulationMode::ReactivePower => svc.reactive_power_setpoint_mvar,
        };
        let (target_value, target_multiplier) = retained_control_target(
            svc_metadata,
            typed_target,
            source_control_is_effective,
            svc.regulating,
            &mut w.warnings,
            local_id,
        );
        ssh.text("RegulatingControl.targetValue", target_value);
        ssh.enumeration(
            "RegulatingControl.targetValueUnitMultiplier",
            w.p.cim_ns,
            &target_multiplier,
        );
        if let Some(deadband) = svc_metadata
            .and_then(|metadata| metadata.properties.get("RegulatingControl.targetDeadband"))
        {
            ssh.text("RegulatingControl.targetDeadband", deadband);
        }
        ssh.close("RegulatingControl");

        if detailed.is_none() {
            write_sv_power_flow(
                &mut sv,
                &local_terminal,
                Some(svc.p),
                Some(svc.q),
                &mut w.warnings,
            );
        }
        write_sv_status(&mut sv, &id, svc.in_service);
    }

    // --- switches -------------------------------------------------------------
    if detailed.is_none_or(|value| value.switches.is_empty()) {
        for (i, switch) in net.switches().iter().enumerate() {
            let fallback = format!("{}-{}-{i}", switch.from, switch.to);
            let local_id = switch.uid.as_deref().unwrap_or(&fallback);
            let id = mrid_or("switch", &fallback, switch.uid.as_deref());
            eq.named(
                "Breaker",
                &id,
                &format!("switch{}-{}-{i}", switch.from, switch.to),
            );
            eq.text("Switch.normalOpen", !switch.closed);
            eq.text("Switch.retained", true);
            if let Some(amps) = switch.current_rating {
                eq.text("Switch.ratedCurrent", amps);
            }
            eq.close("Breaker");
            let first_source =
                detailed.and_then(|value| detailed_terminal(value, "switch", local_id, 1));
            let mut first_record = first_source.cloned();
            if let Some(record) = first_record.as_mut() {
                record.active_power_mw = record.active_power_mw.or(switch.pf);
                record.reactive_power_mvar = record.reactive_power_mvar.or(switch.qf);
            }
            let first_terminal = terminal(
                &mut eq,
                &mut tp,
                &mut ssh,
                &mut sv,
                &id,
                "switch",
                local_id,
                first_record.as_ref(),
                1,
                switch.from,
                true,
                &mut w.warnings,
            );
            let second_source =
                detailed.and_then(|value| detailed_terminal(value, "switch", local_id, 2));
            let mut second_record = second_source.cloned();
            if let Some(record) = second_record.as_mut() {
                record.active_power_mw = record.active_power_mw.or(switch.pt);
                record.reactive_power_mvar = record.reactive_power_mvar.or(switch.qt);
            }
            let second_terminal = terminal(
                &mut eq,
                &mut tp,
                &mut ssh,
                &mut sv,
                &id,
                "switch",
                local_id,
                second_record.as_ref(),
                2,
                switch.to,
                true,
                &mut w.warnings,
            );
            if first_source.is_none() {
                write_sv_power_flow(
                    &mut sv,
                    &first_terminal,
                    switch.pf,
                    switch.qf,
                    &mut w.warnings,
                );
            }
            if second_source.is_none() {
                write_sv_power_flow(
                    &mut sv,
                    &second_terminal,
                    switch.pt,
                    switch.qt,
                    &mut w.warnings,
                );
            }
            if let Some(rating) = switch.thermal_rating {
                w.warnings.push_as(
                    &codes::EMIT_CGMES.field_dropped,
                    format!(
                        "switch `{local_id}` thermal rating {rating} MVA was not emitted: CGMES Switch carries rated current, not an apparent power limit"
                    ),
                );
            }
            ssh.open("Breaker", &id, true);
            ssh.text("Switch.open", !switch.closed);
            ssh.close("Breaker");
        }
    }

    // --- branches ---------------------------------------------------------------
    let mut limit_body = Doc::new(Arc::clone(&retained_metadata));
    for (i, branch) in net.branches().iter().enumerate() {
        let fallback = format!("{}-{}-{i}", branch.from, branch.to);
        let local_id = branch.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("branch", &fallback, branch.uid.as_deref());
        let source_terminal =
            detailed.and_then(|details| detailed_terminal(details, "branch", local_id, 1));
        let fallback_container =
            terminal_voltage_level_mrid(detailed, source_terminal, branch.from);
        let source_container = mapped_equipment_container_mrid(
            detailed,
            "branch",
            local_id,
            Some(fallback_container),
            &mut w.warnings,
        );
        let kv = w.kv(branch.from)?;
        let z_base = kv * kv / net.base_mva();
        let y_base = net.base_mva() / (kv * kv);
        let charging = branch.calc_terminal_charging();
        if !charging.is_matpower_symmetric() {
            w.warnings.push_as(
                &codes::EMIT_CGMES.value_collapsed,
                format!(
                    "branch {} ({}-{}): asymmetric terminal charging folded into the \
                 symmetric bch/gch totals",
                    i + 1,
                    branch.from,
                    branch.to
                ),
            );
        }
        if branch.control.is_some() {
            w.warnings.push_as(
                &codes::EMIT_CGMES.field_dropped,
                format!(
                    "branch {} ({}-{}): automatic tap/phase control data is not \
                 written (fixed in-service step only)",
                    i + 1,
                    branch.from,
                    branch.to
                ),
            );
        }
        let source_metadata = detailed.and_then(|detailed| {
            detailed.component_metadata.iter().find(|metadata| {
                metadata.component.component_type() == "branch"
                    && metadata.component.local_id() == local_id
            })
        });
        let source_is_series_compensator = source_metadata.is_some_and(|metadata| {
            metadata
                .properties
                .get(CGMES_CLASS_PROPERTY)
                .map(String::as_str)
                == Some("SeriesCompensator")
        });
        let emits_as_transformer =
            branch.is_transformer() || source_branch_is_power_transformer(detailed, local_id);
        let series_compensator_has_charging = source_is_series_compensator
            && [charging.g_fr, charging.b_fr, charging.g_to, charging.b_to]
                .into_iter()
                .any(|value| value.abs() > f64::EPSILON);
        if series_compensator_has_charging {
            let unrepresented = [
                "SeriesCompensator.r0",
                "SeriesCompensator.x0",
                "SeriesCompensator.varistorPresent",
                "SeriesCompensator.varistorRatedCurrent",
                "SeriesCompensator.varistorVoltageThreshold",
            ]
            .into_iter()
            .filter(|property| {
                source_metadata.is_some_and(|metadata| metadata.properties.contains_key(*property))
            })
            .collect::<Vec<_>>();
            let fields = if unrepresented.is_empty() {
                "no r0, x0, or varistor fields were present".to_owned()
            } else {
                format!("unrepresented source fields: {}", unrepresented.join(", "))
            };
            w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                "SeriesCompensator `{local_id}` has nonzero shunt charging and is written as ACLineSegment: positive-sequence r/x, charging, terminal connectivity, service status, and limits are preserved; the equipment class is projected; {fields}"
            ));
        }
        if !emits_as_transformer {
            if source_is_series_compensator && !series_compensator_has_charging {
                let fallback_name = branch
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("line{}", i + 1));
                eq.named(
                    "SeriesCompensator",
                    &id,
                    mapped_component_name(detailed, "branch", local_id, &fallback_name),
                );
                eq.text("SeriesCompensator.r", branch.r * z_base);
                eq.text("SeriesCompensator.x", branch.x * z_base);
                for property in [
                    "SeriesCompensator.r0",
                    "SeriesCompensator.x0",
                    "SeriesCompensator.varistorPresent",
                    "SeriesCompensator.varistorRatedCurrent",
                    "SeriesCompensator.varistorVoltageThreshold",
                ] {
                    if let Some(value) =
                        source_metadata.and_then(|metadata| metadata.properties.get(property))
                    {
                        eq.text(property, value);
                    }
                }
                eq.reference("ConductingEquipment.BaseVoltage", &base_of(kv));
                if let Some(container) = &source_container {
                    eq.reference("Equipment.EquipmentContainer", container);
                }
                eq.close("SeriesCompensator");
            } else {
                let fallback_name = branch
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("line{}", i + 1));
                eq.named(
                    "ACLineSegment",
                    &id,
                    mapped_component_name(detailed, "branch", local_id, &fallback_name),
                );
                eq.text("ACLineSegment.r", branch.r * z_base);
                eq.text("ACLineSegment.x", branch.x * z_base);
                eq.text("ACLineSegment.bch", branch.calc_total_charging_b() * y_base);
                let g_total = charging.calc_total_g();
                if g_total != 0.0 {
                    eq.text("ACLineSegment.gch", g_total * y_base);
                }
                eq.reference("ConductingEquipment.BaseVoltage", &base_of(kv));
                if let Some(container) = &source_container {
                    eq.reference("Equipment.EquipmentContainer", container);
                }
                eq.close("ACLineSegment");
            }
        } else {
            // Two-winding transformer: the MATPOWER tap folds into the end-1
            // rated voltage (reader ratio = (u1/kv1)/(u2/kv2)); the phase
            // shift rides a one-step linear phase tap changer on end 1.
            let source_ratio_1 = source_tap_changer(detailed, local_id, 1, TapChangerKind::Ratio);
            let source_ratio_2 = source_tap_changer(detailed, local_id, 2, TapChangerKind::Ratio);
            let rho_1 = source_ratio_1
                .and_then(tap_step)
                .map_or(1.0, |value| value.rho);
            let rho_2 = source_ratio_2
                .and_then(tap_step)
                .map_or(1.0, |value| value.rho);
            let (u1, u2) = (
                kv * branch.calc_effective_tap() * rho_2 / rho_1,
                w.kv(branch.to)?,
            );
            let source_phase_1 = source_tap_changer(detailed, local_id, 1, TapChangerKind::Phase);
            let source_phase_2 = source_tap_changer(detailed, local_id, 2, TapChangerKind::Phase);
            let source_shift = source_phase_1
                .and_then(tap_step)
                .map_or(0.0, |value| value.alpha_degrees)
                - source_phase_2
                    .and_then(tap_step)
                    .map_or(0.0, |value| value.alpha_degrees);
            let has_source_phase = source_phase_1.is_some() || source_phase_2.is_some();
            if has_source_phase && (source_shift - branch.shift).abs() > 1e-9 {
                w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                    "branch {} ({}-{}): fixed phase shift {} degrees differs from the retained tap changer position value {} degrees; the tap changer definition was emitted",
                    i + 1,
                    branch.from,
                    branch.to,
                    branch.shift,
                    source_shift
                ));
            }
            let fallback_name = branch
                .name
                .clone()
                .unwrap_or_else(|| format!("transformer{}", i + 1));
            eq.named(
                "PowerTransformer",
                &id,
                mapped_component_name(detailed, "branch", local_id, &fallback_name),
            );
            if let Some(container) = &source_container {
                eq.reference("Equipment.EquipmentContainer", container);
            }
            eq.close("PowerTransformer");
            for (endno, u) in [(1usize, u1), (2usize, u2)] {
                let end = transformer_end_mrid(detailed, "branch", local_id, &id, endno)?;
                eq.named(
                    "PowerTransformerEnd",
                    &end,
                    &format!("transformer{}-end{endno}", i + 1),
                );
                eq.reference("PowerTransformerEnd.PowerTransformer", &id);
                eq.text("TransformerEnd.endNumber", endno);
                let source_terminal = detailed
                    .and_then(|details| detailed_terminal(details, "branch", local_id, endno));
                eq.reference(
                    "TransformerEnd.Terminal",
                    &terminal_mrid(detailed, source_terminal, &id, endno),
                );
                eq.text("PowerTransformerEnd.ratedU", u);
                let (terminal_g, terminal_b) = if endno == 1 {
                    (charging.g_fr, charging.b_fr)
                } else {
                    (charging.g_to, charging.b_to)
                };
                let end_z_base = u * u / net.base_mva();
                if endno == 1 {
                    eq.text("PowerTransformerEnd.r", branch.r * end_z_base);
                    eq.text("PowerTransformerEnd.x", branch.x * end_z_base);
                } else {
                    eq.text("PowerTransformerEnd.r", 0.0);
                    eq.text("PowerTransformerEnd.x", 0.0);
                }
                eq.text("PowerTransformerEnd.g", terminal_g / end_z_base);
                eq.text("PowerTransformerEnd.b", terminal_b / end_z_base);
                eq.close("PowerTransformerEnd");
            }
            if branch.shift != 0.0 && !has_source_phase {
                let ptc = det_mrid("ptc", &id);
                eq.named(
                    "PhaseTapChangerLinear",
                    &ptc,
                    &format!("transformer{}-shift", i + 1),
                );
                eq.text("TapChanger.lowStep", 0);
                eq.text("TapChanger.highStep", 2);
                eq.text("TapChanger.neutralStep", 0);
                eq.text("TapChanger.normalStep", 1);
                eq.text("TapChanger.neutralU", u1);
                eq.text("TapChanger.ltcFlag", false);
                eq.reference(
                    "PhaseTapChanger.TransformerEnd",
                    &transformer_end_mrid(detailed, "branch", local_id, &id, 1)?,
                );
                eq.text(
                    "PhaseTapChangerLinear.stepPhaseShiftIncrement",
                    branch.shift,
                );
                eq.text("PhaseTapChangerLinear.xMin", branch.x * z_base);
                eq.text("PhaseTapChangerLinear.xMax", branch.x * z_base);
                eq.close("PhaseTapChangerLinear");
                ssh.open("PhaseTapChangerLinear", &ptc, true);
                ssh.text("TapChanger.step", 1);
                ssh.text("TapChanger.controlEnabled", false);
                ssh.close("PhaseTapChangerLinear");
                sv.open("SvTapStep", &det_mrid("svtap", &ptc), false);
                sv.reference("SvTapStep.TapChanger", &ptc);
                sv.text("SvTapStep.position", 1);
                sv.close("SvTapStep");
            }
        }
        let first_terminal = terminal(
            &mut eq,
            &mut tp,
            &mut ssh,
            &mut sv,
            &id,
            "branch",
            local_id,
            None,
            1,
            branch.from,
            branch.in_service,
            &mut w.warnings,
        );
        let second_terminal = terminal(
            &mut eq,
            &mut tp,
            &mut ssh,
            &mut sv,
            &id,
            "branch",
            local_id,
            None,
            2,
            branch.to,
            branch.in_service,
            &mut w.warnings,
        );
        if detailed.is_none()
            && let Some(solution) = branch.solution.as_ref()
        {
            write_sv_power_flow(
                &mut sv,
                &first_terminal,
                Some(solution.pf),
                Some(solution.qf),
                &mut w.warnings,
            );
            write_sv_power_flow(
                &mut sv,
                &second_terminal,
                Some(solution.pt),
                Some(solution.qt),
                &mut w.warnings,
            );
        }
        if v3 {
            let class = if emits_as_transformer {
                "PowerTransformer"
            } else if source_is_series_compensator && !series_compensator_has_charging {
                "SeriesCompensator"
            } else {
                "ACLineSegment"
            };
            ssh.open(class, &id, true);
            ssh.text("Equipment.inService", branch.in_service);
            ssh.close(class);
        }
        write_sv_status(&mut sv, &id, branch.in_service);

        let has_source_limits = detailed.is_some_and(|value| {
            value.operational_limit_groups.iter().any(|group| {
                group.equipment.component_type() == "branch"
                    && group.equipment.local_id() == local_id
            })
        });
        if !has_source_limits {
            // PATL/TATL/TC current limits at terminal 1 through √3·kV.
            let mut rate = |mva: f64, kind: &'static str| {
                if mva <= 0.0 {
                    return;
                }
                if !limit_types_used.contains(&kind) {
                    limit_types_used.push(kind);
                }
                let set = det_mrid("limitset", &format!("{id}:{kind}"));
                limit_body.named(
                    "OperationalLimitSet",
                    &set,
                    &format!("limits-{}-{kind}", i + 1),
                );
                let source_terminal =
                    detailed.and_then(|details| detailed_terminal(details, "branch", local_id, 1));
                limit_body.reference(
                    "OperationalLimitSet.Terminal",
                    &terminal_mrid(detailed, source_terminal, &id, 1),
                );
                limit_body.close("OperationalLimitSet");
                let lim = det_mrid("limit", &format!("{id}:{kind}"));
                limit_body.named("CurrentLimit", &lim, &format!("rate-{}-{kind}", i + 1));
                limit_body.reference("OperationalLimit.OperationalLimitSet", &set);
                limit_body.reference(
                    "OperationalLimit.OperationalLimitType",
                    &det_mrid("limittype", kind),
                );
                let amps = mva * 1000.0 / (3f64.sqrt() * kv);
                if v3 {
                    limit_body.text("CurrentLimit.normalValue", amps);
                } else {
                    limit_body.text("CurrentLimit.value", amps);
                }
                limit_body.close("CurrentLimit");
            };
            rate(branch.rate_a, "patl");
            rate(branch.rate_b, "tatl");
            rate(branch.rate_c, "tc");
            if !branch.rating_sets.is_empty() || branch.current_ratings.is_some() {
                w.warnings.push_as(
                    &codes::EMIT_CGMES.rating_set_dropped,
                    format!(
                        "branch {} ({}-{}): extra rating sets / current ratings beyond \
                     A/B/C have no CGMES slot",
                        i + 1,
                        branch.from,
                        branch.to
                    ),
                );
            }
        }
    }

    // --- three winding transformers ------------------------------------------
    for (i, transformer) in net.transformers_3w().iter().enumerate() {
        let fallback = format!("transformer3w-{i}");
        let local_id = transformer.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("transformer_3w", &fallback, transformer.uid.as_deref());
        eq.named(
            "PowerTransformer",
            &id,
            transformer.name.as_deref().unwrap_or(local_id),
        );
        let source_terminal =
            detailed.and_then(|details| detailed_terminal(details, "branch", local_id, 1));
        let fallback_container =
            terminal_voltage_level_mrid(detailed, source_terminal, transformer.windings[0].bus);
        if let Some(container) = mapped_equipment_container_mrid(
            detailed,
            "branch",
            local_id,
            Some(fallback_container),
            &mut w.warnings,
        ) {
            eq.reference("Equipment.EquipmentContainer", &container);
        }
        eq.close("PowerTransformer");

        let star_impedances = transformer.calc_star_impedances();
        for (index, winding) in transformer.windings.iter().enumerate() {
            let end_number = index + 1;
            if let Some(control) = &winding.control {
                w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                    "three winding transformer `{local_id}` winding {end_number}: automatic transformer control {:?} (enabled={}, tap range {} to {}, controlled band {} to {}, {} positions) has no CGMES writer mapping and was not emitted",
                    control.mode,
                    control.enabled,
                    control.tap_min,
                    control.tap_max,
                    control.band_min,
                    control.band_max,
                    control.ntp
                ));
            }
            let end = transformer_end_mrid(detailed, "branch", local_id, &id, end_number)?;
            let bus_kv = w.kv(winding.bus)?;
            let rated_kv = if winding.nominal_kv > 0.0 {
                winding.nominal_kv
            } else {
                bus_kv
            };
            let z_base = rated_kv * rated_kv / net.base_mva();
            let (star_r, star_x) = star_impedances[index];
            let base_ratio = rated_kv / bus_kv;
            let tap_factor = if base_ratio > 0.0 {
                winding.tap / base_ratio
            } else {
                winding.tap
            };
            let source_ratio =
                source_tap_changer(detailed, local_id, end_number, TapChangerKind::Ratio);
            let source_phase =
                source_tap_changer(detailed, local_id, end_number, TapChangerKind::Phase);
            let ratio_tap = source_ratio.is_none() && (tap_factor - 1.0).abs() > 1e-12;
            let phase_tap = source_phase.is_none() && winding.shift.abs() > 1e-12;
            if let Some(source_ratio) = source_ratio
                && let Some(step) = tap_step(source_ratio)
                && (step.rho - tap_factor).abs() > 1e-9
            {
                w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                    "three winding transformer `{local_id}` winding {end_number}: fixed tap ratio {tap_factor} differs from the retained tap changer position value {}; the tap changer definition was emitted",
                    step.rho
                ));
            }
            if let Some(source_phase) = source_phase
                && let Some(step) = tap_step(source_phase)
                && (step.alpha_degrees - winding.shift).abs() > 1e-9
            {
                w.warnings.push_as(&codes::EMIT_CGMES.value_substituted, format!(
                    "three winding transformer `{local_id}` winding {end_number}: fixed phase shift {} degrees differs from the retained tap changer position value {} degrees; the tap changer definition was emitted",
                    winding.shift, step.alpha_degrees
                ));
            }

            eq.named(
                "PowerTransformerEnd",
                &end,
                &format!(
                    "{}-end{end_number}",
                    transformer.name.as_deref().unwrap_or(local_id)
                ),
            );
            eq.reference("PowerTransformerEnd.PowerTransformer", &id);
            eq.text("TransformerEnd.endNumber", end_number);
            let source_terminal = detailed
                .and_then(|details| detailed_terminal(details, "branch", local_id, end_number));
            eq.reference(
                "TransformerEnd.Terminal",
                &terminal_mrid(detailed, source_terminal, &id, end_number),
            );
            eq.text("PowerTransformerEnd.ratedU", rated_kv);
            eq.text("PowerTransformerEnd.r", star_r * z_base);
            eq.text("PowerTransformerEnd.x", star_x * z_base);
            if index == 0 && transformer.mag_g != 0.0 {
                eq.text("PowerTransformerEnd.g", transformer.mag_g / z_base);
            }
            eq.text(
                "PowerTransformerEnd.b",
                if index == 0 {
                    transformer.mag_b / z_base
                } else {
                    0.0
                },
            );
            eq.close("PowerTransformerEnd");

            if ratio_tap {
                let tap = det_mrid("rtc", &format!("{id}:{end_number}"));
                eq.named(
                    "RatioTapChanger",
                    &tap,
                    &format!(
                        "{}-ratio-{end_number}",
                        transformer.name.as_deref().unwrap_or(local_id)
                    ),
                );
                eq.text("TapChanger.lowStep", 0);
                eq.text("TapChanger.highStep", 2);
                eq.text("TapChanger.neutralStep", 0);
                eq.text("TapChanger.normalStep", 1);
                eq.text("TapChanger.neutralU", rated_kv);
                eq.text("TapChanger.ltcFlag", false);
                eq.reference("RatioTapChanger.TransformerEnd", &end);
                eq.text(
                    "RatioTapChanger.stepVoltageIncrement",
                    (tap_factor - 1.0) * 100.0,
                );
                eq.close("RatioTapChanger");
                ssh.open("RatioTapChanger", &tap, true);
                ssh.text("TapChanger.step", 1);
                ssh.text("TapChanger.controlEnabled", false);
                ssh.close("RatioTapChanger");
                sv.open("SvTapStep", &det_mrid("svtap", &tap), false);
                sv.reference("SvTapStep.TapChanger", &tap);
                sv.text("SvTapStep.position", 1);
                sv.close("SvTapStep");
            }
            if phase_tap {
                let tap = det_mrid("ptc", &format!("{id}:{end_number}"));
                eq.named(
                    "PhaseTapChangerLinear",
                    &tap,
                    &format!(
                        "{}-phase-{end_number}",
                        transformer.name.as_deref().unwrap_or(local_id)
                    ),
                );
                eq.text("TapChanger.lowStep", 0);
                eq.text("TapChanger.highStep", 2);
                eq.text("TapChanger.neutralStep", 0);
                eq.text("TapChanger.normalStep", 1);
                eq.text("TapChanger.neutralU", rated_kv);
                eq.text("TapChanger.ltcFlag", false);
                eq.reference("PhaseTapChanger.TransformerEnd", &end);
                eq.text(
                    "PhaseTapChangerLinear.stepPhaseShiftIncrement",
                    winding.shift,
                );
                eq.text("PhaseTapChangerLinear.xMin", star_x * z_base);
                eq.text("PhaseTapChangerLinear.xMax", star_x * z_base);
                eq.close("PhaseTapChangerLinear");
                ssh.open("PhaseTapChangerLinear", &tap, true);
                ssh.text("TapChanger.step", 1);
                ssh.text("TapChanger.controlEnabled", false);
                ssh.close("PhaseTapChangerLinear");
                sv.open("SvTapStep", &det_mrid("svtap", &tap), false);
                sv.reference("SvTapStep.TapChanger", &tap);
                sv.text("SvTapStep.position", 1);
                sv.close("SvTapStep");
            }

            terminal(
                &mut eq,
                &mut tp,
                &mut ssh,
                &mut sv,
                &id,
                "branch",
                local_id,
                None,
                end_number,
                winding.bus,
                transformer.in_service,
                &mut w.warnings,
            );

            let has_source_limits = detailed.is_some_and(|value| {
                value.operational_limit_groups.iter().any(|group| {
                    group.equipment.component_type() == "branch"
                        && group.equipment.local_id() == local_id
                        && usize::from(group.terminal) == end_number
                })
            });
            if !has_source_limits {
                for (mva, kind) in [
                    (winding.rate_a, "patl"),
                    (winding.rate_b, "tatl"),
                    (winding.rate_c, "tc"),
                ] {
                    if mva <= 0.0 {
                        continue;
                    }
                    if !limit_types_used.contains(&kind) {
                        limit_types_used.push(kind);
                    }
                    let set = det_mrid("limitset", &format!("{id}:{end_number}:{kind}"));
                    limit_body.named(
                        "OperationalLimitSet",
                        &set,
                        &format!("limits-3w-{}-{end_number}-{kind}", i + 1),
                    );
                    limit_body.reference(
                        "OperationalLimitSet.Terminal",
                        &terminal_mrid(detailed, source_terminal, &id, end_number),
                    );
                    limit_body.close("OperationalLimitSet");
                    let limit = det_mrid("limit", &format!("{id}:{end_number}:{kind}"));
                    limit_body.named(
                        "CurrentLimit",
                        &limit,
                        &format!("rate-3w-{}-{end_number}-{kind}", i + 1),
                    );
                    limit_body.reference("OperationalLimit.OperationalLimitSet", &set);
                    limit_body.reference(
                        "OperationalLimit.OperationalLimitType",
                        &det_mrid("limittype", kind),
                    );
                    let amps = mva * 1000.0 / (3f64.sqrt() * bus_kv);
                    if v3 {
                        limit_body.text("CurrentLimit.normalValue", amps);
                    } else {
                        limit_body.text("CurrentLimit.value", amps);
                    }
                    limit_body.close("CurrentLimit");
                }
            }
        }
        if v3 {
            ssh.open("PowerTransformer", &id, true);
            ssh.text("Equipment.inService", transformer.in_service);
            ssh.close("PowerTransformer");
        }
        write_sv_status(&mut sv, &id, transformer.in_service);
    }

    let mut source_limit_types = Vec::new();
    if let Some(detailed) = detailed {
        for tap in &detailed.tap_changers {
            if let Err(message) = write_source_tap_changer(
                &mut eq,
                &mut ssh,
                &mut sv,
                TapWriteContext {
                    network: net,
                    detailed,
                    cim_namespace: w.p.cim_ns,
                    tap,
                },
            ) {
                w.warnings.push_as(
                    &codes::EMIT_CGMES.record_dropped,
                    format!(
                        "tap changer on `{}` winding {} was not emitted: {message}",
                        tap.transformer, tap.winding
                    ),
                );
            }
        }
        for group in &detailed.operational_limit_groups {
            let Some(owner) = equipment_mrid(net, Some(detailed), &group.equipment) else {
                w.warnings.push(format!(
                    "operational limit group `{}` targets unknown equipment `{}` and was not emitted",
                    group.id, group.equipment
                ));
                continue;
            };
            if group.terminal == 0 {
                w.warnings.push(format!(
                    "operational limit group `{}` has terminal 0 and was not emitted",
                    group.id
                ));
                continue;
            }
            let set = mrid_or(
                "source_limit_set",
                &format!("{}:{}:{}", group.equipment, group.terminal, group.id),
                Some(&group.id),
            );
            let mut emits = group.current_limits.is_some();
            if v3 {
                emits |=
                    group.active_power_limits.is_some() || group.apparent_power_limits.is_some();
            } else if group.active_power_limits.is_some() || group.apparent_power_limits.is_some() {
                w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                    "operational limit group `{}`: active and apparent power limits belong to the CIM16 EquipmentOperation profile and were omitted from the four-profile CGMES 2.4.15 output",
                    group.id
                ));
            }
            if !emits {
                continue;
            }
            limit_body.named("OperationalLimitSet", &set, &group.id);
            let reference = TerminalReference {
                equipment: group.equipment.clone(),
                terminal: group.terminal,
            };
            let terminal = terminal_reference_mrid(net, Some(detailed), &reference)
                .unwrap_or_else(|| term_id(&owner, usize::from(group.terminal)));
            limit_body.reference("OperationalLimitSet.Terminal", &terminal);
            limit_body.close("OperationalLimitSet");
            if let Some(limits) = &group.current_limits {
                write_source_loading_limits(
                    &mut limit_body,
                    &mut source_limit_types,
                    LimitWriteContext {
                        version,
                        group_id: &set,
                        class: "CurrentLimit",
                        limits,
                    },
                    &mut w.warnings,
                );
            }
            if v3 {
                if let Some(limits) = &group.active_power_limits {
                    write_source_loading_limits(
                        &mut limit_body,
                        &mut source_limit_types,
                        LimitWriteContext {
                            version,
                            group_id: &set,
                            class: "ActivePowerLimit",
                            limits,
                        },
                        &mut w.warnings,
                    );
                }
                if let Some(limits) = &group.apparent_power_limits {
                    write_source_loading_limits(
                        &mut limit_body,
                        &mut source_limit_types,
                        LimitWriteContext {
                            version,
                            group_id: &set,
                            class: "ApparentPowerLimit",
                            limits,
                        },
                        &mut w.warnings,
                    );
                }
            }
            if group.selected {
                w.warnings.push_as(&codes::EMIT_CGMES.field_dropped, format!(
                    "operational limit group `{}`: CGMES has no selected-group property; all groups were emitted",
                    group.id
                ));
            }
        }
    }

    for kind in &limit_types_used {
        let id = det_mrid("limittype", kind);
        limit_doc.named("OperationalLimitType", &id, kind);
        limit_doc.enumeration(
            "OperationalLimitType.direction",
            w.p.cim_ns,
            "OperationalLimitDirectionKind.absoluteValue",
        );
        if v3 {
            limit_doc.ext_ref(
                w.p.ext.0,
                "OperationalLimitType.kind",
                &limit_kind_uri(kind),
            );
            limit_doc.text("OperationalLimitType.isInfiniteDuration", *kind == "patl");
        } else {
            limit_doc.ext_ref(
                w.p.ext.0,
                "OperationalLimitType.limitType",
                &limit_kind_uri(kind),
            );
            limit_doc.text("OperationalLimitType.acceptableDuration", 900);
        }
        limit_doc.close("OperationalLimitType");
    }
    for value in source_limit_types {
        limit_doc.named(
            "OperationalLimitType",
            &value.id,
            if value.infinite { "PATL" } else { "TATL" },
        );
        limit_doc.enumeration(
            "OperationalLimitType.direction",
            w.p.cim_ns,
            "OperationalLimitDirectionKind.absoluteValue",
        );
        if v3 {
            limit_doc.ext_ref(
                w.p.ext.0,
                "OperationalLimitType.kind",
                &limit_kind_uri(value.kind),
            );
            limit_doc.text("OperationalLimitType.isInfiniteDuration", value.infinite);
            if !value.infinite {
                limit_doc.text(
                    "OperationalLimitType.acceptableDuration",
                    value.acceptable_duration_seconds,
                );
            }
        } else {
            limit_doc.ext_ref(
                w.p.ext.0,
                "OperationalLimitType.limitType",
                &limit_kind_uri(value.kind),
            );
            limit_doc.text(
                "OperationalLimitType.acceptableDuration",
                value.acceptable_duration_seconds,
            );
        }
        limit_doc.close("OperationalLimitType");
    }
    eq.body.push_str(&limit_doc.body);
    eq.body.push_str(&limit_body.body);

    // --- island (SV) --------------------------------------------------------
    let refs: Vec<&crate::network::Bus> = net
        .buses()
        .iter()
        .filter(|b| b.kind == BusType::Ref)
        .collect();
    if let Some(slack) = refs.first() {
        let island = det_mrid("island", "1");
        sv.named("TopologicalIsland", &island, "island");
        sv.reference(
            "TopologicalIsland.AngleRefTopologicalNode",
            &bus_mrid(net, slack.id),
        );
        for bus in net.buses() {
            sv.reference("TopologicalIsland.TopologicalNodes", &bus_mrid(net, bus.id));
        }
        sv.close("TopologicalIsland");
    } else {
        w.warnings.push_as(
            &codes::EMIT_CGMES.reference_missing,
            "no reference bus: the SV island has no angle reference",
        );
    }

    // --- unrepresented families ------------------------------------------------
    if let Some(detailed) = detailed {
        for record in &detailed.equipment_reactive_limits {
            let represented = match record.equipment.component_type() {
                "generator" => net
                    .generators()
                    .iter()
                    .any(|generator| generator.uid.as_deref() == Some(record.equipment.local_id())),
                "voltage_source_converter" => detailed
                    .voltage_source_converters
                    .iter()
                    .any(|converter| converter.component == record.equipment),
                _ => false,
            };
            if !represented {
                w.warnings.push_as(&codes::EMIT_CGMES.record_dropped, format!(
                    "equipment reactive limits for `{}` were not emitted: the CGMES writer has no reactive limits association for component type `{}`",
                    record.equipment,
                    record.equipment.component_type()
                ));
            }
        }
    }
    for (index, line) in net.hvdc().iter().enumerate() {
        let id = line
            .uid
            .as_deref()
            .map_or_else(|| format!("row {}", index + 1), str::to_owned);
        w.warnings.push(format!(
            "HVDC line `{id}` is a two terminal calculation record, not a physical CGMES DC network, and was not emitted. It has no DCConverterUnit operating mode and containment, DC node and terminal polarity identities, or ground or metallic return topology; its optional resistance, nominal voltage, converter technology, setpoints, and loss factors are insufficient to construct those standard records without inventing data. Use the source neutral detailed DC equipment records for CGMES emission"
        ));
    }
    for (what, count) in [
        ("storage unit", net.storage().len()),
        ("area record", net.areas().len()),
        (
            "solver-parameter block",
            usize::from(net.solver().is_some()),
        ),
    ] {
        if count > 0 {
            w.warnings.push(format!(
                "{count} {what}(s) have no CGMES mapping yet and are dropped"
            ));
        }
    }

    let name = if net.name().is_empty() {
        "case"
    } else {
        net.name()
    };
    let stem = format!("powerio_{}", det_mrid("model_set", name));
    let ids: Vec<String> = ["EQ", "TP", "SSH", "SV"]
        .iter()
        .map(|part| det_mrid("model", &format!("{stem}:{part}")))
        .collect();
    let case_date = net.case_metadata().case_date.clone().unwrap_or_else(|| {
        w.warnings.push_as(
            &codes::EMIT_CGMES.value_defaulted,
            "case date is absent; CGMES Model.scenarioTime and Model.created use \
                 2000-01-01T00:00:00Z",
        );
        STAMP.into()
    });
    let files = vec![
        (
            format!("{stem}_EQ.xml"),
            document(&w.p, w.p.eq, &ids[0], name, &case_date, &[], &eq.body),
        ),
        (
            format!("{stem}_TP.xml"),
            document(
                &w.p,
                w.p.tp,
                &ids[1],
                name,
                &case_date,
                &[&ids[0]],
                &tp.body,
            ),
        ),
        (
            format!("{stem}_SSH.xml"),
            document(
                &w.p,
                w.p.ssh,
                &ids[2],
                name,
                &case_date,
                &[&ids[0]],
                &ssh.body,
            ),
        ),
        (
            format!("{stem}_SV.xml"),
            document(
                &w.p,
                w.p.sv,
                &ids[3],
                name,
                &case_date,
                &[&ids[1], &ids[2]],
                &sv.body,
            ),
        ),
    ];
    validate_rdf_graph(&files)?;
    Ok(CgmesFiles {
        files,
        warnings: w.warnings,
    })
}

#[allow(clippy::too_many_lines)] // one XML pass classifies definitions and references by RDF role
pub(super) fn validate_rdf_graph(files: &[(String, String)]) -> Result<()> {
    const RDF_NS: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    const MD_NS: &[u8] = b"http://iec.ch/TC57/61970-552/ModelDescription/1#";

    let mut fragment_definitions = HashMap::<String, String>::new();
    let mut model_definitions = HashMap::<String, String>::new();
    let mut fragment_references = Vec::<(String, &'static str, String)>::new();
    let mut model_references = Vec::<(String, String)>::new();

    for (name, xml) in files {
        let mut reader = NsReader::from_str(xml);
        loop {
            let (namespace, event) = reader.read_resolved_event().map_err(|error| {
                emission_error(format!(
                    "generated CGMES file `{name}` is not valid XML during RDF graph validation: {error}"
                ))
            })?;
            match event {
                Event::Start(element) | Event::Empty(element) => {
                    let is_model_namespace = matches!(
                        namespace,
                        ResolveResult::Bound(ref value) if value.as_ref() == MD_NS
                    );
                    let local_name = element.local_name();
                    let is_full_model = is_model_namespace && local_name.as_ref() == b"FullModel";
                    let is_model_dependency =
                        is_model_namespace && local_name.as_ref() == b"Model.DependentOn";

                    for attribute in element.attributes().with_checks(true) {
                        let attribute = attribute.map_err(|error| {
                            emission_error(format!(
                                "generated CGMES file `{name}` has an invalid XML attribute: {error}"
                            ))
                        })?;
                        let (attribute_namespace, attribute_local_name) =
                            reader.resolve_attribute(attribute.key);
                        if !matches!(
                            attribute_namespace,
                            ResolveResult::Bound(ref value) if value.as_ref() == RDF_NS
                        ) {
                            continue;
                        }
                        let value = attribute
                            .decode_and_unescape_value(reader.decoder())
                            .map_err(|error| {
                                emission_error(format!(
                                    "generated CGMES file `{name}` has an invalid RDF attribute value: {error}"
                                ))
                            })?;
                        let value = value.as_ref();
                        match attribute_local_name.as_ref() {
                            b"ID" => {
                                let id = value.strip_prefix('_').unwrap_or(value);
                                register_rdf_definition(
                                    &mut fragment_definitions,
                                    id,
                                    name,
                                    "mRID",
                                )?;
                            }
                            b"about" if is_full_model => {
                                let id = value.strip_prefix("urn:uuid:").ok_or_else(|| {
                                    emission_error(format!(
                                        "generated CGMES FullModel in `{name}` has rdf:about `{value}` instead of a urn:uuid identifier"
                                    ))
                                })?;
                                register_rdf_definition(
                                    &mut model_definitions,
                                    id,
                                    name,
                                    "model UUID",
                                )?;
                            }
                            b"about" => {
                                if let Some(id) = rdf_object_reference_id(value) {
                                    fragment_references.push((
                                        name.clone(),
                                        "rdf:about",
                                        id.to_owned(),
                                    ));
                                }
                            }
                            b"resource" if is_model_dependency => {
                                let id = value.strip_prefix("urn:uuid:").ok_or_else(|| {
                                    emission_error(format!(
                                        "generated CGMES model dependency in `{name}` has rdf:resource `{value}` instead of a urn:uuid identifier"
                                    ))
                                })?;
                                model_references.push((name.clone(), id.to_owned()));
                            }
                            b"resource" => {
                                if let Some(id) = rdf_object_reference_id(value) {
                                    fragment_references.push((
                                        name.clone(),
                                        "rdf:resource",
                                        id.to_owned(),
                                    ));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Event::DocType(_) => {
                    return Err(emission_error(format!(
                        "generated CGMES file `{name}` contains a DTD"
                    )));
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }

    for (name, attribute, id) in fragment_references {
        if !fragment_definitions.contains_key(&id) {
            return Err(emission_error(format!(
                "generated CGMES file `{name}` contains a dangling {attribute} reference to `{id}`"
            )));
        }
    }
    for (name, id) in model_references {
        if !model_definitions.contains_key(&id) {
            return Err(emission_error(format!(
                "generated CGMES file `{name}` contains a dangling model dependency reference to `{id}`"
            )));
        }
    }
    Ok(())
}

fn register_rdf_definition(
    definitions: &mut HashMap<String, String>,
    id: &str,
    file: &str,
    kind: &str,
) -> Result<()> {
    if id.is_empty() {
        return Err(emission_error(format!(
            "generated CGMES file `{file}` defines an empty {kind}"
        )));
    }
    if let Some(first_file) = definitions.insert(id.to_owned(), file.to_owned()) {
        return Err(emission_error(format!(
            "generated CGMES defines {kind} `{id}` more than once: first in `{first_file}`, then in `{file}`"
        )));
    }
    Ok(())
}

fn rdf_object_reference_id(value: &str) -> Option<&str> {
    value
        .strip_prefix("#_")
        .or_else(|| value.strip_prefix('#'))
        .or_else(|| value.strip_prefix("urn:uuid:"))
        .filter(|id| !id.is_empty())
}

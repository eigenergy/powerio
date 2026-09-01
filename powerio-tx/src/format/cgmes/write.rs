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

use std::collections::HashSet;
use std::fmt::Write as _;

use powerio_core::ComponentId;

use super::{CGMES_CLASS_PROPERTY, CgmesVersion};
use crate::network::{
    AcDcConverterControlMode, ActivePowerControl, BalancedNetwork, BusId, BusType,
    ComponentMetadata, CurveStyle, DcConverterOperatingMode, DcPolarity, DcSwitchKind, DcTerminal,
    DetailedConnectivity, LineCommutatedConverter, LineCommutatedConverterOperatingMode,
    LoadVoltageModel, LoadingLimits, ReactiveLimits, Shunt, StaticVarCompensatorRegulationMode,
    SwitchKind, SwitchedShuntMode, TapChanger, TapChangerKind, TapChangerRegulationMode, Terminal,
    TerminalReference, TopologyEndpoint, TopologyKind, VoltageSourceConverter,
};
use crate::{Error, Result};

/// The emitted profile documents, `(file_name, xml)` in EQ/TP/SSH/SV order,
/// plus every fidelity loss the writer took.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CgmesFiles {
    pub files: Vec<(String, String)>,
    pub warnings: Vec<String>,
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

fn component_name<'a>(
    detailed: &'a DetailedConnectivity,
    component: &ComponentId,
    fallback: &'a str,
) -> &'a str {
    metadata(detailed, component)
        .and_then(|value| value.name.as_deref())
        .unwrap_or(fallback)
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
    equipment_mrid(network, detailed, &reference.equipment)
        .map(|equipment| term_id(&equipment, usize::from(reference.terminal)))
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
) -> String {
    if let Some(detailed) = detailed {
        if let Some(node) = terminal.and_then(|value| value.node.as_ref()) {
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
            && let Some(calculated_bus) = detailed
                .connectivity_nodes
                .iter()
                .find(|value| value.component == *node)
                .and_then(|value| value.calculated_bus)
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
        TopologyEndpoint::Node(component) => detailed
            .connectivity_nodes
            .iter()
            .find(|value| value.component == *component)
            .and_then(|value| value.calculated_bus),
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
}

impl Doc {
    fn new() -> Doc {
        Doc {
            body: String::new(),
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
        self.text("IdentifiedObject.name", name);
    }
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
    warnings: Vec<String>,
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

fn tap_neutral_position(tap: &TapChanger) -> i32 {
    tap.steps
        .iter()
        .min_by(|left, right| {
            let left_distance = (left.rho - 1.0).abs() + left.alpha_degrees.abs();
            let right_distance = (right.rho - 1.0).abs() + right.alpha_degrees.abs();
            left_distance.total_cmp(&right_distance)
        })
        .map_or(tap.low_tap_position, |value| value.position)
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
    let id = det_mrid(
        "source_tap_changer",
        &format!("{}:{}:{kind}", tap.transformer, tap.winding),
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

    eq.named(class, &id, &format!("{kind} tap changer"));
    eq.text("TapChanger.lowStep", tap.low_tap_position);
    let high = tap
        .steps
        .iter()
        .map(|value| value.position)
        .max()
        .unwrap_or(tap.low_tap_position);
    eq.text("TapChanger.highStep", high);
    eq.text("TapChanger.neutralStep", tap_neutral_position(tap));
    eq.text("TapChanger.normalStep", tap_neutral_position(tap));
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
        &det_mrid("xfend", &format!("{owner}:{}", tap.winding)),
    );
    eq.reference(
        match tap.kind {
            TapChangerKind::Ratio => "RatioTapChanger.RatioTapChangerTable",
            TapChangerKind::Phase => "PhaseTapChangerTabular.PhaseTapChangerTable",
        },
        &table,
    );
    eq.close(class);

    write_source_tap_table(eq, context, &id, &table, table_class, point_class, kind);

    if let Some(control_id) = control {
        write_source_tap_control(eq, ssh, context, &owner, &control_id);
    }
    ssh.open(class, &id, true);
    if let Some(position) = tap.tap_position {
        ssh.text("TapChanger.step", position);
    }
    ssh.text("TapChanger.controlEnabled", tap.regulating);
    ssh.close(class);
    sv.open("SvTapStep", &det_mrid("svtap", &id), false);
    sv.reference("SvTapStep.TapChanger", &id);
    sv.text(
        "SvTapStep.position",
        tap.solved_tap_position
            .or(tap.tap_position)
            .unwrap_or_else(|| tap_neutral_position(tap)),
    );
    sv.close("SvTapStep");
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
    owner: &str,
    control_id: &str,
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
    let terminal = tap
        .regulation_terminal
        .as_ref()
        .and_then(|value| terminal_reference_mrid(context.network, Some(context.detailed), value))
        .unwrap_or_else(|| term_id(owner, usize::from(tap.winding)));
    eq.reference("RegulatingControl.Terminal", &terminal);
    eq.close("TapChangerControl");

    ssh.open("TapChangerControl", control_id, true);
    ssh.text("RegulatingControl.enabled", tap.regulating);
    if let Some(value) = tap.regulation_value {
        ssh.text("RegulatingControl.targetValue", value);
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
) {
    let value_property = if context.version == CgmesVersion::V3_0 {
        format!("{}.normalValue", context.class)
    } else {
        format!("{}.value", context.class)
    };
    if let Some(value) = context.limits.permanent_limit.filter(|value| *value > 0.0) {
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
    }
    for (index, limit) in context.limits.temporary_limits.iter().enumerate() {
        if !limit.value.is_finite() || limit.value <= 0.0 {
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
        .and_then(|bus| {
            detailed
                .bus_breaker_buses
                .iter()
                .find(|value| value.component == *bus)
                .and_then(|value| value.calculated_bus)
        })
        .or_else(|| {
            terminal.node.as_ref().and_then(|node| {
                detailed
                    .connectivity_nodes
                    .iter()
                    .find(|value| value.component == *node)
                    .and_then(|value| value.calculated_bus)
            })
        })
}

#[allow(clippy::too_many_arguments)]
fn write_converter_ac_terminals(
    network: &BalancedNetwork,
    detailed: &DetailedConnectivity,
    eq: &mut Doc,
    tp: &mut Doc,
    ssh: &mut Doc,
    converter: &ComponentId,
    owner: &str,
    warnings: &mut Vec<String>,
) {
    let records = detailed
        .terminals
        .iter()
        .filter(|terminal| terminal.equipment == *converter)
        .collect::<Vec<_>>();
    if records.is_empty() {
        warnings.push(format!(
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
        let id = term_id(owner, usize::from(record.terminal));
        eq.named("Terminal", &id, &format!("AC terminal {}", record.terminal));
        eq.reference("Terminal.ConductingEquipment", owner);
        eq.text("ACDCTerminal.sequenceNumber", record.terminal);
        eq.reference(
            "Terminal.ConnectivityNode",
            &connectivity_node_mrid(Some(detailed), Some(record), bus),
        );
        eq.close("Terminal");
        tp.open("Terminal", &id, true);
        tp.reference(
            "Terminal.TopologicalNode",
            &terminal_topological_node_mrid(network, Some(detailed), Some(record), bus),
        );
        tp.close("Terminal");
        ssh.open("Terminal", &id, true);
        ssh.text("ACDCTerminal.connected", record.connected);
        ssh.close("Terminal");
    }
}

fn write_vsc_capability_curve(
    eq: &mut Doc,
    detailed: &DetailedConnectivity,
    converter: &VoltageSourceConverter,
    converter_mrid: &str,
    cim_ns: &str,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let curve = det_mrid("vsc_capability_curve", converter_mrid);
    let (curve_style, points) = match converter.reactive_limits.as_ref()? {
        ReactiveLimits::CapabilityCurve(value) => {
            if !value.properties.is_empty()
                || value
                    .points
                    .iter()
                    .any(|point| !point.properties.is_empty())
            {
                warnings.push(format!(
                    "VsConverter `{}`: reactive capability curve properties have no CGMES field",
                    converter.component
                ));
            }
            (value.curve_style, value.points.clone())
        }
        ReactiveLimits::MinMax(value) => (
            CurveStyle::ConstantYValue,
            vec![crate::network::ReactiveCapabilityCurvePoint {
                active_power_mw: 0.0,
                minimum_reactive_power_mvar: value.minimum_reactive_power_mvar,
                maximum_reactive_power_mvar: value.maximum_reactive_power_mvar,
                properties: std::collections::BTreeMap::default(),
            }],
        ),
    };
    if points.is_empty() {
        warnings.push(format!(
            "VsConverter `{}` has an empty reactive capability curve; no VsCapabilityCurve was emitted",
            converter.component
        ));
        return None;
    }
    eq.named(
        "VsCapabilityCurve",
        &curve,
        component_name(detailed, &converter.component, "VSC capability curve"),
    );
    eq.enumeration(
        "Curve.curveStyle",
        cim_ns,
        match curve_style {
            CurveStyle::ConstantYValue => "CurveStyle.constantYValue",
            CurveStyle::StraightLineYValues => "CurveStyle.straightLineYValues",
        },
    );
    eq.enumeration("Curve.xUnit", cim_ns, "UnitSymbol.W");
    eq.enumeration("Curve.y1Unit", cim_ns, "UnitSymbol.VAr");
    eq.enumeration("Curve.y2Unit", cim_ns, "UnitSymbol.VAr");
    eq.close("VsCapabilityCurve");
    for (index, point) in points.iter().enumerate() {
        let id = det_mrid("vsc_capability_point", &format!("{curve}:{index}"));
        eq.open("CurveData", &id, false);
        eq.reference("CurveData.Curve", &curve);
        eq.text("CurveData.xvalue", point.active_power_mw);
        eq.text("CurveData.y1value", point.minimum_reactive_power_mvar);
        eq.text("CurveData.y2value", point.maximum_reactive_power_mvar);
        eq.close("CurveData");
    }
    Some(curve)
}

fn write_vsc_sv(
    writer: &mut Writer<'_>,
    sv: &mut Doc,
    id: &str,
    converter: &VoltageSourceConverter,
) {
    let valve_voltage = if writer.p.cim_ns == profiles(CgmesVersion::V3_0).cim_ns {
        converter.uv_kv
    } else {
        converter.uf_kv
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
            writer.warnings.push(format!(
                "DCGround `{}`: CGMES DCTerminal has no active power or current field",
                ground.component
            ));
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
            writer.warnings.push(format!(
                "DCBusbar `{}`: rated DC voltage has no CGMES 2.4.15 field",
                busbar.component
            ));
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
            writer.warnings.push(format!(
                "DCBusbar `{}`: CGMES DCTerminal has no active power or current field",
                busbar.component
            ));
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
            writer.warnings.push(format!(
                "DCLineSegment `{}`: CGMES DCTerminal has no active power or current field",
                line.component
            ));
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
            writer.warnings.push(format!(
                "DCSeriesDevice `{}`: CGMES DCTerminal has no active power or current field",
                device.component
            ));
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
        if switch.resistance_ohm.is_some_and(|value| value != 0.0) {
            writer.warnings.push(format!(
                "DC switch `{}`: resistance {} ohm has no CGMES DC switch field",
                switch.component,
                switch.resistance_ohm.unwrap()
            ));
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
        );
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
            writer.warnings.push(format!(
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
            writer.warnings.push(format!(
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
        writer.warnings.push(format!(
            "CsConverter `{}`: reactive model and power factor have no direct CGMES field; PCC p/q carries the available operating assignment",
            converter.component
        ));
    }
    if converter.droop_curve.is_some() {
        writer.warnings.push(format!(
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
        writer.warnings.push(format!(
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
        warnings: Vec::new(),
    };
    let mut eq = Doc::new();
    let mut tp = Doc::new();
    let mut ssh = Doc::new();
    let mut sv = Doc::new();

    if (net.base_mva() - 100.0).abs() > 1e-9 {
        w.warnings.push(format!(
            "system base {} MVA: CGMES carries no MVA base, so a reparse lands \
             per-unit values on 100 MVA",
            net.base_mva()
        ));
    }
    let detailed = net.detailed_connectivity().as_deref();
    let mut active_connectivity_nodes = Vec::new();
    let mut active_bus_breaker_buses = Vec::new();
    let mut active_source_lines = Vec::<ComponentId>::new();
    let mut terminal_generated_connectivity_nodes = HashSet::<ComponentId>::new();
    if let Some(detailed) = detailed {
        let retained_terminals = detailed
            .terminals
            .iter()
            .filter(|terminal| terminal.equipment.component_type() != "cgmes_object")
            .collect::<Vec<_>>();
        terminal_generated_connectivity_nodes.extend(
            retained_terminals
                .iter()
                .filter(|terminal| {
                    terminal.node.is_none()
                        && (v3
                            || matches!(
                                terminal.equipment.component_type(),
                                "voltage_source_converter" | "line_commutated_converter"
                            ))
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
            let active = node.calculated_bus.is_some()
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
                w.warnings.push(format!(
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
                if configured.voltage_level.component_type() == "line"
                    && !active_source_lines.contains(&configured.voltage_level)
                {
                    active_source_lines.push(configured.voltage_level.clone());
                }
            } else {
                w.warnings.push(format!(
                    "source CGMES TopologicalNode `{}` in ConnectivityNodeContainer `{}` is not connected to a calculated bus or retained equipment and was omitted from fresh CGMES emission; no VoltageLevel or BaseVoltage was generated for that container",
                    configured.component, configured.voltage_level
                ));
            }
        }
    }
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
                node.calculated_bus,
                "ConnectivityNode",
                &node.component,
            )?;
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
    if let Some(detailed) = detailed.filter(|value| {
        !value.voltage_levels.is_empty()
            || !value.connectivity_nodes.is_empty()
            || !value.bus_breaker_buses.is_empty()
    }) {
        let voltage_level_missing_substation = detailed.voltage_levels.iter().any(|level| {
            level.substation.as_ref().is_none_or(|substation| {
                !detailed
                    .substations
                    .iter()
                    .any(|value| value.component == *substation)
            })
        });
        let needs_fallback_substation =
            voltage_level_missing_substation || !generated_voltage_levels.is_empty();
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
                w.warnings.push(format!(
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
                w.warnings.push(
                    "voltage levels without a declared substation were placed in the PowerIO substation"
                        .into(),
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
        for line in &active_source_lines {
            let id = component_mrid(detailed, line);
            eq.named("Line", &id, component_name(detailed, line, line.local_id()));
            eq.reference("Line.Region", &subregion);
            eq.close("Line");
        }

        for node in &active_connectivity_nodes {
            let id = component_mrid(detailed, &node.component);
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
            if let Some(bus) = node.calculated_bus {
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
                let affected = node.calculated_bus.map_or_else(
                    || {
                        format!(
                            "ConnectivityNode `{}` with no calculated bus",
                            node.component
                        )
                    },
                    |bus| format!("bus {bus} through ConnectivityNode `{}`", node.component),
                );
                w.warnings.push(format!(
                    "source ConnectivityNodeContainer `{}` for {affected} was not a typed VoltageLevel; during fresh CGMES emission, that topology was placed in generated VoltageLevel `{}`",
                    node.voltage_level,
                    topology_voltage_level_mrid(detailed, &node.voltage_level)
                ));
            }
        }
        for configured in &active_bus_breaker_buses {
            let typed_level = detailed
                .voltage_levels
                .iter()
                .find(|level| level.component == configured.voltage_level);
            let has_connectivity_nodes = active_connectivity_nodes
                .iter()
                .any(|node| node.voltage_level == configured.voltage_level);
            let is_bus_breaker = typed_level.map_or(!has_connectivity_nodes, |level| {
                level.topology_kind == TopologyKind::BusBreaker
            });
            if is_bus_breaker
                || terminal_generated_connectivity_nodes.contains(&configured.component)
            {
                let id = det_mrid("connectivity_node", &configured.component.to_string());
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
                w.warnings.push(format!(
                    "source ConnectivityNodeContainer `{}` for TopologicalNode `{}` was not a typed VoltageLevel; during fresh CGMES emission, that topology was placed in generated VoltageLevel `{}`",
                    configured.voltage_level,
                    configured.component,
                    topology_voltage_level_mrid(detailed, &configured.voltage_level)
                ));
            }
            if let Some(bus) = configured
                .calculated_bus
                .and_then(|id| net.buses().iter().find(|bus| bus.id == id))
            {
                let svv = det_mrid("svvoltage", &id);
                sv.open("SvVoltage", &svv, false);
                sv.reference("SvVoltage.TopologicalNode", &id);
                sv.text("SvVoltage.v", bus.vm * bus.base_kv);
                sv.text("SvVoltage.angle", bus.va);
                sv.close("SvVoltage");
            }
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
            w.warnings.push(format!(
                "bus {}: emergency voltage band (evhi/evlo) has no CGMES slot",
                bus.id
            ));
        }
    }

    // A terminal: EQ definition and connectivity, TP topology, SSH connection.
    let terminal = |eq: &mut Doc,
                    tp: &mut Doc,
                    ssh: &mut Doc,
                    owner: &str,
                    component_type: &str,
                    local_id: &str,
                    seq: usize,
                    bus: BusId,
                    connected: bool| {
        let record = detailed
            .and_then(|detailed| detailed_terminal(detailed, component_type, local_id, seq));
        let id = term_id(owner, seq);
        eq.open("Terminal", &id, false);
        eq.reference("Terminal.ConductingEquipment", owner);
        eq.text("ACDCTerminal.sequenceNumber", seq);
        if v3 || record.and_then(|value| value.node.as_ref()).is_some() {
            eq.reference(
                "Terminal.ConnectivityNode",
                &connectivity_node_mrid(detailed, record, bus),
            );
        }
        eq.close("Terminal");
        tp.open("Terminal", &id, true);
        tp.reference(
            "Terminal.TopologicalNode",
            &terminal_topological_node_mrid(net, detailed, record, bus),
        );
        tp.close("Terminal");
        ssh.open("Terminal", &id, true);
        ssh.text(
            "ACDCTerminal.connected",
            record.map_or(connected, |value| value.connected),
        );
        ssh.close("Terminal");
        id
    };

    if let Some(detailed) = detailed {
        for busbar in &detailed.busbar_sections {
            let Some(bus) = detailed
                .connectivity_nodes
                .iter()
                .find(|node| node.component == busbar.node)
                .and_then(|node| node.calculated_bus)
            else {
                w.warnings.push(format!(
                    "busbar section `{}` has no calculated bus and was not emitted",
                    busbar.component.local_id()
                ));
                continue;
            };
            let id = component_mrid(detailed, &busbar.component);
            let level = detailed
                .voltage_levels
                .iter()
                .find(|level| level.component == busbar.voltage_level);
            eq.named(
                "BusbarSection",
                &id,
                component_name(detailed, &busbar.component, busbar.component.local_id()),
            );
            eq.reference(
                "Equipment.EquipmentContainer",
                &component_mrid(detailed, &busbar.voltage_level),
            );
            if let Some(level) = level {
                eq.reference(
                    "ConductingEquipment.BaseVoltage",
                    &base_of(level.nominal_kv),
                );
            }
            eq.close("BusbarSection");
            terminal(
                &mut eq,
                &mut tp,
                &mut ssh,
                &id,
                "busbar_section",
                busbar.component.local_id(),
                1,
                bus,
                true,
            );
        }
        for switch in &detailed.switches {
            let (Some(first), Some(second)) = (
                endpoint_bus(detailed, &switch.endpoint1),
                endpoint_bus(detailed, &switch.endpoint2),
            ) else {
                w.warnings.push(format!(
                    "switch `{}` has an endpoint without a calculated bus and was not emitted",
                    switch.component.local_id()
                ));
                continue;
            };
            let id = component_mrid(detailed, &switch.component);
            let class = switch_class(switch.kind);
            eq.named(
                class,
                &id,
                component_name(detailed, &switch.component, switch.component.local_id()),
            );
            eq.reference(
                "Equipment.EquipmentContainer",
                &component_mrid(detailed, &switch.voltage_level),
            );
            eq.text("Switch.normalOpen", switch.open);
            eq.text("Switch.retained", switch.retained);
            if let Some(current_rating) = net
                .switches()
                .iter()
                .find(|value| value.uid.as_deref() == Some(switch.component.local_id()))
                .and_then(|value| value.current_rating)
            {
                eq.text("Switch.ratedCurrent", current_rating);
            }
            eq.close(class);
            terminal(
                &mut eq,
                &mut tp,
                &mut ssh,
                &id,
                "switch",
                switch.component.local_id(),
                1,
                first,
                true,
            );
            terminal(
                &mut eq,
                &mut tp,
                &mut ssh,
                &id,
                "switch",
                switch.component.local_id(),
                2,
                second,
                true,
            );
            ssh.open(class, &id, true);
            ssh.text("Switch.open", switch.open);
            ssh.close(class);
        }
        if !detailed.internal_connections.is_empty() {
            w.warnings.push(format!(
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
    let mut limit_doc = Doc::new();

    // --- loads ------------------------------------------------------------
    for (i, load) in net.loads().iter().enumerate() {
        let fallback = format!("{}-{i}", load.bus);
        let local_id = load.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("load", &fallback, load.uid.as_deref());
        let record = detailed.and_then(|value| detailed_terminal(value, "load", local_id, 1));
        eq.named("EnergyConsumer", &id, &format!("load{}-{i}", load.bus));
        eq.reference(
            "Equipment.EquipmentContainer",
            &terminal_voltage_level_mrid(detailed, record, load.bus),
        );
        let response = load
            .voltage_model
            .as_ref()
            .map(|_| det_mrid("load_response", &id));
        if let Some(response) = &response {
            eq.reference("EnergyConsumer.LoadResponse", response);
        }
        eq.close("EnergyConsumer");
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
                        w.warnings.push(format!(
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
                        w.warnings.push(format!(
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
            &id,
            "load",
            local_id,
            1,
            load.bus,
            load.in_service,
        );
        ssh.open("EnergyConsumer", &id, true);
        ssh.text("EnergyConsumer.p", load.p);
        ssh.text("EnergyConsumer.q", load.q);
        if v3 {
            ssh.text("Equipment.inService", load.in_service);
        }
        ssh.close("EnergyConsumer");
    }

    // --- generators --------------------------------------------------------
    for (i, machine) in net.generators().iter().enumerate() {
        let fallback = format!("{}-{i}", machine.bus);
        let local_id = machine.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("generator", &fallback, machine.uid.as_deref());
        let unit = det_mrid("genunit", &id);
        eq.named(
            "GeneratingUnit",
            &unit,
            &format!("gen{}-{i}-unit", machine.bus),
        );
        eq.text("GeneratingUnit.minOperatingP", machine.pmin);
        eq.text("GeneratingUnit.maxOperatingP", machine.pmax);
        eq.close("GeneratingUnit");
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
                ssh.open("GeneratingUnit", &unit, true);
                ssh.text("GeneratingUnit.normalPF", participation_factor);
                ssh.close("GeneratingUnit");
            }
            if !active_power_control.participate {
                w.warnings.push(format!(
                    "generator `{local_id}`: participate=false{} cannot be represented by CGMES GeneratingUnit.normalPF",
                    active_power_control
                        .participation_factor
                        .map_or("", |_| " with a participation factor")
                ));
            } else if active_power_control.participation_factor.is_none() {
                w.warnings.push(format!(
                    "generator `{local_id}`: participate=true without a participation factor cannot be represented by CGMES GeneratingUnit.normalPF"
                ));
            }
            if active_power_control.droop_percent.is_some() {
                w.warnings.push(format!(
                    "generator `{local_id}`: active power control droop has no CGMES property"
                ));
            }
            if active_power_control
                .minimum_target_active_power_mw
                .is_some()
                || active_power_control
                    .maximum_target_active_power_mw
                    .is_some()
            {
                w.warnings.push(format!(
                    "generator `{local_id}`: active power control target limits have no CGMES property"
                ));
            }
        }
        let control = det_mrid("regcontrol", &id);
        eq.named(
            "SynchronousMachine",
            &id,
            &format!("gen{}-{i}", machine.bus),
        );
        eq.enumeration(
            "SynchronousMachine.type",
            w.p.cim_ns,
            "SynchronousMachineKind.generator",
        );
        eq.text("SynchronousMachine.maxQ", machine.qmax);
        eq.text("SynchronousMachine.minQ", machine.qmin);
        if machine.mbase > 0.0 {
            eq.text("RotatingMachine.ratedS", machine.mbase);
        }
        eq.reference("RotatingMachine.GeneratingUnit", &unit);
        eq.reference("RegulatingCondEq.RegulatingControl", &control);
        let record = detailed.and_then(|value| detailed_terminal(value, "generator", local_id, 1));
        eq.reference(
            "Equipment.EquipmentContainer",
            &terminal_voltage_level_mrid(detailed, record, machine.bus),
        );
        eq.close("SynchronousMachine");
        let term = terminal(
            &mut eq,
            &mut tp,
            &mut ssh,
            &id,
            "generator",
            local_id,
            1,
            machine.bus,
            machine.in_service,
        );
        eq.named(
            "RegulatingControl",
            &control,
            &format!("gen{}-{i}-avr", machine.bus),
        );
        eq.enumeration(
            "RegulatingControl.mode",
            w.p.cim_ns,
            "RegulatingControlModeKind.voltage",
        );
        eq.reference("RegulatingControl.Terminal", &term);
        eq.close("RegulatingControl");
        ssh.open("SynchronousMachine", &id, true);
        ssh.text("RotatingMachine.p", -machine.pg);
        ssh.text("RotatingMachine.q", -machine.qg);
        if net
            .buses()
            .iter()
            .any(|b| b.id == machine.bus && b.kind == BusType::Ref)
        {
            ssh.text("SynchronousMachine.referencePriority", 1);
        }
        if v3 {
            ssh.text("Equipment.inService", machine.in_service);
        }
        ssh.close("SynchronousMachine");
        ssh.open("RegulatingControl", &control, true);
        ssh.text("RegulatingControl.enabled", true);
        ssh.text(
            "RegulatingControl.targetValue",
            machine.vg * w.kv(machine.bus)?,
        );
        ssh.close("RegulatingControl");
        if machine.regulated_bus.is_some_and(|b| b != machine.bus) {
            w.warnings.push(format!(
                "generator at bus {}: remote regulated bus is written as local \
                 regulation (the control terminal is the machine's own)",
                machine.bus
            ));
        }
        if machine.cost.is_some() {
            w.warnings.push(format!(
                "generator at bus {}: cost curves have no CGMES slot",
                machine.bus
            ));
        }
        if machine.has_caps() {
            w.warnings.push(format!(
                "generator at bus {}: capability/ramp columns have no CGMES slot",
                machine.bus
            ));
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
            w.warnings.push(format!(
                "storage `{id}`: active power control has no CGMES battery mapping"
            ));
        }
    }

    // --- shunts -------------------------------------------------------------
    for (i, shunt) in net.shunts().iter().enumerate() {
        let fallback = format!("{}-{i}", shunt.bus);
        let local_id = shunt.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("shunt", &fallback, shunt.uid.as_deref());
        let kv = w.kv(shunt.bus)?;
        let sections = expanded_shunt_sections(shunt);
        let sections = if sections.is_empty() {
            vec![(shunt.g, shunt.b)]
        } else {
            sections
        };
        let (section_count, section_error) = shunt_section_count(shunt, &sections);
        let section_scale = 1.0 + shunt.g.abs() + shunt.b.abs();
        if section_error > 1e-9 * section_scale {
            w.warnings.push(format!(
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
        let control_id = shunt.control.as_ref().map(|_| det_mrid("regcontrol", &id));
        eq.named(class, &id, &format!("shunt{}-{i}", shunt.bus));
        if linear {
            eq.text(
                "LinearShuntCompensator.bPerSection",
                sections[0].1 / (kv * kv),
            );
            eq.text(
                "LinearShuntCompensator.gPerSection",
                sections[0].0 / (kv * kv),
            );
        }
        eq.text("ShuntCompensator.maximumSections", sections.len());
        eq.text("ShuntCompensator.normalSections", section_count);
        eq.text("ShuntCompensator.nomU", kv);
        if let Some(control) = &control_id {
            eq.reference("RegulatingCondEq.RegulatingControl", control);
        }
        let record = detailed.and_then(|value| detailed_terminal(value, "shunt", local_id, 1));
        eq.reference(
            "Equipment.EquipmentContainer",
            &terminal_voltage_level_mrid(detailed, record, shunt.bus),
        );
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
                eq.text("NonlinearShuntCompensatorPoint.g", g / (kv * kv));
                eq.close("NonlinearShuntCompensatorPoint");
            }
        }
        let term = terminal(
            &mut eq,
            &mut tp,
            &mut ssh,
            &id,
            "shunt",
            local_id,
            1,
            shunt.bus,
            shunt.in_service,
        );
        ssh.open(class, &id, true);
        ssh.text("ShuntCompensator.sections", section_count);
        ssh.text(
            "RegulatingCondEq.controlEnabled",
            shunt
                .control
                .as_ref()
                .is_some_and(|control| control.mode != SwitchedShuntMode::Locked),
        );
        if v3 {
            ssh.text("Equipment.inService", shunt.in_service);
        }
        ssh.close(class);
        if let (Some(control), Some(control_id)) = (&shunt.control, control_id) {
            eq.named(
                "RegulatingControl",
                &control_id,
                &format!("shunt{}-{i}-control", shunt.bus),
            );
            eq.enumeration(
                "RegulatingControl.mode",
                w.p.cim_ns,
                "RegulatingControlModeKind.voltage",
            );
            eq.reference("RegulatingControl.Terminal", &term);
            eq.close("RegulatingControl");
            let controlled_bus = control.control_bus.unwrap_or(shunt.bus);
            let controlled_kv = w.kv(controlled_bus)?;
            ssh.open("RegulatingControl", &control_id, true);
            ssh.text(
                "RegulatingControl.targetValue",
                (control.vhigh + control.vlow) * controlled_kv / 2.0,
            );
            ssh.text(
                "RegulatingControl.targetDeadband",
                (control.vhigh - control.vlow).abs() * controlled_kv,
            );
            ssh.close("RegulatingControl");
            if control.control_bus.is_some_and(|bus| bus != shunt.bus) {
                w.warnings.push(format!(
                    "shunt at bus {}: remote regulated bus {} is written as local regulation because the balanced model does not identify a terminal on the regulated equipment",
                    shunt.bus, controlled_bus
                ));
            }
        }
    }

    // --- static VAR compensators ---------------------------------------------
    for (i, svc) in net.static_var_compensators().iter().enumerate() {
        let fallback = format!("{}-{i}", svc.bus);
        let local_id = svc.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("static_var_compensator", &fallback, svc.uid.as_deref());
        let control = det_mrid("regcontrol", &id);
        let record = detailed
            .and_then(|value| detailed_terminal(value, "static_var_compensator", local_id, 1));
        eq.named("StaticVarCompensator", &id, &format!("svc{}-{i}", svc.bus));
        eq.reference(
            "Equipment.EquipmentContainer",
            &terminal_voltage_level_mrid(detailed, record, svc.bus),
        );
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
            &id,
            "static_var_compensator",
            local_id,
            1,
            svc.bus,
            svc.in_service,
        );
        eq.named(
            "RegulatingControl",
            &control,
            &format!("svc{}-{i}-control", svc.bus),
        );
        eq.enumeration(
            "RegulatingControl.mode",
            w.p.cim_ns,
            match svc.regulation_mode {
                StaticVarCompensatorRegulationMode::Voltage => "RegulatingControlModeKind.voltage",
                StaticVarCompensatorRegulationMode::ReactivePower => {
                    "RegulatingControlModeKind.reactivePower"
                }
            },
        );
        let regulation_terminal = svc
            .regulating_terminal
            .as_ref()
            .and_then(|value| terminal_reference_mrid(net, detailed, value))
            .unwrap_or_else(|| local_terminal.clone());
        eq.reference("RegulatingControl.Terminal", &regulation_terminal);
        eq.close("RegulatingControl");

        ssh.open("StaticVarCompensator", &id, true);
        ssh.text("StaticVarCompensator.q", svc.q);
        ssh.text("RegulatingCondEq.controlEnabled", svc.regulating);
        if v3 {
            ssh.text("Equipment.inService", svc.in_service);
        }
        ssh.close("StaticVarCompensator");
        ssh.open("RegulatingControl", &control, true);
        ssh.text("RegulatingControl.enabled", svc.regulating);
        ssh.text(
            "RegulatingControl.targetValue",
            match svc.regulation_mode {
                StaticVarCompensatorRegulationMode::Voltage => svc.voltage_setpoint_kv,
                StaticVarCompensatorRegulationMode::ReactivePower => {
                    svc.reactive_power_setpoint_mvar
                }
            },
        );
        ssh.close("RegulatingControl");

        let flow = det_mrid("svpowerflow", &local_terminal);
        sv.open("SvPowerFlow", &flow, false);
        sv.reference("SvPowerFlow.Terminal", &local_terminal);
        sv.text("SvPowerFlow.p", svc.p);
        sv.text("SvPowerFlow.q", svc.q);
        sv.close("SvPowerFlow");
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
            terminal(
                &mut eq,
                &mut tp,
                &mut ssh,
                &id,
                "switch",
                local_id,
                1,
                switch.from,
                true,
            );
            terminal(
                &mut eq, &mut tp, &mut ssh, &id, "switch", local_id, 2, switch.to, true,
            );
            ssh.open("Breaker", &id, true);
            ssh.text("Switch.open", !switch.closed);
            ssh.close("Breaker");
        }
    }

    // --- branches ---------------------------------------------------------------
    let mut limit_body = Doc::new();
    for (i, branch) in net.branches().iter().enumerate() {
        let fallback = format!("{}-{}-{i}", branch.from, branch.to);
        let local_id = branch.uid.as_deref().unwrap_or(&fallback);
        let id = mrid_or("branch", &fallback, branch.uid.as_deref());
        let kv = w.kv(branch.from)?;
        let z_base = kv * kv / net.base_mva();
        let y_base = net.base_mva() / (kv * kv);
        let charging = branch.calc_terminal_charging();
        if !charging.is_matpower_symmetric() {
            w.warnings.push(format!(
                "branch {} ({}-{}): asymmetric terminal charging folded into the \
                 symmetric bch/gch totals",
                i + 1,
                branch.from,
                branch.to
            ));
        }
        if branch.control.is_some() {
            w.warnings.push(format!(
                "branch {} ({}-{}): automatic tap/phase control data is not \
                 written (fixed in-service step only)",
                i + 1,
                branch.from,
                branch.to
            ));
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
            w.warnings.push(format!(
                "SeriesCompensator `{local_id}` has nonzero shunt charging and is written as ACLineSegment: positive-sequence r/x, charging, terminal connectivity, service status, and limits are preserved; the equipment class is projected; {fields}"
            ));
        }
        if !branch.is_transformer() {
            if source_is_series_compensator && !series_compensator_has_charging {
                eq.named("SeriesCompensator", &id, &format!("line{}", i + 1));
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
                eq.close("SeriesCompensator");
            } else {
                eq.named("ACLineSegment", &id, &format!("line{}", i + 1));
                eq.text("ACLineSegment.r", branch.r * z_base);
                eq.text("ACLineSegment.x", branch.x * z_base);
                eq.text("ACLineSegment.bch", branch.calc_total_charging_b() * y_base);
                let g_total = charging.calc_total_g();
                if g_total != 0.0 {
                    eq.text("ACLineSegment.gch", g_total * y_base);
                }
                eq.reference("ConductingEquipment.BaseVoltage", &base_of(kv));
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
                w.warnings.push(format!(
                    "branch {} ({}-{}): fixed phase shift {} degrees differs from the retained tap changer position value {} degrees; the tap changer definition was emitted",
                    i + 1,
                    branch.from,
                    branch.to,
                    branch.shift,
                    source_shift
                ));
            }
            eq.named("PowerTransformer", &id, &format!("transformer{}", i + 1));
            eq.close("PowerTransformer");
            for (endno, u) in [(1usize, u1), (2usize, u2)] {
                let end = det_mrid("xfend", &format!("{id}:{endno}"));
                eq.named(
                    "PowerTransformerEnd",
                    &end,
                    &format!("transformer{}-end{endno}", i + 1),
                );
                eq.reference("PowerTransformerEnd.PowerTransformer", &id);
                eq.text("TransformerEnd.endNumber", endno);
                eq.reference("TransformerEnd.Terminal", &term_id(&id, endno));
                eq.text("PowerTransformerEnd.ratedU", u);
                if endno == 1 {
                    let zb1 = u * u / net.base_mva();
                    eq.text("PowerTransformerEnd.r", branch.r * zb1);
                    eq.text("PowerTransformerEnd.x", branch.x * zb1);
                    eq.text(
                        "PowerTransformerEnd.b",
                        branch.calc_total_charging_b() / zb1,
                    );
                } else {
                    eq.text("PowerTransformerEnd.r", 0.0);
                    eq.text("PowerTransformerEnd.x", 0.0);
                    eq.text("PowerTransformerEnd.b", 0.0);
                }
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
                    &det_mrid("xfend", &format!("{id}:1")),
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
        terminal(
            &mut eq,
            &mut tp,
            &mut ssh,
            &id,
            "branch",
            local_id,
            1,
            branch.from,
            branch.in_service,
        );
        terminal(
            &mut eq,
            &mut tp,
            &mut ssh,
            &id,
            "branch",
            local_id,
            2,
            branch.to,
            branch.in_service,
        );
        if v3 {
            let class = if branch.is_transformer() {
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
                limit_body.reference("OperationalLimitSet.Terminal", &term_id(&id, 1));
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
                w.warnings.push(format!(
                    "branch {} ({}-{}): extra rating sets / current ratings beyond \
                     A/B/C have no CGMES slot",
                    i + 1,
                    branch.from,
                    branch.to
                ));
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
        eq.close("PowerTransformer");

        let star_impedances = transformer.calc_star_impedances();
        for (index, winding) in transformer.windings.iter().enumerate() {
            let end_number = index + 1;
            let end = det_mrid("xfend", &format!("{id}:{end_number}"));
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
                w.warnings.push(format!(
                    "three winding transformer `{local_id}` winding {end_number}: fixed tap ratio {tap_factor} differs from the retained tap changer position value {}; the tap changer definition was emitted",
                    step.rho
                ));
            }
            if let Some(source_phase) = source_phase
                && let Some(step) = tap_step(source_phase)
                && (step.alpha_degrees - winding.shift).abs() > 1e-9
            {
                w.warnings.push(format!(
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
            eq.reference("TransformerEnd.Terminal", &term_id(&id, end_number));
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
                &id,
                "branch",
                local_id,
                end_number,
                winding.bus,
                transformer.in_service,
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
                    limit_body.reference("OperationalLimitSet.Terminal", &term_id(&id, end_number));
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
                w.warnings.push(format!(
                    "tap changer on `{}` winding {} was not emitted: {message}",
                    tap.transformer, tap.winding
                ));
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
                w.warnings.push(format!(
                    "operational limit group `{}`: active and apparent power limits belong to the CIM16 EquipmentOperation profile and were omitted from the four-profile CGMES 2.4.15 output",
                    group.id
                ));
            }
            if !emits {
                continue;
            }
            limit_body.named("OperationalLimitSet", &set, &group.id);
            limit_body.reference(
                "OperationalLimitSet.Terminal",
                &term_id(&owner, usize::from(group.terminal)),
            );
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
                    );
                }
            }
            if group.selected {
                w.warnings.push(format!(
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
        w.warnings
            .push("no reference bus: the SV island has no angle reference".into());
    }

    // --- unrepresented families ------------------------------------------------
    for (what, count) in [
        ("storage unit", net.storage().len()),
        ("HVDC line", net.hvdc().len()),
        (
            "area record",
            net.areas()
                .iter()
                .filter(|a| a.slack_bus.is_some() || a.net_interchange != 0.0)
                .count(),
        ),
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
        w.warnings.push(
            "case date is absent; CGMES Model.scenarioTime and Model.created use \
                 2000-01-01T00:00:00Z"
                .into(),
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
    Ok(CgmesFiles {
        files,
        warnings: w.warnings,
    })
}

//! Merged CGMES profiles into a [`BalancedNetwork`].
//!
//! `TopologicalNode` is the bus. Terminals tie conducting equipment to nodes
//! (directly in 2.4.15 TP; through `ConnectivityNode.TopologicalNode` in
//! 3.0). SSH carries the operating point (`p`/`q`, switch position, tap steps,
//! sections); SV supplies solved voltage, tap, and terminal power values.
//! Missing SSH degrades gracefully — vendor exports like the CIGRE MV set
//! ship EQ/TP/SV only, so element `p`/`q` falls back to the terminal's
//! `SvPowerFlow`.
//!
//! CGMES values are MW/MVAr/kV/ohm/S; per-unit lands on a 100 MVA system
//! base. Everything the mapping does not consume is counted per class into
//! the parse warnings, never dropped silently.

use std::collections::{BTreeMap, HashMap};

use powerio_core::ComponentId;

use super::xml::{CimDocument, CimObject, ModelHeader, PropValue, parse_cimxml};
use super::{CGMES_CLASS_PROPERTY, CgmesVersion};
use crate::network::{
    AcDcConverterControlMode, ActivePowerControl, BalancedNetwork, Branch, BranchCharging, Bus,
    BusBreakerBus, BusId, BusType, BusbarSection, CalculatedBus, ComponentAlias, ComponentMetadata,
    ConnectivityNode, CurveStyle, DcBusbar, DcConverterOperatingMode, DcConverterUnit, DcGround,
    DcLine, DcNode, DcPolarity, DcSeriesDevice, DcSwitch, DcSwitchKind, DcTerminal,
    DcTopologicalNode, DetailedConnectivity, ExternalIdentifier, Generator, Impedance,
    LineCommutatedConverter, LineCommutatedConverterOperatingMode, Load, LoadVoltageModel,
    LoadingLimits, OperationalLimitGroup, ReactiveCapabilityCurve, ReactiveCapabilityCurvePoint,
    ReactiveLimits, Shunt, ShuntBlock, SourceFormat, StaticVarCompensator,
    StaticVarCompensatorRegulationMode, Substation, Switch, SwitchKind, SwitchedShuntControl,
    SwitchedShuntMode, TapChanger, TapChangerKind, TapChangerRegulationMode, TapChangerStep,
    TemporaryLimit, Terminal, TerminalReference, TopologyEndpoint, TopologyKind, TopologySwitch,
    Transformer3W, VoltageLevel, VoltageSourceConverter, Winding,
};
use crate::{Error, Result};

const FMT: &str = "CGMES";
/// CGMES has no system MVA base; every per-unit value lands on this one.
const SYSTEM_MVA: f64 = 100.0;

pub(crate) struct Parsed {
    pub(crate) network: BalancedNetwork,
    pub(crate) warnings: Vec<String>,
}

/// The per-file role, from the `md:FullModel` profile URIs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Profile {
    Model,
    /// Diagram layout / geography / dynamics: valid parts of a set, but
    /// nothing the balanced model consumes; counted, not merged.
    Presentation,
}

fn classify(header: Option<&ModelHeader>) -> Profile {
    let Some(header) = header else {
        return Profile::Model;
    };
    let presentation = header.profiles.iter().all(|p| {
        p.contains("DiagramLayout") || p.contains("GeographicalLocation") || p.contains("Dynamics")
    });
    if presentation && !header.profiles.is_empty() {
        Profile::Presentation
    } else {
        Profile::Model
    }
}

/// One object merged across the profile files.
struct Merged {
    id: String,
    class: String,
    defined: bool,
    props: Vec<(String, PropValue)>,
}

/// The merged object store plus the id and class indexes the mapping reads.
struct Store {
    objects: Vec<Merged>,
    by_id: HashMap<String, usize>,
}

/// CIM100 SSH serializes the inherited `Equipment.inService` property in an
/// `Equipment rdf:about` element, even when EQ defines the same object as a
/// concrete subclass such as `BusbarSection`. It is an extension of that
/// object, not a second object with a conflicting class.
fn is_profile_extension_class(class: &str) -> bool {
    class == "Equipment"
}

impl Store {
    fn merge(&mut self, doc: CimDocument) -> Result<()> {
        for CimObject {
            class,
            id,
            definition,
            props,
        } in doc.objects
        {
            if id.is_empty() {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!("{class} object has no rdf:ID or rdf:about identifier"),
                });
            }
            if let Some(&at) = self.by_id.get(&id) {
                if self.objects[at].class != class {
                    if !definition && is_profile_extension_class(&class) {
                        self.objects[at].props.extend(props);
                        continue;
                    }
                    if !self.objects[at].defined
                        && is_profile_extension_class(&self.objects[at].class)
                    {
                        self.objects[at].class = class;
                    } else {
                        return Err(Error::FormatRead {
                            format: FMT,
                            message: format!(
                                "RDF identifier `{id}` is used for both {} and {class}",
                                self.objects[at].class
                            ),
                        });
                    }
                }
                if definition && self.objects[at].defined {
                    return Err(Error::FormatRead {
                        format: FMT,
                        message: format!("RDF identifier `{id}` is defined more than once"),
                    });
                }
                self.objects[at].defined |= definition;
                self.objects[at].props.extend(props);
            } else {
                self.by_id.insert(id.clone(), self.objects.len());
                self.objects.push(Merged {
                    id,
                    class,
                    defined: definition,
                    props,
                });
            }
        }
        Ok(())
    }

    fn class_of(&self, id: &str) -> Option<&str> {
        self.by_id
            .get(id)
            .map(|&at| self.objects[at].class.as_str())
    }

    fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    /// Ids of every object of `class`, in first-definition order.
    fn of_class<'a>(&'a self, class: &'a str) -> impl Iterator<Item = &'a str> {
        self.objects
            .iter()
            .filter(move |o| o.class == class)
            .map(|o| o.id.as_str())
    }

    fn prop(&self, id: &str, key: &str) -> Option<&PropValue> {
        let &at = self.by_id.get(id)?;
        self.objects[at]
            .props
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    fn text(&self, id: &str, key: &str) -> Option<&str> {
        self.prop(id, key).map(PropValue::as_str)
    }

    fn f(&self, id: &str, key: &str) -> Option<f64> {
        self.text(id, key)?.trim().parse().ok()
    }

    fn boolean(&self, id: &str, key: &str) -> Option<bool> {
        match self.text(id, key)?.trim() {
            "true" | "TRUE" | "True" | "1" => Some(true),
            "false" | "FALSE" | "False" | "0" => Some(false),
            _ => None,
        }
    }

    /// A reference property's target id.
    fn refv(&self, id: &str, key: &str) -> Option<&str> {
        match self.prop(id, key)? {
            PropValue::Ref(target) => Some(target),
            PropValue::Text(_) => None,
        }
    }

    /// The `value` half of an `EnumClass.value` reference.
    fn enum_value(&self, id: &str, key: &str) -> Option<&str> {
        self.refv(id, key)?.rsplit('.').next()
    }

    fn name(&self, id: &str) -> String {
        self.text(id, "IdentifiedObject.name")
            .map_or_else(|| id.to_string(), str::to_string)
    }
}

/// Terminal wiring: equipment → its terminals (sequence order), terminal →
/// topological node, and the terminal SSH connection value.
struct Wiring {
    of_equipment: HashMap<String, Vec<String>>,
    node_of: HashMap<String, String>,
    connected: HashMap<String, bool>,
}

impl Wiring {
    fn build(store: &Store) -> Wiring {
        let mut of_equipment: HashMap<String, Vec<(f64, String)>> = HashMap::new();
        let mut node_of = HashMap::new();
        let mut connected = HashMap::new();
        for id in store.of_class("Terminal") {
            if let Some(eq) = store.refv(id, "Terminal.ConductingEquipment") {
                let seq = store
                    .f(id, "ACDCTerminal.sequenceNumber")
                    .or_else(|| store.f(id, "Terminal.sequenceNumber"))
                    .unwrap_or(1.0);
                of_equipment
                    .entry(eq.to_string())
                    .or_default()
                    .push((seq, id.to_string()));
            }
            // 2.4.15 TP links the terminal itself; 3.0 links through the
            // connectivity node. Either way the terminal lands on a TN.
            let tn = store.refv(id, "Terminal.TopologicalNode").or_else(|| {
                let cn = store.refv(id, "Terminal.ConnectivityNode")?;
                store.refv(cn, "ConnectivityNode.TopologicalNode")
            });
            if let Some(tn) = tn {
                node_of.insert(id.to_string(), tn.to_string());
            }
            if let Some(is_connected) = store.boolean(id, "ACDCTerminal.connected") {
                connected.insert(id.to_string(), is_connected);
            }
        }
        let of_equipment = of_equipment
            .into_iter()
            .map(|(eq, mut terms)| {
                terms.sort_by(|a, b| a.0.total_cmp(&b.0));
                (eq, terms.into_iter().map(|(_, t)| t).collect())
            })
            .collect();
        Wiring {
            of_equipment,
            node_of,
            connected,
        }
    }

    fn terminals(&self, equipment: &str) -> &[String] {
        self.of_equipment.get(equipment).map_or(&[], Vec::as_slice)
    }

    fn node(&self, terminal: &str) -> Option<&str> {
        self.node_of.get(terminal).map(String::as_str)
    }

    /// In service as far as the terminals say: every terminal connected
    /// (missing SSH data reads as connected).
    fn energized(&self, equipment: &str) -> bool {
        self.terminals(equipment)
            .iter()
            .all(|t| self.connected.get(t).copied().unwrap_or(true))
    }
}

/// Everything the element builders share.
struct Mapper<'a> {
    store: &'a Store,
    wiring: Wiring,
    bus_of_tn: HashMap<String, BusId>,
    kv_of: HashMap<BusId, f64>,
    /// Terminal → solved SvPowerFlow (p, q), the SSH fallback.
    sv_flow: HashMap<String, (f64, f64)>,
    warnings: &'a mut Vec<String>,
}

impl Mapper<'_> {
    fn bus_of_equipment_terminal(&mut self, equipment: &str, index: usize) -> Option<BusId> {
        let terminal = self.wiring.terminals(equipment).get(index)?.clone();
        let tn = self.wiring.node(&terminal)?.to_string();
        self.bus_of_tn.get(tn.as_str()).copied()
    }

    fn kv(&self, bus: BusId) -> f64 {
        let kv = self.kv_of.get(&bus).copied().unwrap_or(0.0);
        if kv > 0.0 { kv } else { 1.0 }
    }

    /// SSH value with SvPowerFlow fallback (vendor sets without SSH).
    fn power(&self, equipment: &str, key: &str) -> (f64, f64) {
        let store = self.store;
        if let (Some(p), Some(q)) = (
            store.f(equipment, &format!("{key}.p")),
            store.f(equipment, &format!("{key}.q")),
        ) {
            return (p, q);
        }
        for terminal in self.wiring.terminals(equipment) {
            if let Some(&(p, q)) = self.sv_flow.get(terminal) {
                return (p, q);
            }
        }
        (0.0, 0.0)
    }

    fn in_service(&self, equipment: &str) -> bool {
        self.store
            .boolean(equipment, "Equipment.inService")
            .unwrap_or_else(|| self.wiring.energized(equipment))
    }
}

/// Read already acquired CGMES XML profile documents as one case.
pub(crate) fn read_cgmes_documents(
    documents: Vec<(String, String)>,
    name_hint: Option<&str>,
) -> Result<Parsed> {
    let mut warnings = Vec::new();
    let mut store = Store {
        objects: Vec::new(),
        by_id: HashMap::new(),
    };
    let mut versions: Vec<CgmesVersion> = Vec::new();
    let mut description: Option<String> = None;
    let mut scenario_time: Option<String> = None;
    let mut skipped: Vec<String> = Vec::new();
    let mut has_eq = false;
    let mut has_tp = false;

    for (name, text) in documents {
        let doc = parse_cimxml(&text)?;
        if let Some(ns) = &doc.cim_namespace {
            if let Some(version) = CgmesVersion::from_namespace(ns) {
                if !versions.contains(&version) {
                    versions.push(version);
                }
            }
        }
        if classify(doc.header.as_ref()) == Profile::Presentation {
            skipped.push(name);
            continue;
        }
        if let Some(header) = doc.header.as_ref() {
            has_eq |= header.profiles.iter().any(|profile| {
                profile.contains("/EquipmentCore/") || profile.contains("/CIM/CoreEquipment")
            });
            has_tp |= header.profiles.iter().any(|profile| {
                profile.contains("/Topology/") || profile.contains("/CIM/Topology-")
            });
            scenario_time = scenario_time.or_else(|| header.scenario_time.clone());
        }
        if description.is_none() {
            description = doc.header.as_ref().and_then(|h| h.description.clone());
        }
        store.merge(doc)?;
    }

    let version = match versions.as_slice() {
        [] => {
            return Err(Error::FormatRead {
                format: FMT,
                message: "no file declares a CIM16/CIM100 namespace; not a CGMES set".into(),
            });
        }
        [one] => *one,
        many => {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "one profile set declares more than one CGMES release: {}",
                    many.iter()
                        .map(|version| version.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    };
    validate_critical_references(&store)?;
    if !skipped.is_empty() {
        warnings.push(format!(
            "presentation/dynamics parts not mapped: {}",
            skipped.join(", ")
        ));
    }
    if !has_eq || !has_tp {
        let missing = match (has_eq, has_tp) {
            (false, false) => "EQ and TP",
            (false, true) => "EQ",
            (true, false) => "TP",
            (true, true) => unreachable!(),
        };
        return Err(Error::FormatRead {
            format: FMT,
            message: format!("CGMES profile set is missing required {missing} profile data"),
        });
    }

    let network = build(
        &store,
        version,
        description,
        scenario_time,
        name_hint,
        &mut warnings,
    )?;
    Ok(Parsed { network, warnings })
}

fn validate_critical_references(store: &Store) -> Result<()> {
    const REQUIRED: [&str; 30] = [
        "Terminal.ConductingEquipment",
        "Terminal.TopologicalNode",
        "Terminal.ConnectivityNode",
        "ConnectivityNode.TopologicalNode",
        "TopologicalNode.BaseVoltage",
        "PowerTransformerEnd.PowerTransformer",
        "TransformerEnd.Terminal",
        "SvVoltage.TopologicalNode",
        "SvPowerFlow.Terminal",
        "RatioTapChanger.TransformerEnd",
        "PhaseTapChanger.TransformerEnd",
        "TapChanger.TapChangerControl",
        "RegulatingControl.Terminal",
        "OperationalLimitSet.Terminal",
        "OperationalLimitSet.Equipment",
        "OperationalLimit.OperationalLimitSet",
        "OperationalLimit.OperationalLimitType",
        "SvTapStep.TapChanger",
        "DCTerminal.DCConductingEquipment",
        "ACDCConverterDCTerminal.DCConductingEquipment",
        "DCBaseTerminal.DCNode",
        "DCBaseTerminal.DCTopologicalNode",
        "DCNode.DCTopologicalNode",
        "ACDCConverter.PccTerminal",
        "DCNode.DCEquipmentContainer",
        "DCTopologicalNode.DCEquipmentContainer",
        "DCConverterUnit.Substation",
        "Equipment.EquipmentContainer",
        "VsConverter.CapabilityCurve",
        "CurveData.Curve",
    ];
    for object in &store.objects {
        for (property, value) in &object.props {
            if !REQUIRED.contains(&property.as_str()) {
                continue;
            }
            let PropValue::Ref(target) = value else {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "{} `{}` property {property} is not an RDF resource reference",
                        object.class, object.id
                    ),
                });
            };
            if !store.contains(target) {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "{} `{}` property {property} references missing `{target}`",
                        object.class, object.id
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Switch classes, all mapping to the neutral switch record.
const SWITCH_CLASSES: [&str; 8] = [
    "Breaker",
    "Disconnector",
    "LoadBreakSwitch",
    "Switch",
    "Fuse",
    "Jumper",
    "GroundDisconnector",
    "DisconnectingCircuitBreaker",
];

/// Load classes sharing the EnergyConsumer attribute set.
const LOAD_CLASSES: [&str; 4] = [
    "EnergyConsumer",
    "ConformLoad",
    "NonConformLoad",
    "StationSupply",
];

/// Classes the mapping consumes structurally (no "unmapped" warning).
const CONSUMED: &[&str] = &[
    "TopologicalNode",
    "ConnectivityNode",
    "Terminal",
    "BaseVoltage",
    "BaseFrequency",
    "SvVoltage",
    "SvPowerFlow",
    "SvTapStep",
    "SvShuntCompensatorSections",
    "TopologicalIsland",
    "ACLineSegment",
    "SeriesCompensator",
    "PowerTransformer",
    "PowerTransformerEnd",
    "RatioTapChanger",
    "PhaseTapChangerLinear",
    "PhaseTapChangerSymmetrical",
    "PhaseTapChangerAsymmetrical",
    "PhaseTapChangerTabular",
    "RatioTapChangerTable",
    "RatioTapChangerTablePoint",
    "PhaseTapChangerTable",
    "PhaseTapChangerTablePoint",
    "SynchronousMachine",
    "GeneratingUnit",
    "ThermalGeneratingUnit",
    "HydroGeneratingUnit",
    "WindGeneratingUnit",
    "SolarGeneratingUnit",
    "NuclearGeneratingUnit",
    "ExternalNetworkInjection",
    "LinearShuntCompensator",
    "EquivalentInjection",
    "StaticVarCompensator",
    "DCConverterUnit",
    "DCNode",
    "DCTopologicalNode",
    "DCTerminal",
    "ACDCConverterDCTerminal",
    "DCGround",
    "DCBusbar",
    "DCLine",
    "DCLineSegment",
    "DCSeriesDevice",
    "DCSwitch",
    "DCBreaker",
    "DCDisconnector",
    "VsConverter",
    "CsConverter",
    "VsCapabilityCurve",
    "CurveData",
];

#[allow(clippy::too_many_lines)] // the element families map in one ordered pass
fn build(
    store: &Store,
    version: CgmesVersion,
    description: Option<String>,
    scenario_time: Option<String>,
    name_hint: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<BalancedNetwork> {
    let wiring = Wiring::build(store);

    // Buses from TopologicalNode, ids in definition order.
    let mut buses = Vec::new();
    let mut bus_of_tn = HashMap::new();
    let mut kv_of = HashMap::new();
    for (i, tn) in store.of_class("TopologicalNode").enumerate() {
        let id = BusId(i + 1);
        let kv = store
            .refv(tn, "TopologicalNode.BaseVoltage")
            .and_then(|bv| store.f(bv, "BaseVoltage.nominalVoltage"))
            .unwrap_or(0.0);
        let mut bus = Bus::new(id, BusType::Pq, kv);
        bus.name = Some(store.name(tn));
        bus.uid = Some(tn.to_string());
        buses.push(bus);
        bus_of_tn.insert(tn.to_string(), id);
        kv_of.insert(id, kv);
    }
    if buses.is_empty() {
        return Err(Error::FormatRead {
            format: FMT,
            message: "no TopologicalNode records; the bus-branch reader needs the TP \
                      part of the set (node-breaker collapse is follow-up work)"
                .into(),
        });
    }

    // SV voltage values onto the buses.
    for sv in store.of_class("SvVoltage") {
        let Some(bus) = store
            .refv(sv, "SvVoltage.TopologicalNode")
            .and_then(|tn| bus_of_tn.get(tn))
        else {
            continue;
        };
        let bus = &mut buses[bus.0 - 1];
        if let Some(v) = store.f(sv, "SvVoltage.v") {
            let kv = if bus.base_kv > 0.0 { bus.base_kv } else { 1.0 };
            bus.vm = v / kv;
        }
        if let Some(angle) = store.f(sv, "SvVoltage.angle") {
            bus.va = angle;
        }
    }
    let mut sv_flow = HashMap::new();
    for sv in store.of_class("SvPowerFlow") {
        if let (Some(terminal), Some(p), Some(q)) = (
            store.refv(sv, "SvPowerFlow.Terminal"),
            store.f(sv, "SvPowerFlow.p"),
            store.f(sv, "SvPowerFlow.q"),
        ) {
            sv_flow.insert(terminal.to_string(), (p, q));
        }
    }

    let mut mapper = Mapper {
        store,
        wiring,
        bus_of_tn,
        kv_of,
        sv_flow,
        warnings,
    };

    let mut loads = read_loads(&mut mapper);
    let (mut generators, ref_candidates) = read_machines(&mut mapper)?;
    let shunts = read_shunts(&mut mapper);
    let static_var_compensators = read_static_var_compensators(&mut mapper)?;
    let (branches, transformers_3w) = read_branches(&mut mapper, version);
    let switches = read_switches(&mut mapper);
    read_equivalent_injections(&mut mapper, &mut loads, &mut generators);

    // Reference bus: the island's angle reference, else the best candidate
    // (lowest referencePriority, then an external injection, then the
    // largest machine).
    let mut reference: Option<BusId> = None;
    for island in store.of_class("TopologicalIsland") {
        if let Some(bus) = store
            .refv(island, "TopologicalIsland.AngleRefTopologicalNode")
            .and_then(|tn| mapper.bus_of_tn.get(tn))
        {
            reference = Some(*bus);
            break;
        }
    }
    let reference = reference.or(ref_candidates);
    match reference {
        Some(id) => buses[id.0 - 1].kind = BusType::Ref,
        None => mapper.warnings.push(
            "no angle reference in the set (no TopologicalIsland, reference \
             priority, or external injection); matrix consumers will report \
             the missing slack"
                .into(),
        ),
    }
    for generator in &generators {
        let bus = &mut buses[generator.bus.0 - 1];
        if bus.kind == BusType::Pq {
            bus.kind = BusType::Pv;
        }
    }

    warn_unmapped(store, mapper.warnings);
    let detailed = build_detailed_connectivity(&mut mapper, version)?;

    let base_frequency = store
        .of_class("BaseFrequency")
        .next()
        .and_then(|f| store.f(f, "BaseFrequency.frequency"))
        .unwrap_or_else(|| {
            mapper
                .warnings
                .push("no BaseFrequency record; assuming 50 Hz".into());
            50.0
        });
    mapper.warnings.push(format!(
        "{}: per-unit values normalized onto a 100 MVA system base (CGMES \
         carries none)",
        version.label()
    ));

    let name = description
        .filter(|d| !d.is_empty())
        .or_else(|| name_hint.map(str::to_string))
        .unwrap_or_else(|| "cgmes case".into());
    let mut net = BalancedNetwork::new(name, SYSTEM_MVA);
    net.case_metadata_mut().case_date = scenario_time;
    net.case_metadata_mut().source_model_format = Some(version.label().to_string());
    *net.base_frequency_mut() = base_frequency;
    *net.buses_mut() = buses;
    *net.loads_mut() = loads;
    *net.shunts_mut() = shunts;
    *net.static_var_compensators_mut() = static_var_compensators;
    *net.branches_mut() = branches;
    *net.transformers_3w_mut() = transformers_3w;
    *net.switches_mut() = switches;
    *net.generators_mut() = generators;
    *net.detailed_connectivity_mut() = Some(std::sync::Arc::new(detailed));
    *net.source_format_mut() = SourceFormat::Cgmes;
    net.assign_missing_component_ids();
    net.check_references(FMT)?;
    Ok(net)
}

fn component_id(kind: &str, id: &str) -> Result<ComponentId> {
    ComponentId::new(kind, id).map_err(|error| Error::FormatRead {
        format: FMT,
        message: error.to_string(),
    })
}

fn component_type(class: &str) -> &'static str {
    match class {
        "Substation" => "substation",
        "VoltageLevel" => "voltage_level",
        "TopologicalNode" => "bus",
        "ConnectivityNode" => "connectivity_node",
        "BusbarSection" => "busbar_section",
        "EnergyConsumer"
        | "ConformLoad"
        | "NonConformLoad"
        | "StationSupply"
        | "EquivalentInjection" => "load",
        "SynchronousMachine" | "ExternalNetworkInjection" => "generator",
        "LinearShuntCompensator" | "NonlinearShuntCompensator" => "shunt",
        "StaticVarCompensator" => "static_var_compensator",
        "Breaker"
        | "Disconnector"
        | "LoadBreakSwitch"
        | "Switch"
        | "Fuse"
        | "Jumper"
        | "GroundDisconnector"
        | "DisconnectingCircuitBreaker" => "switch",
        "ACLineSegment" | "SeriesCompensator" | "PowerTransformer" => "branch",
        "Line" => "line",
        "Terminal" => "terminal",
        "DCConverterUnit" => "dc_converter_unit",
        "DCNode" => "dc_node",
        "DCTopologicalNode" => "dc_topological_node",
        "DCGround" => "dc_ground",
        "DCBusbar" => "dc_busbar",
        "DCLine" => "dc_line_container",
        "DCLineSegment" => "dc_line",
        "DCSeriesDevice" => "dc_series_device",
        "DCSwitch" | "DCBreaker" | "DCDisconnector" => "dc_switch",
        "VsConverter" => "voltage_source_converter",
        "CsConverter" => "line_commutated_converter",
        "DCTerminal" | "ACDCConverterDCTerminal" => "dc_terminal",
        _ => "cgmes_object",
    }
}

fn equipment_voltage_level<'a>(store: &'a Store, equipment: &str) -> Option<&'a str> {
    store
        .refv(equipment, "Equipment.EquipmentContainer")
        .filter(|container| store.class_of(container) == Some("VoltageLevel"))
}

fn terminal_voltage_level<'a>(store: &'a Store, terminal: &str) -> Option<&'a str> {
    if let Some(node) = store.refv(terminal, "Terminal.ConnectivityNode") {
        return store.refv(node, "ConnectivityNode.ConnectivityNodeContainer");
    }
    if let Some(node) = store.refv(terminal, "Terminal.TopologicalNode") {
        return store.refv(node, "TopologicalNode.ConnectivityNodeContainer");
    }
    store
        .refv(terminal, "Terminal.ConductingEquipment")
        .and_then(|equipment| equipment_voltage_level(store, equipment))
}

fn solved_voltage(store: &Store, topological_node: &str) -> (Option<f64>, Option<f64>) {
    store
        .of_class("SvVoltage")
        .find(|value| store.refv(value, "SvVoltage.TopologicalNode") == Some(topological_node))
        .map_or((None, None), |value| {
            (
                store.f(value, "SvVoltage.v"),
                store.f(value, "SvVoltage.angle"),
            )
        })
}

#[allow(clippy::too_many_lines)] // one ordered pass builds every linked topology table
fn build_detailed_connectivity(
    mapper: &mut Mapper<'_>,
    version: CgmesVersion,
) -> Result<DetailedConnectivity> {
    let store = mapper.store;
    let substations = store
        .of_class("Substation")
        .map(|id| {
            Ok(Substation {
                component: component_id("substation", id)?,
                country: store
                    .enum_value(id, "entsoe:Substation.Country")
                    .map(str::to_string),
                operator: None,
                geographical_tags: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let voltage_levels = store
        .of_class("VoltageLevel")
        .map(|id| {
            let base = store
                .refv(id, "VoltageLevel.BaseVoltage")
                .and_then(|base| store.f(base, "BaseVoltage.nominalVoltage"))
                .unwrap_or(0.0);
            let has_nodes = store.of_class("ConnectivityNode").any(|node| {
                store.refv(node, "ConnectivityNode.ConnectivityNodeContainer") == Some(id)
            });
            let mut buses = store
                .of_class("TopologicalNode")
                .filter(|node| {
                    store.refv(node, "TopologicalNode.ConnectivityNodeContainer") == Some(id)
                })
                .filter_map(|node| mapper.bus_of_tn.get(node).copied())
                .collect::<Vec<_>>();
            buses.sort_unstable();
            buses.dedup();
            Ok(VoltageLevel {
                component: component_id("voltage_level", id)?,
                substation: store
                    .refv(id, "VoltageLevel.Substation")
                    .map(|substation| component_id("substation", substation))
                    .transpose()?,
                nominal_kv: base,
                low_voltage_limit_kv: store.f(id, "VoltageLevel.lowVoltageLimit"),
                high_voltage_limit_kv: store.f(id, "VoltageLevel.highVoltageLimit"),
                topology_kind: if has_nodes {
                    TopologyKind::NodeBreaker
                } else {
                    TopologyKind::BusBreaker
                },
                buses,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut connectivity_nodes = Vec::new();
    for id in store.of_class("ConnectivityNode") {
        if let Some(level) = store.refv(id, "ConnectivityNode.ConnectivityNodeContainer") {
            connectivity_nodes.push(ConnectivityNode {
                component: component_id("connectivity_node", id)?,
                voltage_level: component_id(
                    component_type(store.class_of(level).unwrap_or("")),
                    level,
                )?,
                node_number: None,
                calculated_bus: store
                    .refv(id, "ConnectivityNode.TopologicalNode")
                    .and_then(|node| mapper.bus_of_tn.get(node).copied()),
            });
        }
    }

    let mut bus_breaker_buses = Vec::new();
    for id in store.of_class("TopologicalNode") {
        if let Some(level) = store.refv(id, "TopologicalNode.ConnectivityNodeContainer") {
            let (voltage_kv, angle_degrees) = solved_voltage(store, id);
            bus_breaker_buses.push(BusBreakerBus {
                component: component_id("bus", id)?,
                voltage_level: component_id(
                    component_type(store.class_of(level).unwrap_or("")),
                    level,
                )?,
                calculated_bus: mapper.bus_of_tn.get(id).copied(),
                voltage_kv,
                angle_degrees,
            });
        }
    }

    let mut calculated_buses = Vec::new();
    for id in store.of_class("TopologicalNode") {
        let (Some(level), Some(calculated_bus)) = (
            store.refv(id, "TopologicalNode.ConnectivityNodeContainer"),
            mapper.bus_of_tn.get(id).copied(),
        ) else {
            continue;
        };
        let nodes = store
            .of_class("ConnectivityNode")
            .filter(|node| store.refv(node, "ConnectivityNode.TopologicalNode") == Some(id))
            .map(|node| component_id("connectivity_node", node))
            .collect::<Result<Vec<_>>>()?;
        if nodes.is_empty() {
            continue;
        }
        let (voltage_kv, angle_degrees) = solved_voltage(store, id);
        calculated_buses.push(CalculatedBus {
            voltage_level: component_id(
                component_type(store.class_of(level).unwrap_or("")),
                level,
            )?,
            calculated_bus,
            nodes,
            voltage_kv,
            angle_degrees,
        });
    }

    let mut busbar_sections = Vec::new();
    for id in store.of_class("BusbarSection") {
        let Some(terminal) = mapper.wiring.terminals(id).first() else {
            continue;
        };
        let Some(node) = store.refv(terminal, "Terminal.ConnectivityNode") else {
            continue;
        };
        let Some(level) = terminal_voltage_level(store, terminal) else {
            continue;
        };
        busbar_sections.push(BusbarSection {
            component: component_id("busbar_section", id)?,
            voltage_level: component_id(
                component_type(store.class_of(level).unwrap_or("")),
                level,
            )?,
            node: component_id("connectivity_node", node)?,
        });
    }

    let mut terminals = Vec::new();
    for id in store.of_class("Terminal") {
        let Some(equipment) = store.refv(id, "Terminal.ConductingEquipment") else {
            continue;
        };
        let Some(level) = terminal_voltage_level(store, id) else {
            continue;
        };
        let sequence = store
            .f(id, "ACDCTerminal.sequenceNumber")
            .or_else(|| store.f(id, "Terminal.sequenceNumber"))
            .unwrap_or(1.0);
        let terminal = u8::try_from(sequence as u64).unwrap_or(u8::MAX);
        let bus = store.refv(id, "Terminal.TopologicalNode");
        let converter_power = matches!(
            store.class_of(equipment),
            Some("VsConverter" | "CsConverter")
        )
        .then(|| {
            let pcc = store
                .refv(equipment, "ACDCConverter.PccTerminal")
                .or_else(|| {
                    mapper
                        .wiring
                        .terminals(equipment)
                        .first()
                        .map(String::as_str)
                });
            (pcc == Some(id)).then(|| {
                (
                    store.f(equipment, "ACDCConverter.p"),
                    store.f(equipment, "ACDCConverter.q"),
                )
            })
        })
        .flatten();
        let solved_power = mapper.sv_flow.get(id).copied();
        terminals.push(Terminal {
            equipment: component_id(
                component_type(store.class_of(equipment).unwrap_or("")),
                equipment,
            )?,
            terminal,
            voltage_level: component_id(
                component_type(store.class_of(level).unwrap_or("")),
                level,
            )?,
            bus: bus.map(|value| component_id("bus", value)).transpose()?,
            connectable_bus: bus.map(|value| component_id("bus", value)).transpose()?,
            node: store
                .refv(id, "Terminal.ConnectivityNode")
                .map(|value| component_id("connectivity_node", value))
                .transpose()?,
            connected: store.boolean(id, "ACDCTerminal.connected").unwrap_or(true),
            active_power_mw: solved_power
                .map(|value| value.0)
                .or_else(|| converter_power.and_then(|value| value.0)),
            reactive_power_mvar: solved_power
                .map(|value| value.1)
                .or_else(|| converter_power.and_then(|value| value.1)),
        });
    }

    let mut switches = Vec::new();
    for class in SWITCH_CLASSES {
        for id in store.of_class(class) {
            let terms = mapper.wiring.terminals(id);
            let (Some(first), Some(second)) = (terms.first(), terms.get(1)) else {
                continue;
            };
            let Some(level) = terminal_voltage_level(store, first) else {
                continue;
            };
            let endpoint = |terminal: &str| -> Result<TopologyEndpoint> {
                if let Some(node) = store.refv(terminal, "Terminal.ConnectivityNode") {
                    Ok(TopologyEndpoint::Node(component_id(
                        "connectivity_node",
                        node,
                    )?))
                } else if let Some(bus) = store.refv(terminal, "Terminal.TopologicalNode") {
                    Ok(TopologyEndpoint::Bus(component_id("bus", bus)?))
                } else {
                    Err(Error::FormatRead {
                        format: FMT,
                        message: format!("switch `{id}` terminal has no topology connection"),
                    })
                }
            };
            switches.push(TopologySwitch {
                component: component_id("switch", id)?,
                voltage_level: component_id(
                    component_type(store.class_of(level).unwrap_or("")),
                    level,
                )?,
                kind: match class {
                    "Disconnector" | "GroundDisconnector" => SwitchKind::Disconnector,
                    "LoadBreakSwitch" => SwitchKind::LoadBreakSwitch,
                    _ => SwitchKind::Breaker,
                },
                endpoint1: endpoint(first)?,
                endpoint2: endpoint(second)?,
                open: store
                    .boolean(id, "Switch.open")
                    .or_else(|| store.boolean(id, "Switch.normalOpen"))
                    .unwrap_or(false),
                retained: store.boolean(id, "Switch.retained").unwrap_or(false),
            });
        }
    }

    let component_metadata = store
        .objects
        .iter()
        .map(|object| {
            let mut properties = BTreeMap::new();
            if object.class == "SeriesCompensator" {
                properties.insert(CGMES_CLASS_PROPERTY.into(), object.class.clone());
                for property in [
                    "SeriesCompensator.r0",
                    "SeriesCompensator.x0",
                    "SeriesCompensator.varistorRatedCurrent",
                    "SeriesCompensator.varistorVoltageThreshold",
                ] {
                    if let Some(value) = store.f(&object.id, property) {
                        properties.insert(property.into(), value.to_string());
                    }
                }
                if let Some(value) = store.boolean(&object.id, "SeriesCompensator.varistorPresent")
                {
                    properties.insert(
                        "SeriesCompensator.varistorPresent".into(),
                        value.to_string(),
                    );
                }
            }
            Ok(ComponentMetadata {
                component: component_id(component_type(&object.class), &object.id)?,
                name: store
                    .text(&object.id, "IdentifiedObject.name")
                    .map(str::to_string),
                aliases: store
                    .text(&object.id, "IdentifiedObject.shortName")
                    .map(|value| {
                        vec![ComponentAlias {
                            value: value.to_string(),
                            alias_type: Some("short_name".into()),
                        }]
                    })
                    .unwrap_or_default(),
                external_identifiers: vec![ExternalIdentifier {
                    value: object.id.clone(),
                    authority: Some("CGMES".into()),
                }],
                properties,
                fictitious: store
                    .boolean(&object.id, "IdentifiedObject.isFictitious")
                    .unwrap_or(false),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let operational_limit_groups = read_operational_limit_groups(mapper, version)?;
    let tap_changers = read_tap_changers(mapper)?;
    let dc = read_dc_equipment(mapper, version)?;

    Ok(DetailedConnectivity {
        component_metadata,
        subnetworks: Vec::new(),
        substations,
        voltage_levels,
        bus_breaker_buses,
        calculated_buses,
        connectivity_nodes,
        busbar_sections,
        terminals,
        switches,
        internal_connections: Vec::new(),
        operational_limit_groups,
        tap_changers,
        equipment_reactive_limits: Vec::new(),
        boundary_lines: Vec::new(),
        tie_lines: Vec::new(),
        dc_converter_units: dc.converter_units,
        dc_topological_nodes: dc.topological_nodes,
        dc_nodes: dc.nodes,
        dc_grounds: dc.grounds,
        dc_busbars: dc.busbars,
        dc_lines: dc.lines,
        dc_series_devices: dc.series_devices,
        dc_switches: dc.switches,
        voltage_source_converters: dc.voltage_source_converters,
        line_commutated_converters: dc.line_commutated_converters,
    })
}

#[derive(Default)]
struct ReadDcEquipment {
    converter_units: Vec<DcConverterUnit>,
    topological_nodes: Vec<DcTopologicalNode>,
    nodes: Vec<DcNode>,
    grounds: Vec<DcGround>,
    busbars: Vec<DcBusbar>,
    lines: Vec<DcLine>,
    series_devices: Vec<DcSeriesDevice>,
    switches: Vec<DcSwitch>,
    voltage_source_converters: Vec<VoltageSourceConverter>,
    line_commutated_converters: Vec<LineCommutatedConverter>,
}

struct DcTerminalWiring {
    by_equipment: HashMap<String, Vec<String>>,
}

impl DcTerminalWiring {
    fn build(store: &Store) -> Result<Self> {
        let mut by_equipment = HashMap::<String, Vec<(Option<u32>, String)>>::new();
        for class in ["DCTerminal", "ACDCConverterDCTerminal"] {
            for terminal in store.of_class(class) {
                let equipment_property = if class == "DCTerminal" {
                    "DCTerminal.DCConductingEquipment"
                } else {
                    "ACDCConverterDCTerminal.DCConductingEquipment"
                };
                if let Some(equipment) = store.refv(terminal, equipment_property) {
                    let sequence = optional_u32(store, terminal, "ACDCTerminal.sequenceNumber")?;
                    by_equipment
                        .entry(equipment.to_string())
                        .or_default()
                        .push((sequence, terminal.to_string()));
                }
            }
        }
        Ok(Self {
            by_equipment: by_equipment
                .into_iter()
                .map(|(equipment, mut terminals)| {
                    terminals.sort_by(|left, right| match (left.0, right.0) {
                        (Some(left), Some(right)) => left.cmp(&right),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => left.1.cmp(&right.1),
                    });
                    (
                        equipment,
                        terminals
                            .into_iter()
                            .map(|(_, terminal)| terminal)
                            .collect(),
                    )
                })
                .collect(),
        })
    }

    fn terminals(&self, equipment: &str) -> &[String] {
        self.by_equipment.get(equipment).map_or(&[], Vec::as_slice)
    }
}

fn dc_terminal(
    store: &Store,
    wiring: &DcTerminalWiring,
    equipment: &str,
    index: usize,
) -> Result<Option<DcTerminal>> {
    let Some(terminal) = wiring.terminals(equipment).get(index) else {
        return Ok(None);
    };
    Ok(Some(DcTerminal {
        component: Some(component_id("dc_terminal", terminal)?),
        sequence_number: optional_u32(store, terminal, "ACDCTerminal.sequenceNumber")?,
        dc_node: store
            .refv(terminal, "DCBaseTerminal.DCNode")
            .map(|node| component_id("dc_node", node))
            .transpose()?,
        dc_topological_node: store
            .refv(terminal, "DCBaseTerminal.DCTopologicalNode")
            .map(|node| component_id("dc_topological_node", node))
            .transpose()?,
        polarity: match store.enum_value(terminal, "ACDCConverterDCTerminal.polarity") {
            Some("positive") => Some(DcPolarity::Positive),
            Some("middle") => Some(DcPolarity::Middle),
            Some("negative") => Some(DcPolarity::Negative),
            Some(value) => {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "ACDCConverterDCTerminal `{terminal}` has unknown polarity `{value}`"
                    ),
                });
            }
            None => None,
        },
        connected: store.boolean(terminal, "ACDCTerminal.connected"),
        active_power_mw: None,
        current_a: None,
    }))
}

fn converter_pcc_terminal(store: &Store, converter: &str) -> Result<Option<TerminalReference>> {
    let Some(terminal) = store.refv(converter, "ACDCConverter.PccTerminal") else {
        return Ok(None);
    };
    let Some(equipment) = store.refv(terminal, "Terminal.ConductingEquipment") else {
        return Ok(None);
    };
    let Some(sequence) = optional_u32(store, terminal, "ACDCTerminal.sequenceNumber")? else {
        return Ok(None);
    };
    let terminal_number = u8::try_from(sequence).map_err(|_| Error::FormatRead {
        format: FMT,
        message: format!("Terminal `{terminal}` sequence number {sequence} exceeds 255"),
    })?;
    Ok(Some(TerminalReference {
        equipment: component_id(
            component_type(store.class_of(equipment).unwrap_or_default()),
            equipment,
        )?,
        terminal: terminal_number,
    }))
}

fn converter_control_mode(
    store: &Store,
    converter: &str,
    class: &str,
    warnings: &mut Vec<String>,
) -> Option<AcDcConverterControlMode> {
    let value = store.enum_value(converter, &format!("{class}.pPccControl"));
    match (class, value) {
        ("VsConverter", Some("udc")) | ("CsConverter", Some("dcVoltage")) => {
            Some(AcDcConverterControlMode::DcVoltage)
        }
        ("VsConverter", Some("pPcc")) | ("CsConverter", Some("activePower")) => {
            Some(AcDcConverterControlMode::ActivePowerAtPcc)
        }
        ("VsConverter", Some("pPccAndUdcDroop")) => {
            Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroop)
        }
        ("VsConverter", Some("pPccAndUdcDroopWithCompensation")) => {
            Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation)
        }
        ("VsConverter", Some("pPccAndUdcDroopPilot")) => {
            Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopPilot)
        }
        ("CsConverter", Some("dcCurrent")) => Some(AcDcConverterControlMode::DcCurrent),
        (_, Some(other)) => {
            warnings.push(format!(
                "{class} {}: {class}.pPccControl `{other}` is unknown and was not assigned",
                store.name(converter)
            ));
            None
        }
        (_, None) => None,
    }
}

fn vsc_reactive_limits(
    store: &Store,
    converter: &str,
    warnings: &mut Vec<String>,
) -> Option<ReactiveLimits> {
    let curve = store.refv(converter, "VsConverter.CapabilityCurve")?;
    let curve_style = match store.enum_value(curve, "Curve.curveStyle") {
        Some("constantYValue") => CurveStyle::ConstantYValue,
        Some("straightLineYValues") => CurveStyle::StraightLineYValues,
        Some(value) => {
            warnings.push(format!(
                "VsCapabilityCurve {}: Curve.curveStyle `{value}` is unknown and reactive limits were not assigned",
                store.name(curve)
            ));
            return None;
        }
        None => {
            warnings.push(format!(
                "VsCapabilityCurve {}: required Curve.curveStyle is absent and reactive limits were not assigned",
                store.name(curve)
            ));
            return None;
        }
    };
    for (property, expected) in [
        ("Curve.xUnit", "W"),
        ("Curve.y1Unit", "VAr"),
        ("Curve.y2Unit", "VAr"),
    ] {
        match store.enum_value(curve, property) {
            Some(value) if value == expected => {}
            Some(value) => {
                warnings.push(format!(
                    "VsCapabilityCurve {}: {property} `UnitSymbol.{value}` is unsupported; expected `UnitSymbol.{expected}`, so reactive limits were not assigned",
                    store.name(curve)
                ));
                return None;
            }
            None => {
                warnings.push(format!(
                    "VsCapabilityCurve {}: {property} is absent; expected `UnitSymbol.{expected}`, so reactive limits were not assigned",
                    store.name(curve)
                ));
                return None;
            }
        }
    }
    let mut points = {
        store
            .of_class("CurveData")
            .filter(|point| store.refv(point, "CurveData.Curve") == Some(curve))
            .filter_map(|point| {
                Some(ReactiveCapabilityCurvePoint {
                    active_power_mw: store.f(point, "CurveData.xvalue")?,
                    minimum_reactive_power_mvar: store.f(point, "CurveData.y1value")?,
                    maximum_reactive_power_mvar: store.f(point, "CurveData.y2value")?,
                    properties: BTreeMap::new(),
                })
            })
            .collect::<Vec<_>>()
    };
    points.sort_by(|left, right| left.active_power_mw.total_cmp(&right.active_power_mw));
    Some(ReactiveLimits::CapabilityCurve(ReactiveCapabilityCurve {
        curve_style,
        properties: BTreeMap::new(),
        points,
    }))
}

fn optional_u32(store: &Store, id: &str, property: &str) -> Result<Option<u32>> {
    let Some(value) = store.f(id, property) else {
        return Ok(None);
    };
    if value < 0.0 || value.fract() != 0.0 || value > f64::from(u32::MAX) {
        return Err(Error::FormatRead {
            format: FMT,
            message: format!("{id} property {property} is not a valid unsigned integer: {value}"),
        });
    }
    Ok(Some(value as u32))
}

fn optional_component_reference(
    store: &Store,
    id: &str,
    property: &str,
) -> Result<Option<ComponentId>> {
    store
        .refv(id, property)
        .map(|target| {
            component_id(
                component_type(store.class_of(target).unwrap_or_default()),
                target,
            )
        })
        .transpose()
}

fn dc_converter_operating_mode(store: &Store, id: &str) -> Result<DcConverterOperatingMode> {
    match store.enum_value(id, "DCConverterUnit.operationMode") {
        Some("bipolar") => Ok(DcConverterOperatingMode::Bipolar),
        Some("monopolarGroundReturn") => Ok(DcConverterOperatingMode::MonopolarGroundReturn),
        Some("monopolarMetallicReturn") => Ok(DcConverterOperatingMode::MonopolarMetallicReturn),
        Some(value) => Err(Error::FormatRead {
            format: FMT,
            message: format!("DCConverterUnit `{id}` has unknown operation mode `{value}`"),
        }),
        None => Err(Error::FormatRead {
            format: FMT,
            message: format!("DCConverterUnit `{id}` has no DCConverterUnit.operationMode"),
        }),
    }
}

fn lcc_operating_mode(
    store: &Store,
    id: &str,
    warnings: &mut Vec<String>,
) -> Option<LineCommutatedConverterOperatingMode> {
    match store.enum_value(id, "CsConverter.operatingMode") {
        Some("rectifier") => Some(LineCommutatedConverterOperatingMode::Rectifier),
        Some("inverter") => Some(LineCommutatedConverterOperatingMode::Inverter),
        Some(value) => {
            warnings.push(format!(
                "CsConverter {}: CsConverter.operatingMode `{value}` is unknown and was not assigned",
                store.name(id)
            ));
            None
        }
        None => None,
    }
}

fn vsc_voltage_regulator_on(store: &Store, id: &str, warnings: &mut Vec<String>) -> Option<bool> {
    match store.enum_value(id, "VsConverter.qPccControl") {
        Some("voltagePcc") => Some(true),
        Some("reactivePcc") => Some(false),
        Some(value) => {
            warnings.push(format!(
                "VsConverter {}: VsConverter.qPccControl `{value}` is unknown and was not assigned",
                store.name(id)
            ));
            None
        }
        None => None,
    }
}

fn cgmes_2_unit_converter_rated_dc_voltage(
    store: &Store,
    equipment_class: &str,
    equipment: &str,
    unit: &str,
) -> Result<f64> {
    let mut values = Vec::new();
    for class in ["VsConverter", "CsConverter"] {
        for converter in store
            .of_class(class)
            .filter(|converter| store.refv(converter, "Equipment.EquipmentContainer") == Some(unit))
        {
            let Some(value) = store.f(converter, "ACDCConverter.ratedUdc") else {
                continue;
            };
            if !value.is_finite() || value <= 0.0 {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "CGMES 2.4.15 {equipment_class} `{equipment}` has no DCConductingEquipment.ratedUdc; converter `{converter}` in DCConverterUnit `{unit}` has nonpositive or nonfinite ACDCConverter.ratedUdc `{value}`"
                    ),
                });
            }
            if !values
                .iter()
                .any(|existing: &f64| existing.to_bits() == value.to_bits())
            {
                values.push(value);
            }
        }
    }

    let value = match values.as_slice() {
        [] => {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "CGMES 2.4.15 {equipment_class} `{equipment}` has no DCConductingEquipment.ratedUdc and DCConverterUnit `{unit}` has no explicit positive ACDCConverter.ratedUdc"
                ),
            });
        }
        [value] => *value,
        _ => {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "CGMES 2.4.15 {equipment_class} `{equipment}` has no DCConductingEquipment.ratedUdc and DCConverterUnit `{unit}` has conflicting ACDCConverter.ratedUdc values: {}",
                    values
                        .iter()
                        .map(f64::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    };
    Ok(value)
}

fn cgmes_2_ground_rated_dc_voltage(
    store: &Store,
    ground: &str,
    warnings: &mut Vec<String>,
) -> Result<f64> {
    let unit = store
        .refv(ground, "Equipment.EquipmentContainer")
        .filter(|container| store.class_of(container) == Some("DCConverterUnit"))
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: format!(
                "CGMES 2.4.15 DCGround `{ground}` has no DCConductingEquipment.ratedUdc and is not contained by a DCConverterUnit"
            ),
        })?;
    let value = cgmes_2_unit_converter_rated_dc_voltage(store, "DCGround", ground, unit)?;
    warnings.push(format!(
        "CGMES 2.4.15 DCGround `{ground}` has no DCConductingEquipment.ratedUdc; derived {value} kV from the unique positive ACDCConverter.ratedUdc in DCConverterUnit `{unit}`"
    ));
    Ok(value)
}

fn cgmes_2_line_rated_dc_voltage(
    store: &Store,
    wiring: &DcTerminalWiring,
    line: &str,
    warnings: &mut Vec<String>,
) -> Result<f64> {
    let terminals = wiring.terminals(line);
    if terminals.len() != 2 {
        return Err(Error::FormatRead {
            format: FMT,
            message: format!(
                "CGMES 2.4.15 DCLineSegment `{line}` has no DCConductingEquipment.ratedUdc and does not have exactly two DC terminals"
            ),
        });
    }

    let mut endpoints = Vec::with_capacity(2);
    for terminal in terminals {
        let (node, container_property) = if let Some(node) =
            store.refv(terminal, "DCBaseTerminal.DCNode")
        {
            (node, "DCNode.DCEquipmentContainer")
        } else if let Some(node) = store.refv(terminal, "DCBaseTerminal.DCTopologicalNode") {
            (node, "DCTopologicalNode.DCEquipmentContainer")
        } else {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "CGMES 2.4.15 DCLineSegment `{line}` has no DCConductingEquipment.ratedUdc; terminal `{terminal}` has neither DCBaseTerminal.DCNode nor DCBaseTerminal.DCTopologicalNode"
                ),
            });
        };
        let unit = store
            .refv(node, container_property)
            .filter(|container| store.class_of(container) == Some("DCConverterUnit"))
            .ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: format!(
                    "CGMES 2.4.15 DCLineSegment `{line}` has no DCConductingEquipment.ratedUdc; endpoint node `{node}` is not contained by a DCConverterUnit"
                ),
            })?;
        let value = cgmes_2_unit_converter_rated_dc_voltage(store, "DCLineSegment", line, unit)?;
        endpoints.push((terminal.as_str(), node, unit, value));
    }

    let first = endpoints[0];
    let second = endpoints[1];
    if first.3.to_bits() != second.3.to_bits() {
        return Err(Error::FormatRead {
            format: FMT,
            message: format!(
                "CGMES 2.4.15 DCLineSegment `{line}` has no DCConductingEquipment.ratedUdc; endpoint DCConverterUnit `{}` has ACDCConverter.ratedUdc {} kV but endpoint DCConverterUnit `{}` has {} kV",
                first.2, first.3, second.2, second.3
            ),
        });
    }

    warnings.push(format!(
        "CGMES 2.4.15 DCLineSegment `{line}` has no DCConductingEquipment.ratedUdc; derived {} kV because terminal `{}` node `{}` belongs to DCConverterUnit `{}` and terminal `{}` node `{}` belongs to DCConverterUnit `{}`, and both units have the same unique positive ACDCConverter.ratedUdc",
        first.3, first.0, first.1, first.2, second.0, second.1, second.2
    ));
    Ok(first.3)
}

#[allow(clippy::too_many_lines)]
fn read_dc_equipment(mapper: &mut Mapper<'_>, version: CgmesVersion) -> Result<ReadDcEquipment> {
    let store = mapper.store;
    let wiring = DcTerminalWiring::build(store)?;
    let mut result = ReadDcEquipment::default();

    for id in store.of_class("DCConverterUnit") {
        result.converter_units.push(DcConverterUnit {
            component: component_id("dc_converter_unit", id)?,
            substation: store
                .refv(id, "DCConverterUnit.Substation")
                .map(|substation| component_id("substation", substation))
                .transpose()?,
            operation_mode: dc_converter_operating_mode(store, id)?,
        });
    }

    for id in store.of_class("DCTopologicalNode") {
        result.topological_nodes.push(DcTopologicalNode {
            component: component_id("dc_topological_node", id)?,
            dc_converter_unit: store
                .refv(id, "DCTopologicalNode.DCEquipmentContainer")
                .map(|unit| component_id("dc_converter_unit", unit))
                .transpose()?,
        });
    }

    for id in store.of_class("DCNode") {
        result.nodes.push(DcNode {
            component: component_id("dc_node", id)?,
            nominal_voltage_kv: None,
            dc_converter_unit: store
                .refv(id, "DCNode.DCEquipmentContainer")
                .map(|unit| component_id("dc_converter_unit", unit))
                .transpose()?,
            dc_topological_node: store
                .refv(id, "DCNode.DCTopologicalNode")
                .map(|node| component_id("dc_topological_node", node))
                .transpose()?,
            voltage_kv: None,
        });
    }

    for id in store.of_class("DCGround") {
        let Some(dc_terminal) = dc_terminal(store, &wiring, id, 0)? else {
            mapper.warnings.push(format!(
                "DCGround {}: missing DC terminal; skipped",
                store.name(id)
            ));
            continue;
        };
        let rated_dc_voltage_kv = store.f(id, "DCConductingEquipment.ratedUdc");
        let rated_dc_voltage_kv =
            if rated_dc_voltage_kv.is_none() && version == CgmesVersion::V2_4_15 {
                Some(cgmes_2_ground_rated_dc_voltage(store, id, mapper.warnings)?)
            } else {
                rated_dc_voltage_kv
            };
        result.grounds.push(DcGround {
            component: component_id("dc_ground", id)?,
            equipment_container: optional_component_reference(
                store,
                id,
                "Equipment.EquipmentContainer",
            )?,
            dc_terminal,
            rated_dc_voltage_kv,
            resistance_ohm: store.f(id, "DCGround.r"),
            inductance_h: store.f(id, "DCGround.inductance"),
        });
    }
    for id in store.of_class("DCBusbar") {
        let Some(dc_terminal) = dc_terminal(store, &wiring, id, 0)? else {
            mapper.warnings.push(format!(
                "DCBusbar {}: missing DC terminal; skipped",
                store.name(id)
            ));
            continue;
        };
        result.busbars.push(DcBusbar {
            component: component_id("dc_busbar", id)?,
            equipment_container: optional_component_reference(
                store,
                id,
                "Equipment.EquipmentContainer",
            )?,
            dc_terminal,
            rated_dc_voltage_kv: store.f(id, "DCConductingEquipment.ratedUdc"),
        });
    }
    for id in store.of_class("DCLineSegment") {
        let (Some(dc_terminal1), Some(dc_terminal2)) = (
            dc_terminal(store, &wiring, id, 0)?,
            dc_terminal(store, &wiring, id, 1)?,
        ) else {
            mapper.warnings.push(format!(
                "DCLineSegment {}: fewer than two DC terminals; skipped",
                store.name(id)
            ));
            continue;
        };
        let rated_dc_voltage_kv = store.f(id, "DCConductingEquipment.ratedUdc");
        let rated_dc_voltage_kv =
            if rated_dc_voltage_kv.is_none() && version == CgmesVersion::V2_4_15 {
                Some(cgmes_2_line_rated_dc_voltage(
                    store,
                    &wiring,
                    id,
                    mapper.warnings,
                )?)
            } else {
                rated_dc_voltage_kv
            };
        result.lines.push(DcLine {
            component: component_id("dc_line", id)?,
            equipment_container: optional_component_reference(
                store,
                id,
                "Equipment.EquipmentContainer",
            )?,
            dc_terminal1,
            dc_terminal2,
            rated_dc_voltage_kv,
            resistance_ohm: store.f(id, "DCLineSegment.resistance"),
            inductance_h: store.f(id, "DCLineSegment.inductance"),
            capacitance_f: store.f(id, "DCLineSegment.capacitance"),
            length_km: store.f(id, "DCLineSegment.length"),
        });
    }
    for id in store.of_class("DCSeriesDevice") {
        let (Some(dc_terminal1), Some(dc_terminal2)) = (
            dc_terminal(store, &wiring, id, 0)?,
            dc_terminal(store, &wiring, id, 1)?,
        ) else {
            mapper.warnings.push(format!(
                "DCSeriesDevice {}: fewer than two DC terminals; skipped",
                store.name(id)
            ));
            continue;
        };
        result.series_devices.push(DcSeriesDevice {
            component: component_id("dc_series_device", id)?,
            equipment_container: optional_component_reference(
                store,
                id,
                "Equipment.EquipmentContainer",
            )?,
            dc_terminal1,
            dc_terminal2,
            rated_dc_voltage_kv: store
                .f(id, "DCConductingEquipment.ratedUdc")
                .or_else(|| store.f(id, "DCSeriesDevice.ratedUdc")),
            resistance_ohm: store.f(id, "DCSeriesDevice.resistance"),
            inductance_h: store.f(id, "DCSeriesDevice.inductance"),
        });
    }
    for class in ["DCSwitch", "DCBreaker", "DCDisconnector"] {
        for id in store.of_class(class) {
            let (Some(first), Some(second)) = (
                dc_terminal(store, &wiring, id, 0)?,
                dc_terminal(store, &wiring, id, 1)?,
            ) else {
                mapper.warnings.push(format!(
                    "{class} {}: fewer than two DC terminals; skipped",
                    store.name(id)
                ));
                continue;
            };
            let open =
                store
                    .boolean(id, "Switch.open")
                    .or(match (first.connected, second.connected) {
                        (Some(first), Some(second)) => Some(!first || !second),
                        (Some(connected), None) | (None, Some(connected)) => Some(!connected),
                        (None, None) => None,
                    });
            result.switches.push(DcSwitch {
                component: component_id("dc_switch", id)?,
                equipment_container: optional_component_reference(
                    store,
                    id,
                    "Equipment.EquipmentContainer",
                )?,
                dc_terminal1: first,
                dc_terminal2: second,
                kind: match class {
                    "DCSwitch" => DcSwitchKind::Switch,
                    "DCBreaker" => DcSwitchKind::Breaker,
                    _ => DcSwitchKind::Disconnector,
                },
                rated_dc_voltage_kv: store.f(id, "DCConductingEquipment.ratedUdc"),
                open,
                resistance_ohm: None,
            });
        }
    }

    for id in store.of_class("VsConverter") {
        let (Some(dc_terminal1), Some(dc_terminal2)) = (
            dc_terminal(store, &wiring, id, 0)?,
            dc_terminal(store, &wiring, id, 1)?,
        ) else {
            mapper.warnings.push(format!(
                "VsConverter {}: fewer than two DC terminals; skipped",
                store.name(id)
            ));
            continue;
        };
        result
            .voltage_source_converters
            .push(VoltageSourceConverter {
                component: component_id("voltage_source_converter", id)?,
                dc_converter_unit: optional_component_reference(
                    store,
                    id,
                    "Equipment.EquipmentContainer",
                )?,
                dc_terminal1,
                dc_terminal2,
                base_apparent_power_mva: store.f(id, "ACDCConverter.baseS"),
                minimum_active_power_mw: store.f(id, "ACDCConverter.minP"),
                maximum_active_power_mw: store.f(id, "ACDCConverter.maxP"),
                minimum_dc_voltage_kv: store.f(id, "ACDCConverter.minUdc"),
                maximum_dc_voltage_kv: store.f(id, "ACDCConverter.maxUdc"),
                rated_dc_voltage_kv: store.f(id, "ACDCConverter.ratedUdc"),
                valve_u0_kv: store.f(id, "ACDCConverter.valveU0"),
                number_of_valves: optional_u32(store, id, "ACDCConverter.numberOfValves")?,
                idle_loss_mw: store.f(id, "ACDCConverter.idleLoss"),
                switching_loss_mw_per_ampere: store.f(id, "ACDCConverter.switchingLoss"),
                resistive_loss_ohm: store.f(id, "ACDCConverter.resistiveLoss"),
                control_mode: converter_control_mode(store, id, "VsConverter", mapper.warnings),
                active_power_at_pcc_mw: store.f(id, "ACDCConverter.p"),
                reactive_power_at_pcc_mvar: store.f(id, "ACDCConverter.q"),
                target_active_power_mw: store.f(id, "ACDCConverter.targetPpcc"),
                target_dc_voltage_kv: store.f(id, "ACDCConverter.targetUdc"),
                pcc_terminal: converter_pcc_terminal(store, id)?,
                droop_curve: None,
                droop: store.f(id, "VsConverter.droop"),
                droop_compensation: store.f(id, "VsConverter.droopCompensation"),
                q_share: store.f(id, "VsConverter.qShare"),
                maximum_modulation_index: store.f(id, "VsConverter.maxModulationIndex"),
                maximum_valve_current_a: store.f(id, "VsConverter.maxValveCurrent"),
                voltage_regulator_on: vsc_voltage_regulator_on(store, id, mapper.warnings),
                voltage_setpoint_kv: store.f(id, "VsConverter.targetUpcc"),
                reactive_power_setpoint_mvar: store.f(id, "VsConverter.targetQpcc"),
                reactive_limits: vsc_reactive_limits(store, id, mapper.warnings),
                pole_loss_active_power_mw: store.f(id, "ACDCConverter.poleLossP"),
                dc_current_a: store.f(id, "ACDCConverter.idc"),
                ac_voltage_kv: store.f(id, "ACDCConverter.uc"),
                dc_voltage_kv: store.f(id, "ACDCConverter.udc"),
                delta_degrees: store.f(id, "VsConverter.delta"),
                uf_kv: store.f(id, "VsConverter.uf"),
                uv_kv: store.f(id, "VsConverter.uv"),
            });
    }
    for id in store.of_class("CsConverter") {
        let (Some(dc_terminal1), Some(dc_terminal2)) = (
            dc_terminal(store, &wiring, id, 0)?,
            dc_terminal(store, &wiring, id, 1)?,
        ) else {
            mapper.warnings.push(format!(
                "CsConverter {}: fewer than two DC terminals; skipped",
                store.name(id)
            ));
            continue;
        };
        result
            .line_commutated_converters
            .push(LineCommutatedConverter {
                component: component_id("line_commutated_converter", id)?,
                dc_converter_unit: optional_component_reference(
                    store,
                    id,
                    "Equipment.EquipmentContainer",
                )?,
                dc_terminal1,
                dc_terminal2,
                base_apparent_power_mva: store.f(id, "ACDCConverter.baseS"),
                minimum_active_power_mw: store.f(id, "ACDCConverter.minP"),
                maximum_active_power_mw: store.f(id, "ACDCConverter.maxP"),
                minimum_dc_voltage_kv: store.f(id, "ACDCConverter.minUdc"),
                maximum_dc_voltage_kv: store.f(id, "ACDCConverter.maxUdc"),
                rated_dc_voltage_kv: store.f(id, "ACDCConverter.ratedUdc"),
                valve_u0_kv: store.f(id, "ACDCConverter.valveU0"),
                number_of_valves: optional_u32(store, id, "ACDCConverter.numberOfValves")?,
                idle_loss_mw: store.f(id, "ACDCConverter.idleLoss"),
                switching_loss_mw_per_ampere: store.f(id, "ACDCConverter.switchingLoss"),
                resistive_loss_ohm: store.f(id, "ACDCConverter.resistiveLoss"),
                control_mode: converter_control_mode(store, id, "CsConverter", mapper.warnings),
                active_power_at_pcc_mw: store.f(id, "ACDCConverter.p"),
                reactive_power_at_pcc_mvar: store.f(id, "ACDCConverter.q"),
                target_active_power_mw: store.f(id, "ACDCConverter.targetPpcc"),
                target_dc_voltage_kv: store.f(id, "ACDCConverter.targetUdc"),
                pcc_terminal: converter_pcc_terminal(store, id)?,
                droop_curve: None,
                reactive_model: None,
                power_factor: None,
                operating_mode: lcc_operating_mode(store, id, mapper.warnings),
                rated_dc_current_a: store.f(id, "CsConverter.ratedIdc"),
                minimum_alpha_degrees: store.f(id, "CsConverter.minAlpha"),
                maximum_alpha_degrees: store.f(id, "CsConverter.maxAlpha"),
                minimum_gamma_degrees: store.f(id, "CsConverter.minGamma"),
                maximum_gamma_degrees: store.f(id, "CsConverter.maxGamma"),
                target_alpha_degrees: store.f(id, "CsConverter.targetAlpha"),
                target_gamma_degrees: store.f(id, "CsConverter.targetGamma"),
                target_dc_current_a: store.f(id, "CsConverter.targetIdc"),
                pole_loss_active_power_mw: store.f(id, "ACDCConverter.poleLossP"),
                dc_current_a: store.f(id, "ACDCConverter.idc"),
                ac_voltage_kv: store.f(id, "ACDCConverter.uc"),
                dc_voltage_kv: store.f(id, "ACDCConverter.udc"),
                alpha_degrees: store.f(id, "CsConverter.alpha"),
                gamma_degrees: store.f(id, "CsConverter.gamma"),
            });
    }
    Ok(result)
}

fn terminal_reference(store: &Store, terminal: &str) -> Result<Option<TerminalReference>> {
    let Some(equipment) = store.refv(terminal, "Terminal.ConductingEquipment") else {
        return Ok(None);
    };
    let sequence = store
        .f(terminal, "ACDCTerminal.sequenceNumber")
        .or_else(|| store.f(terminal, "Terminal.sequenceNumber"))
        .unwrap_or(1.0);
    let terminal = u8::try_from(sequence.round() as u64).unwrap_or(u8::MAX);
    Ok(Some(TerminalReference {
        equipment: component_id(
            component_type(store.class_of(equipment).unwrap_or_default()),
            equipment,
        )?,
        terminal,
    }))
}

fn read_static_var_compensators(mapper: &mut Mapper<'_>) -> Result<Vec<StaticVarCompensator>> {
    let mut values = Vec::new();
    for id in mapper.store.of_class("StaticVarCompensator") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            mapper.warnings.push(format!(
                "StaticVarCompensator {}: no terminal on a topological node; skipped",
                mapper.store.name(id)
            ));
            continue;
        };
        let store = mapper.store;
        let inductive_rating = store
            .f(id, "StaticVarCompensator.inductiveRating")
            .unwrap_or(0.0);
        let capacitive_rating = store
            .f(id, "StaticVarCompensator.capacitiveRating")
            .unwrap_or(0.0);
        let mut svc = StaticVarCompensator::new(
            bus,
            if inductive_rating == 0.0 {
                0.0
            } else {
                1.0 / inductive_rating
            },
            if capacitive_rating == 0.0 {
                0.0
            } else {
                1.0 / capacitive_rating
            },
        );
        if inductive_rating == 0.0 || capacitive_rating == 0.0 {
            mapper.warnings.push(format!(
                "StaticVarCompensator {}: zero or missing inductive/capacitive rating has no finite susceptance; using 0 S for that bound",
                store.name(id)
            ));
        }

        let control = store.refv(id, "RegulatingCondEq.RegulatingControl");
        let mode = control
            .and_then(|value| store.enum_value(value, "RegulatingControl.mode"))
            .or_else(|| store.enum_value(id, "StaticVarCompensator.sVCControlMode"));
        svc.regulation_mode =
            if mode.is_some_and(|value| value.eq_ignore_ascii_case("reactivePower")) {
                StaticVarCompensatorRegulationMode::ReactivePower
            } else {
                StaticVarCompensatorRegulationMode::Voltage
            };
        let target = control.and_then(|value| store.f(value, "RegulatingControl.targetValue"));
        svc.voltage_setpoint_kv =
            if svc.regulation_mode == StaticVarCompensatorRegulationMode::Voltage {
                target
                    .or_else(|| store.f(id, "StaticVarCompensator.voltageSetPoint"))
                    .unwrap_or(0.0)
            } else {
                store
                    .f(id, "StaticVarCompensator.voltageSetPoint")
                    .unwrap_or(0.0)
            };
        svc.reactive_power_setpoint_mvar =
            if svc.regulation_mode == StaticVarCompensatorRegulationMode::ReactivePower {
                target.unwrap_or_else(|| store.f(id, "StaticVarCompensator.q").unwrap_or(0.0))
            } else {
                0.0
            };
        svc.regulating = store
            .boolean(id, "RegulatingCondEq.controlEnabled")
            .or_else(|| control.and_then(|value| store.boolean(value, "RegulatingControl.enabled")))
            .unwrap_or(false);
        svc.regulating_terminal = control
            .and_then(|value| store.refv(value, "RegulatingControl.Terminal"))
            .map(|terminal| terminal_reference(store, terminal))
            .transpose()?
            .flatten();
        let flow = mapper
            .wiring
            .terminals(id)
            .iter()
            .find_map(|terminal| mapper.sv_flow.get(terminal).copied());
        svc.p = store
            .f(id, "StaticVarCompensator.p")
            .or_else(|| flow.map(|value| value.0))
            .unwrap_or(0.0);
        svc.q = store
            .f(id, "StaticVarCompensator.q")
            .or_else(|| flow.map(|value| value.1))
            .unwrap_or(0.0);
        svc.in_service = mapper.in_service(id);
        svc.uid = Some(id.to_string());
        values.push(svc);
    }
    Ok(values)
}

fn limit_kind<'a>(store: &'a Store, limit_type: &str, version: CgmesVersion) -> Option<&'a str> {
    match version {
        CgmesVersion::V2_4_15 => {
            store.enum_value(limit_type, "entsoe:OperationalLimitType.limitType")
        }
        CgmesVersion::V3_0 => store.enum_value(limit_type, "eu:OperationalLimitType.kind"),
    }
}

fn loading_limits(
    store: &Store,
    set: &str,
    class: &str,
    version: CgmesVersion,
    warnings: &mut Vec<String>,
) -> Option<LoadingLimits> {
    let mut result = LoadingLimits::default();
    let mut found = false;
    for object in &store.objects {
        if object.class != class
            || store.refv(&object.id, "OperationalLimit.OperationalLimitSet") != Some(set)
        {
            continue;
        }
        let Some(limit_type) = store.refv(&object.id, "OperationalLimit.OperationalLimitType")
        else {
            warnings.push(format!(
                "{class} `{}` has no OperationalLimitType and was not retained",
                object.id
            ));
            continue;
        };
        let value = store
            .f(&object.id, &format!("{class}.normalValue"))
            .or_else(|| store.f(&object.id, &format!("{class}.value")));
        let Some(value) = value.filter(|value| value.is_finite() && *value > 0.0) else {
            warnings.push(format!(
                "{class} `{}` has a missing, nonfinite, or nonpositive value and was not retained",
                object.id
            ));
            continue;
        };
        found = true;
        let kind = limit_kind(store, limit_type, version);
        let permanent = store
            .boolean(limit_type, "OperationalLimitType.isInfiniteDuration")
            .unwrap_or_else(|| kind == Some("patl"));
        let name = store.name(&object.id);
        if permanent {
            if result.permanent_limit.is_some() {
                warnings.push(format!(
                    "OperationalLimitSet `{set}` contains several permanent {class} values; the smallest is retained"
                ));
            }
            if result.permanent_limit.is_none_or(|current| value < current) {
                result.permanent_limit = Some(value);
                result.permanent_limit_name = Some(name);
            }
        } else {
            let duration = store
                .f(limit_type, "OperationalLimitType.acceptableDuration")
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(0.0)
                .round() as u64;
            result.temporary_limits.push(TemporaryLimit {
                name,
                value,
                acceptable_duration_seconds: duration,
                fictitious: store
                    .boolean(&object.id, "IdentifiedObject.isFictitious")
                    .unwrap_or(false),
            });
        }
    }
    found.then_some(result)
}

fn read_operational_limit_groups(
    mapper: &mut Mapper<'_>,
    version: CgmesVersion,
) -> Result<Vec<OperationalLimitGroup>> {
    let store = mapper.store;
    let mut groups = Vec::new();
    let mut warnings = Vec::new();
    for set in store.of_class("OperationalLimitSet") {
        let targets: Vec<(String, u8)> =
            if let Some(terminal) = store.refv(set, "OperationalLimitSet.Terminal") {
                terminal_reference(store, terminal)?
                    .map(|value| vec![(value.equipment.local_id().to_string(), value.terminal)])
                    .unwrap_or_default()
            } else if let Some(equipment) = store.refv(set, "OperationalLimitSet.Equipment") {
                let terminals = mapper.wiring.terminals(equipment);
                if terminals.is_empty() {
                    vec![(equipment.to_string(), 1)]
                } else {
                    terminals
                        .iter()
                        .enumerate()
                        .map(|(index, _)| {
                            (
                                equipment.to_string(),
                                u8::try_from(index + 1).unwrap_or(u8::MAX),
                            )
                        })
                        .collect()
                }
            } else {
                warnings.push(format!(
                    "OperationalLimitSet `{set}` has neither a Terminal nor Equipment target"
                ));
                continue;
            };
        let current_limits = loading_limits(store, set, "CurrentLimit", version, &mut warnings);
        let active_power_limits =
            loading_limits(store, set, "ActivePowerLimit", version, &mut warnings);
        let apparent_power_limits =
            loading_limits(store, set, "ApparentPowerLimit", version, &mut warnings);
        if current_limits.is_none()
            && active_power_limits.is_none()
            && apparent_power_limits.is_none()
        {
            continue;
        }
        for (equipment, terminal) in targets {
            let class = store.class_of(&equipment).unwrap_or_default();
            groups.push(OperationalLimitGroup {
                equipment: component_id(component_type(class), &equipment)?,
                terminal,
                id: set.to_string(),
                properties: BTreeMap::new(),
                selected: false,
                current_limits: current_limits.clone(),
                active_power_limits: active_power_limits.clone(),
                apparent_power_limits: apparent_power_limits.clone(),
            });
        }
    }
    mapper.warnings.extend(warnings);
    Ok(groups)
}

fn tap_control_mode(store: &Store, tap: &str) -> Option<TapChangerRegulationMode> {
    let control = store.refv(tap, "TapChanger.TapChangerControl")?;
    match store.enum_value(control, "RegulatingControl.mode")? {
        "voltage" => Some(TapChangerRegulationMode::Voltage),
        "reactivePower" => Some(TapChangerRegulationMode::ReactivePower),
        "activePower" => Some(TapChangerRegulationMode::ActivePower),
        "currentFlow" => Some(TapChangerRegulationMode::Current),
        _ => None,
    }
}

fn table_tap_steps(store: &Store, tap: &str, kind: TapChangerKind) -> Option<Vec<TapChangerStep>> {
    let (table_property, point_class, point_table_property) = match kind {
        TapChangerKind::Ratio => (
            "RatioTapChanger.RatioTapChangerTable",
            "RatioTapChangerTablePoint",
            "RatioTapChangerTablePoint.RatioTapChangerTable",
        ),
        TapChangerKind::Phase => (
            "PhaseTapChangerTabular.PhaseTapChangerTable",
            "PhaseTapChangerTablePoint",
            "PhaseTapChangerTablePoint.PhaseTapChangerTable",
        ),
    };
    let table = store.refv(tap, table_property)?;
    let mut steps = store
        .of_class(point_class)
        .filter(|point| store.refv(point, point_table_property) == Some(table))
        .map(|point| TapChangerStep {
            position: store
                .f(point, "TapChangerTablePoint.step")
                .unwrap_or(0.0)
                .round() as i32,
            rho: store.f(point, "TapChangerTablePoint.ratio").unwrap_or(1.0),
            alpha_degrees: store
                .f(point, "PhaseTapChangerTablePoint.angle")
                .unwrap_or(0.0),
            resistance_deviation_percent: store.f(point, "TapChangerTablePoint.r").unwrap_or(0.0),
            reactance_deviation_percent: store.f(point, "TapChangerTablePoint.x").unwrap_or(0.0),
            conductance_deviation_percent: store.f(point, "TapChangerTablePoint.g").unwrap_or(0.0),
            susceptance_deviation_percent: store.f(point, "TapChangerTablePoint.b").unwrap_or(0.0),
        })
        .collect::<Vec<_>>();
    steps.sort_by_key(|step| step.position);
    (!steps.is_empty()).then_some(steps)
}

fn calculated_tap_steps(
    store: &Store,
    tap: &str,
    kind: TapChangerKind,
) -> Result<Vec<TapChangerStep>> {
    let low = store.f(tap, "TapChanger.lowStep").unwrap_or(0.0).round() as i32;
    let high = store
        .f(tap, "TapChanger.highStep")
        .unwrap_or(f64::from(low))
        .round() as i32;
    if high < low || i64::from(high) - i64::from(low) > 10_000 {
        return Err(Error::FormatRead {
            format: FMT,
            message: format!("tap changer `{tap}` has an invalid low/high step range"),
        });
    }
    let neutral = store
        .f(tap, "TapChanger.neutralStep")
        .unwrap_or(f64::from(low))
        .round() as i32;
    let class = store.class_of(tap).unwrap_or_default();
    let voltage_increment = store
        .f(tap, "RatioTapChanger.stepVoltageIncrement")
        .or_else(|| store.f(tap, "PhaseTapChangerNonLinear.voltageStepIncrement"))
        .unwrap_or(0.0);
    let phase_increment = store
        .f(tap, "PhaseTapChangerLinear.stepPhaseShiftIncrement")
        .or_else(|| store.f(tap, "PhaseTapChangerSymmetrical.stepPhaseShiftIncrement"));
    let winding_angle = store
        .f(tap, "PhaseTapChangerAsymmetrical.windingConnectionAngle")
        .unwrap_or(90.0)
        .to_radians();
    Ok((low..=high)
        .map(|position| {
            let offset = f64::from(position - neutral);
            let (rho, alpha_degrees) = match (kind, class) {
                (TapChangerKind::Ratio, _) => (1.0 + offset * voltage_increment / 100.0, 0.0),
                (TapChangerKind::Phase, "PhaseTapChangerAsymmetrical") => {
                    let increment = offset * voltage_increment / 100.0;
                    let dx = 1.0 + increment * winding_angle.cos();
                    let dy = increment * winding_angle.sin();
                    (dx.hypot(dy), dy.atan2(dx).to_degrees())
                }
                (TapChangerKind::Phase, "PhaseTapChangerSymmetrical") => {
                    let alpha = phase_increment.map_or_else(
                        || 2.0 * (offset * voltage_increment / 200.0).atan().to_degrees(),
                        |increment| offset * increment,
                    );
                    (1.0, alpha)
                }
                (TapChangerKind::Phase, _) => (1.0, offset * phase_increment.unwrap_or(0.0)),
            };
            TapChangerStep {
                position,
                rho,
                alpha_degrees,
                resistance_deviation_percent: 0.0,
                reactance_deviation_percent: 0.0,
                conductance_deviation_percent: 0.0,
                susceptance_deviation_percent: 0.0,
            }
        })
        .collect())
}

fn read_tap_changers(mapper: &mut Mapper<'_>) -> Result<Vec<TapChanger>> {
    let store = mapper.store;
    let mut result = Vec::new();
    for end in store.of_class("PowerTransformerEnd") {
        let Some(transformer) = store.refv(end, "PowerTransformerEnd.PowerTransformer") else {
            continue;
        };
        let winding = store
            .f(end, "TransformerEnd.endNumber")
            .unwrap_or(1.0)
            .round() as u8;
        let mut taps = Vec::new();
        if let Some(tap) = ratio_tap_changer(store, end) {
            taps.push((tap.to_string(), TapChangerKind::Ratio));
        }
        if let Some(tap) = phase_tap_changer(store, end) {
            taps.push((tap, TapChangerKind::Phase));
        }
        for (tap, kind) in taps {
            let low = store.f(&tap, "TapChanger.lowStep").unwrap_or(0.0).round() as i32;
            let control = store.refv(&tap, "TapChanger.TapChangerControl");
            let steps = table_tap_steps(store, &tap, kind)
                .map_or_else(|| calculated_tap_steps(store, &tap, kind), Ok)?;
            let tap_position = store
                .f(&tap, "TapChanger.step")
                .or_else(|| store.f(&tap, "TapChanger.normalStep"))
                .or_else(|| store.f(&tap, "TapChanger.neutralStep"))
                .unwrap_or(f64::from(low))
                .round() as i32;
            let solved_tap_position = sv_tap_step(store, &tap).map(|value| value.round() as i32);
            result.push(TapChanger {
                transformer: component_id("branch", transformer)?,
                winding,
                kind,
                tap_position: Some(tap_position),
                solved_tap_position,
                low_tap_position: low,
                load_tap_changing_capabilities: store
                    .boolean(&tap, "TapChanger.ltcFlag")
                    .unwrap_or(false),
                regulating: store
                    .boolean(&tap, "TapChanger.controlEnabled")
                    .or_else(|| {
                        control.and_then(|value| store.boolean(value, "RegulatingControl.enabled"))
                    })
                    .unwrap_or(false),
                regulation_mode: tap_control_mode(store, &tap),
                regulation_value: control
                    .and_then(|value| store.f(value, "RegulatingControl.targetValue")),
                target_deadband: control
                    .and_then(|value| store.f(value, "RegulatingControl.targetDeadband")),
                regulation_terminal: control
                    .and_then(|value| store.refv(value, "RegulatingControl.Terminal"))
                    .map(|terminal| terminal_reference(store, terminal))
                    .transpose()?
                    .flatten(),
                steps,
            });
        }
    }
    Ok(result)
}

fn normalized_load_coefficients(
    values: [f64; 3],
    quantity: &str,
    response: &str,
    warnings: &mut Vec<String>,
) -> Option<[f64; 3]> {
    if !values.iter().all(|value| value.is_finite()) {
        warnings.push(format!(
            "LoadResponseCharacteristic `{response}` has a nonfinite {quantity} coefficient and was not applied"
        ));
        return None;
    }
    let sum: f64 = values.iter().sum();
    if sum.abs() <= f64::EPSILON {
        warnings.push(format!(
            "LoadResponseCharacteristic `{response}` has {quantity} coefficients that sum to zero and was not applied"
        ));
        return None;
    }
    if (sum - 1.0).abs() > 1e-9 {
        warnings.push(format!(
            "LoadResponseCharacteristic `{response}` has {quantity} coefficients that sum to {sum}; they were normalized to one"
        ));
    }
    Some(values.map(|value| value / sum))
}

fn read_load_voltage_model(
    mapper: &mut Mapper<'_>,
    load: &str,
    p: f64,
    q: f64,
) -> Option<LoadVoltageModel> {
    let store = mapper.store;
    let response = store.refv(load, "EnergyConsumer.LoadResponse")?;
    if store
        .boolean(response, "LoadResponseCharacteristic.exponentModel")
        .unwrap_or(false)
    {
        let gamma_p = store
            .f(response, "LoadResponseCharacteristic.pVoltageExponent")
            .unwrap_or(0.0);
        let gamma_q = store
            .f(response, "LoadResponseCharacteristic.qVoltageExponent")
            .unwrap_or(0.0);
        return if gamma_p == 0.0 && gamma_q == 0.0 {
            Some(LoadVoltageModel::ConstantPower)
        } else {
            Some(LoadVoltageModel::Exponential {
                p,
                q,
                v_nom: None,
                gamma_p,
                gamma_q,
            })
        };
    }

    let coefficient = |name: &str| store.f(response, &format!("LoadResponseCharacteristic.{name}"));
    let p_coefficients = normalized_load_coefficients(
        [
            coefficient("pConstantPower")?,
            coefficient("pConstantCurrent")?,
            coefficient("pConstantImpedance")?,
        ],
        "active power",
        response,
        mapper.warnings,
    )?;
    let q_coefficients = normalized_load_coefficients(
        [
            coefficient("qConstantPower")?,
            coefficient("qConstantCurrent")?,
            coefficient("qConstantImpedance")?,
        ],
        "reactive power",
        response,
        mapper.warnings,
    )?;
    let is_constant_power = |coefficients: [f64; 3]| {
        (coefficients[0] - 1.0).abs() <= 1e-12
            && coefficients[1].abs() <= 1e-12
            && coefficients[2].abs() <= 1e-12
    };
    if is_constant_power(p_coefficients) && is_constant_power(q_coefficients) {
        Some(LoadVoltageModel::ConstantPower)
    } else {
        Some(LoadVoltageModel::Zip {
            p_constant_power: p * p_coefficients[0],
            q_constant_power: q * q_coefficients[0],
            p_constant_current: p * p_coefficients[1],
            q_constant_current: q * q_coefficients[1],
            p_constant_impedance: p * p_coefficients[2],
            q_constant_impedance: q * q_coefficients[2],
            v_nom: None,
            load_type: None,
            scaling: None,
        })
    }
}

fn read_loads(mapper: &mut Mapper<'_>) -> Vec<Load> {
    let mut loads = Vec::new();
    for class in LOAD_CLASSES {
        for id in mapper.store.of_class(class) {
            let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
                mapper.warnings.push(format!(
                    "{class} {}: no terminal on a topological node; skipped",
                    mapper.store.name(id)
                ));
                continue;
            };
            let (p, q) = mapper.power(id, "EnergyConsumer");
            let mut load = Load::new(bus, p, q);
            load.voltage_model = read_load_voltage_model(mapper, id, p, q);
            load.in_service = mapper.in_service(id);
            load.uid = Some(id.to_string());
            loads.push(load);
        }
    }
    loads
}

fn read_machines(mapper: &mut Mapper<'_>) -> Result<(Vec<Generator>, Option<BusId>)> {
    let mut generators = Vec::new();
    let mut best: Option<(f64, BusId)> = None;
    let mut external: Option<BusId> = None;
    let mut largest: Option<(f64, BusId)> = None;
    for id in mapper.store.of_class("SynchronousMachine") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            mapper.warnings.push(format!(
                "SynchronousMachine {}: no terminal on a topological node; skipped",
                mapper.store.name(id)
            ));
            continue;
        };
        let store = mapper.store;
        let (p, q) = mapper.power(id, "RotatingMachine");
        let mut generator = Generator::new(bus);
        generator.pg = -p;
        generator.qg = -q;
        generator.qmax = store.f(id, "SynchronousMachine.maxQ").unwrap_or(0.0);
        generator.qmin = store.f(id, "SynchronousMachine.minQ").unwrap_or(0.0);
        generator.mbase = store.f(id, "RotatingMachine.ratedS").unwrap_or(0.0);
        generator.in_service = mapper.in_service(id);
        generator.uid = Some(id.to_string());
        if let Some(unit) = store.refv(id, "RotatingMachine.GeneratingUnit") {
            generator.pmin = store.f(unit, "GeneratingUnit.minOperatingP").unwrap_or(0.0);
            generator.pmax = store.f(unit, "GeneratingUnit.maxOperatingP").unwrap_or(0.0);
            if let Some(text) = store.text(unit, "GeneratingUnit.normalPF") {
                let participation_factor =
                    text.trim().parse::<f64>().map_err(|_| Error::FormatRead {
                        format: FMT,
                        message: format!(
                            "GeneratingUnit {} has a nonnumeric normalPF `{text}`",
                            store.name(unit)
                        ),
                    })?;
                if !participation_factor.is_finite() || participation_factor < 0.0 {
                    return Err(Error::FormatRead {
                        format: FMT,
                        message: format!(
                            "GeneratingUnit {} has invalid normalPF `{text}`; expected a finite nonnegative distributed slack participation factor",
                            store.name(unit)
                        ),
                    });
                }
                let mut control = ActivePowerControl::new(true);
                control.participation_factor = Some(participation_factor);
                generator.active_power_control = Some(control);
            }
        }
        apply_regulation(mapper, id, &mut generator);
        if let Some(priority) = mapper.store.f(id, "SynchronousMachine.referencePriority") {
            if priority > 0.0 && best.is_none_or(|(p0, _)| priority < p0) {
                best = Some((priority, bus));
            }
        }
        if largest.is_none_or(|(s, _)| generator.mbase > s) {
            largest = Some((generator.mbase, bus));
        }
        generators.push(generator);
    }
    for id in mapper.store.of_class("ExternalNetworkInjection") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            continue;
        };
        let store = mapper.store;
        let (p, q) = mapper.power(id, "ExternalNetworkInjection");
        let mut generator = Generator::new(bus);
        generator.pg = -p;
        generator.qg = -q;
        generator.pmax = store.f(id, "ExternalNetworkInjection.maxP").unwrap_or(0.0);
        generator.pmin = store.f(id, "ExternalNetworkInjection.minP").unwrap_or(0.0);
        generator.qmax = store.f(id, "ExternalNetworkInjection.maxQ").unwrap_or(0.0);
        generator.qmin = store.f(id, "ExternalNetworkInjection.minQ").unwrap_or(0.0);
        generator.in_service = mapper.in_service(id);
        generator.uid = Some(id.to_string());
        apply_regulation(mapper, id, &mut generator);
        if external.is_none() {
            external = Some(bus);
        }
        generators.push(generator);
    }
    let reference = best
        .map(|(_, bus)| bus)
        .or(external)
        .or(largest.map(|(_, b)| b));
    Ok((generators, reference))
}

/// Voltage-mode `RegulatingControl` → `vg` (target over the regulated node's
/// base) and the remote regulated bus when it is not the machine's own.
fn apply_regulation(mapper: &mut Mapper<'_>, machine: &str, generator: &mut Generator) {
    let store = mapper.store;
    let Some(control) = store.refv(machine, "RegulatingCondEq.RegulatingControl") else {
        return;
    };
    if mapper.store.enum_value(control, "RegulatingControl.mode") != Some("voltage") {
        return;
    }
    let Some(target) = store.f(control, "RegulatingControl.targetValue") else {
        return;
    };
    let regulated = store
        .refv(control, "RegulatingControl.Terminal")
        .and_then(|t| mapper.wiring.node(t))
        .and_then(|tn| mapper.bus_of_tn.get(tn))
        .copied();
    if let Some(bus) = regulated {
        let kv = mapper.kv(bus);
        if target > 0.0 {
            generator.vg = target / kv;
        }
        if bus != generator.bus {
            generator.regulated_bus = Some(bus);
        }
    }
}

fn read_shunts(mapper: &mut Mapper<'_>) -> Vec<Shunt> {
    let mut shunts = read_linear_shunts(mapper);
    shunts.extend(read_nonlinear_shunts(mapper));
    shunts
}

fn read_linear_shunts(mapper: &mut Mapper<'_>) -> Vec<Shunt> {
    let mut shunts = Vec::new();
    for id in mapper.store.of_class("LinearShuntCompensator") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            continue;
        };
        let store = mapper.store;
        let kv = mapper.kv(bus);
        let sections = store
            .f(id, "ShuntCompensator.sections")
            .or_else(|| sv_sections(store, id))
            .or_else(|| store.f(id, "ShuntCompensator.normalSections"))
            .unwrap_or(1.0)
            .max(0.0)
            .round() as usize;
        let maximum_sections = store
            .f(id, "ShuntCompensator.maximumSections")
            .unwrap_or(sections.max(1) as f64)
            .max(sections as f64)
            .round() as usize;
        // Siemens × kV² = MW/MVAr injected at nominal voltage, the model's
        // 1 p.u. convention.
        let g_per_section = store
            .f(id, "LinearShuntCompensator.gPerSection")
            .unwrap_or(0.0)
            * (kv * kv);
        let b_per_section = store
            .f(id, "LinearShuntCompensator.bPerSection")
            .unwrap_or(0.0)
            * (kv * kv);
        let blocks = vec![ShuntBlock::with_admittance(
            maximum_sections.min(u32::MAX as usize) as u32,
            g_per_section,
            b_per_section,
        )];
        let mut shunt = Shunt::new(
            bus,
            g_per_section * sections as f64,
            b_per_section * sections as f64,
        );
        shunt.in_service = mapper.in_service(id);
        shunt.control = shunt_control(mapper, id, blocks, maximum_sections > 1);
        shunt.uid = Some(id.to_string());
        shunts.push(shunt);
    }
    shunts
}

fn read_nonlinear_shunts(mapper: &mut Mapper<'_>) -> Vec<Shunt> {
    let mut shunts = Vec::new();
    for id in mapper.store.of_class("NonlinearShuntCompensator") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            continue;
        };
        let store = mapper.store;
        let kv2 = mapper.kv(bus).powi(2);
        let sections = store
            .f(id, "ShuntCompensator.sections")
            .or_else(|| sv_sections(store, id))
            .or_else(|| store.f(id, "ShuntCompensator.normalSections"))
            .unwrap_or(0.0)
            .max(0.0)
            .round() as usize;
        let mut points: Vec<_> = store
            .of_class("NonlinearShuntCompensatorPoint")
            .filter(|point| {
                store.refv(
                    point,
                    "NonlinearShuntCompensatorPoint.NonlinearShuntCompensator",
                ) == Some(id)
            })
            .filter_map(|point| {
                let number = store
                    .f(point, "NonlinearShuntCompensatorPoint.sectionNumber")?
                    .round() as usize;
                (number > 0).then_some((
                    number,
                    store
                        .f(point, "NonlinearShuntCompensatorPoint.g")
                        .unwrap_or(0.0)
                        * kv2,
                    store
                        .f(point, "NonlinearShuntCompensatorPoint.b")
                        .unwrap_or(0.0)
                        * kv2,
                ))
            })
            .collect();
        points.sort_by_key(|point| point.0);
        let maximum_sections = store
            .f(id, "ShuntCompensator.maximumSections")
            .unwrap_or(points.len() as f64)
            .max(sections as f64)
            .round() as usize;
        if points.len() != maximum_sections
            || points
                .iter()
                .enumerate()
                .any(|(index, point)| point.0 != index + 1)
        {
            mapper.warnings.push(format!(
                "NonlinearShuntCompensator {id}: section point numbers do not cover 1 through {maximum_sections}"
            ));
        }
        let blocks: Vec<_> = points
            .iter()
            .map(|(_, g, b)| ShuntBlock::with_admittance(1, *g, *b))
            .collect();
        let active = sections.min(blocks.len());
        let g = blocks.iter().take(active).map(|block| block.g).sum();
        let b = blocks.iter().take(active).map(|block| block.b).sum();
        let mut shunt = Shunt::new(bus, g, b);
        shunt.in_service = mapper.in_service(id);
        shunt.control = shunt_control(mapper, id, blocks, true);
        shunt.uid = Some(id.to_string());
        shunts.push(shunt);
    }
    shunts
}

fn shunt_control(
    mapper: &Mapper<'_>,
    shunt: &str,
    blocks: Vec<ShuntBlock>,
    keep_without_regulation: bool,
) -> Option<SwitchedShuntControl> {
    let store = mapper.store;
    let regulation = store.refv(shunt, "RegulatingCondEq.RegulatingControl");
    if regulation.is_none() && !keep_without_regulation {
        return None;
    }
    let enabled = store
        .boolean(shunt, "RegulatingCondEq.controlEnabled")
        .or_else(|| {
            regulation.and_then(|control| store.boolean(control, "RegulatingControl.enabled"))
        })
        .unwrap_or(false);
    let regulated_bus = regulation
        .and_then(|control| store.refv(control, "RegulatingControl.Terminal"))
        .and_then(|terminal| mapper.wiring.node(terminal))
        .and_then(|node| mapper.bus_of_tn.get(node))
        .copied();
    let regulated_kv = regulated_bus.map_or(1.0, |bus| mapper.kv(bus));
    let target = regulation
        .and_then(|control| store.f(control, "RegulatingControl.targetValue"))
        .unwrap_or(0.0);
    let deadband = regulation
        .and_then(|control| store.f(control, "RegulatingControl.targetDeadband"))
        .unwrap_or(0.0);
    Some(SwitchedShuntControl {
        mode: if enabled {
            SwitchedShuntMode::Discrete
        } else {
            SwitchedShuntMode::Locked
        },
        vhigh: (target + deadband / 2.0) / regulated_kv,
        vlow: (target - deadband / 2.0) / regulated_kv,
        control_bus: regulated_bus,
        regulating_terminal: None,
        rmpct: 100.0,
        blocks,
    })
}

fn sv_sections(store: &Store, shunt: &str) -> Option<f64> {
    store.of_class("SvShuntCompensatorSections").find_map(|sv| {
        (store.refv(sv, "SvShuntCompensatorSections.ShuntCompensator") == Some(shunt))
            .then(|| store.f(sv, "SvShuntCompensatorSections.sections"))
            .flatten()
    })
}

fn read_switches(mapper: &mut Mapper<'_>) -> Vec<Switch> {
    let mut switches = Vec::new();
    let mut internal = 0usize;
    for class in SWITCH_CLASSES {
        for id in mapper.store.of_class(class) {
            let (Some(from), Some(to)) = (
                mapper.bus_of_equipment_terminal(id, 0),
                mapper.bus_of_equipment_terminal(id, 1),
            ) else {
                continue;
            };
            if from == to {
                // Closed inside one topological node: already collapsed by
                // the topology processor that produced TP.
                internal += 1;
                continue;
            }
            let store = mapper.store;
            let open = store
                .boolean(id, "Switch.open")
                .or_else(|| store.boolean(id, "Switch.normalOpen"))
                .unwrap_or(false);
            let mut switch = Switch::new(from, to, !open);
            switch.current_rating = store.f(id, "Switch.ratedCurrent");
            switch.uid = Some(id.to_string());
            switches.push(switch);
        }
    }
    if internal > 0 {
        mapper.warnings.push(format!(
            "{internal} switch(es) internal to one topological node are represented \
             by the topology itself"
        ));
    }
    switches
}

/// Boundary-point injections become loads: they model the neighboring
/// network's net demand at the tie node.
fn read_equivalent_injections(
    mapper: &mut Mapper<'_>,
    loads: &mut Vec<Load>,
    generators: &mut Vec<Generator>,
) {
    let mut count = 0usize;
    for id in mapper.store.of_class("EquivalentInjection") {
        let Some(bus) = mapper.bus_of_equipment_terminal(id, 0) else {
            continue;
        };
        let (p, q) = mapper.power(id, "EquivalentInjection");
        let regulation = mapper
            .store
            .boolean(id, "EquivalentInjection.regulationStatus")
            .unwrap_or(false);
        if regulation {
            let mut generator = Generator::new(bus);
            generator.pg = -p;
            generator.qg = -q;
            generator.uid = Some(id.to_string());
            generators.push(generator);
        } else {
            let mut load = Load::new(bus, p, q);
            load.in_service = mapper.in_service(id);
            load.uid = Some(id.to_string());
            loads.push(load);
        }
        count += 1;
    }
    if count > 0 {
        mapper.warnings.push(format!(
            "{count} EquivalentInjection(s) at boundary nodes mapped to \
             loads/generators (p/q at the tie point)"
        ));
    }
}

fn read_branches(
    mapper: &mut Mapper<'_>,
    version: CgmesVersion,
) -> (Vec<Branch>, Vec<Transformer3W>) {
    let mut branches = Vec::new();
    for id in mapper.store.of_class("ACLineSegment") {
        let (Some(from), Some(to)) = (
            mapper.bus_of_equipment_terminal(id, 0),
            mapper.bus_of_equipment_terminal(id, 1),
        ) else {
            mapper.warnings.push(format!(
                "ACLineSegment {}: terminals do not land on two topological \
                 nodes (boundary line without its boundary set?); skipped",
                mapper.store.name(id)
            ));
            continue;
        };
        let store = mapper.store;
        let kv = mapper.kv(from);
        let z_base = kv * kv / SYSTEM_MVA;
        let y_base = SYSTEM_MVA / (kv * kv);
        let mut branch = Branch::new(
            from,
            to,
            store.f(id, "ACLineSegment.r").unwrap_or(0.0) / z_base,
            store.f(id, "ACLineSegment.x").unwrap_or(0.0) / z_base,
        );
        branch.b = store.f(id, "ACLineSegment.bch").unwrap_or(0.0) / y_base;
        let g = store.f(id, "ACLineSegment.gch").unwrap_or(0.0) / y_base;
        if g != 0.0 {
            let half_b = branch.b / 2.0;
            branch.charging = Some(BranchCharging::new(g / 2.0, half_b, g / 2.0, half_b));
        }
        branch.in_service = mapper.in_service(id);
        branch.uid = Some(id.to_string());
        apply_limits(mapper, id, &mut branch, kv, version);
        branches.push(branch);
    }
    for id in mapper.store.of_class("SeriesCompensator") {
        let (Some(from), Some(to)) = (
            mapper.bus_of_equipment_terminal(id, 0),
            mapper.bus_of_equipment_terminal(id, 1),
        ) else {
            mapper.warnings.push(format!(
                "SeriesCompensator {}: terminals do not land on two topological nodes; skipped",
                mapper.store.name(id)
            ));
            continue;
        };
        let kv = mapper.kv(from);
        let z_base = kv * kv / SYSTEM_MVA;
        let mut branch = Branch::new(
            from,
            to,
            mapper.store.f(id, "SeriesCompensator.r").unwrap_or(0.0) / z_base,
            mapper.store.f(id, "SeriesCompensator.x").unwrap_or(0.0) / z_base,
        );
        branch.in_service = mapper.in_service(id);
        branch.uid = Some(id.to_string());
        apply_limits(mapper, id, &mut branch, kv, version);
        branches.push(branch);
    }
    let transformers_3w = read_transformers(mapper, &mut branches, version);
    (branches, transformers_3w)
}

fn read_transformers(
    mapper: &mut Mapper<'_>,
    branches: &mut Vec<Branch>,
    version: CgmesVersion,
) -> Vec<Transformer3W> {
    // Ends grouped per transformer, ordered by endNumber.
    let mut ends_of: BTreeMap<String, Vec<(f64, String)>> = BTreeMap::new();
    for end in mapper.store.of_class("PowerTransformerEnd") {
        if let Some(xf) = mapper
            .store
            .refv(end, "PowerTransformerEnd.PowerTransformer")
        {
            let number = mapper
                .store
                .f(end, "TransformerEnd.endNumber")
                .unwrap_or(1.0);
            ends_of
                .entry(xf.to_string())
                .or_default()
                .push((number, end.to_string()));
        }
    }
    let mut transformers_3w = Vec::new();
    let mut unsupported = 0usize;
    for (xf, mut ends) in ends_of {
        ends.sort_by(|a, b| a.0.total_cmp(&b.0));
        if ends.len() == 3 {
            if let Some(transformer) = read_three_winding_transformer(mapper, &xf, &ends, version) {
                transformers_3w.push(transformer);
            }
            continue;
        }
        if ends.len() != 2 {
            unsupported += 1;
            continue;
        }
        let (end1, end2) = (ends[0].1.as_str(), ends[1].1.as_str());
        if let Some(branch) = read_two_winding_transformer(mapper, &xf, end1, end2, version) {
            branches.push(branch);
        }
    }
    if unsupported > 0 {
        mapper.warnings.push(format!(
            "{unsupported} PowerTransformer record(s) have neither two nor three windings and were skipped"
        ));
    }
    transformers_3w
}

fn read_two_winding_transformer(
    mapper: &mut Mapper<'_>,
    transformer_id: &str,
    end1: &str,
    end2: &str,
    version: CgmesVersion,
) -> Option<Branch> {
    let store = mapper.store;
    let end_terminal = |end: &str| {
        store
            .refv(end, "TransformerEnd.Terminal")
            .and_then(|terminal| mapper.wiring.node(terminal))
            .and_then(|node| mapper.bus_of_tn.get(node))
            .copied()
    };
    let (Some(from), Some(to)) = (end_terminal(end1), end_terminal(end2)) else {
        mapper.warnings.push(format!(
            "PowerTransformer {}: ends do not land on topological nodes; skipped",
            store.name(transformer_id)
        ));
        return None;
    };

    let rated = |end: &str| store.f(end, "PowerTransformerEnd.ratedU").unwrap_or(0.0);
    let (u1, u2) = (rated(end1), rated(end2));
    let pu = |end: &str, key: &str, u: f64| {
        let u = if u > 0.0 { u } else { 1.0 };
        store.f(end, key).unwrap_or(0.0) / (u * u / SYSTEM_MVA)
    };
    let r = pu(end1, "PowerTransformerEnd.r", u1) + pu(end2, "PowerTransformerEnd.r", u2);
    let x = pu(end1, "PowerTransformerEnd.x", u1) + pu(end2, "PowerTransformerEnd.x", u2);
    let mut branch = Branch::new(from, to, r, x);
    let b_pu = |end: &str, u: f64| {
        let u = if u > 0.0 { u } else { 1.0 };
        store.f(end, "PowerTransformerEnd.b").unwrap_or(0.0) * (u * u / SYSTEM_MVA)
    };
    let (b1, b2) = (b_pu(end1, u1), b_pu(end2, u2));
    if b1 != 0.0 || b2 != 0.0 {
        branch.b = b1 + b2;
        branch.charging = Some(BranchCharging::new(0.0, b1, 0.0, b2));
    }

    let (kv1, kv2) = (mapper.kv(from), mapper.kv(to));
    let mut tap = safe_div(u1, kv1) / safe_div(u2, kv2);
    for (end, invert) in [(end1, false), (end2, true)] {
        if let Some(ratio_tap) = ratio_tap_changer(store, end) {
            let factor = ratio_tap_factor(mapper, ratio_tap);
            if invert {
                tap /= factor;
            } else {
                tap *= factor;
            }
        }
        if let Some(phase_tap) = phase_tap_changer(store, end) {
            branch.shift += phase_shift_deg(mapper, &phase_tap, invert);
        }
    }
    branch.tap = tap;
    let end_connected = |end: &str| {
        store
            .refv(end, "TransformerEnd.Terminal")
            .and_then(|terminal| mapper.wiring.connected.get(terminal))
            .copied()
            .unwrap_or(true)
    };
    branch.in_service =
        mapper.in_service(transformer_id) && end_connected(end1) && end_connected(end2);
    branch.uid = Some(transformer_id.to_string());
    apply_limits(mapper, transformer_id, &mut branch, kv1, version);
    for end in [end1, end2] {
        apply_limits(mapper, end, &mut branch, kv1, version);
    }
    Some(branch)
}

fn read_three_winding_transformer(
    mapper: &mut Mapper<'_>,
    transformer_id: &str,
    ends: &[(f64, String)],
    version: CgmesVersion,
) -> Option<Transformer3W> {
    let end_ids = [ends[0].1.clone(), ends[1].1.clone(), ends[2].1.clone()];
    let (terminals, buses) = transformer_end_buses(mapper, transformer_id, &end_ids)?;

    let mut windings = [
        Winding::new(buses[0]),
        Winding::new(buses[1]),
        Winding::new(buses[2]),
    ];
    let mut star_r = [0.0; 3];
    let mut star_x = [0.0; 3];
    let mut mag_g = 0.0;
    let mut mag_b = 0.0;
    for index in 0..3 {
        let end = &end_ids[index];
        let bus = buses[index];
        let rated_kv = mapper
            .store
            .f(end, "PowerTransformerEnd.ratedU")
            .unwrap_or_else(|| mapper.kv(bus));
        let rated_kv = if rated_kv > 0.0 {
            rated_kv
        } else {
            mapper.kv(bus)
        };
        let z_base = rated_kv * rated_kv / SYSTEM_MVA;
        star_r[index] = mapper.store.f(end, "PowerTransformerEnd.r").unwrap_or(0.0) / z_base;
        star_x[index] = mapper.store.f(end, "PowerTransformerEnd.x").unwrap_or(0.0) / z_base;
        mag_g += mapper.store.f(end, "PowerTransformerEnd.g").unwrap_or(0.0) * z_base;
        mag_b += mapper.store.f(end, "PowerTransformerEnd.b").unwrap_or(0.0) * z_base;

        let mut tap = safe_div(rated_kv, mapper.kv(bus));
        if let Some(ratio_tap) = ratio_tap_changer(mapper.store, end) {
            tap *= ratio_tap_factor(mapper, ratio_tap);
        }
        let shift = phase_tap_changer(mapper.store, end)
            .map_or(0.0, |phase_tap| phase_shift_deg(mapper, &phase_tap, false));

        let mut limits = Branch::new(bus, bus, 0.0, 1.0);
        apply_limits_to_targets(
            mapper.store,
            &[terminals[index].as_str(), end.as_str()],
            &mut limits,
            mapper.kv(bus),
            version,
        );
        windings[index] = Winding {
            bus,
            tap,
            shift,
            nominal_kv: rated_kv,
            rate_a: limits.rate_a,
            rate_b: limits.rate_b,
            rate_c: limits.rate_c,
        };
    }

    let impedance = |a: usize, b: usize| {
        Impedance::new(star_r[a] + star_r[b], star_x[a] + star_x[b], SYSTEM_MVA)
    };
    let connected = terminals.iter().all(|terminal| {
        mapper
            .wiring
            .connected
            .get(terminal)
            .copied()
            .unwrap_or(true)
    });
    let mut transformer = Transformer3W::new(
        windings,
        [impedance(0, 1), impedance(1, 2), impedance(2, 0)],
    );
    transformer.mag_g = mag_g;
    transformer.mag_b = mag_b;
    transformer.in_service = mapper.in_service(transformer_id) && connected;
    transformer.name = Some(mapper.store.name(transformer_id));
    transformer.uid = Some(transformer_id.to_string());
    Some(transformer)
}

fn transformer_end_buses(
    mapper: &mut Mapper<'_>,
    transformer_id: &str,
    end_ids: &[String; 3],
) -> Option<(Vec<String>, Vec<BusId>)> {
    let mut terminals = Vec::with_capacity(3);
    let mut buses = Vec::with_capacity(3);
    for end in end_ids {
        let Some(terminal) = mapper
            .store
            .refv(end, "TransformerEnd.Terminal")
            .map(str::to_string)
        else {
            mapper.warnings.push(format!(
                "PowerTransformer {}: winding {} has no terminal; skipped",
                mapper.store.name(transformer_id),
                terminals.len() + 1
            ));
            return None;
        };
        let Some(bus) = mapper
            .wiring
            .node(&terminal)
            .and_then(|node| mapper.bus_of_tn.get(node))
            .copied()
        else {
            mapper.warnings.push(format!(
                "PowerTransformer {}: winding {} does not land on a topological node; skipped",
                mapper.store.name(transformer_id),
                terminals.len() + 1
            ));
            return None;
        };
        terminals.push(terminal);
        buses.push(bus);
    }
    Some((terminals, buses))
}

fn safe_div(a: f64, b: f64) -> f64 {
    if a > 0.0 && b > 0.0 { a / b } else { 1.0 }
}

/// The in-effect ratio factor of a ratio tap changer: SSH step, else the SV
/// tap step, else neutral.
fn ratio_tap_factor(mapper: &Mapper<'_>, rtc: &str) -> f64 {
    let store = mapper.store;
    let neutral = store.f(rtc, "TapChanger.neutralStep").unwrap_or(0.0);
    let step = store
        .f(rtc, "TapChanger.step")
        .or_else(|| sv_tap_step(store, rtc))
        .unwrap_or(neutral);
    if let Some(steps) = table_tap_steps(store, rtc, TapChangerKind::Ratio)
        && let Some(value) = steps
            .iter()
            .find(|value| value.position == step.round() as i32)
    {
        return value.rho;
    }
    let increment = store
        .f(rtc, "RatioTapChanger.stepVoltageIncrement")
        .unwrap_or(0.0);
    1.0 + (step - neutral) * increment / 100.0
}

fn sv_tap_step(store: &Store, tap_changer: &str) -> Option<f64> {
    store.of_class("SvTapStep").find_map(|sv| {
        (store.refv(sv, "SvTapStep.TapChanger") == Some(tap_changer))
            .then(|| store.f(sv, "SvTapStep.position"))
            .flatten()
    })
}

fn ratio_tap_changer<'a>(store: &'a Store, end: &str) -> Option<&'a str> {
    store
        .refv(end, "TransformerEnd.RatioTapChanger")
        .or_else(|| {
            store
                .of_class("RatioTapChanger")
                .find(|tap| store.refv(tap, "RatioTapChanger.TransformerEnd") == Some(end))
        })
}

fn phase_tap_changer(store: &Store, end: &str) -> Option<String> {
    store
        .refv(end, "TransformerEnd.PhaseTapChanger")
        .or_else(|| {
            [
                "PhaseTapChangerLinear",
                "PhaseTapChangerSymmetrical",
                "PhaseTapChangerAsymmetrical",
                "PhaseTapChangerTabular",
            ]
            .into_iter()
            .find_map(|class| {
                store
                    .of_class(class)
                    .find(|tap| store.refv(tap, "PhaseTapChanger.TransformerEnd") == Some(end))
            })
        })
        .map(str::to_string)
}

/// Best-effort phase shift in degrees. The linear changer is exact; the
/// asymmetrical/symmetrical/tabular families warn and use what they can.
fn phase_shift_deg(mapper: &mut Mapper<'_>, ptc: &str, invert: bool) -> f64 {
    let store = mapper.store;
    let class = store.class_of(ptc).unwrap_or("PhaseTapChanger").to_string();
    let neutral = store.f(ptc, "TapChanger.neutralStep").unwrap_or(0.0);
    let step = store
        .f(ptc, "TapChanger.step")
        .or_else(|| sv_tap_step(store, ptc))
        .unwrap_or(neutral);
    let degrees = match class.as_str() {
        "PhaseTapChangerLinear" => {
            (step - neutral)
                * store
                    .f(ptc, "PhaseTapChangerLinear.stepPhaseShiftIncrement")
                    .unwrap_or(0.0)
        }
        "PhaseTapChangerTabular" => table_tap_steps(store, ptc, TapChangerKind::Phase)
            .and_then(|steps| {
                steps
                    .into_iter()
                    .find(|value| value.position == step.round() as i32)
            })
            .map_or(0.0, |value| value.alpha_degrees),
        "PhaseTapChangerSymmetrical" | "PhaseTapChangerAsymmetrical" => {
            calculated_tap_steps(store, ptc, TapChangerKind::Phase)
                .ok()
                .and_then(|steps| {
                    steps
                        .into_iter()
                        .find(|value| value.position == step.round() as i32)
                })
                .map_or(0.0, |value| value.alpha_degrees)
        }
        other => {
            mapper.warnings.push(format!(
                "{other} {}: phase tap changer type is not supported in the calculation projection; using zero shift",
                store.name(ptc)
            ));
            0.0
        }
    };
    if invert { -degrees } else { degrees }
}

/// PATL → `rate_a`, TATL → `rate_b`, TC → `rate_c`, from the operational
/// limit sets on the equipment's terminals (or the equipment itself).
/// Current limits convert through √3·kV; apparent/active limits are MVA/MW.
fn apply_limits(
    mapper: &mut Mapper<'_>,
    equipment: &str,
    branch: &mut Branch,
    kv: f64,
    version: CgmesVersion,
) {
    let mut terminals: Vec<&str> = mapper
        .wiring
        .terminals(equipment)
        .iter()
        .map(String::as_str)
        .collect();
    terminals.push(equipment);
    apply_limits_to_targets(mapper.store, &terminals, branch, kv, version);
}

fn apply_limits_to_targets(
    store: &Store,
    targets: &[&str],
    branch: &mut Branch,
    kv: f64,
    version: CgmesVersion,
) {
    // Limit sets point at their terminal/equipment, so scan sets once.
    for set in store.of_class("OperationalLimitSet") {
        let target = store
            .refv(set, "OperationalLimitSet.Terminal")
            .or_else(|| store.refv(set, "OperationalLimitSet.Equipment"));
        if !target.is_some_and(|target| targets.contains(&target)) {
            continue;
        }
        for limit in store.objects.iter().filter_map(|o| {
            matches!(
                o.class.as_str(),
                "CurrentLimit" | "ApparentPowerLimit" | "ActivePowerLimit"
            )
            .then_some(o.id.as_str())
        }) {
            if store.refv(limit, "OperationalLimit.OperationalLimitSet") != Some(set) {
                continue;
            }
            let Some(limit_type) = store.refv(limit, "OperationalLimit.OperationalLimitType")
            else {
                continue;
            };
            let kind = match version {
                CgmesVersion::V2_4_15 => {
                    store.enum_value(limit_type, "entsoe:OperationalLimitType.limitType")
                }
                CgmesVersion::V3_0 => store.enum_value(limit_type, "eu:OperationalLimitType.kind"),
            };
            let class = store.class_of(limit).unwrap_or_default();
            let value = store
                .f(limit, &format!("{class}.value"))
                .or_else(|| store.f(limit, &format!("{class}.normalValue")))
                .unwrap_or(0.0);
            let mva = if class == "CurrentLimit" {
                3f64.sqrt() * kv * value / 1000.0
            } else {
                value
            };
            let slot = match kind {
                Some("patl") => Some(&mut branch.rate_a),
                Some("tatl") => Some(&mut branch.rate_b),
                Some("tc" | "tct") => Some(&mut branch.rate_c),
                _ => None,
            };
            if let Some(slot) = slot {
                // Several sets can constrain one branch; the binding limit
                // is the smallest.
                if *slot == 0.0 || mva < *slot {
                    *slot = mva;
                }
            }
        }
    }
}

/// One warning per class the mapping did not consume, with its count.
fn warn_unmapped(store: &Store, warnings: &mut Vec<String>) {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for object in &store.objects {
        let class = object.class.as_str();
        let consumed = CONSUMED.contains(&class)
            || SWITCH_CLASSES.contains(&class)
            || LOAD_CLASSES.contains(&class)
            || class.contains("Limit")
            || class.contains("TapChanger")
            || class.ends_with("GeneratingUnit")
            || matches!(
                class,
                // Containment/administrative hierarchy: implied by the
                // source neutral records or used while building controls.
                "Substation"
                    | "VoltageLevel"
                    | "BusbarSection"
                    | "RegulatingControl"
                    | "TapChangerControl"
                    | "LoadResponseCharacteristic"
                    | "OperationalLimitSet"
                    | "OperationalLimitType"
                    | "NonlinearShuntCompensator"
                    | "NonlinearShuntCompensatorPoint"
            );
        if !consumed {
            *counts.entry(class).or_default() += 1;
        }
    }
    for (class, count) in counts {
        warnings.push(format!(
            "{count} {class} object(s) have no electrical or hierarchy mapping; only their identity metadata is retained"
        ));
    }
}

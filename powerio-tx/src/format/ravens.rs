//! Read and write MG-RAVENS JSON, the LANL CIM-derived interchange schema
//! (<https://github.com/lanl-ansi/MG-RAVENS>).
//!
//! RAVENS serializes a CIM100 profile (`mgravens24v1`) as one JSON document:
//! concrete classes are hash tables keyed by object name, nested along the CIM
//! class hierarchy (`PowerSystemResource` → `Equipment` → …), values are SI
//! (watts, volts, ohms, siemens, radians), and cross references are name
//! pointers of the form `Class::'name'`. The writer follows the conventions of
//! the upstream MATPOWER converter (`ravens/parsers/mpc2ravens.py`) so powerio
//! output drops into the RAVENS toolchain unchanged: `ConnectivityNode` per
//! bus with an inline `SvVoltage`, voltage-limit sets marking the slack via an
//! `OperationalLimitType` named `slack`, `SynchronousMachine` records with the
//! CIM load sign convention (injection is negative `p`/`q`), branches split
//! into `ACLineSegment` (no tap, no shift) and two-end `PowerTransformer`, and
//! the MVA base carried in an `AlgorithmSettings` record.
//!
//! The reader accepts the balanced subset those conventions produce. RAVENS is
//! natively multiconductor (its distribution documents carry per-phase objects
//! and impedance matrices); a document with multiconductor markers is rejected
//! with a pointer at the distribution surface rather than read lossily.
//!
//! The upstream schema restricts the supported vocabulary. Its MATPOWER converter
//! emits two fields absent from its UML/template: `ConnectivityNode.SvVoltage`
//! and `MySettings`. This writer retains those converter conventions; the
//! external validation gate checks them with a narrow, typed supplement to
//! the upstream schema. Element `extras` are reported as fidelity warnings,
//! never smuggled into the document.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::{TextEmission, finish, jnum};
use crate::diagnostics::codes::EMIT_RAVENS as F;
use crate::diagnostics::{Diagnostics, codes};
use crate::network::{
    BalancedNetwork, BalancedNetworkTables, Branch, Bus, BusId, BusType, GenCost, Generator, Load,
    LoadVoltageModel, Shunt, SourceFormat,
};
use crate::{Error, Result};

const FMT: &str = "MG-RAVENS JSON";
/// Schema release the writer targets; the upstream repository tags none, so
/// this mirrors the `RavensVersion` its own examples embed.
const RAVENS_VERSION: &str = "RAVENSv0.3.0-dev";
const RAVENS_VERSION_DATE: &str = "2025-03-11";
const CIM_VERSION: &str = "IEC61970CIM100";
const CIM_VERSION_DATE: &str = "2019-04-01";

/// The fixed operational-limit type names the upstream MATPOWER converter
/// keys every limit against: continuous (5e9 s) low/high, emergency (2.5e9 s)
/// low/high, and the `slack` marker type.
const TYPE_LOW_CONT: &str = "lowType_5000000000.0s";
const TYPE_HIGH_CONT: &str = "highType_5000000000.0s";
const TYPE_LOW_EMER: &str = "lowType_2500000000.0s";
const TYPE_HIGH_EMER: &str = "highType_2500000000.0s";
const TYPE_SLACK: &str = "slack";

const WATT: f64 = 1e6;
const VOLT: f64 = 1e3;

/// A deterministic mRID for an object with no imported id: the shared CIM
/// derivation under the MG-RAVENS namespace. The upstream converter writes a
/// fresh random UUID per run; a name-derived id keeps the canonical output
/// idempotent (write, parse, write is byte stable) and diffs meaningful. The
/// schema types `mRID` as a plain string with UUIDs recommended, so a
/// deterministic UUID is valid. When the element carries a `uid` (a
/// round-tripped source mRID), the caller passes it through instead.
fn det_mrid(kind: &str, name: &str) -> String {
    powerio_core::cim::deterministic_mrid(powerio_core::cim::ravens::NAMESPACE, kind, name)
}

fn pointer(class: &str, name: &str) -> Value {
    Value::String(format!("{class}::'{name}'"))
}

/// `Ravens.cimObjectType`, `IdentifiedObject.mRID`, `IdentifiedObject.name`:
/// the identity triple every emitted object starts from.
fn identified(kind: &str, name: &str, uid: Option<&str>) -> Map<String, Value> {
    let mut obj = Map::new();
    obj.insert("Ravens.cimObjectType".into(), Value::String(kind.into()));
    obj.insert(
        "IdentifiedObject.mRID".into(),
        Value::String(uid.map_or_else(|| det_mrid(kind, name), str::to_owned)),
    );
    obj.insert("IdentifiedObject.name".into(), Value::String(name.into()));
    obj
}

fn terminal(owner: &str, seq: u64, bus_key: &str) -> Value {
    let mut t = identified("Terminal", &format!("{owner}_T{seq}"), None);
    t.insert("ACDCTerminal.sequenceNumber".into(), Value::from(seq));
    t.insert(
        "Terminal.ConnectivityNode".into(),
        pointer("ConnectivityNode", bus_key),
    );
    Value::Object(t)
}

fn limit_type(name: &str, direction: &str, duration: f64) -> Value {
    let mut t = identified("OperationalLimitType", name, None);
    t.insert(
        "OperationalLimitType.direction".into(),
        Value::String(format!("OperationalLimitDirectionKind.{direction}")),
    );
    t.insert(
        "OperationalLimitType.acceptableDuration".into(),
        jnum(duration),
    );
    Value::Object(t)
}

/// One `VoltageLimit`/`ActivePowerLimit` entry of an `OperationalLimitSet`.
fn limit_value(kind: &str, name: &str, description: Option<&str>, value: f64, ty: &str) -> Value {
    let mut lim = identified(kind, name, None);
    if let Some(description) = description {
        lim.insert(
            "IdentifiedObject.description".into(),
            Value::String(description.into()),
        );
    }
    lim.insert(format!("{kind}.value"), jnum(value));
    lim.insert(
        "OperationalLimit.OperationalLimitType".into(),
        pointer("OperationalLimitType", ty),
    );
    Value::Object(lim)
}

fn limit_set(name: &str, values: Vec<Value>) -> Value {
    let mut set = identified("OperationalLimitSet", name, None);
    set.insert(
        "OperationalLimitSet.OperationalLimitValue".into(),
        Value::Array(values),
    );
    Value::Object(set)
}

/// Allocate `base<bus>` style names, suffixing `_2`, `_3`, … for the second
/// and later elements sharing a bus so the name-keyed hash tables stay unique.
fn positional_name(base: &str, bus: BusId, seen: &mut BTreeMap<BusId, usize>) -> String {
    let n = seen.entry(bus).or_insert(0);
    *n += 1;
    if *n == 1 {
        format!("{base}{bus}")
    } else {
        format!("{base}{bus}_{n}")
    }
}

/// `BaseV_<kv>` name for a nominal voltage, shared by every element at that
/// base. `kv` displays via Rust's shortest round-trip float form.
fn base_voltage_name(kv: f64) -> String {
    format!("BaseV_{kv}")
}

/// Effective conversion base: a MATPOWER case may carry `base_kv` 0 (case14),
/// which would put 0/0 into every SI conversion. 1 kV keeps the conversions
/// invertible — the writer and reader apply the same fallback, so per-unit
/// values round-trip — at the cost of non-physical SI magnitudes, warned once.
fn eff_kv(kv: f64) -> f64 {
    if kv > 0.0 { kv } else { 1.0 }
}

/// Degrees to radians such that a reparse (radians back to degrees) and
/// rewrite reproduce the same bits. The plain conversion double-rounds
/// (`x.to_radians().to_degrees().to_radians()` can drift one ULP), which
/// would break canonical-write idempotence; one cycle of the composed map
/// reaches its fixed point (the loop is a guard, observed to exit on the
/// first pass across exhaustive sampling).
#[allow(clippy::float_cmp)] // the fixed point check is bit exact by design
fn stable_radians(deg: f64) -> f64 {
    let mut rad = deg.to_radians();
    for _ in 0..4 {
        let cycled = rad.to_degrees().to_radians();
        if cycled == rad || !cycled.is_finite() {
            break;
        }
        rad = cycled;
    }
    rad
}

/// Split a trailing digit run so hash keys order numerically (`line10` after
/// `line2`); this writer and the upstream converter both number elements.
fn natural_key(name: &str) -> (String, u64, String) {
    let stem_len = name.len() - name.bytes().rev().take_while(u8::is_ascii_digit).count();
    let (stem, digits) = name.split_at(stem_len);
    (
        stem.to_owned(),
        digits.parse().unwrap_or(0),
        name.to_owned(),
    )
}

fn warn_extras(kind: &str, name: &str, extras: &crate::network::Extras, out: &mut Diagnostics) {
    if extras.is_empty() {
        return;
    }
    let keys: Vec<&str> = extras.keys().map(String::as_str).collect();
    out.push(
        &F.extras_dropped,
        format!(
            "{kind} {name}: extras [{}] have no supported MG-RAVENS representation",
            keys.join(", ")
        ),
    );
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Serialize `net` as an MG-RAVENS JSON document, reporting every model field
/// the schema (as exercised by the upstream MATPOWER converter's conventions)
/// cannot carry.
#[must_use]
pub(crate) fn write_ravens_json(net: &BalancedNetwork) -> TextEmission {
    let mut warnings = Diagnostics::new();

    let mut base_voltages = Map::new();
    for bus in net.buses() {
        let name = base_voltage_name(bus.base_kv);
        if !base_voltages.contains_key(&name) {
            let mut bv = identified("BaseVoltage", &name, None);
            bv.insert(
                "BaseVoltage.nominalVoltage".into(),
                jnum(bus.base_kv * VOLT),
            );
            base_voltages.insert(name, Value::Object(bv));
        }
    }

    let mut op_sets = Map::new();
    let mut nodes = Map::new();
    let kv_of: BTreeMap<BusId, f64> = net.buses().iter().map(|b| (b.id, b.base_kv)).collect();
    if net.buses().iter().any(|b| b.base_kv <= 0.0) {
        warnings.push(
            &F.value_substituted,
            "some buses carry base_kv 0; the SI conversions use 1 kV for them, \
             so ohm/siemens/volt magnitudes are not physical (per-unit values \
             still round-trip)",
        );
    }
    warn_bus_kind_projection(net, &mut warnings);
    for bus in net.buses() {
        write_bus(bus, &mut nodes, &mut op_sets, &mut warnings);
    }

    let mut loads = Map::new();
    let mut lrc_extra = Map::new();
    let mut load_seen = BTreeMap::new();
    for load in net.loads() {
        write_load(
            load,
            &kv_of,
            &mut loads,
            &mut lrc_extra,
            &mut load_seen,
            &mut warnings,
        );
    }

    let mut shunts = Map::new();
    let mut shunt_seen = BTreeMap::new();
    for shunt in net.shunts() {
        write_shunt(shunt, &kv_of, &mut shunts, &mut shunt_seen, &mut warnings);
    }

    let mut gens = Map::new();
    let mut costs = Map::new();
    for (i, generator) in net.generators().iter().enumerate() {
        write_generator(generator, i, &kv_of, &mut gens, &mut costs, &mut warnings);
    }

    let mut lines = Map::new();
    let mut transformers = Map::new();
    let mut ratio_taps = Map::new();
    let mut phase_taps = Map::new();
    for (i, branch) in net.branches().iter().enumerate() {
        write_branch(
            branch,
            i,
            net.base_mva(),
            &kv_of,
            &mut lines,
            &mut transformers,
            &mut ratio_taps,
            &mut phase_taps,
            &mut op_sets,
            &mut warnings,
        );
    }

    warn_unrepresented(net, &mut warnings);

    let mut root = Map::new();
    root.insert("Versions".into(), versions());
    root.insert("ConnectivityNode".into(), Value::Object(nodes));
    root.insert(
        "PowerSystemResource".into(),
        power_system_resources(
            lines,
            transformers,
            loads,
            gens,
            shunts,
            ratio_taps,
            phase_taps,
            unconnected_bus_anchors(net),
        ),
    );
    root.insert("BaseVoltage".into(), Value::Object(base_voltages));
    root.insert("OperationalLimitSet".into(), Value::Object(op_sets));
    root.insert("OperationalLimitType".into(), limit_types());
    if !costs.is_empty() {
        root.insert("ProducerCostFunction".into(), Value::Object(costs));
    }
    root.insert(
        "LoadResponseCharacteristic".into(),
        load_response_table(lrc_extra),
    );
    root.insert("MySettings".into(), settings(net.base_mva()));

    finish(&F, root, warnings)
}

fn warn_bus_kind_projection(net: &BalancedNetwork, warnings: &mut Diagnostics) {
    let regulating_buses: BTreeSet<BusId> = net
        .generators()
        .iter()
        .filter(|generator| generator.in_service && generator.voltage_regulation_on)
        .map(|generator| generator.bus)
        .collect();
    for bus in net.buses() {
        let recovered_kind = if bus.kind == BusType::Ref {
            BusType::Ref
        } else if regulating_buses.contains(&bus.id) {
            BusType::Pv
        } else {
            BusType::Pq
        };
        if recovered_kind != bus.kind {
            warnings.push(
                &F.value_substituted,
                format!(
                    "bus {}: {:?} bus kind recovers as {recovered_kind:?} from the \
                     slack marker and active voltage-regulating machines",
                    bus.id, bus.kind
                ),
            );
        }
    }
}

/// A native one-terminal connector anchors the nominal voltage of a bus
/// with no emitted electrical equipment. Without it the SI voltage state
/// cannot be converted back to per unit, even though its BaseVoltage exists.
fn unconnected_bus_anchors(net: &BalancedNetwork) -> Map<String, Value> {
    let anchored: BTreeSet<BusId> = net
        .loads()
        .iter()
        .map(|load| load.bus)
        .chain(net.shunts().iter().map(|shunt| shunt.bus))
        .chain(net.generators().iter().map(|generator| generator.bus))
        .chain(
            net.branches()
                .iter()
                .flat_map(|branch| [branch.from, branch.to]),
        )
        .collect();
    net.buses()
        .iter()
        .filter(|bus| !anchored.contains(&bus.id))
        .map(|bus| {
            let name = format!("bus{}_base", bus.id);
            let mut obj = identified("Connector", &name, None);
            obj.insert(
                "ConductingEquipment.BaseVoltage".into(),
                pointer("BaseVoltage", &base_voltage_name(bus.base_kv)),
            );
            obj.insert(
                "ConductingEquipment.Terminals".into(),
                Value::Array(vec![terminal(&name, 1, &bus.id.to_string())]),
            );
            (name, Value::Object(obj))
        })
        .collect()
}

fn versions() -> Value {
    let mut cim = Map::new();
    cim.insert(
        "Ravens.cimObjectType".into(),
        Value::String("IEC61970CIMVersion".into()),
    );
    cim.insert(
        "IEC61970CIMVersion.version".into(),
        Value::String(CIM_VERSION.into()),
    );
    cim.insert(
        "IEC61970CIMVersion.date".into(),
        Value::String(CIM_VERSION_DATE.into()),
    );
    let mut ravens = Map::new();
    ravens.insert(
        "Ravens.cimObjectType".into(),
        Value::String("RavensVersion".into()),
    );
    ravens.insert(
        "RavensVersion.version".into(),
        Value::String(RAVENS_VERSION.into()),
    );
    ravens.insert(
        "RavensVersion.date".into(),
        Value::String(RAVENS_VERSION_DATE.into()),
    );
    let mut versions = Map::new();
    versions.insert("IEC61970CIMVersion".into(), Value::Object(cim));
    versions.insert("RavensVersion".into(), Value::Object(ravens));
    Value::Object(versions)
}

/// The CIM class-hierarchy nesting the schema requires: concrete hash tables
/// sit under their abstract ancestors.
#[allow(clippy::too_many_arguments)]
fn power_system_resources(
    lines: Map<String, Value>,
    transformers: Map<String, Value>,
    loads: Map<String, Value>,
    gens: Map<String, Value>,
    shunts: Map<String, Value>,
    ratio_taps: Map<String, Value>,
    phase_taps: Map<String, Value>,
    connectors: Map<String, Value>,
) -> Value {
    let mut conductor = Map::new();
    conductor.insert("ACLineSegment".into(), Value::Object(lines));
    let mut regulating = Map::new();
    regulating.insert("RotatingMachine".into(), Value::Object(gens));
    regulating.insert("ShuntCompensator".into(), Value::Object(shunts));
    let mut energy_connection = Map::new();
    energy_connection.insert("EnergyConsumer".into(), Value::Object(loads));
    energy_connection.insert("RegulatingCondEq".into(), Value::Object(regulating));
    let mut conducting = Map::new();
    conducting.insert("Conductor".into(), Value::Object(conductor));
    conducting.insert("PowerTransformer".into(), Value::Object(transformers));
    conducting.insert("EnergyConnection".into(), Value::Object(energy_connection));
    if !connectors.is_empty() {
        conducting.insert("Connector".into(), Value::Object(connectors));
    }
    let mut equipment = Map::new();
    equipment.insert("ConductingEquipment".into(), Value::Object(conducting));
    let mut tap_changer = Map::new();
    tap_changer.insert("RatioTapChanger".into(), Value::Object(ratio_taps));
    tap_changer.insert("PhaseTapChanger".into(), Value::Object(phase_taps));
    let mut psr = Map::new();
    psr.insert("Equipment".into(), Value::Object(equipment));
    psr.insert("TapChanger".into(), Value::Object(tap_changer));
    Value::Object(psr)
}

fn limit_types() -> Value {
    let mut types = Map::new();
    // The upstream converter emits this same fixed set on every export (its
    // emergency-duration entries carry copy-pasted 5e9 display names; the
    // names here are corrected to match their own durations).
    types.insert(
        TYPE_LOW_CONT.into(),
        limit_type(TYPE_LOW_CONT, "low", 5_000_000_000.0),
    );
    types.insert(
        TYPE_HIGH_CONT.into(),
        limit_type(TYPE_HIGH_CONT, "high", 5_000_000_000.0),
    );
    types.insert(
        TYPE_LOW_EMER.into(),
        limit_type(TYPE_LOW_EMER, "low", 2_500_000_000.0),
    );
    types.insert(
        TYPE_HIGH_EMER.into(),
        limit_type(TYPE_HIGH_EMER, "high", 2_500_000_000.0),
    );
    types.insert(
        TYPE_SLACK.into(),
        limit_type(TYPE_SLACK, "low", 5_000_000_000.0),
    );
    Value::Object(types)
}

/// The three canonical ZIP anchors the upstream converter defines, plus any
/// per-load characteristics a mixed ZIP or exponential model produced.
fn load_response_table(extra: Map<String, Value>) -> Value {
    let mut table = Map::new();
    for (name, field) in [
        ("Constant kVA", "ConstantPower"),
        ("Constant I", "ConstantCurrent"),
        ("Constant Z", "ConstantImpedance"),
    ] {
        let mut lrc = identified("LoadResponseCharacteristic", name, None);
        lrc.insert(format!("LoadResponseCharacteristic.p{field}"), jnum(100.0));
        lrc.insert(format!("LoadResponseCharacteristic.q{field}"), jnum(100.0));
        table.insert(name.into(), Value::Object(lrc));
    }
    table.extend(extra);
    Value::Object(table)
}

/// `AlgorithmSettings` record carrying the system MVA base, the upstream
/// convention for the one scalar RAVENS has no CIM slot for.
fn settings(base_mva: f64) -> Value {
    let mut setting = Map::new();
    setting.insert(
        "Ravens.cimObjectType".into(),
        Value::String("GenericApplicationSetting".into()),
    );
    setting.insert("name".into(), Value::String("baseMVA".into()));
    setting.insert("value".into(), jnum(base_mva));
    setting.insert("type".into(), Value::String("float".into()));
    let mut my = identified("AlgorithmSettings", "MySettings", None);
    my.insert(
        "ApplicationSettings.Settings".into(),
        Value::Array(vec![Value::Object(setting)]),
    );
    Value::Object(my)
}

fn write_bus(
    bus: &Bus,
    nodes: &mut Map<String, Value>,
    op_sets: &mut Map<String, Value>,
    warnings: &mut Diagnostics,
) {
    let key = bus.id.to_string();
    let set_name = format!("OpLimVbus{}", bus.id);
    let volts = eff_kv(bus.base_kv) * VOLT;

    let mut limits = Vec::new();
    if bus.kind == BusType::Ref {
        limits.push(limit_value(
            "VoltageLimit",
            &format!("{set_name}_slack"),
            Some("magnitude"),
            volts,
            TYPE_SLACK,
        ));
    }
    limits.push(limit_value(
        "VoltageLimit",
        &format!("{set_name}_low"),
        Some("magnitude"),
        bus.vmin * volts,
        TYPE_LOW_CONT,
    ));
    limits.push(limit_value(
        "VoltageLimit",
        &format!("{set_name}_high"),
        Some("magnitude"),
        bus.vmax * volts,
        TYPE_HIGH_CONT,
    ));
    op_sets.insert(set_name.clone(), limit_set(&set_name, limits));

    let mut sv = Map::new();
    sv.insert(
        "Ravens.cimObjectType".into(),
        Value::String("SvVoltage".into()),
    );
    sv.insert(
        "IdentifiedObject.mRID".into(),
        Value::String(det_mrid("SvVoltage", &key)),
    );
    sv.insert("SvVoltage.v".into(), jnum(bus.vm * volts));
    sv.insert("SvVoltage.angle".into(), jnum(stable_radians(bus.va)));

    let mut node = identified("ConnectivityNode", &key, bus.uid.as_deref());
    if let Some(name) = &bus.name {
        node.insert(
            "IdentifiedObject.description".into(),
            Value::String(name.clone()),
        );
    }
    node.insert(
        "ConnectivityNode.SvVoltage".into(),
        Value::Array(vec![Value::Object(sv)]),
    );
    node.insert(
        "ConnectivityNode.OperationalLimitSet".into(),
        pointer("OperationalLimitSet", &set_name),
    );
    nodes.insert(key, Value::Object(node));

    if bus.evhi.is_some() || bus.evlo.is_some() {
        warnings.push(
            &F.field_dropped,
            format!(
                "bus {}: emergency voltage band (evhi/evlo) has no MG-RAVENS slot",
                bus.id
            ),
        );
    }
    if bus.area != 1 || bus.zone != 1 {
        warnings.push(
            &F.field_dropped,
            format!(
                "bus {}: area={} and zone={} have no balanced MG-RAVENS mapping; \
                 both recover at 1",
                bus.id, bus.area, bus.zone
            ),
        );
    }
    warn_extras("bus", &bus.id.to_string(), &bus.extras, warnings);
}

fn write_load(
    load: &Load,
    kv_of: &BTreeMap<BusId, f64>,
    loads: &mut Map<String, Value>,
    lrc_extra: &mut Map<String, Value>,
    seen: &mut BTreeMap<BusId, usize>,
    warnings: &mut Diagnostics,
) {
    let name = positional_name("load", load.bus, seen);
    let mut obj = identified("EnergyConsumer", &name, load.uid.as_deref());
    if let Some(kv) = kv_of.get(&load.bus) {
        obj.insert(
            "ConductingEquipment.BaseVoltage".into(),
            pointer("BaseVoltage", &base_voltage_name(*kv)),
        );
    }
    obj.insert("EnergyConsumer.p".into(), jnum(load.p * WATT));
    obj.insert("EnergyConsumer.q".into(), jnum(load.q * WATT));
    obj.insert("Equipment.inService".into(), Value::Bool(load.in_service));
    obj.insert(
        "EnergyConsumer.LoadResponse".into(),
        pointer(
            "LoadResponseCharacteristic",
            &load_response_name(load, &name, lrc_extra, warnings),
        ),
    );
    obj.insert(
        "ConductingEquipment.Terminals".into(),
        Value::Array(vec![terminal(&name, 1, &load.bus.to_string())]),
    );
    warn_extras("load", &name, &load.extras, warnings);
    loads.insert(name, Value::Object(obj));
}

/// Resolve a load's voltage model to a `LoadResponseCharacteristic` name,
/// adding a per-load record for shapes the three canonical anchors don't
/// cover. Percentages follow CIM semantics: each part as percent of the
/// nominal power.
fn load_response_name(
    load: &Load,
    name: &str,
    lrc_extra: &mut Map<String, Value>,
    warnings: &mut Diagnostics,
) -> String {
    match &load.voltage_model {
        None | Some(LoadVoltageModel::ConstantPower) => "Constant kVA".into(),
        Some(LoadVoltageModel::Zip {
            p_constant_power,
            q_constant_power,
            p_constant_current,
            q_constant_current,
            p_constant_impedance,
            q_constant_impedance,
            v_nom,
            load_type,
            scaling,
        }) => {
            for (component, total, parts) in [
                (
                    "active",
                    load.p,
                    [
                        *p_constant_power,
                        *p_constant_current,
                        *p_constant_impedance,
                    ],
                ),
                (
                    "reactive",
                    load.q,
                    [
                        *q_constant_power,
                        *q_constant_current,
                        *q_constant_impedance,
                    ],
                ),
            ] {
                if total == 0.0 && parts.iter().any(|part| *part != 0.0) {
                    warnings.push(
                        &F.value_collapsed,
                        format!(
                            "load {name}: nonzero {component} ZIP components sum to zero; \
                             MG-RAVENS percentages cannot represent them with zero nominal \
                             power, so the {component} voltage dependence is collapsed to zero"
                        ),
                    );
                }
            }
            if v_nom.is_some() || load_type.is_some() || scaling.is_some() {
                warnings.push(
                    &F.field_dropped,
                    format!(
                        "load {name}: ZIP v_nom/load_type/scaling metadata has no \
                     MG-RAVENS slot"
                    ),
                );
            }
            let lrc_name = format!("lrc_{name}");
            let mut lrc = identified("LoadResponseCharacteristic", &lrc_name, None);
            for (field, part, total) in [
                ("pConstantPower", *p_constant_power, load.p),
                ("qConstantPower", *q_constant_power, load.q),
                ("pConstantCurrent", *p_constant_current, load.p),
                ("qConstantCurrent", *q_constant_current, load.q),
                ("pConstantImpedance", *p_constant_impedance, load.p),
                ("qConstantImpedance", *q_constant_impedance, load.q),
            ] {
                lrc.insert(
                    format!("LoadResponseCharacteristic.{field}"),
                    jnum(zip_percentage(part, total)),
                );
            }
            lrc_extra.insert(lrc_name.clone(), Value::Object(lrc));
            lrc_name
        }
        Some(LoadVoltageModel::Exponential {
            p,
            q,
            v_nom,
            gamma_p,
            gamma_q,
        }) => {
            warn_exponential_projection(load, name, *p, *q, *v_nom, warnings);
            let lrc_name = format!("lrc_{name}");
            let mut lrc = identified("LoadResponseCharacteristic", &lrc_name, None);
            lrc.insert(
                "LoadResponseCharacteristic.exponentModel".into(),
                Value::Bool(true),
            );
            lrc.insert(
                "LoadResponseCharacteristic.pVoltageExponent".into(),
                jnum(*gamma_p),
            );
            lrc.insert(
                "LoadResponseCharacteristic.qVoltageExponent".into(),
                jnum(*gamma_q),
            );
            lrc_extra.insert(lrc_name.clone(), Value::Object(lrc));
            lrc_name
        }
    }
}

fn zip_percentage(part: f64, total: f64) -> f64 {
    if total == 0.0 {
        0.0
    } else {
        100.0 * part / total
    }
}

#[allow(clippy::float_cmp)] // Exact comparison detects any discarded coefficient assignment.
fn warn_exponential_projection(
    load: &Load,
    name: &str,
    p: f64,
    q: f64,
    v_nom: Option<f64>,
    warnings: &mut Diagnostics,
) {
    if let Some(v_nom) = v_nom {
        warnings.push(
            &F.field_dropped,
            format!(
                "load {name}: exponential v_nom={v_nom} has no MG-RAVENS \
                 load-response slot; the voltage law uses the default reference"
            ),
        );
    }
    if p != load.p || q != load.q {
        warnings.push(
            &F.value_collapsed,
            format!(
                "load {name}: exponential coefficients p={p}, q={q} differ from \
                 load power; the MG-RAVENS voltage law uses p={}, q={}",
                load.p, load.q
            ),
        );
    }
}

fn write_shunt(
    shunt: &Shunt,
    kv_of: &BTreeMap<BusId, f64>,
    shunts: &mut Map<String, Value>,
    seen: &mut BTreeMap<BusId, usize>,
    warnings: &mut Diagnostics,
) {
    let name = positional_name("shunt", shunt.bus, seen);
    let kv = kv_of.get(&shunt.bus).copied().unwrap_or(1.0);
    let eff = eff_kv(kv);
    // Model fields are MW/MVAr injected at V = 1 p.u. on the bus base; the
    // physical device admittance is that power over the base voltage squared
    // (MVAr / kV² = S), which is what `bPerSection`/`gPerSection` carry.
    let mut obj = identified("LinearShuntCompensator", &name, shunt.uid.as_deref());
    obj.insert(
        "LinearShuntCompensator.bPerSection".into(),
        jnum(shunt.b / (eff * eff)),
    );
    obj.insert(
        "LinearShuntCompensator.gPerSection".into(),
        jnum(shunt.g / (eff * eff)),
    );
    obj.insert("ShuntCompensator.normalSections".into(), Value::from(1u64));
    obj.insert("ShuntCompensator.maximumSections".into(), Value::from(1u64));
    obj.insert("ShuntCompensator.sections".into(), jnum(1.0));
    obj.insert("Equipment.inService".into(), Value::Bool(shunt.in_service));
    obj.insert(
        "ConductingEquipment.BaseVoltage".into(),
        pointer("BaseVoltage", &base_voltage_name(kv)),
    );
    obj.insert(
        "ConductingEquipment.Terminals".into(),
        Value::Array(vec![terminal(&name, 1, &shunt.bus.to_string())]),
    );
    if shunt.control.is_some() {
        warnings.push(
            &F.value_collapsed,
            format!(
                "shunt {name}: switched-shunt control blocks are written as a fixed \
             single-section compensator"
            ),
        );
    }
    if let Some(count) = shunt.section_count {
        warnings.push(
            &F.value_collapsed,
            format!(
                "shunt {name}: assigned section_count={count} is not preserved; \
                 total admittance is restated as one fixed section"
            ),
        );
    }
    warn_extras("shunt", &name, &shunt.extras, warnings);
    shunts.insert(name, Value::Object(obj));
}

fn write_generator(
    generator: &Generator,
    index: usize,
    kv_of: &BTreeMap<BusId, f64>,
    gens: &mut Map<String, Value>,
    costs: &mut Map<String, Value>,
    warnings: &mut Diagnostics,
) {
    let name = format!("gen{}", index + 1);
    let mut obj = identified("SynchronousMachine", &name, generator.uid.as_deref());
    // CIM load convention: consumption positive, so an injecting machine
    // carries negative p/q.
    obj.insert("RotatingMachine.p".into(), jnum(-generator.pg * WATT));
    obj.insert("RotatingMachine.q".into(), jnum(-generator.qg * WATT));
    obj.insert(
        "SynchronousMachine.maxQ".into(),
        jnum(generator.qmax * WATT),
    );
    obj.insert(
        "SynchronousMachine.minQ".into(),
        jnum(generator.qmin * WATT),
    );
    if generator.mbase > 0.0 {
        obj.insert(
            "RotatingMachine.ratedS".into(),
            jnum(generator.mbase * WATT),
        );
    }
    obj.insert(
        "Equipment.inService".into(),
        Value::Bool(generator.in_service),
    );
    obj.insert(
        "RegulatingCondEq.controlEnabled".into(),
        Value::Bool(generator.voltage_regulation_on),
    );
    if let Some(kv) = kv_of.get(&generator.bus) {
        obj.insert(
            "ConductingEquipment.BaseVoltage".into(),
            pointer("BaseVoltage", &base_voltage_name(*kv)),
        );
    }
    obj.insert(
        "ConductingEquipment.Terminals".into(),
        Value::Array(vec![terminal(&name, 1, &generator.bus.to_string())]),
    );

    let mut unit = identified("GeneratingUnit", &format!("{name}_GenUnit"), None);
    unit.insert(
        "GeneratingUnit.minOperatingP".into(),
        jnum(generator.pmin * WATT),
    );
    unit.insert(
        "GeneratingUnit.maxOperatingP".into(),
        jnum(generator.pmax * WATT),
    );
    obj.insert("RotatingMachine.GeneratingUnit".into(), Value::Object(unit));

    if let Some(cost) = &generator.cost {
        let cost_name = format!("cost_{name}");
        match cost_value(cost, &cost_name) {
            Ok(value) => {
                obj.insert(
                    "RotatingMachine.CostFunction".into(),
                    pointer("ProducerCostFunction", &cost_name),
                );
                costs.insert(cost_name, value);
            }
            Err(reason) => warnings.push(&F.field_dropped, format!("generator {name}: {reason}")),
        }
    }

    if (generator.vg - 1.0).abs() > 1e-12 {
        warnings.push(
            &F.field_dropped,
            format!(
                "generator {name}: voltage setpoint vg={} has no slot in the \
             MG-RAVENS machine mapping",
                generator.vg
            ),
        );
    }
    if generator.regulated_bus.is_some() {
        warnings.push(
            &F.field_dropped,
            format!("generator {name}: remote regulated bus has no MG-RAVENS slot"),
        );
    }
    if generator.has_caps() {
        warnings.push(
            &F.field_dropped,
            format!("generator {name}: capability/ramp columns have no MG-RAVENS slot"),
        );
    }
    warn_generator_metadata(generator, &name, warnings);
    gens.insert(name, Value::Object(obj));
}

fn warn_generator_metadata(generator: &Generator, name: &str, warnings: &mut Diagnostics) {
    if generator.energy_source != crate::network::GeneratorEnergySource::Other {
        warnings.push(
            &F.field_dropped,
            format!(
                "generator {name}: energy_source={:?} has no balanced MG-RAVENS \
                 mapping and recovers as Other",
                generator.energy_source
            ),
        );
    }
    if generator.regulating_terminal.is_some() {
        warnings.push(
            &F.field_dropped,
            format!(
                "generator {name}: exact regulating_terminal reference has no \
                 balanced MG-RAVENS mapping"
            ),
        );
    }
    if generator.active_power_control.is_some() {
        warnings.push(
            &F.field_dropped,
            format!(
                "generator {name}: active_power_control governor and distributed-slack \
                 settings have no balanced MG-RAVENS mapping"
            ),
        );
    }
}

/// A polynomial cost as a `ProducerCostFunction`, or the reason it can't be.
fn cost_value(cost: &GenCost, name: &str) -> std::result::Result<Value, String> {
    if cost.model != 2 {
        return Err("piecewise-linear cost (model 1) has no MG-RAVENS \
                    polynomial mapping; cost dropped"
            .into());
    }
    if cost.ncost > 3 || cost.coeffs.len() > 3 {
        return Err(format!(
            "polynomial cost of degree {} exceeds the quadratic \
             ProducerCostFunction mapping; cost dropped",
            cost.coeffs.len().max(cost.ncost).saturating_sub(1)
        ));
    }
    let mut params = Vec::new();
    // Model coefficients are highest order first; RAVENS rows are (power,
    // coefficient) pairs, emitted constant term first like the upstream
    // converter.
    for (i, coeff) in cost.coeffs.iter().rev().enumerate() {
        let mut param = Map::new();
        param.insert(
            "PolynomialCostParameter.power".into(),
            Value::from(i as u64),
        );
        param.insert(
            "PolynomialCostParameter.costCoefficient".into(),
            jnum(*coeff),
        );
        params.push(Value::Object(param));
    }
    let mut obj = identified("ProducerCostFunction", name, None);
    obj.insert(
        "ProducerCostFunction.CostParameters".into(),
        Value::Array(params),
    );
    obj.insert(
        "ProducerCostFunction.startupCost".into(),
        jnum(cost.startup),
    );
    obj.insert(
        "ProducerCostFunction.shutdownCost".into(),
        jnum(cost.shutdown),
    );
    Ok(Value::Object(obj))
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)] // one branch row fans out into limits, a line or a two-end transformer, and taps
fn write_branch(
    branch: &Branch,
    index: usize,
    base_mva: f64,
    kv_of: &BTreeMap<BusId, f64>,
    lines: &mut Map<String, Value>,
    transformers: &mut Map<String, Value>,
    ratio_taps: &mut Map<String, Value>,
    phase_taps: &mut Map<String, Value>,
    op_sets: &mut Map<String, Value>,
    warnings: &mut Diagnostics,
) {
    let n = index + 1;
    // Impedances leave per unit through the from-side voltage base, the
    // upstream converter's convention (z_base = kV² / MVA_base = Ω).
    let raw_kv_from = kv_of.get(&branch.from).copied().unwrap_or(1.0);
    let raw_kv_to = kv_of.get(&branch.to).copied().unwrap_or(1.0);
    let kv = eff_kv(raw_kv_from);
    let z_base = kv * kv / base_mva;
    let y_base = base_mva / (kv * kv);

    let set_name = format!("OpLimbranch{n}");
    let limits = vec![
        limit_value(
            "ActivePowerLimit",
            &format!("{set_name}_rate_a"),
            None,
            branch.rate_a * WATT,
            TYPE_HIGH_CONT,
        ),
        limit_value(
            "ActivePowerLimit",
            &format!("{set_name}_rate_b"),
            None,
            branch.rate_b * WATT,
            TYPE_HIGH_EMER,
        ),
        limit_value(
            "VoltageLimit",
            &format!("{set_name}_angles_low"),
            Some("angle"),
            stable_radians(branch.angmin),
            TYPE_LOW_CONT,
        ),
        limit_value(
            "VoltageLimit",
            &format!("{set_name}_angles_high"),
            Some("angle"),
            stable_radians(branch.angmax),
            TYPE_HIGH_CONT,
        ),
    ];
    op_sets.insert(set_name.clone(), limit_set(&set_name, limits));

    let charging = branch.calc_terminal_charging();
    if !charging.is_matpower_symmetric() {
        warnings.push(
            &F.value_collapsed,
            format!(
                "branch {n} ({}-{}): asymmetric terminal charging folded into the \
             total bch projection",
                branch.from, branch.to
            ),
        );
    }
    if branch.rate_c != 0.0 {
        warnings.push(
            &F.field_dropped,
            format!(
                "branch {n} ({}-{}): rate_c has no MG-RAVENS limit-type slot \
             (rate_a and rate_b map)",
                branch.from, branch.to
            ),
        );
    }
    if branch.control.is_some() {
        warnings.push(
            &F.field_dropped,
            format!(
                "branch {n} ({}-{}): automatic tap/phase control data has no \
             MG-RAVENS slot; the fixed ratio is written",
                branch.from, branch.to
            ),
        );
    }
    if !branch.rating_sets.is_empty() || branch.current_ratings.is_some() {
        warnings.push(
            &F.rating_set_dropped,
            format!(
                "branch {n} ({}-{}): extra rating sets / current ratings have no \
             MG-RAVENS slot",
                branch.from, branch.to
            ),
        );
    }
    if let Some(name) = &branch.name {
        warnings.push(
            &F.field_dropped,
            format!(
                "branch {n} ({}-{}): source equipment name `{name}` is not preserved \
                 by the balanced MG-RAVENS positional naming convention",
                branch.from, branch.to
            ),
        );
    }
    if branch.solution.is_some() {
        warnings.push(
            &F.field_dropped,
            format!(
                "branch {n} ({}-{}): assigned solution flows (pf/qf/pt/qt) have no \
                 balanced MG-RAVENS mapping",
                branch.from, branch.to
            ),
        );
    }
    warn_extras(
        "branch",
        &format!("{n} ({}-{})", branch.from, branch.to),
        &branch.extras,
        warnings,
    );

    if !branch.is_transformer() {
        let name = format!("line{n}");
        let mut obj = identified("ACLineSegment", &name, branch.uid.as_deref());
        obj.insert("Equipment.inService".into(), Value::Bool(branch.in_service));
        // Anchors the base voltage of buses whose only equipment is lines
        // (the upstream converter leaves lines unanchored and relies on its
        // load-per-PQ-bus habit instead).
        obj.insert(
            "ConductingEquipment.BaseVoltage".into(),
            pointer("BaseVoltage", &base_voltage_name(raw_kv_from)),
        );
        obj.insert("ACLineSegment.r".into(), jnum(branch.r * z_base));
        obj.insert("ACLineSegment.x".into(), jnum(branch.x * z_base));
        obj.insert(
            "ACLineSegment.bch".into(),
            jnum(branch.calc_total_charging_b() * y_base),
        );
        obj.insert(
            "Equipment.OperationalLimitSet".into(),
            pointer("OperationalLimitSet", &set_name),
        );
        obj.insert(
            "ConductingEquipment.Terminals".into(),
            Value::Array(vec![
                terminal(&name, 1, &branch.from.to_string()),
                terminal(&name, 2, &branch.to.to_string()),
            ]),
        );
        lines.insert(name, Value::Object(obj));
        return;
    }

    let name = format!("transformer{n}");
    let tap_name = format!("tap{n}");
    let shift_name = format!("shift{n}");

    let mut end1 = identified("PowerTransformerEnd", &format!("{name}_end1"), None);
    end1.insert("TransformerEnd.endNumber".into(), Value::from(1u64));
    end1.insert(
        "TransformerEnd.BaseVoltage".into(),
        pointer("BaseVoltage", &base_voltage_name(raw_kv_from)),
    );
    end1.insert("PowerTransformerEnd.r".into(), jnum(0.0));
    end1.insert("PowerTransformerEnd.x".into(), jnum(0.0));
    end1.insert("PowerTransformerEnd.b".into(), jnum(0.0));
    end1.insert(
        "TransformerEnd.RatioTapChanger".into(),
        pointer("RatioTapChanger", &tap_name),
    );
    end1.insert(
        "TransformerEnd.PhaseTapChanger".into(),
        pointer("PhaseTapChanger", &shift_name),
    );
    end1.insert(
        "ConductingEquipment.Terminals".into(),
        Value::Array(vec![terminal(&name, 1, &branch.from.to_string())]),
    );

    let mut end2 = identified("PowerTransformerEnd", &format!("{name}_end2"), None);
    end2.insert("TransformerEnd.endNumber".into(), Value::from(2u64));
    end2.insert(
        "TransformerEnd.BaseVoltage".into(),
        pointer("BaseVoltage", &base_voltage_name(raw_kv_to)),
    );
    end2.insert("PowerTransformerEnd.r".into(), jnum(branch.r * z_base));
    end2.insert("PowerTransformerEnd.x".into(), jnum(branch.x * z_base));
    end2.insert(
        "PowerTransformerEnd.b".into(),
        jnum(branch.calc_total_charging_b() * y_base),
    );
    end2.insert(
        "ConductingEquipment.Terminals".into(),
        Value::Array(vec![terminal(&name, 2, &branch.to.to_string())]),
    );

    let mut obj = identified("PowerTransformer", &name, branch.uid.as_deref());
    obj.insert("Equipment.inService".into(), Value::Bool(branch.in_service));
    obj.insert(
        "Equipment.OperationalLimitSet".into(),
        pointer("OperationalLimitSet", &set_name),
    );
    obj.insert(
        "PowerTransformer.PowerTransformerEnd".into(),
        Value::Array(vec![Value::Object(end1), Value::Object(end2)]),
    );
    transformers.insert(name.clone(), Value::Object(obj));

    // `effective_tap` writes a shift-only transformer as ratio 1 rather than
    // the raw 0 the branch row may carry.
    let mut ratio = identified("TapChangerRatio", &format!("{tap_name}_ratio"), None);
    ratio.insert(
        "TapChangerRatio.ptRatio".into(),
        jnum(branch.calc_effective_tap()),
    );
    let mut tap = identified("RatioTapChanger", &tap_name, None);
    tap.insert("TapChanger.TapChangerRatio".into(), Value::Object(ratio));
    ratio_taps.insert(tap_name, Value::Object(tap));

    let mut angle = identified("TapChangerRatio", &format!("{shift_name}_angle"), None);
    angle.insert(
        "TapChangerRatio.ptRatio".into(),
        jnum(stable_radians(branch.shift)),
    );
    let mut shift = identified("PhaseTapChanger", &shift_name, None);
    shift.insert("TapChanger.TapChangerRatio".into(), Value::Object(angle));
    phase_taps.insert(shift_name, Value::Object(shift));
}

/// Element families with no MG-RAVENS mapping in the balanced profile, each
/// reported once with its count.
fn warn_unrepresented(net: &BalancedNetwork, warnings: &mut Diagnostics) {
    let families: [(&str, usize); 7] = [
        ("switch", net.switches().len()),
        ("storage unit", net.storage().len()),
        ("HVDC line", net.hvdc().len()),
        ("three-winding transformer", net.transformers_3w().len()),
        (
            "static VAR compensator",
            net.static_var_compensators().len(),
        ),
        (
            "area record",
            net.areas()
                .iter()
                .filter(|a| {
                    a.slack_bus.is_some()
                        || a.net_interchange != 0.0
                        || a.tolerance != 0.0
                        || a.name.is_some()
                })
                .count(),
        ),
        (
            "solver-parameter block",
            usize::from(net.solver().is_some()),
        ),
    ];
    for (what, count) in families {
        if count > 0 {
            warnings.push(
                &F.record_dropped,
                format!(
                    "{count} {what}(s) have no MG-RAVENS balanced-profile mapping \
                 and are dropped"
                ),
            );
        }
    }
    if net.geo().is_some() || net.buses().iter().any(|b| b.location.is_some()) {
        warnings.push(
            &F.field_dropped,
            "geographic metadata is not written (a CIM Location mapping is \
             future distribution-profile work)",
        );
    }
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Every recorded object: its concrete `Ravens.cimObjectType`, the hash key it
/// is filed under (the pointer-resolution name), and its properties.
#[derive(Debug)]
struct Obj {
    kind: String,
    name: String,
    props: Map<String, Value>,
}

/// Name-keyed object index, walked out of the class-hierarchy nesting so the
/// reader does not depend on the exact abstract-class levels a producer chose.
struct Doc {
    objects: Vec<Obj>,
}

impl Doc {
    fn collect(root: &Map<String, Value>) -> Doc {
        let mut objects = Vec::new();
        for (key, value) in root {
            walk(Some(key), value, &mut objects);
        }
        Doc { objects }
    }

    fn of_kind(&self, kind: &str) -> impl Iterator<Item = &Obj> {
        self.objects.iter().filter(move |o| o.kind == kind)
    }

    /// Objects of one concrete kind in natural name order. Hash tables carry
    /// no order, and BTreeMap iteration is lexicographic (`line10` before
    /// `line2`); the numbering conventions make natural order the source
    /// element order.
    fn sorted_of_kind(&self, kind: &str) -> Vec<&Obj> {
        let mut objects: Vec<&Obj> = self.of_kind(kind).collect();
        objects.sort_by_key(|o| natural_key(&o.name));
        objects
    }

    /// Resolve a `Class::'name'` pointer to the named object. RAVENS pointers
    /// name the target's hash key; the class in the pointer may be an
    /// ancestor of the object's concrete type, so matching is by name with
    /// the class as a tiebreaker.
    fn resolve(&self, pointer: &Value) -> Option<&Obj> {
        let text = pointer.as_str()?;
        let (class, rest) = text.split_once("::'")?;
        let name = rest.strip_suffix('\'')?;
        self.objects
            .iter()
            .find(|o| o.name == name && o.kind == class)
            .or_else(|| self.objects.iter().find(|o| o.name == name))
    }
}

fn walk(name: Option<&str>, value: &Value, out: &mut Vec<Obj>) {
    match value {
        Value::Object(map) => {
            if let Some(kind) = map.get("Ravens.cimObjectType").and_then(Value::as_str) {
                let fallback = map
                    .get("IdentifiedObject.name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                out.push(Obj {
                    kind: kind.to_owned(),
                    name: name.unwrap_or(fallback).to_owned(),
                    props: map.clone(),
                });
            }
            for (key, child) in map {
                walk(Some(key), child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(None, item, out);
            }
        }
        _ => {}
    }
}

fn fprop(obj: &Obj, key: &str) -> Option<f64> {
    obj.props.get(key).and_then(Value::as_f64)
}

fn f_or(obj: &Obj, key: &str, default: f64) -> f64 {
    fprop(obj, key).unwrap_or(default)
}

fn in_service(obj: &Obj) -> bool {
    obj.props
        .get("Equipment.inService")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn mrid(obj: &Obj) -> Option<String> {
    obj.props
        .get("IdentifiedObject.mRID")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The connectivity nodes an equipment's inline `Terminals` point at, in
/// sequence-number order.
fn terminals(obj: &Obj) -> Vec<String> {
    let Some(Value::Array(items)) = obj.props.get("ConductingEquipment.Terminals") else {
        return Vec::new();
    };
    let mut with_seq: Vec<(u64, String)> = items
        .iter()
        .filter_map(|t| {
            let t = t.as_object()?;
            let seq = t
                .get("ACDCTerminal.sequenceNumber")
                .and_then(Value::as_u64)
                .unwrap_or(1);
            let cn = t.get("Terminal.ConnectivityNode").and_then(Value::as_str)?;
            let (_, rest) = cn.split_once("::'")?;
            Some((seq, rest.strip_suffix('\'')?.to_owned()))
        })
        .collect();
    with_seq.sort_by_key(|(seq, _)| *seq);
    with_seq.into_iter().map(|(_, cn)| cn).collect()
}

/// Transformer-end terminals live on the ends, not the transformer.
fn end_terminal(end: &Map<String, Value>) -> Option<String> {
    let items = end.get("ConductingEquipment.Terminals")?.as_array()?;
    let t = items.first()?.as_object()?;
    let cn = t.get("Terminal.ConnectivityNode")?.as_str()?;
    let (_, rest) = cn.split_once("::'")?;
    Some(rest.strip_suffix('\'')?.to_owned())
}

#[allow(clippy::too_many_lines)] // the element families read in source order, one block each
pub(crate) fn parse_ravens_source(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let root_value: Value = serde_json::from_str(source).map_err(|e| Error::FormatRead {
        format: FMT,
        message: e.to_string(),
    })?;
    let Value::Object(root) = &root_value else {
        return Err(Error::FormatRead {
            format: FMT,
            message: "top level is not an object".into(),
        });
    };

    let doc = Doc::collect(root);
    reject_multiconductor(&doc, root)?;

    let base_mva = base_mva(&doc).unwrap_or_else(|| {
        warnings.push(
            &codes::READ_RAVENS_VALUE_DEFAULTED,
            "no AlgorithmSettings baseMVA record; the per-unit conversion \
             assumes 100 MVA",
        );
        100.0
    });

    let (buses, bus_ids, kv_of) = read_buses(&doc, root, warnings)?;
    let bus_id = |name: &str, what: &str, warnings: &mut Diagnostics| -> Option<BusId> {
        let id = bus_ids.get(name).copied();
        if id.is_none() {
            warnings.push(
                &codes::READ_RAVENS_REFERENCE_DROPPED,
                format!(
                    "{what}: terminal points at unknown ConnectivityNode `{name}`; \
                 element skipped"
                ),
            );
        }
        id
    };

    let mut loads = Vec::new();
    for obj in doc.sorted_of_kind("EnergyConsumer") {
        let Some(bus) = terminals(obj)
            .first()
            .and_then(|cn| bus_id(cn, &format!("load {}", obj.name), warnings))
        else {
            continue;
        };
        let mut load = Load::new(
            bus,
            f_or(obj, "EnergyConsumer.p", 0.0) / WATT,
            f_or(obj, "EnergyConsumer.q", 0.0) / WATT,
        );
        load.in_service = in_service(obj);
        load.uid = mrid(obj);
        load.voltage_model = load_voltage_model(&doc, obj, &load);
        loads.push(load);
    }

    let mut shunts = Vec::new();
    for obj in doc.sorted_of_kind("LinearShuntCompensator") {
        let Some(bus) = terminals(obj)
            .first()
            .and_then(|cn| bus_id(cn, &format!("shunt {}", obj.name), warnings))
        else {
            continue;
        };
        let eff = eff_kv(kv_of.get(&bus).copied().unwrap_or(1.0));
        let sections = f_or(
            obj,
            "ShuntCompensator.sections",
            f_or(obj, "ShuntCompensator.normalSections", 1.0),
        );
        // `(eff * eff)` groups as the writer's divisor so the multiply mirrors
        // the divide bit exactly (sections is 1.0 on written documents).
        let mut shunt = Shunt::new(
            bus,
            f_or(obj, "LinearShuntCompensator.gPerSection", 0.0) * sections * (eff * eff),
            f_or(obj, "LinearShuntCompensator.bPerSection", 0.0) * sections * (eff * eff),
        );
        shunt.in_service = in_service(obj);
        shunt.uid = mrid(obj);
        shunts.push(shunt);
    }

    let mut generators = Vec::new();
    let mut ref_buses: Vec<BusId> = Vec::new();
    let mut machines: Vec<&Obj> = doc
        .objects
        .iter()
        .filter(|o| o.kind == "SynchronousMachine" || o.kind == "AsynchronousMachine")
        .collect();
    machines.sort_by_key(|o| natural_key(&o.name));
    for obj in machines {
        let Some(bus) = terminals(obj)
            .first()
            .and_then(|cn| bus_id(cn, &format!("generator {}", obj.name), warnings))
        else {
            continue;
        };
        let mut generator = Generator::new(bus);
        generator.pg = -f_or(obj, "RotatingMachine.p", 0.0) / WATT;
        generator.qg = -f_or(obj, "RotatingMachine.q", 0.0) / WATT;
        generator.qmax = f_or(obj, "SynchronousMachine.maxQ", 0.0) / WATT;
        generator.qmin = f_or(obj, "SynchronousMachine.minQ", 0.0) / WATT;
        generator.mbase = f_or(obj, "RotatingMachine.ratedS", 0.0) / WATT;
        generator.in_service = in_service(obj);
        generator.voltage_regulation_on = obj
            .props
            .get("RegulatingCondEq.controlEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        generator.uid = mrid(obj);
        if let Some(Value::Object(unit)) = obj.props.get("RotatingMachine.GeneratingUnit") {
            generator.pmin = unit
                .get("GeneratingUnit.minOperatingP")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                / WATT;
            generator.pmax = unit
                .get("GeneratingUnit.maxOperatingP")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                / WATT;
        }
        if let Some(cost) = obj
            .props
            .get("RotatingMachine.CostFunction")
            .and_then(|p| doc.resolve(p))
        {
            match read_cost(cost) {
                Ok(value) => generator.cost = Some(value),
                Err(reason) => warnings.push(
                    &codes::READ_RAVENS_RECORD_DROPPED,
                    format!(
                        "generator {}: cost {} dropped: {reason}",
                        obj.name, cost.name
                    ),
                ),
            }
        }
        generators.push(generator);
    }
    for obj in doc.sorted_of_kind("EnergySource") {
        let Some(bus) = terminals(obj)
            .first()
            .and_then(|cn| bus_id(cn, &format!("source {}", obj.name), warnings))
        else {
            continue;
        };
        let mut generator = Generator::new(bus);
        generator.in_service = in_service(obj);
        if let Some(kv) = kv_of.get(&bus)
            && let Some(v) = fprop(obj, "EnergySource.voltageMagnitude")
        {
            generator.vg = v / (eff_kv(*kv) * VOLT);
        }
        generator.uid = mrid(obj);
        warnings.push(
            &codes::READ_RAVENS_VALUE_COLLAPSED,
            format!(
                "source {}: EnergySource mapped to a generator, slack when in service \
                 (no P/Q bounds in the source record)",
                obj.name
            ),
        );
        if generator.in_service {
            ref_buses.push(bus);
        }
        generators.push(generator);
    }

    let mut branches = Vec::new();
    // Lines and transformers share one numbering space (`line3` and
    // `transformer4` are source branch rows 3 and 4), so the merged
    // number-first sort recovers the source branch order.
    let mut branch_objs: Vec<&Obj> = doc
        .objects
        .iter()
        .filter(|o| o.kind == "ACLineSegment" || o.kind == "PowerTransformer")
        .collect();
    branch_objs.sort_by_key(|o| {
        let (stem, number, full) = natural_key(&o.name);
        (number, stem, full)
    });
    for obj in branch_objs {
        if obj.kind == "PowerTransformer" {
            if let Some(branch) = read_transformer(&doc, obj, base_mva, &kv_of, &bus_ids, warnings)
            {
                branches.push(branch);
            }
            continue;
        }
        let ends = terminals(obj);
        let (Some(from), Some(to)) = (
            ends.first()
                .and_then(|cn| bus_id(cn, &format!("line {}", obj.name), warnings)),
            ends.get(1)
                .and_then(|cn| bus_id(cn, &format!("line {}", obj.name), warnings)),
        ) else {
            continue;
        };
        let kv = eff_kv(kv_of.get(&from).copied().unwrap_or(1.0));
        let z_base = kv * kv / base_mva;
        // Divide by the writer's own multiplier (not the reciprocal
        // constant): same-constant mul→div round trips bit exactly, mixed
        // constants double-round and break canonical idempotence.
        let y_base = base_mva / (kv * kv);
        let mut branch = Branch::new(
            from,
            to,
            f_or(obj, "ACLineSegment.r", 0.0) / z_base,
            f_or(obj, "ACLineSegment.x", 0.0) / z_base,
        );
        branch.b = f_or(obj, "ACLineSegment.bch", 0.0) / y_base;
        branch.in_service = in_service(obj);
        branch.uid = mrid(obj);
        read_branch_limits(&doc, obj, &mut branch);
        branches.push(branch);
    }

    let mut buses = buses;
    apply_bus_kinds(&mut buses, &generators, &ref_buses);
    warn_unread_kinds(&doc, warnings);

    let name = name_hint.unwrap_or("case").to_string();
    let net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        geo: None,
        case_metadata: crate::network::CaseMetadata::default(),
        detailed_connectivity: None,
        generated_uids: std::sync::Arc::default(),
        buses: buses.into(),
        loads: loads.into(),
        shunts: shunts.into(),
        static_var_compensators: Vec::new().into(),
        branches: branches.into(),
        switches: Vec::new().into(),
        generators: generators.into(),
        storage: Vec::new().into(),
        hvdc: Vec::new().into(),
        transformers_3w: Vec::new().into(),
        areas: Vec::new().into(),
        solver: None,
        source_format: SourceFormat::RavensJson,
    });
    net.check_references(FMT)?;
    Ok(net)
}

/// A document carrying per-phase or wire-geometry objects is a multiconductor
/// distribution case; reading it as balanced would silently discard the
/// phase detail, so refuse with a pointer at the right surface.
fn reject_multiconductor(doc: &Doc, root: &Map<String, Value>) -> Result<()> {
    let found: Vec<&str> = powerio_core::cim::ravens::MULTICONDUCTOR_MARKERS
        .iter()
        .filter(|t| doc.objects.iter().any(|o| &o.kind == *t) || root.contains_key(**t))
        .copied()
        .collect();
    if found.is_empty() {
        return Ok(());
    }
    Err(Error::FormatRead {
        format: FMT,
        message: format!(
            "document carries multiconductor objects ({}); the balanced \
             transmission reader would drop the per-phase detail. The \
             distribution surface (powerio-dist, format `ravens-json` on a \
             MulticonductorNetwork) reads it; `powerio::parse` routes it \
             there by content",
            found.join(", ")
        ),
    })
}

fn base_mva(doc: &Doc) -> Option<f64> {
    for obj in doc.of_kind("AlgorithmSettings") {
        if let Some(Value::Array(settings)) = obj.props.get("ApplicationSettings.Settings") {
            for setting in settings {
                let Some(setting) = setting.as_object() else {
                    continue;
                };
                if setting.get("name").and_then(Value::as_str) == Some("baseMVA") {
                    return setting.get("value").and_then(Value::as_f64);
                }
            }
        }
    }
    None
}

type BusTables = (Vec<Bus>, BTreeMap<String, BusId>, BTreeMap<BusId, f64>);

/// Buses from the `ConnectivityNode` hash: ids parse from the node names when
/// every name is an integer (the upstream convention), else are assigned
/// positionally; base kV comes from the `BaseVoltage` pointers of attached
/// equipment; vm/va from the inline `SvVoltage`; the voltage band and slack
/// marker from the node's `OperationalLimitSet`.
#[allow(clippy::too_many_lines)] // id assignment, kV voting, and per-node state in one ordered pass
fn read_buses(
    doc: &Doc,
    root: &Map<String, Value>,
    warnings: &mut Diagnostics,
) -> Result<BusTables> {
    let Some(Value::Object(node_table)) = root.get("ConnectivityNode") else {
        return Err(Error::FormatRead {
            format: FMT,
            message: "document has no top-level ConnectivityNode table".into(),
        });
    };
    let mut names: Vec<&String> = node_table.keys().collect();
    let numeric: Option<Vec<usize>> = names.iter().map(|n| n.parse::<usize>().ok()).collect();
    let ids: BTreeMap<String, BusId> = if let Some(numbers) = numeric {
        let mut pairs: Vec<(usize, &String)> =
            numbers.into_iter().zip(names.iter().copied()).collect();
        pairs.sort_unstable();
        pairs
            .into_iter()
            .map(|(id, name)| (name.clone(), BusId::new(id)))
            .collect()
    } else {
        warnings.push(
            &codes::READ_RAVENS_VALUE_SUBSTITUTED,
            "ConnectivityNode names are not all integers; bus ids are \
             assigned positionally and the names kept as bus names",
        );
        names.sort_unstable();
        names
            .iter()
            .enumerate()
            .map(|(i, name)| ((*name).clone(), BusId::new(i + 1)))
            .collect()
    };

    // Base kV votes: every piece of equipment that carries both a BaseVoltage
    // pointer and a terminal ties its nominal voltage to that node. Loads,
    // machines, shunts, and lines carry `ConductingEquipment.BaseVoltage`;
    // transformer ends carry `TransformerEnd.BaseVoltage` (an end changes
    // base, so only its own terminal is tied).
    let mut kv_votes: BTreeMap<BusId, f64> = BTreeMap::new();
    for obj in &doc.objects {
        let Some(bv) = obj
            .props
            .get("ConductingEquipment.BaseVoltage")
            .or_else(|| obj.props.get("TransformerEnd.BaseVoltage"))
            .and_then(|p| doc.resolve(p))
        else {
            continue;
        };
        let Some(kv) = fprop(bv, "BaseVoltage.nominalVoltage").map(|v| v / VOLT) else {
            continue;
        };
        for cn in terminals(obj) {
            if let Some(&id) = ids.get(&cn) {
                let vote = kv_votes.entry(id).or_insert(kv);
                if (*vote - kv).abs() > 1e-9 {
                    warnings.push(
                        &codes::READ_RAVENS_VALUE_SUBSTITUTED,
                        format!(
                            "bus {id}: conflicting BaseVoltage values ({vote} kV vs \
                         {kv} kV); keeping the first"
                        ),
                    );
                }
            }
        }
    }

    let mut buses = Vec::new();
    let mut kv_of = BTreeMap::new();
    let mut degenerate_kv = false;
    // The id map is string-keyed (lexicographic: "10" before "2"); emit buses
    // in id order.
    let mut ordered: Vec<(&String, BusId)> = ids.iter().map(|(name, &id)| (name, id)).collect();
    ordered.sort_by_key(|&(_, id)| id);
    for (name, id) in ordered {
        let node = node_table
            .get(name)
            .and_then(Value::as_object)
            .ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: format!("ConnectivityNode `{name}` is not an object"),
            })?;
        let kv = kv_votes.get(&id).copied().unwrap_or_else(|| {
            warnings.push(
                &codes::READ_RAVENS_VALUE_DEFAULTED,
                format!(
                    "bus {id}: no attached equipment names a BaseVoltage; base kV \
                 defaults to 1"
                ),
            );
            1.0
        });
        degenerate_kv |= kv <= 0.0;
        kv_of.insert(id, kv);
        let mut bus = Bus::new(id, BusType::Pq, kv);
        if name != &id.to_string() {
            bus.name = Some(name.clone());
        } else if let Some(description) = node
            .get("IdentifiedObject.description")
            .and_then(Value::as_str)
        {
            bus.name = Some(description.to_owned());
        }
        bus.uid = node
            .get("IdentifiedObject.mRID")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let volts = eff_kv(kv) * VOLT;
        if let Some(sv) = node
            .get("ConnectivityNode.SvVoltage")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_object)
        {
            if let Some(v) = sv.get("SvVoltage.v").and_then(Value::as_f64) {
                bus.vm = v / volts;
            }
            if let Some(angle) = sv.get("SvVoltage.angle").and_then(Value::as_f64) {
                bus.va = angle.to_degrees();
            }
        }
        if let Some(set) = node
            .get("ConnectivityNode.OperationalLimitSet")
            .and_then(|p| doc.resolve(p))
        {
            for limit in limit_values(set) {
                let Some(value) = limit.get("VoltageLimit.value").and_then(Value::as_f64) else {
                    continue;
                };
                match limit_type_name(&limit).as_deref() {
                    Some(TYPE_SLACK) => bus.kind = BusType::Ref,
                    Some(name) if name.starts_with("lowType") => bus.vmin = value / volts,
                    Some(name) if name.starts_with("highType") => bus.vmax = value / volts,
                    _ => {}
                }
            }
        }
        buses.push(bus);
    }
    if degenerate_kv {
        warnings.push(
            &codes::READ_RAVENS_VALUE_SUBSTITUTED,
            "some buses carry base_kv 0; the SI conversions use 1 kV for them \
             (mirroring the writer), so per-unit values round-trip",
        );
    }
    Ok((buses, ids, kv_of))
}

fn limit_values(set: &Obj) -> Vec<Map<String, Value>> {
    set.props
        .get("OperationalLimitSet.OperationalLimitValue")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_object).cloned().collect())
        .unwrap_or_default()
}

fn limit_type_name(limit: &Map<String, Value>) -> Option<String> {
    let text = limit
        .get("OperationalLimit.OperationalLimitType")?
        .as_str()?;
    let (_, rest) = text.split_once("::'")?;
    Some(rest.strip_suffix('\'')?.to_owned())
}

fn read_branch_limits(doc: &Doc, obj: &Obj, branch: &mut Branch) {
    let Some(set) = obj
        .props
        .get("Equipment.OperationalLimitSet")
        .and_then(|p| doc.resolve(p))
    else {
        return;
    };
    for limit in limit_values(set) {
        let kind = limit
            .get("Ravens.cimObjectType")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let ty = limit_type_name(&limit).unwrap_or_default();
        if kind == "ActivePowerLimit" {
            let Some(value) = limit.get("ActivePowerLimit.value").and_then(Value::as_f64) else {
                continue;
            };
            match ty.as_str() {
                TYPE_HIGH_CONT => branch.rate_a = value / WATT,
                TYPE_HIGH_EMER => branch.rate_b = value / WATT,
                _ => {}
            }
        } else if kind == "VoltageLimit"
            && limit
                .get("IdentifiedObject.description")
                .and_then(Value::as_str)
                == Some("angle")
        {
            let Some(value) = limit.get("VoltageLimit.value").and_then(Value::as_f64) else {
                continue;
            };
            if ty.starts_with("lowType") {
                branch.angmin = value.to_degrees();
            } else if ty.starts_with("highType") {
                branch.angmax = value.to_degrees();
            }
        }
    }
}

fn read_transformer(
    doc: &Doc,
    obj: &Obj,
    base_mva: f64,
    kv_of: &BTreeMap<BusId, f64>,
    bus_ids: &BTreeMap<String, BusId>,
    warnings: &mut Diagnostics,
) -> Option<Branch> {
    let Some(Value::Array(ends)) = obj.props.get("PowerTransformer.PowerTransformerEnd") else {
        warnings.push(
            &codes::READ_RAVENS_RECORD_DROPPED,
            format!(
                "transformer {}: no PowerTransformerEnd records; skipped",
                obj.name
            ),
        );
        return None;
    };
    let mut ends: Vec<&Map<String, Value>> = ends.iter().filter_map(Value::as_object).collect();
    if ends.len() != 2 {
        warnings.push(
            &codes::READ_RAVENS_RECORD_DROPPED,
            format!(
                "transformer {}: {} ends (only two-winding transformers map); \
             skipped",
                obj.name,
                ends.len()
            ),
        );
        return None;
    }
    ends.sort_by_key(|e| {
        e.get("TransformerEnd.endNumber")
            .and_then(Value::as_u64)
            .unwrap_or(1)
    });
    let cn_from = end_terminal(ends[0])?;
    let cn_to = end_terminal(ends[1])?;
    let (Some(&from), Some(&to)) = (bus_ids.get(&cn_from), bus_ids.get(&cn_to)) else {
        warnings.push(
            &codes::READ_RAVENS_REFERENCE_DROPPED,
            format!(
                "transformer {}: terminal points at an unknown ConnectivityNode; \
             skipped",
                obj.name
            ),
        );
        return None;
    };

    let kv = eff_kv(kv_of.get(&from).copied().unwrap_or(1.0));
    let z_base = kv * kv / base_mva;
    let end_f = |end: &Map<String, Value>, key: &str| -> f64 {
        end.get(key).and_then(Value::as_f64).unwrap_or(0.0)
    };
    let r = (end_f(ends[0], "PowerTransformerEnd.r") + end_f(ends[1], "PowerTransformerEnd.r"))
        / z_base;
    let x = (end_f(ends[0], "PowerTransformerEnd.x") + end_f(ends[1], "PowerTransformerEnd.x"))
        / z_base;
    let mut branch = Branch::new(from, to, r, x);
    // See the line reader: divide by the writer's multiplier for bit-exact
    // round trips.
    let y_base = base_mva / (kv * kv);
    branch.b = (end_f(ends[0], "PowerTransformerEnd.b") + end_f(ends[1], "PowerTransformerEnd.b"))
        / y_base;
    branch.in_service = in_service(obj);
    branch.uid = mrid(obj);
    branch.tap = 1.0;

    for end in &ends {
        if let Some(tap) = end
            .get("TransformerEnd.RatioTapChanger")
            .and_then(|p| doc.resolve(p))
            && let Some(ratio) = tap_ratio(tap)
        {
            branch.tap = ratio;
        }
        if let Some(shift) = end
            .get("TransformerEnd.PhaseTapChanger")
            .and_then(|p| doc.resolve(p))
            && let Some(angle) = tap_ratio(shift)
        {
            branch.shift = angle.to_degrees();
        }
    }
    read_branch_limits(doc, obj, &mut branch);
    Some(branch)
}

/// The `TapChanger.TapChangerRatio.ptRatio` payload of a RAVENS tap-changer
/// record (the upstream converter stores the MATPOWER ratio or the phase
/// shift in radians there).
fn tap_ratio(obj: &Obj) -> Option<f64> {
    obj.props
        .get("TapChanger.TapChangerRatio")?
        .as_object()?
        .get("TapChangerRatio.ptRatio")?
        .as_f64()
}

fn read_cost(obj: &Obj) -> std::result::Result<GenCost, &'static str> {
    let params = obj
        .props
        .get("ProducerCostFunction.CostParameters")
        .and_then(Value::as_array)
        .filter(|params| !params.is_empty())
        .ok_or("no cost parameter array")?;
    let mut by_power = BTreeMap::new();
    for param in params {
        let power = param
            .get("PolynomialCostParameter.power")
            .and_then(Value::as_u64)
            .filter(|power| *power <= 2)
            .ok_or("only polynomial powers 0, 1 and 2 have a balanced-profile mapping")?;
        let coefficient = param
            .get("PolynomialCostParameter.costCoefficient")
            .and_then(Value::as_f64)
            .ok_or("polynomial cost coefficient is missing or nonnumeric")?;
        if by_power.insert(power, coefficient).is_some() {
            return Err("duplicate polynomial power");
        }
    }
    let degree = *by_power
        .last_key_value()
        .expect("nonempty cost parameters")
        .0;
    let coeffs = (0..=degree)
        .rev()
        .map(|power| by_power.get(&power).copied().unwrap_or(0.0))
        .collect();
    Ok(GenCost::new(
        2,
        f_or(obj, "ProducerCostFunction.startupCost", 0.0),
        f_or(obj, "ProducerCostFunction.shutdownCost", 0.0),
        coeffs,
    ))
}

#[allow(clippy::float_cmp)] // 100.0 is the canonical anchors' literal, not a computed value
fn load_voltage_model(doc: &Doc, obj: &Obj, load: &Load) -> Option<LoadVoltageModel> {
    let response = obj
        .props
        .get("EnergyConsumer.LoadResponse")
        .and_then(|p| doc.resolve(p))?;
    let part = |field: &str| {
        f_or(
            response,
            &format!("LoadResponseCharacteristic.{field}"),
            0.0,
        )
    };
    if response
        .props
        .get("LoadResponseCharacteristic.exponentModel")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Some(LoadVoltageModel::Exponential {
            p: load.p,
            q: load.q,
            v_nom: None,
            gamma_p: part("pVoltageExponent"),
            gamma_q: part("qVoltageExponent"),
        });
    }
    let zip = LoadVoltageModel::Zip {
        p_constant_power: load.p * part("pConstantPower") / 100.0,
        q_constant_power: load.q * part("qConstantPower") / 100.0,
        p_constant_current: load.p * part("pConstantCurrent") / 100.0,
        q_constant_current: load.q * part("qConstantCurrent") / 100.0,
        p_constant_impedance: load.p * part("pConstantImpedance") / 100.0,
        q_constant_impedance: load.q * part("qConstantImpedance") / 100.0,
        v_nom: None,
        load_type: None,
        scaling: None,
    };
    // The canonical "Constant kVA" anchor is the model's constant-power
    // default; don't materialize a ZIP record for it.
    if part("pConstantPower") == 100.0
        && part("qConstantPower") == 100.0
        && part("pConstantCurrent") == 0.0
        && part("pConstantImpedance") == 0.0
        && part("qConstantCurrent") == 0.0
        && part("qConstantImpedance") == 0.0
    {
        return None;
    }
    Some(zip)
}

/// PQ/PV/REF assignment: online voltage-regulating machines make their buses
/// PV; slack markers and online `EnergySource` records make their buses REF.
fn apply_bus_kinds(buses: &mut [Bus], generators: &[Generator], ref_buses: &[BusId]) {
    let regulating_buses: BTreeSet<BusId> = generators
        .iter()
        .filter(|generator| generator.in_service && generator.voltage_regulation_on)
        .map(|generator| generator.bus)
        .collect();
    for bus in buses.iter_mut() {
        if ref_buses.contains(&bus.id) {
            bus.kind = BusType::Ref;
        } else if bus.kind == BusType::Ref {
            // Marked by the slack limit type in read_buses; keep it.
        } else if regulating_buses.contains(&bus.id) {
            bus.kind = BusType::Pv;
        }
    }
    // Generator voltage setpoints remain the documented 1 p.u. default.
}

/// Object kinds the balanced reader consumed nothing from, one warning per
/// kind with a count, so a document leaning on unmodeled CIM classes is loud.
fn warn_unread_kinds(doc: &Doc, warnings: &mut Diagnostics) {
    const READ: [&str; 15] = [
        "ConnectivityNode",
        "SvVoltage",
        "Terminal",
        "BaseVoltage",
        "OperationalLimitSet",
        "OperationalLimitType",
        "VoltageLimit",
        "ActivePowerLimit",
        "EnergyConsumer",
        "LinearShuntCompensator",
        "SynchronousMachine",
        "AsynchronousMachine",
        "EnergySource",
        "ACLineSegment",
        "Connector",
    ];
    const READ_STRUCTURAL: [&str; 10] = [
        "PowerTransformer",
        "PowerTransformerEnd",
        "RatioTapChanger",
        "PhaseTapChanger",
        "TapChangerRatio",
        "GeneratingUnit",
        "ProducerCostFunction",
        "LoadResponseCharacteristic",
        "AlgorithmSettings",
        "GenericApplicationSetting",
    ];
    const VERSION_KINDS: [&str; 2] = ["IEC61970CIMVersion", "RavensVersion"];
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for obj in &doc.objects {
        if !READ.contains(&obj.kind.as_str())
            && !READ_STRUCTURAL.contains(&obj.kind.as_str())
            && !VERSION_KINDS.contains(&obj.kind.as_str())
        {
            *counts.entry(obj.kind.as_str()).or_default() += 1;
        }
    }
    for (kind, count) in counts {
        warnings.push(
            &codes::READ_RAVENS_RETAINED_SOURCE_ONLY,
            format!(
                "{count} {kind} object(s) have no balanced-model mapping and stay \
             only in the retained source"
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::test_parse::parse_file;
    use std::path::PathBuf;

    fn case(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data")
            .join(name)
    }

    /// The reader on in-memory text: the network and its rendered findings.
    fn parse_ravens_json(content: &str) -> Result<(BalancedNetwork, Vec<String>)> {
        let mut warnings = Diagnostics::new();
        let network = parse_ravens_source(content, None, &mut warnings)?;
        Ok((network, warnings.lines()))
    }

    fn network(name: &str) -> BalancedNetwork {
        parse_file(case(name), None).unwrap().network
    }

    fn two_bus_network() -> BalancedNetwork {
        let mut net = BalancedNetwork::in_memory(
            "two-bus",
            100.0,
            vec![
                Bus::new(BusId::new(1), BusType::Ref, 20.0),
                Bus::new(BusId::new(2), BusType::Pq, 20.0),
            ],
            vec![Branch::new(BusId::new(1), BusId::new(2), 0.01, 0.1)],
        );
        net.generators_mut().push(Generator::new(BusId::new(1)));
        net.loads_mut().push(Load::new(BusId::new(2), 10.0, 3.0));
        net
    }

    #[test]
    fn only_online_voltage_regulating_machines_promote_pq_buses() {
        let mut net = two_bus_network();
        let mut generator = Generator::new(BusId::new(2));
        generator.voltage_regulation_on = false;
        net.generators_mut().push(generator);
        for (in_service, regulating) in [(true, false), (false, true)] {
            net.generators_mut()[1].in_service = in_service;
            net.generators_mut()[1].voltage_regulation_on = regulating;
            let emitted = write_ravens_json(&net);
            assert!(emitted.diagnostics.is_empty(), "{:?}", emitted.diagnostics);
            let (back, findings) = parse_ravens_json(&emitted.text).unwrap();
            assert!(findings.is_empty(), "{findings:?}");
            assert_eq!(back.buses()[1].kind, BusType::Pq);
            assert_eq!(back.generators()[1].in_service, in_service);
            assert_eq!(back.generators()[1].voltage_regulation_on, regulating);
        }
    }

    #[test]
    fn an_inactive_energy_source_is_not_reenabled_or_made_a_slack() {
        let net = two_bus_network();
        let mut doc: Value = serde_json::from_str(&write_ravens_json(&net).text).unwrap();
        doc["PowerSystemResource"]["Equipment"]["ConductingEquipment"]["EnergyConnection"]["EnergySource"] = serde_json::json!({
            "source2": {
                "Ravens.cimObjectType": "EnergySource",
                "Equipment.inService": false,
                "ConductingEquipment.BaseVoltage": "BaseVoltage::'BaseV_20'",
                "ConductingEquipment.Terminals": [terminal("source2", 1, "2")],
                "EnergySource.voltageMagnitude": 20_000.0
            }
        });
        let (back, _) = parse_ravens_json(&doc.to_string()).unwrap();
        assert_eq!(back.generators().len(), 2);
        assert!(!back.generators()[1].in_service);
        assert_eq!(back.buses()[1].kind, BusType::Pq);
    }

    #[test]
    fn native_connector_preserves_an_unequipped_bus_voltage_base() {
        let mut net = two_bus_network();
        let mut bus = Bus::new(BusId::new(3), BusType::Pq, 35.0);
        bus.vm = 1.02;
        net.buses_mut().push(bus);
        let emitted = write_ravens_json(&net);
        assert!(emitted.diagnostics.is_empty(), "{:?}", emitted.diagnostics);
        let (back, findings) = parse_ravens_json(&emitted.text).unwrap();
        assert!(findings.is_empty(), "{findings:?}");
        assert_projection(&net, &back);
        let restored = &back.buses()[2];
        assert!((restored.base_kv - 35.0).abs() < 1e-12);
        assert!((restored.vm - 1.02).abs() < 1e-12);
        assert!((restored.vmin - 0.9).abs() < 1e-12);
        assert!((restored.vmax - 1.1).abs() < 1e-12);
        assert_same_text(&emitted.text, &write_ravens_json(&back).text);
    }

    #[test]
    fn sparse_polynomial_cost_preserves_explicit_powers() {
        let mut net = two_bus_network();
        net.generators_mut()[0].cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.11, 0.0, 150.0]));
        let mut doc: Value = serde_json::from_str(&write_ravens_json(&net).text).unwrap();
        doc["ProducerCostFunction"]["cost_gen1"]["ProducerCostFunction.CostParameters"]
            .as_array_mut()
            .unwrap()
            .retain(|term| term["PolynomialCostParameter.power"] != 1);
        let (back, findings) = parse_ravens_json(&doc.to_string()).unwrap();
        assert!(findings.is_empty(), "{findings:?}");
        let cost = back.generators()[0].cost.as_ref().unwrap();
        assert_eq!(cost.coeffs, [0.11, 0.0, 150.0]);
        assert_eq!(cost.ncost, 3);

        doc["ProducerCostFunction"]["cost_gen1"]["ProducerCostFunction.CostParameters"][0]["PolynomialCostParameter.power"] =
            Value::from(u64::MAX);
        let (back, findings) = parse_ravens_json(&doc.to_string()).unwrap();
        assert!(back.generators()[0].cost.is_none());
        assert!(
            findings.iter().any(|finding| {
                finding.contains("READ.RAVENS.RECORD_DROPPED")
                    && finding.contains("polynomial powers")
            }),
            "{findings:?}"
        );
    }

    #[test]
    fn unrepresented_static_var_compensators_are_diagnosed() {
        let mut net = two_bus_network();
        net.static_var_compensators_mut()
            .push(crate::network::StaticVarCompensator::new(
                BusId::new(2),
                -0.002,
                0.002,
            ));
        let emitted = write_ravens_json(&net);
        let findings = emitted.render_diagnostics();
        assert!(
            findings.iter().any(|finding| {
                finding.contains("EMIT.RAVENS.RECORD_DROPPED")
                    && finding.contains("1 static VAR compensator")
            }),
            "{findings:?}"
        );
    }

    #[test]
    fn dropped_typed_metadata_is_named_while_source_mrids_are_preserved() {
        use crate::network::{
            ActivePowerControl, BranchSolution, GeneratorEnergySource, TerminalReference,
        };

        let mut net = two_bus_network();
        // GridFM's zero area/zone assignments also recover as the RAVENS
        // profile's one-based defaults and need a loss finding.
        net.buses_mut()[1].area = 0;
        net.buses_mut()[1].zone = 0;
        let branch = &mut net.branches_mut()[0];
        branch.uid = Some("source-line".into());
        branch.name = Some("North line".into());
        branch.solution = Some(BranchSolution::new(10.0, 3.0, -9.9, -2.9));
        let generator = &mut net.generators_mut()[0];
        generator.uid = Some("source-generator".into());
        generator.energy_source = GeneratorEnergySource::Wind;
        generator.active_power_control = Some(ActivePowerControl::new(true));
        generator.regulating_terminal = Some(TerminalReference {
            equipment: powerio_core::ComponentId::new("generator", "source-generator").unwrap(),
            terminal: 1,
        });
        let mut shunt = Shunt::new(BusId::new(2), 0.0, 12.0);
        shunt.uid = Some("source-shunt".into());
        shunt.section_count = Some(3);
        net.shunts_mut().push(shunt);

        let emitted = write_ravens_json(&net);
        let findings = emitted.render_diagnostics();
        for field in [
            "area=0 and zone=0",
            "source equipment name `North line`",
            "solution flows (pf/qf/pt/qt)",
            "energy_source=Wind",
            "active_power_control",
            "regulating_terminal",
            "section_count=3",
        ] {
            assert!(
                findings.iter().any(|finding| finding.contains(field)),
                "missing {field}: {findings:?}"
            );
        }
        assert!(findings.iter().all(|finding| !finding.contains("identity")));
        let (back, _) = parse_ravens_json(&emitted.text).unwrap();
        assert_eq!((back.buses()[1].area, back.buses()[1].zone), (1, 1));
        assert_eq!(back.branches()[0].uid, net.branches()[0].uid);
        assert!(back.branches()[0].name.is_none());
        assert!(back.branches()[0].solution.is_none());
        assert_eq!(back.generators()[0].uid, net.generators()[0].uid);
        assert_eq!(
            back.generators()[0].energy_source,
            GeneratorEnergySource::Other
        );
        assert!(back.generators()[0].active_power_control.is_none());
        assert!(back.generators()[0].regulating_terminal.is_none());
        assert_eq!(back.shunts()[0].uid, net.shunts()[0].uid);
        assert!(back.shunts()[0].section_count.is_none());
        assert!((back.shunts()[0].b - 12.0).abs() < 1e-12);
    }

    #[test]
    fn zero_net_zip_components_have_a_specific_finite_projection_warning() {
        let mut net = two_bus_network();
        let load = &mut net.loads_mut()[0];
        load.p = 0.0;
        load.voltage_model = Some(LoadVoltageModel::Zip {
            p_constant_power: 10.0,
            q_constant_power: load.q,
            p_constant_current: -10.0,
            q_constant_current: 0.0,
            p_constant_impedance: 0.0,
            q_constant_impedance: 0.0,
            v_nom: None,
            load_type: None,
            scaling: None,
        });
        let emitted = write_ravens_json(&net);
        let findings = emitted.render_diagnostics();
        assert!(
            findings.iter().any(|finding| {
                finding.contains("EMIT.RAVENS.VALUE_COLLAPSED")
                    && finding.contains("nonzero active ZIP components sum to zero")
            }),
            "{findings:?}"
        );
        let (back, _) = parse_ravens_json(&emitted.text).unwrap();
        assert!(back.loads()[0].p.abs() < 1e-12);
        assert!((back.loads()[0].q - 3.0).abs() < 1e-12);
    }

    #[test]
    fn cancelling_reactive_zip_parts_are_not_mistaken_for_constant_power() {
        let mut net = two_bus_network();
        net.loads_mut()[0].voltage_model = Some(LoadVoltageModel::Zip {
            p_constant_power: 10.0,
            q_constant_power: 3.0,
            p_constant_current: 0.0,
            q_constant_current: -0.3,
            p_constant_impedance: 0.0,
            q_constant_impedance: 0.3,
            v_nom: None,
            load_type: None,
            scaling: None,
        });
        let emitted = write_ravens_json(&net);
        assert!(emitted.diagnostics.is_empty(), "{:?}", emitted.diagnostics);
        let (back, findings) = parse_ravens_json(&emitted.text).unwrap();
        assert!(findings.is_empty(), "{findings:?}");
        let Some(LoadVoltageModel::Zip {
            q_constant_current,
            q_constant_impedance,
            ..
        }) = &back.loads()[0].voltage_model
        else {
            panic!("reactive voltage dependence was discarded");
        };
        assert!((q_constant_current + 0.3).abs() < 1e-12);
        assert!((q_constant_impedance - 0.3).abs() < 1e-12);
    }

    #[test]
    fn exponential_reference_voltage_and_coefficient_losses_are_diagnosed() {
        let mut net = two_bus_network();
        net.loads_mut()[0].voltage_model = Some(LoadVoltageModel::Exponential {
            p: 12.0,
            q: 4.0,
            v_nom: Some(0.9),
            gamma_p: 2.0,
            gamma_q: 1.0,
        });
        let emitted = write_ravens_json(&net);
        let findings = emitted.render_diagnostics();
        assert!(
            findings.iter().any(|finding| {
                finding.contains("EMIT.RAVENS.FIELD_DROPPED")
                    && finding.contains("exponential v_nom=")
            }),
            "{findings:?}"
        );
        assert!(
            findings.iter().any(|finding| {
                finding.contains("EMIT.RAVENS.VALUE_COLLAPSED")
                    && finding.contains("exponential coefficients")
            }),
            "{findings:?}"
        );
        let (back, _) = parse_ravens_json(&emitted.text).unwrap();
        assert!(matches!(
            back.loads()[0].voltage_model,
            Some(LoadVoltageModel::Exponential { v_nom: None, .. })
        ));
    }

    #[test]
    fn deterministic_mrids_are_uuid_shaped_and_stable() {
        let a = det_mrid("ConnectivityNode", "1");
        let b = det_mrid("ConnectivityNode", "1");
        let c = det_mrid("ConnectivityNode", "2");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'5', "version nibble");
    }

    /// The full-projection round-trip comparison the RAVENS mapping carries.
    fn assert_projection(net: &BalancedNetwork, rt: &BalancedNetwork) {
        assert_eq!(rt.buses().len(), net.buses().len());
        assert_eq!(rt.loads().len(), net.loads().len());
        assert_eq!(rt.generators().len(), net.generators().len());
        assert_eq!(rt.branches().len(), net.branches().len());
        assert_eq!(rt.shunts().len(), net.shunts().len());
        for (a, b) in net.buses().iter().zip(rt.buses()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.kind, b.kind, "bus {} kind", a.id);
            assert!((a.vm - b.vm).abs() < 1e-12, "bus {} vm", a.id);
            assert!((a.vmax - b.vmax).abs() < 1e-12, "bus {} vmax", a.id);
        }
        for (a, b) in net.branches().iter().zip(rt.branches()) {
            assert_eq!((a.from, a.to), (b.from, b.to));
            assert!((a.r - b.r).abs() < 1e-12, "r {} vs {}", a.r, b.r);
            assert!((a.x - b.x).abs() < 1e-12);
            assert!((a.b - b.b).abs() < 1e-12);
            assert!((a.calc_effective_tap() - b.calc_effective_tap()).abs() < 1e-12);
            assert!((a.shift - b.shift).abs() < 1e-9);
            assert!((a.rate_a - b.rate_a).abs() < 1e-9);
            assert!((a.rate_b - b.rate_b).abs() < 1e-9);
        }
        for (a, b) in net.generators().iter().zip(rt.generators()) {
            assert_eq!(a.bus, b.bus);
            assert!((a.pg - b.pg).abs() < 1e-9);
            assert!((a.qg - b.qg).abs() < 1e-9);
            assert!((a.qmax - b.qmax).abs() < 1e-9);
            assert!((a.pmin - b.pmin).abs() < 1e-9);
            assert!((a.pmax - b.pmax).abs() < 1e-9);
            match (&a.cost, &b.cost) {
                (Some(ca), Some(cb)) => {
                    let (qa, la) = ca.calc_quadratic().unwrap();
                    let (qb, lb) = cb.calc_quadratic().unwrap();
                    assert!((qa - qb).abs() < 1e-9 && (la - lb).abs() < 1e-9);
                }
                (a, b) => assert_eq!(a.is_some(), b.is_some()),
            }
        }
        for (a, b) in net.loads().iter().zip(rt.loads()) {
            assert_eq!(a.bus, b.bus);
            assert!((a.p - b.p).abs() < 1e-9);
            assert!((a.q - b.q).abs() < 1e-9);
        }
        for (a, b) in net.shunts().iter().zip(rt.shunts()) {
            assert_eq!(a.bus, b.bus);
            assert!((a.b - b.b).abs() < 1e-9, "shunt b {} vs {}", a.b, b.b);
            assert!((a.g - b.g).abs() < 1e-9);
        }
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact-value conventions are the assertion
    fn case118_writes_upstream_conventions_and_round_trips() {
        let net = network("case118.m");
        let conv = write_ravens_json(&net);
        let doc: Value = serde_json::from_str(&conv.text).unwrap();

        // Upstream mpc2ravens conventions: negative machine injection in
        // watts, ohm-domain line impedance through the from-side base.
        let gen1 = &doc["PowerSystemResource"]["Equipment"]["ConductingEquipment"]["EnergyConnection"]
            ["RegulatingCondEq"]["RotatingMachine"]["gen1"];
        assert_eq!(
            gen1["RotatingMachine.p"].as_f64().unwrap(),
            -net.generators()[0].pg * 1e6
        );
        let line1 = &doc["PowerSystemResource"]["Equipment"]["ConductingEquipment"]["Conductor"]["ACLineSegment"]
            ["line1"];
        let kv = net.buses()[0].base_kv;
        let z_base = kv * kv / net.base_mva();
        assert!(
            (line1["ACLineSegment.r"].as_f64().unwrap() - net.branches()[0].r * z_base).abs()
                < 1e-12
        );

        // A tap branch becomes a two-end PowerTransformer with the ratio on a
        // RatioTapChanger, never an ACLineSegment.
        let tap_index = net
            .branches()
            .iter()
            .position(Branch::is_transformer)
            .unwrap();
        let xf = &doc["PowerSystemResource"]["Equipment"]["ConductingEquipment"]["PowerTransformer"]
            [&format!("transformer{}", tap_index + 1)];
        assert!(xf.is_object(), "transformer{} missing", tap_index + 1);
        let tap = &doc["PowerSystemResource"]["TapChanger"]["RatioTapChanger"]
            [&format!("tap{}", tap_index + 1)];
        assert_eq!(
            tap["TapChanger.TapChangerRatio"]["TapChangerRatio.ptRatio"]
                .as_f64()
                .unwrap(),
            net.branches()[tap_index].tap
        );

        // Slack marker on the reference bus.
        let slack = net.buses().iter().find(|b| b.kind == BusType::Ref).unwrap();
        let slack_set = &doc["OperationalLimitSet"][&format!("OpLimVbus{}", slack.id)];
        let has_slack = slack_set["OperationalLimitSet.OperationalLimitValue"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| {
                l["OperationalLimit.OperationalLimitType"]
                    .as_str()
                    .unwrap()
                    .contains("slack")
            });
        assert!(has_slack);

        let (back, _) = parse_ravens_json(&conv.text).unwrap();
        assert_projection(&net, &back);
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact-value conventions are the assertion
    fn degenerate_base_kv_round_trips_per_unit_values() {
        // case14 carries base_kv 0 on every bus; the writer and reader share
        // the 1 kV fallback, so per-unit values survive and both sides warn.
        let net = network("case14.m");
        let conv = write_ravens_json(&net);
        let warnings = conv.render_diagnostics();
        assert!(
            warnings.iter().any(|w| w.contains("base_kv 0")),
            "writer warnings: {warnings:?}"
        );
        let (back, warnings) = parse_ravens_json(&conv.text).unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("base_kv 0")),
            "reader warnings: {warnings:?}"
        );
        assert_projection(&net, &back);
        assert_eq!(back.buses()[0].base_kv, 0.0, "base_kv fidelity");
    }

    #[test]
    fn canonical_write_is_idempotent() {
        let net = network("case118.m");
        let first = write_ravens_json(&net);
        let (reparsed, _) = parse_ravens_json(&first.text).unwrap();
        // The value carries no retained source, so this is the canonical
        // writer again, not the echo tier.
        let second = write_ravens_json(&reparsed);
        assert_same_text(&first.text, &second.text);
    }

    /// Byte equality with a readable failure: the first differing line and
    /// its neighbours, not two whole documents.
    fn assert_same_text(a: &str, b: &str) {
        if a == b {
            return;
        }
        let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
        let at = la
            .iter()
            .zip(&lb)
            .position(|(x, y)| x != y)
            .unwrap_or(la.len().min(lb.len()));
        let window =
            |lines: &[&str]| lines[at.saturating_sub(2)..(at + 3).min(lines.len())].join("\n");
        panic!(
            "texts differ at line {}:\n--- first\n{}\n--- second\n{}",
            at + 1,
            window(&la),
            window(&lb)
        );
    }

    #[test]
    fn multiconductor_documents_are_refused_with_guidance() {
        let text = r#"{
            "ConnectivityNode": {"a": {"Ravens.cimObjectType": "ConnectivityNode"}},
            "PowerSystemResource": {"Equipment": {"ConductingEquipment": {"Conductor": {
                "ACLineSegment": {"l1": {
                    "Ravens.cimObjectType": "ACLineSegment",
                    "ACLineSegment.ACLineSegmentPhase": [
                        {"Ravens.cimObjectType": "ACLineSegmentPhase"}
                    ]
                }}
            }}}}
        }"#;
        let err = parse_ravens_json(text).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("multiconductor"), "got: {message}");
        assert!(message.contains("powerio-dist"), "got: {message}");
    }

    #[test]
    fn unknown_kinds_warn_but_do_not_fail() {
        let net = network("case9.m");
        let text = write_ravens_json(&net).text;
        let mut doc: Value = serde_json::from_str(&text).unwrap();
        doc["ProposedAssetOption"] = serde_json::json!({
            "opt1": {"Ravens.cimObjectType": "ProposedAssetOption"}
        });
        let (_, warnings) = parse_ravens_json(&doc.to_string()).unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("ProposedAssetOption")
                && w.contains("READ.RAVENS.RETAINED_SOURCE_ONLY")),
            "warnings: {warnings:?}"
        );
    }

    #[test]
    fn zip_and_exponential_loads_round_trip() {
        let mut net = network("case9.m");
        let (p0, q0) = (net.loads()[0].p, net.loads()[0].q);
        net.loads_mut()[0].voltage_model = Some(LoadVoltageModel::Zip {
            p_constant_power: p0 * 0.5,
            q_constant_power: q0 * 0.5,
            p_constant_current: p0 * 0.3,
            q_constant_current: q0 * 0.3,
            p_constant_impedance: p0 * 0.2,
            q_constant_impedance: q0 * 0.2,
            v_nom: None,
            load_type: None,
            scaling: None,
        });
        let (p1, q1) = (net.loads()[1].p, net.loads()[1].q);
        net.loads_mut()[1].voltage_model = Some(LoadVoltageModel::Exponential {
            p: p1,
            q: q1,
            v_nom: None,
            gamma_p: 1.2,
            gamma_q: 2.0,
        });
        let conv = write_ravens_json(&net);
        let (back, _) = parse_ravens_json(&conv.text).unwrap();
        match back.loads()[0].voltage_model.as_ref().unwrap() {
            LoadVoltageModel::Zip {
                p_constant_current, ..
            } => {
                assert!((p_constant_current - p0 * 0.3).abs() < 1e-9);
            }
            other => panic!("expected ZIP, got {other:?}"),
        }
        match back.loads()[1].voltage_model.as_ref().unwrap() {
            LoadVoltageModel::Exponential { gamma_p, .. } => {
                assert!((gamma_p - 1.2).abs() < 1e-12);
            }
            other => panic!("expected exponential, got {other:?}"),
        }
    }

    #[test]
    fn unrepresentable_families_warn() {
        let net = network("t_case9_dcline.m");
        let conv = write_ravens_json(&net);
        let warnings = conv.render_diagnostics();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("HVDC") && w.contains("EMIT.RAVENS.RECORD_DROPPED")),
            "warnings: {warnings:?}"
        );
    }
}

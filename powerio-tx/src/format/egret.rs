//! Read and write a [`BalancedNetwork`] as egret `ModelData` JSON.
//!
//! egret groups the network under `elements` (bus, load, branch, generator,
//! shunt, dc_branch) with a small `system` block; values stay in MW/MVAr,
//! degrees, with the base in `system.baseMVA`. Loads and shunts are first-class
//! on the `BalancedNetwork`, generator cost becomes a polynomial/piecewise `cost_curve`,
//! and a branch with a nonzero raw tap or a phase shift is typed `transformer`.
//!
//! The reader takes the power flow ModelData subset: numeric bus ids (as
//! matpower- and pglib-derived files have), scalar element values. Unit
//! commitment cases (`system.time_keys`, time-series values) are rejected. A
//! same format writes return the retained source like every other format.

use serde_json::{Map, Value};

use super::decode::lenient_table;
use super::{TextEmission, finish, jnum, warn_extra_branch_rating_sets};
use crate::diagnostics::Diagnostics;
use crate::diagnostics::codes::EMIT_EGRET as F;
use crate::network::{
    BalancedNetwork, BalancedNetworkTables, Branch, Bus, BusId, BusType, Extras, GenCost,
    Generator, Hvdc, Load, LoadVoltageModel, Shunt, SourceFormat,
};
use crate::{Error, Result};

const FMT: &str = "egret JSON";

#[must_use]
pub fn write_egret_json(net: &BalancedNetwork) -> TextEmission {
    let mut warnings = Diagnostics::new();

    let mut bus = Map::new();
    for b in net.buses() {
        bus.insert(b.id.to_string(), bus_obj(b));
    }

    // egret keys each load/shunt; use a global running suffix (load_1, load_2, …)
    // so several loads on one bus stay distinct.
    let mut load = Map::new();
    for (i, l) in net.loads().iter().enumerate() {
        load.insert(format!("load_{}", i + 1), load_obj(l));
    }
    let mut shunt = Map::new();
    for (i, s) in net.shunts().iter().enumerate() {
        shunt.insert(format!("shunt_{}", i + 1), shunt_obj(s));
    }

    let mut branch = Map::new();
    for (i, br) in net.branches().iter().enumerate() {
        branch.insert((i + 1).to_string(), branch_obj(br));
    }

    let mut generator = Map::new();
    for (i, g) in net.generators().iter().enumerate() {
        generator.insert((i + 1).to_string(), gen_obj(g, &mut warnings));
    }
    let with_caps = net.generators().iter().filter(|g| g.has_caps()).count();
    if with_caps > 0 {
        warnings.push(&F.field_dropped, format!(
            "generator capability/ramp columns dropped for {with_caps} generator(s): the egret generator records written here carry none"
        ));
    }

    let mut dc_branch = Map::new();
    for (i, dc) in net.hvdc().iter().enumerate() {
        dc_branch.insert((i + 1).to_string(), dc_branch_obj(dc, &mut warnings));
    }

    warn_egret_writer_losses(net, &mut warnings);

    let mut elements = Map::new();
    elements.insert("bus".into(), Value::Object(bus));
    elements.insert("load".into(), Value::Object(load));
    elements.insert("shunt".into(), Value::Object(shunt));
    elements.insert("branch".into(), Value::Object(branch));
    elements.insert("generator".into(), Value::Object(generator));
    elements.insert("dc_branch".into(), Value::Object(dc_branch));

    let mut system = Map::new();
    system.insert("baseMVA".into(), jnum(net.base_mva()));
    match reference_bus(net) {
        Some(r) => {
            system.insert("reference_bus".into(), Value::String(r.id.to_string()));
            system.insert("reference_bus_angle".into(), jnum(r.va));
        }
        None => warnings.push(
            &F.reference_missing,
            "no single reference bus (BusType::Ref); system.reference_bus omitted",
        ),
    }

    let mut root = Map::new();
    root.insert("elements".into(), Value::Object(elements));
    root.insert("system".into(), Value::Object(system));

    finish(&F, root, warnings)
}

fn warn_egret_writer_losses(net: &BalancedNetwork, warnings: &mut Diagnostics) {
    super::warn_dropped_areas(&F, "egret JSON", net, warnings);
    if !net.transformers_3w().is_empty() {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} 3-winding transformer(s) dropped: the egret writer emits no 3-winding record",
                net.transformers_3w().len()
            ),
        );
    }
    if net
        .buses()
        .iter()
        .any(|b| b.evhi.is_some() || b.evlo.is_some())
    {
        warnings.push(
            &F.field_dropped,
            "emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band",
        );
    }
    if !net.storage().is_empty() {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} storage unit(s) dropped: egret storage mapping not implemented",
                net.storage().len()
            ),
        );
    }
    let voltage_loads = net
        .loads()
        .iter()
        .filter(|l| {
            l.voltage_model
                .as_ref()
                .is_some_and(LoadVoltageModel::has_non_matpower_fields)
        })
        .count();
    if voltage_loads > 0 {
        warnings.push(&F.field_dropped, format!(
            "{voltage_loads} voltage dependent load model(s) dropped: egret load records carry static p_load/q_load only"
        ));
    }
    let terminal_charging = net
        .branches()
        .iter()
        .filter(|b| b.has_non_matpower_charging())
        .count();
    if terminal_charging > 0 {
        warnings.push(&F.value_collapsed, format!(
            "{terminal_charging} branch terminal admittance record(s) collapsed to total susceptance: egret branches cannot carry conductance or asymmetric terminal charging"
        ));
    }
    let current_ratings = net
        .branches()
        .iter()
        .filter(|b| b.current_ratings.is_some())
        .count();
    if current_ratings > 0 {
        warnings.push(&F.field_dropped, format!(
            "{current_ratings} branch current rating record(s) dropped: egret branch records carry MVA ratings only"
        ));
    }
    warn_extra_branch_rating_sets(&F, "egret JSON", net, warnings);
    let branch_solutions = net
        .branches()
        .iter()
        .filter(|b| b.solution.is_some())
        .count();
    if branch_solutions > 0 {
        warnings.push(&F.field_dropped, format!(
            "{branch_solutions} branch solution value set(s) dropped: egret branch result fields are not written"
        ));
    }
}

fn reference_bus(net: &BalancedNetwork) -> Option<&Bus> {
    let mut refs = net.buses().iter().filter(|b| b.kind == BusType::Ref);
    let first = refs.next()?;
    if refs.next().is_some() {
        None // not a single, unambiguous reference bus
    } else {
        Some(first)
    }
}

fn bustype(kind: BusType) -> &'static str {
    match kind {
        BusType::Pq => "PQ",
        BusType::Pv => "PV",
        BusType::Ref => "ref",
        BusType::Isolated => "isolated",
    }
}

fn bus_obj(b: &Bus) -> Value {
    let mut m = Map::new();
    m.insert("base_kv".into(), jnum(b.base_kv));
    m.insert(
        "matpower_bustype".into(),
        Value::String(bustype(b.kind).into()),
    );
    m.insert("vm".into(), jnum(b.vm));
    m.insert("va".into(), jnum(b.va));
    m.insert("v_min".into(), jnum(b.vmin));
    m.insert("v_max".into(), jnum(b.vmax));
    m.insert("area".into(), Value::String(b.area.to_string()));
    m.insert("zone".into(), Value::String(b.zone.to_string()));
    if let Some(name) = &b.name {
        m.insert("name".into(), Value::String(name.clone()));
    }
    Value::Object(m)
}

fn load_obj(l: &Load) -> Value {
    let mut m = Map::new();
    m.insert("bus".into(), Value::String(l.bus.to_string()));
    m.insert("p_load".into(), jnum(l.p));
    m.insert("q_load".into(), jnum(l.q));
    m.insert("in_service".into(), Value::Bool(l.in_service));
    Value::Object(m)
}

fn shunt_obj(s: &Shunt) -> Value {
    let mut m = Map::new();
    m.insert("bus".into(), Value::String(s.bus.to_string()));
    m.insert("shunt_type".into(), Value::String("fixed".into()));
    m.insert("gs".into(), jnum(s.g));
    m.insert("bs".into(), jnum(s.b));
    Value::Object(m)
}

fn branch_obj(br: &Branch) -> Value {
    let mut m = Map::new();
    m.insert("from_bus".into(), Value::String(br.from.to_string()));
    m.insert("to_bus".into(), Value::String(br.to.to_string()));
    m.insert("resistance".into(), jnum(br.r));
    m.insert("reactance".into(), jnum(br.x));
    m.insert(
        "charging_susceptance".into(),
        jnum(br.calc_total_charging_b()),
    );
    m.insert("in_service".into(), Value::Bool(br.in_service));
    m.insert("angle_diff_min".into(), jnum(br.angmin));
    m.insert("angle_diff_max".into(), jnum(br.angmax));
    if br.is_transformer() {
        m.insert("branch_type".into(), Value::String("transformer".into()));
        m.insert(
            "transformer_tap_ratio".into(),
            jnum(br.calc_effective_tap()),
        );
        m.insert("transformer_phase_shift".into(), jnum(br.shift));
    } else {
        m.insert("branch_type".into(), Value::String("line".into()));
    }
    // egret treats a zero rating as "unset"; emit only nonzero limits.
    if br.rate_a != 0.0 {
        m.insert("rating_long_term".into(), jnum(br.rate_a));
    }
    if br.rate_b != 0.0 {
        m.insert("rating_short_term".into(), jnum(br.rate_b));
    }
    if br.rate_c != 0.0 {
        m.insert("rating_emergency".into(), jnum(br.rate_c));
    }
    Value::Object(m)
}

fn gen_obj(g: &Generator, warnings: &mut Diagnostics) -> Value {
    let mut m = Map::new();
    m.insert("bus".into(), Value::String(g.bus.to_string()));
    m.insert("generator_type".into(), Value::String("thermal".into()));
    m.insert("in_service".into(), Value::Bool(g.in_service));
    m.insert("pg".into(), jnum(g.pg));
    m.insert("qg".into(), jnum(g.qg));
    m.insert("vg".into(), jnum(g.vg));
    m.insert("mbase".into(), jnum(g.mbase));
    m.insert("p_min".into(), jnum(g.pmin));
    m.insert("p_max".into(), jnum(g.pmax));
    m.insert("q_min".into(), jnum(g.qmin));
    m.insert("q_max".into(), jnum(g.qmax));
    if let Some(cost) = &g.cost {
        if let Some(curve) = cost_curve(cost) {
            m.insert("p_cost".into(), curve);
            // The reader defaults both to zero, so only nonzero values need
            // stating — and a zero-cost write then reads back identically.
            if cost.startup != 0.0 {
                m.insert("startup_cost".into(), jnum(cost.startup));
            }
            if cost.shutdown != 0.0 {
                m.insert("shutdown_cost".into(), jnum(cost.shutdown));
            }
        } else {
            warnings.push(&F.field_dropped, format!(
                "generator at bus {} has a cost model egret's writer can't express; cost dropped",
                g.bus
            ));
        }
    }
    Value::Object(m)
}

/// An egret `dc_branch` element, the inverse of [`read_dc_branch`]. egret's
/// `dc_branch` states the same power, voltage, and loss fields the MATPOWER
/// `dcline` row does, so every one of them is named here; only the `dclinecost`
/// curve has no egret counterpart.
fn dc_branch_obj(dc: &Hvdc, warnings: &mut Diagnostics) -> Value {
    if dc.cost.is_some() {
        warnings.push(
            &F.field_dropped,
            format!(
                "dcline {} -> {} cost curve dropped: egret dc_branch records carry no cost",
                dc.from, dc.to
            ),
        );
    }
    let mut m = Map::new();
    m.insert("from_bus".into(), Value::String(dc.from.to_string()));
    m.insert("to_bus".into(), Value::String(dc.to.to_string()));
    m.insert("in_service".into(), Value::Bool(dc.in_service));
    m.insert("pf".into(), jnum(dc.pf));
    m.insert("pt".into(), jnum(dc.pt));
    m.insert("qf".into(), jnum(dc.qf));
    m.insert("qt".into(), jnum(dc.qt));
    m.insert("vf".into(), jnum(dc.vf));
    m.insert("vt".into(), jnum(dc.vt));
    m.insert("pmin".into(), jnum(dc.pmin));
    m.insert("pmax".into(), jnum(dc.pmax));
    m.insert("qminf".into(), jnum(dc.qminf));
    m.insert("qmaxf".into(), jnum(dc.qmaxf));
    m.insert("qmint".into(), jnum(dc.qmint));
    m.insert("qmaxt".into(), jnum(dc.qmaxt));
    m.insert("loss0".into(), jnum(dc.loss0));
    m.insert("loss_factor".into(), jnum(dc.loss1));
    Value::Object(m)
}

/// egret `cost_curve`. MATPOWER model 2 (polynomial) maps to a degree→coefficient
/// map; model 1 (piecewise linear) maps to `(mw, cost)` breakpoints.
fn cost_curve(cost: &GenCost) -> Option<Value> {
    let mut curve = Map::new();
    curve.insert("data_type".into(), Value::String("cost_curve".into()));
    match cost.model {
        2 => {
            // coeffs are highest-order first: coeffs[i] multiplies p^(k-1-i),
            // where k = coeffs.len() (== ncost for a well-formed polynomial).
            let mut values = Map::new();
            let k = cost.coeffs.len();
            for (i, &c) in cost.coeffs.iter().enumerate() {
                values.insert((k - 1 - i).to_string(), jnum(c));
            }
            curve.insert("cost_curve_type".into(), Value::String("polynomial".into()));
            curve.insert("values".into(), Value::Object(values));
            Some(Value::Object(curve))
        }
        1 => {
            let points: Vec<Value> = cost
                .coeffs
                .chunks_exact(2)
                .map(|pt| Value::Array(vec![jnum(pt[0]), jnum(pt[1])]))
                .collect();
            curve.insert("cost_curve_type".into(), Value::String("piecewise".into()));
            curve.insert("values".into(), Value::Array(points));
            Some(Value::Object(curve))
        }
        _ => None,
    }
}

/// Owned-source entry used by the format hub: parse by borrowing `source`, then
/// move the buffer into the retained source (no copy, byte-exact round-trip).
/// `name_hint` (e.g. a file stem) names the network when the JSON has no
/// `model_name`.
pub(crate) fn parse_egret_source(source: &str, name_hint: Option<&str>) -> Result<BalancedNetwork> {
    // Typed decoding, no generic JSON tree: known fields land in struct
    // slots and only each element's unknown remainder is retained as extras
    // (#293). Present-but-mistyped scalar fields still error, now at decode.
    let document: Document = serde_json::from_str(source).map_err(|e| bad(e.to_string()))?;
    if document
        .system
        .as_ref()
        .is_some_and(|system| system.time_keys.is_some())
    {
        return Err(bad(
            "egret unit commitment cases (system.time_keys) are not supported here; \
             `parse_egret_time_series` reads the scalar network profile sequence",
        ));
    }
    build_from_document(document, name_hint)
}

/// The typed document. Unknown top-level and system keys are ignored, as the
/// tree walk before it ignored them.
#[derive(Default, serde::Deserialize)]
struct Document {
    #[serde(default)]
    model_name: Option<Value>,
    #[serde(default)]
    system: Option<SystemSection>,
    #[serde(default)]
    elements: Option<Elements>,
}

#[derive(Default, serde::Deserialize)]
struct SystemSection {
    #[serde(rename = "baseMVA", default)]
    base_mva: Option<Value>,
    #[serde(default)]
    time_keys: Option<Value>,
}

#[derive(Default, serde::Deserialize)]
struct Elements {
    #[serde(default, deserialize_with = "lenient_table")]
    bus: Vec<(String, BusRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    load: Vec<(String, LoadRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    shunt: Vec<(String, ShuntRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    branch: Vec<(String, BranchRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    generator: Vec<(String, GenRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    dc_branch: Vec<(String, DcBranchRow)>,
}

/// The shared build both entries use: the typed document, decoded from text
/// (scalar) or converted from the substituted tree (sequence).
fn build_from_document(document: Document, name_hint: Option<&str>) -> Result<BalancedNetwork> {
    let system = document
        .system
        .ok_or_else(|| bad("missing `system` object"))?;
    let base_mva = system
        .base_mva
        .as_ref()
        .and_then(Value::as_f64)
        .ok_or_else(|| bad("missing numeric system.baseMVA"))?;
    let elements = document
        .elements
        .ok_or_else(|| bad("missing `elements` object"))?;
    let name = document
        .model_name
        .as_ref()
        .and_then(Value::as_str)
        .or(name_hint)
        .unwrap_or("case")
        .to_string();

    let buses = sorted_table(elements.bus)
        .into_iter()
        .map(|(key, row)| read_bus(&key, row))
        .collect::<Result<Vec<_>>>()?;
    let loads = sorted_table(elements.load)
        .into_iter()
        .map(|(_, row)| read_load(row))
        .collect::<Result<Vec<_>>>()?;
    let shunts = sorted_table(elements.shunt)
        .into_iter()
        .map(|(_, row)| read_shunt(row))
        .collect::<Result<Vec<_>>>()?;
    let branches = sorted_table(elements.branch)
        .into_iter()
        .map(|(_, row)| read_branch(row))
        .collect::<Result<Vec<_>>>()?;
    let generators = sorted_table(elements.generator)
        .into_iter()
        .map(|(_, row)| read_gen(&row))
        .collect::<Result<Vec<_>>>()?;
    let hvdc = sorted_table(elements.dc_branch)
        .into_iter()
        .map(|(_, row)| read_dc_branch(row))
        .collect::<Result<Vec<_>>>()?;

    let net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        geo: None,
        buses: buses.into(),
        loads: loads.into(),
        shunts: shunts.into(),
        branches: branches.into(),
        switches: Vec::new().into(),
        generators: generators.into(),
        storage: Vec::new().into(),
        hvdc: hvdc.into(),
        transformers_3w: Vec::new().into(),
        areas: Vec::new().into(),
        solver: None,
        source_format: SourceFormat::EgretJson,
    });
    net.check_references(FMT)?;
    Ok(net)
}

/// Elements sorted by the integer in the key: a bare id or a labeled key's
/// trailing index, keeping `load_2` before `load_10` so a re-emit reproduces
/// the writer's element order.
fn sorted_table<T>(mut rows: Vec<(String, T)>) -> Vec<(String, T)> {
    rows.sort_by(|(a, _), (b, _)| num_key(a).cmp(&num_key(b)).then_with(|| a.cmp(b)));
    rows
}

/// One time varying attribute of an Egret sequence: which element row it
/// patches and its per point values (index 0 already substituted into the
/// base document).
struct VaryingAttribute {
    section: String,
    element: String,
    attribute: String,
    row: usize,
    values: Vec<Value>,
}

/// Parse an Egret `ModelData` document with `system.time_keys` into a
/// balanced network time series, applying Egret's own scalar snapshot rule:
/// a `{"data_type": "time_series", "values": [...]}` attribute varies by
/// point and everything else is one static statement. The static tables are
/// shared between points — each point clones the network handle and copies
/// only the tables a varying attribute touches.
///
/// # Errors
/// A document without `time_keys` (parse it as one scalar snapshot), a
/// varying attribute outside the supported scalar network profile, a values
/// list whose length disagrees with `time_keys`, or any scalar profile error.
pub fn parse_egret_time_series(
    content: &str,
    name_hint: Option<&str>,
) -> Result<powerio_core::TimeSeries<BalancedNetwork>> {
    let root: Value = serde_json::from_str(content).map_err(|e| bad(e.to_string()))?;
    let Value::Object(mut root) = root else {
        return Err(bad("top level is not a JSON object"));
    };

    let system = obj(&root, "system").ok_or_else(|| bad("missing `system` object"))?;
    let time_keys: Vec<String> = match system.get("time_keys") {
        Some(Value::Array(keys)) => keys
            .iter()
            .map(|k| match k {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        Some(other) => {
            return Err(bad(format!("`system.time_keys` is not an array: {other}")));
        }
        None => {
            return Err(bad(
                "`system.time_keys` is absent; parse the document as one scalar snapshot",
            ));
        }
    };
    if time_keys.is_empty() {
        return Err(bad("`system.time_keys` is empty"));
    }

    // Substitute every time series attribute with its first value, recording
    // where it patches. The substituted document is then exactly the point 0
    // scalar snapshot, and the scalar reader validates it as one.
    let mut varying: Vec<VaryingAttribute> = Vec::new();
    {
        let elements = root
            .get_mut("elements")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| bad("missing `elements` object"))?;
        for (section, table) in elements.iter_mut() {
            let Some(table) = table.as_object_mut() else {
                continue;
            };
            let order: Vec<String> = {
                let frozen: &Map<String, Value> = table;
                sorted_kv(frozen)
                    .into_iter()
                    .map(|(k, _)| k.clone())
                    .collect()
            };
            for (row, key) in order.iter().enumerate() {
                let Some(element) = table.get_mut(key).and_then(Value::as_object_mut) else {
                    continue;
                };
                let series_attributes: Vec<String> = element
                    .iter()
                    .filter(|(_, value)| is_time_series(value))
                    .map(|(attribute, _)| attribute.clone())
                    .collect();
                for attribute in series_attributes {
                    let Some(slot) = element.get_mut(&attribute) else {
                        continue;
                    };
                    let values =
                        take_series_values(slot, time_keys.len(), section, key, &attribute)?;
                    *slot = values[0].clone();
                    varying.push(VaryingAttribute {
                        section: section.clone(),
                        element: key.clone(),
                        attribute,
                        row,
                        values,
                    });
                }
            }
        }
    }

    let document: Document =
        serde_json::from_value(Value::Object(root)).map_err(|e| bad(e.to_string()))?;
    let base = build_from_document(document, name_hint)?;
    let mut networks = Vec::with_capacity(time_keys.len());
    networks.push(base.clone());
    for point in 1..time_keys.len() {
        let mut network = base.clone();
        for attribute in &varying {
            apply_varying(&mut network, attribute, point)?;
        }
        networks.push(network);
    }

    let time_points = time_keys
        .iter()
        .map(|key| powerio_core::TimePoint::new(key.clone(), None))
        .collect::<std::result::Result<Vec<_>, powerio_core::Error>>()
        .map_err(|e| bad(e.to_string()))?;
    powerio_core::TimeSeries::new(time_points, networks).map_err(|e| bad(e.to_string()))
}

/// Whether the document declares the Egret time series axis: a
/// `system.time_keys` value selects the sequence reader. A document this
/// probe cannot decode answers `false` and fails in the scalar reader with
/// its own wording.
#[must_use]
pub fn egret_declares_time_series(content: &str) -> bool {
    #[derive(Default, serde::Deserialize)]
    struct ProbedSystem {
        time_keys: Option<serde_json::Value>,
    }
    #[derive(Default, serde::Deserialize)]
    struct Probe {
        #[serde(default)]
        system: ProbedSystem,
    }
    serde_json::from_str::<Probe>(content).is_ok_and(|probe| probe.system.time_keys.is_some())
}

fn is_time_series(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|o| o.get("data_type").and_then(Value::as_str) == Some("time_series"))
}

/// The values list of one time series attribute, length checked against the
/// time axis.
fn take_series_values(
    slot: &Value,
    points: usize,
    section: &str,
    element: &str,
    attribute: &str,
) -> Result<Vec<Value>> {
    let values = slot
        .as_object()
        .and_then(|o| o.get("values"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            bad(format!(
                "{section} {element}: time series `{attribute}` has no `values` list"
            ))
        })?;
    if values.len() != points {
        return Err(bad(format!(
            "{section} {element}: time series `{attribute}` states {} values for {points} time keys",
            values.len()
        )));
    }
    Ok(values.clone())
}

/// Patch one varying attribute's value for `point` into the network, on the
/// same row order the scalar reader used. An attribute outside the supported
/// scalar network profile is refused by name.
fn apply_varying(
    network: &mut BalancedNetwork,
    varying: &VaryingAttribute,
    point: usize,
) -> Result<()> {
    let value = &varying.values[point];
    let outside = || {
        bad(format!(
            "{} {}: time varying `{}` is outside the supported scalar network profile",
            varying.section, varying.element, varying.attribute
        ))
    };
    let number = || {
        value.as_f64().ok_or_else(|| {
            bad(format!(
                "{} {}: time series `{}` value at point {point} is not a number: {value}",
                varying.section, varying.element, varying.attribute
            ))
        })
    };
    let boolean = || match value {
        Value::Bool(b) => Ok(*b),
        other => Err(bad(format!(
            "{} {}: time series `{}` value at point {point} is not a boolean: {other}",
            varying.section, varying.element, varying.attribute
        ))),
    };
    let row = varying.row;
    match varying.section.as_str() {
        "load" => {
            let load = &mut network.loads_mut()[row];
            match varying.attribute.as_str() {
                "p_load" => load.p = number()?,
                "q_load" => load.q = number()?,
                "in_service" => load.in_service = boolean()?,
                _ => return Err(outside()),
            }
        }
        "generator" => {
            let generator = &mut network.generators_mut()[row];
            match varying.attribute.as_str() {
                "pg" => generator.pg = number()?,
                "qg" => generator.qg = number()?,
                "p_min" => generator.pmin = number()?,
                "p_max" => generator.pmax = number()?,
                "q_min" => generator.qmin = number()?,
                "q_max" => generator.qmax = number()?,
                "vg" => generator.vg = number()?,
                "in_service" => generator.in_service = boolean()?,
                _ => return Err(outside()),
            }
        }
        "shunt" => {
            let shunt = &mut network.shunts_mut()[row];
            match varying.attribute.as_str() {
                "gs" => shunt.g = number()?,
                "bs" => shunt.b = number()?,
                "in_service" => shunt.in_service = boolean()?,
                _ => return Err(outside()),
            }
        }
        "branch" => {
            let branch = &mut network.branches_mut()[row];
            match varying.attribute.as_str() {
                "rating_long_term" => branch.rate_a = number()?,
                "rating_short_term" => branch.rate_b = number()?,
                "rating_emergency" => branch.rate_c = number()?,
                "transformer_tap_ratio" => branch.tap = number()?,
                "transformer_phase_shift" => branch.shift = number()?,
                "in_service" => branch.in_service = boolean()?,
                _ => return Err(outside()),
            }
        }
        "bus" => {
            let bus = &mut network.buses_mut()[row];
            match varying.attribute.as_str() {
                "vm" => bus.vm = number()?,
                "va" => bus.va = number()?,
                _ => return Err(outside()),
            }
        }
        _ => return Err(outside()),
    }
    Ok(())
}

fn bustype_from_str(s: &str) -> BusType {
    match s {
        "PV" => BusType::Pv,
        "ref" => BusType::Ref,
        "isolated" => BusType::Isolated,
        _ => BusType::Pq,
    }
}

fn bad(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FMT,
        message: message.into(),
    }
}

fn obj<'a>(v: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    v.get(key).and_then(Value::as_object)
}

/// Element entries sorted by the integer in the key: a bare id (`"1".."m"`, the
/// bus/branch/generator keys) or the trailing index of a labeled key
/// (`"load_10"` → 10). Keeps `load_2` before `load_10` so a re-emit reproduces
/// the writer's element order (which keys by enumeration index).
fn sorted_kv(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut items: Vec<(&String, &Value)> = map.iter().collect();
    items.sort_by(|(a, _), (b, _)| num_key(a).cmp(&num_key(b)).then_with(|| a.cmp(b)));
    items
}

/// The trailing run of digits as an integer (`"5"` → 5, `"load_10"` → 10); a key
/// with no trailing digits sorts last. Scans bytes from the end, no allocation.
fn num_key(k: &str) -> i64 {
    let start = k.len() - k.bytes().rev().take_while(u8::is_ascii_digit).count();
    k[start..].parse::<i64>().unwrap_or(i64::MAX)
}

/// A non-negative integral id from an f64 (egret writes some ids as numbers).
/// Fractional values are refused here; the range bound is the shared
/// [`crate::format::id_from_f64`] policy.
fn integral_id(x: f64) -> Option<usize> {
    (x.fract() == 0.0)
        .then(|| crate::format::id_from_f64(x, "id").ok())
        .flatten()
}

/// A bus id from a JSON value: a numeric string (egret's convention) or a bare
/// number. `None` for a non-integer, negative, out-of-range, or non-numeric
/// value (named buses aren't representable in the integer `BusId` space).
fn parse_id(v: &Value) -> Option<usize> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            s.parse::<usize>()
                .ok()
                .filter(|&x| x <= BusId::MAX.0)
                .or_else(|| s.parse::<f64>().ok().and_then(integral_id))
        }
        Value::Number(n) => n
            .as_u64()
            .filter(|&x| x <= BusId::MAX.0 as u64)
            .map(|x| x as usize)
            .or_else(|| n.as_f64().and_then(integral_id)),
        _ => None,
    }
}

/// A bus reference cell: a numeric string (egret's convention) or a bare
/// number, kept raw so the error can name the exact value.
fn id_cell(slot: Option<&Value>, key: &str) -> Result<BusId> {
    let raw = slot.ok_or_else(|| bad(format!("element missing `{key}`")))?;
    parse_id(raw)
        .map(BusId)
        .ok_or_else(|| bad(format!("`{key}` is not a numeric bus id: {raw}")))
}

/// A numeric-or-numeric-string id cell with a default when absent.
fn usize_cell(slot: Option<&Value>, key: &str, default: usize) -> Result<usize> {
    match slot {
        None | Some(&Value::Null) => Ok(default),
        Some(x) => {
            parse_id(x).ok_or_else(|| bad(format!("`{key}` is not a non-negative integer: {x}")))
        }
    }
}

/// A present-but-mistyped scalar is an error, not a silent default — the
/// stance every scalar helper here has always taken, so a garbled number
/// cannot quietly become a plausible `0.0` and corrupt the matrices
/// downstream.
fn num_cell(slot: Option<&Value>, key: &str, default: f64) -> Result<f64> {
    match slot {
        None | Some(&Value::Null) => Ok(default),
        Some(x) => x
            .as_f64()
            .ok_or_else(|| bad(format!("`{key}` is not a number: {x}"))),
    }
}

fn bool_cell(slot: Option<&Value>, key: &str, default: bool) -> Result<bool> {
    match slot {
        None | Some(&Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(x) => Err(bad(format!("`{key}` is not a boolean: {x}"))),
    }
}

#[derive(Default, serde::Deserialize)]
struct BusRow {
    #[serde(default)]
    matpower_bustype: Option<Value>,
    #[serde(default)]
    vm: Option<Value>,
    #[serde(default)]
    va: Option<Value>,
    #[serde(default)]
    base_kv: Option<Value>,
    #[serde(default)]
    v_max: Option<Value>,
    #[serde(default)]
    v_min: Option<Value>,
    #[serde(default)]
    area: Option<Value>,
    #[serde(default)]
    zone: Option<Value>,
    #[serde(default)]
    name: Option<Value>,
    #[serde(flatten)]
    extras: Extras,
}

fn read_bus(key: &str, row: BusRow) -> Result<Bus> {
    let id = key
        .trim()
        .parse::<usize>()
        .map_err(|_| bad(format!("bus key is not a numeric id: {key:?}")))?;
    Ok(Bus {
        id: BusId(id),
        kind: bustype_from_str(
            row.matpower_bustype
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or("PQ"),
        ),
        vm: num_cell(row.vm.as_ref(), "vm", 1.0)?,
        va: num_cell(row.va.as_ref(), "va", 0.0)?,
        base_kv: num_cell(row.base_kv.as_ref(), "base_kv", 0.0)?,
        vmax: num_cell(row.v_max.as_ref(), "v_max", 1.1)?,
        vmin: num_cell(row.v_min.as_ref(), "v_min", 0.9)?,
        evhi: None,
        evlo: None,
        area: usize_cell(row.area.as_ref(), "area", 0)?,
        zone: usize_cell(row.zone.as_ref(), "zone", 0)?,
        name: row
            .name
            .as_ref()
            .and_then(Value::as_str)
            .map(str::to_string),
        uid: None,
        location: None,
        extras: row.extras,
    })
}

#[derive(Default, serde::Deserialize)]
struct LoadRow {
    #[serde(default)]
    bus: Option<Value>,
    #[serde(default)]
    p_load: Option<Value>,
    #[serde(default)]
    q_load: Option<Value>,
    #[serde(default)]
    in_service: Option<Value>,
    #[serde(flatten)]
    extras: Extras,
}

fn read_load(row: LoadRow) -> Result<Load> {
    Ok(Load {
        bus: id_cell(row.bus.as_ref(), "bus")?,
        p: num_cell(row.p_load.as_ref(), "p_load", 0.0)?,
        q: num_cell(row.q_load.as_ref(), "q_load", 0.0)?,
        voltage_model: None,
        in_service: bool_cell(row.in_service.as_ref(), "in_service", true)?,
        uid: None,
        extras: row.extras,
    })
}

#[derive(Default, serde::Deserialize)]
struct ShuntRow {
    #[serde(default)]
    bus: Option<Value>,
    #[serde(default)]
    gs: Option<Value>,
    #[serde(default)]
    bs: Option<Value>,
    #[serde(default)]
    in_service: Option<Value>,
    /// Absorbed so it stays out of extras, as the exclude list always did.
    #[serde(rename = "shunt_type", default)]
    _shunt_type: Option<serde::de::IgnoredAny>,
    #[serde(flatten)]
    extras: Extras,
}

fn read_shunt(row: ShuntRow) -> Result<Shunt> {
    Ok(Shunt {
        bus: id_cell(row.bus.as_ref(), "bus")?,
        g: num_cell(row.gs.as_ref(), "gs", 0.0)?,
        b: num_cell(row.bs.as_ref(), "bs", 0.0)?,
        in_service: bool_cell(row.in_service.as_ref(), "in_service", true)?,
        control: None,
        uid: None,
        extras: row.extras,
    })
}

#[derive(Default, serde::Deserialize)]
struct BranchRow {
    #[serde(default)]
    from_bus: Option<Value>,
    #[serde(default)]
    to_bus: Option<Value>,
    #[serde(default)]
    resistance: Option<Value>,
    #[serde(default)]
    reactance: Option<Value>,
    #[serde(default)]
    charging_susceptance: Option<Value>,
    #[serde(default)]
    rating_long_term: Option<Value>,
    #[serde(default)]
    rating_short_term: Option<Value>,
    #[serde(default)]
    rating_emergency: Option<Value>,
    #[serde(default)]
    branch_type: Option<Value>,
    #[serde(default)]
    transformer_tap_ratio: Option<Value>,
    #[serde(default)]
    transformer_phase_shift: Option<Value>,
    #[serde(default)]
    in_service: Option<Value>,
    #[serde(default)]
    angle_diff_min: Option<Value>,
    #[serde(default)]
    angle_diff_max: Option<Value>,
    #[serde(flatten)]
    extras: Extras,
}

fn read_branch(row: BranchRow) -> Result<Branch> {
    let is_xf = row.branch_type.as_ref().and_then(Value::as_str) == Some("transformer");
    Ok(Branch {
        from: id_cell(row.from_bus.as_ref(), "from_bus")?,
        to: id_cell(row.to_bus.as_ref(), "to_bus")?,
        r: num_cell(row.resistance.as_ref(), "resistance", 0.0)?,
        x: num_cell(row.reactance.as_ref(), "reactance", 0.0)?,
        b: num_cell(
            row.charging_susceptance.as_ref(),
            "charging_susceptance",
            0.0,
        )?,
        charging: None,
        rate_a: num_cell(row.rating_long_term.as_ref(), "rating_long_term", 0.0)?,
        rate_b: num_cell(row.rating_short_term.as_ref(), "rating_short_term", 0.0)?,
        rate_c: num_cell(row.rating_emergency.as_ref(), "rating_emergency", 0.0)?,
        rating_sets: Vec::new(),
        current_ratings: None,
        tap: if is_xf {
            num_cell(
                row.transformer_tap_ratio.as_ref(),
                "transformer_tap_ratio",
                1.0,
            )?
        } else {
            0.0
        },
        shift: num_cell(
            row.transformer_phase_shift.as_ref(),
            "transformer_phase_shift",
            0.0,
        )?,
        in_service: bool_cell(row.in_service.as_ref(), "in_service", true)?,
        angmin: num_cell(row.angle_diff_min.as_ref(), "angle_diff_min", -360.0)?,
        angmax: num_cell(row.angle_diff_max.as_ref(), "angle_diff_max", 360.0)?,
        control: None,
        solution: None,
        uid: None,
        route: None,
        extras: row.extras,
    })
}

#[derive(Default, serde::Deserialize)]
struct GenRow {
    #[serde(default)]
    bus: Option<Value>,
    #[serde(default)]
    pg: Option<Value>,
    #[serde(default)]
    qg: Option<Value>,
    #[serde(default)]
    p_max: Option<Value>,
    #[serde(default)]
    p_min: Option<Value>,
    #[serde(default)]
    q_max: Option<Value>,
    #[serde(default)]
    q_min: Option<Value>,
    #[serde(default)]
    vg: Option<Value>,
    #[serde(default)]
    mbase: Option<Value>,
    #[serde(default)]
    in_service: Option<Value>,
    #[serde(default)]
    startup_cost: Option<Value>,
    #[serde(default)]
    shutdown_cost: Option<Value>,
    #[serde(default)]
    p_cost: Option<Value>,
}

fn read_gen(row: &GenRow) -> Result<Generator> {
    let startup = num_cell(row.startup_cost.as_ref(), "startup_cost", 0.0)?;
    let shutdown = num_cell(row.shutdown_cost.as_ref(), "shutdown_cost", 0.0)?;
    // A present `p_cost` that doesn't parse is a hard error, not a silent drop:
    // the same stance the scalar field helpers take, so a malformed cost curve
    // can't quietly become a free generator.
    let cost = match row.p_cost.as_ref().filter(|pc| !pc.is_null()) {
        None => None,
        Some(pc) => Some(read_cost(pc, startup, shutdown).ok_or_else(|| {
            bad("`p_cost` is present but has an unrecognized or malformed cost_curve")
        })?),
    };
    Ok(Generator {
        bus: id_cell(row.bus.as_ref(), "bus")?,
        pg: num_cell(row.pg.as_ref(), "pg", 0.0)?,
        qg: num_cell(row.qg.as_ref(), "qg", 0.0)?,
        pmax: num_cell(row.p_max.as_ref(), "p_max", 0.0)?,
        pmin: num_cell(row.p_min.as_ref(), "p_min", 0.0)?,
        qmax: num_cell(row.q_max.as_ref(), "q_max", 0.0)?,
        qmin: num_cell(row.q_min.as_ref(), "q_min", 0.0)?,
        vg: num_cell(row.vg.as_ref(), "vg", 1.0)?,
        mbase: num_cell(row.mbase.as_ref(), "mbase", 100.0)?,
        in_service: bool_cell(row.in_service.as_ref(), "in_service", true)?,
        cost,
        caps: Default::default(),
        regulated_bus: None,
        uid: None,
    })
}

#[derive(Default, serde::Deserialize)]
struct DcBranchRow {
    #[serde(default)]
    from_bus: Option<Value>,
    #[serde(default)]
    to_bus: Option<Value>,
    #[serde(default)]
    in_service: Option<Value>,
    #[serde(default)]
    pf: Option<Value>,
    #[serde(default)]
    pt: Option<Value>,
    #[serde(default)]
    qf: Option<Value>,
    #[serde(default)]
    qt: Option<Value>,
    #[serde(default)]
    vf: Option<Value>,
    #[serde(default)]
    vt: Option<Value>,
    #[serde(default)]
    pmin: Option<Value>,
    #[serde(default)]
    pmax: Option<Value>,
    #[serde(default)]
    qminf: Option<Value>,
    #[serde(default)]
    qmaxf: Option<Value>,
    #[serde(default)]
    qmint: Option<Value>,
    #[serde(default)]
    qmaxt: Option<Value>,
    #[serde(default)]
    loss0: Option<Value>,
    #[serde(default)]
    loss_factor: Option<Value>,
    #[serde(flatten)]
    extras: Extras,
}

fn read_dc_branch(row: DcBranchRow) -> Result<Hvdc> {
    Ok(Hvdc {
        from: id_cell(row.from_bus.as_ref(), "from_bus")?,
        to: id_cell(row.to_bus.as_ref(), "to_bus")?,
        in_service: bool_cell(row.in_service.as_ref(), "in_service", true)?,
        pf: num_cell(row.pf.as_ref(), "pf", 0.0)?,
        pt: num_cell(row.pt.as_ref(), "pt", 0.0)?,
        qf: num_cell(row.qf.as_ref(), "qf", 0.0)?,
        qt: num_cell(row.qt.as_ref(), "qt", 0.0)?,
        vf: num_cell(row.vf.as_ref(), "vf", 1.0)?,
        vt: num_cell(row.vt.as_ref(), "vt", 1.0)?,
        pmin: num_cell(row.pmin.as_ref(), "pmin", 0.0)?,
        pmax: num_cell(row.pmax.as_ref(), "pmax", 0.0)?,
        qminf: num_cell(row.qminf.as_ref(), "qminf", 0.0)?,
        qmaxf: num_cell(row.qmaxf.as_ref(), "qmaxf", 0.0)?,
        qmint: num_cell(row.qmint.as_ref(), "qmint", 0.0)?,
        qmaxt: num_cell(row.qmaxt.as_ref(), "qmaxt", 0.0)?,
        loss0: num_cell(row.loss0.as_ref(), "loss0", 0.0)?,
        loss1: num_cell(row.loss_factor.as_ref(), "loss_factor", 0.0)?,
        cost: None,
        uid: None,
        extras: row.extras,
    })
}

fn read_cost(p_cost: &Value, startup: f64, shutdown: f64) -> Option<GenCost> {
    let m = p_cost.as_object()?;
    match m.get("cost_curve_type").and_then(Value::as_str)? {
        "polynomial" => {
            // The exponent keys size the coefficient vector below; an
            // unbounded key (a few bytes of JSON) would drive an arbitrarily
            // large allocation. No physical cost curve goes past a handful of
            // terms; keys beyond the cap are dropped like non-numeric ones.
            const MAX_COST_EXPONENT: usize = 64;
            let values = m.get("values")?.as_object()?;
            let pairs: Vec<(usize, f64)> = values
                .iter()
                .filter_map(|(k, c)| Some((k.parse().ok()?, c.as_f64()?)))
                .filter(|(e, _)| *e <= MAX_COST_EXPONENT)
                .collect();
            let max_exp = pairs.iter().map(|(e, _)| *e).max()?;
            let mut coeffs = vec![0.0; max_exp + 1]; // index 0 = highest order
            for (e, c) in pairs {
                coeffs[max_exp - e] = c;
            }
            let ncost = coeffs.len();
            Some(GenCost {
                model: 2,
                startup,
                shutdown,
                ncost,
                coeffs,
            })
        }
        "piecewise" => {
            let values = m.get("values")?.as_array()?;
            let mut coeffs = Vec::with_capacity(values.len() * 2);
            for pt in values {
                let pair = pt.as_array()?;
                coeffs.push(pair.first()?.as_f64()?);
                coeffs.push(pair.get(1)?.as_f64()?);
            }
            Some(GenCost {
                model: 1,
                startup,
                shutdown,
                ncost: values.len(),
                coeffs,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    fn parse_egret_json(content: &str) -> Result<BalancedNetwork> {
        parse_egret_source(content, None)
    }

    use super::*;
    use crate::network::BusType;

    #[test]
    fn oversized_cost_exponent_is_dropped_not_allocated() {
        // The exponent key sizes the coefficient vector; unbounded it would be
        // an allocation of that many f64s from a few bytes of JSON, and a key
        // at usize::MAX would wrap `max_exp + 1` to zero and index out of
        // bounds.
        let all_oversized: Value = serde_json::json!({
            "cost_curve_type": "polynomial",
            "values": {"100000000000": 5.0, "18446744073709551615": 1.0}
        });
        assert!(read_cost(&all_oversized, 0.0, 0.0).is_none());

        let mixed: Value = serde_json::json!({
            "cost_curve_type": "polynomial",
            "values": {"2": 3.0, "100000000000": 1.0}
        });
        let cost = read_cost(&mixed, 0.0, 0.0).unwrap();
        assert_eq!(cost.coeffs, vec![3.0, 0.0, 0.0]);
    }

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/egret")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn reads_buses_loads_branches_and_reference() {
        let net = parse_egret_json(&fixture("case30.json")).unwrap();
        assert!((net.base_mva() - 100.0).abs() < 1e-9);
        assert_eq!(net.buses().len(), 30);
        assert_eq!(net.loads().len(), 20);
        assert_eq!(net.shunts().len(), 2);
        assert_eq!(net.branches().len(), 41);
        assert_eq!(net.generators().len(), 6);
        // Exactly one reference bus, parsed from matpower_bustype.
        let refs = net
            .buses()
            .iter()
            .filter(|b| b.kind == BusType::Ref)
            .count();
        assert_eq!(refs, 1);
    }

    #[test]
    fn inverts_transformer_and_polynomial_cost() {
        let net = parse_egret_json(&fixture("case14.json")).unwrap();
        // case14 has tap-changing transformers (raw tap != 0 ⇒ is_transformer).
        assert!(net.branches().iter().any(Branch::is_transformer));
        // Generators carry a polynomial cost, highest order first.
        let cost = net
            .generators()
            .iter()
            .find_map(|g| g.cost.as_ref())
            .expect("a generator cost");
        assert_eq!(cost.model, 2);
        assert_eq!(cost.coeffs.len(), cost.ncost);
    }

    #[test]
    fn maps_dc_branch_to_hvdc() {
        let net = parse_egret_json(&fixture("dcline3.json")).unwrap();
        assert_eq!(net.hvdc().len(), 1);
        let dc = &net.hvdc()[0];
        assert_eq!((dc.from, dc.to), (BusId(1), BusId(3)));
        assert!((dc.loss1 - 0.1).abs() < 1e-12); // loss_factor → loss1
    }

    #[test]
    fn rejects_unit_commitment_time_series() {
        let uc =
            r#"{"elements":{"bus":{"1":{}}},"system":{"baseMVA":100.0,"time_keys":["1","2"]}}"#;
        let err = parse_egret_json(uc).unwrap_err();
        assert!(matches!(err, Error::FormatRead { .. }));
    }

    #[test]
    fn rejects_present_but_malformed_numeric_field() {
        // A present-but-non-numeric value must error, not silently default to 0.0
        // (which for a reactance would drop the branch from every matrix). Absent
        // fields still default, so the baseline parses.
        let base = r#"{"elements":{"bus":{"1":{"matpower_bustype":"ref"},
            "2":{"matpower_bustype":"PQ"}},"branch":{"1":{"from_bus":"1","to_bus":"2",
            "reactance":REACT}}},"system":{"baseMVA":100.0,"reference_bus":"1"}}"#;
        assert!(parse_egret_json(&base.replace("REACT", "0.1")).is_ok());
        let err = parse_egret_json(&base.replace("REACT", "\"oops\"")).unwrap_err();
        assert!(matches!(err, Error::FormatRead { .. }));
    }

    #[test]
    fn piecewise_cost_round_trips() {
        // The piecewise (model 1) path has its own (mw, cost) breakpoint layout,
        // distinct from the polynomial path, and no vendored fixture exercises it.
        // Round-trip it through cost_curve + read_cost so a transposed or dropped
        // breakpoint can't slip by.
        let cost = GenCost {
            model: 1,
            startup: 10.0,
            shutdown: 5.0,
            ncost: 3,
            coeffs: vec![0.0, 0.0, 50.0, 1000.0, 100.0, 2500.0],
        };
        let curve = cost_curve(&cost).expect("model 1 maps to a piecewise curve");
        let back = read_cost(&curve, 10.0, 5.0).expect("piecewise curve reads back");
        assert_eq!(back.model, 1);
        assert_eq!(back.ncost, 3);
        assert_eq!(back.coeffs, cost.coeffs);
        assert_eq!((back.startup, back.shutdown), (10.0, 5.0));
    }

    #[test]
    fn dc_branch_reads_every_power_field() {
        // dcline3.json leaves most dc_branch fields at their defaults, so pin the
        // full field-name → Hvdc mapping here; a swapped key (pmax read into pmin)
        // would otherwise ship silently.
        let v = serde_json::json!({
            "from_bus": "1", "to_bus": "2", "in_service": true,
            "pf": 10.0, "pt": -9.5, "qf": 1.5, "qt": -1.0,
            "vf": 1.02, "vt": 0.99, "pmin": -50.0, "pmax": 60.0,
            "qminf": -5.0, "qmaxf": 5.0, "qmint": -4.0, "qmaxt": 4.5,
            "loss0": 0.2, "loss_factor": 0.03
        });
        let row: DcBranchRow = serde_json::from_value(v).unwrap();
        let h = read_dc_branch(row).unwrap();
        assert_eq!((h.from, h.to), (BusId(1), BusId(2)));
        assert_eq!((h.pf, h.pt, h.qf, h.qt), (10.0, -9.5, 1.5, -1.0));
        assert_eq!((h.vf, h.vt), (1.02, 0.99));
        assert_eq!((h.pmin, h.pmax), (-50.0, 60.0));
        assert_eq!((h.qminf, h.qmaxf, h.qmint, h.qmaxt), (-5.0, 5.0, -4.0, 4.5));
        assert_eq!((h.loss0, h.loss1), (0.2, 0.03));
    }

    #[test]
    fn unrecognized_element_fields_are_preserved_as_extras() {
        // A field with no model slot must survive the read as extras.
        // Consumed fields and the writer's own `shunt_type` stamp must
        // stay out of extras.
        let doc = r#"{"elements":{
            "bus":{"1":{"matpower_bustype":"ref","vm":1.0,"vendor_ext":42},
                   "2":{"matpower_bustype":"PQ"}},
            "load":{"load_1":{"bus":"1","p_load":1.0,"q_load":0.5,"owner":"co-op"}},
            "shunt":{"shunt_1":{"bus":"1","gs":0.0,"bs":5.0,"shunt_type":"fixed"}},
            "branch":{"1":{"from_bus":"1","to_bus":"2","reactance":0.1,"pf":12.5}},
            "dc_branch":{"1":{"from_bus":"1","to_bus":"2","rating_long_term":30.0}}},
            "system":{"baseMVA":100.0,"reference_bus":"1"}}"#;
        let net = parse_egret_json(doc).unwrap();
        assert_eq!(
            net.buses()[0].extras.get("vendor_ext"),
            Some(&Value::from(42))
        );
        assert!(!net.buses()[0].extras.contains_key("vm"));
        assert_eq!(
            net.loads()[0].extras.get("owner"),
            Some(&Value::String("co-op".into()))
        );
        assert!(
            net.shunts()[0].extras.is_empty(),
            "{:?}",
            net.shunts()[0].extras
        );
        assert_eq!(net.branches()[0].extras.get("pf"), Some(&Value::from(12.5)));
        assert_eq!(
            net.hvdc()[0].extras.get("rating_long_term"),
            Some(&Value::from(30.0))
        );
    }

    #[test]
    fn rejects_present_but_malformed_cost() {
        // A present `p_cost` the reader can't interpret is an error, not a silently
        // free generator (cost dropped to None).
        let v = serde_json::json!({
            "bus": "1", "pg": 0.0, "qg": 0.0,
            "p_max": 1.0, "p_min": 0.0, "q_max": 1.0, "q_min": -1.0,
            "p_cost": {"data_type": "cost_curve", "cost_curve_type": "bogus", "values": {}}
        });
        let row: GenRow = serde_json::from_value(v).unwrap();
        assert!(matches!(read_gen(&row), Err(Error::FormatRead { .. })));
    }

    #[test]
    fn time_keys_produce_a_series_with_shared_static_tables() {
        let doc = r#"{
            "model_name": "uc2",
            "elements": {
                "bus": {"1": {"matpower_bustype": "ref", "base_kv": 138.0},
                        "2": {"matpower_bustype": "PQ", "base_kv": 138.0}},
                "load": {"load_1": {"bus": "2",
                    "p_load": {"data_type": "time_series", "values": [10.0, 20.0]},
                    "q_load": 3.0}},
                "generator": {"1": {"bus": "1", "pg": 12.0, "qg": 0.0,
                    "p_min": 0.0, "p_max": 50.0, "q_min": -10.0, "q_max": 10.0}},
                "branch": {"1": {"from_bus": "1", "to_bus": "2",
                    "resistance": 0.01, "reactance": 0.1, "charging_susceptance": 0.0,
                    "rating_long_term": 100.0, "rating_short_term": 100.0,
                    "rating_emergency": 100.0, "transformer_phase_shift": 0.0}}
            },
            "system": {"baseMVA": 100.0, "time_keys": ["t1", "t2"]}
        }"#;
        let series = parse_egret_time_series(doc, None).unwrap();
        assert_eq!(series.len(), 2);
        let first = &series.values()[0];
        let second = &series.values()[1];
        assert!((first.loads()[0].p - 10.0).abs() < f64::EPSILON);
        assert!((second.loads()[0].p - 20.0).abs() < f64::EPSILON);
        // The untouched static tables are the same allocation on every point;
        // only the varied load table was copied.
        assert!(std::ptr::eq(
            first.buses().as_ptr(),
            second.buses().as_ptr()
        ));
        assert!(std::ptr::eq(
            first.generators().as_ptr(),
            second.generators().as_ptr()
        ));
        assert!(!std::ptr::eq(
            first.loads().as_ptr(),
            second.loads().as_ptr()
        ));
        assert_eq!(series.time_points()[1].label(), "t2");
    }

    #[test]
    fn a_varying_attribute_outside_the_profile_is_refused() {
        let doc = r#"{
            "elements": {
                "bus": {"1": {"matpower_bustype": "ref", "base_kv": 1.0}},
                "load": {"load_1": {"bus": "1", "p_load": 1.0, "q_load": 0.0,
                    "area": {"data_type": "time_series", "values": [1.0, 2.0]}}}
            },
            "system": {"baseMVA": 100.0, "time_keys": ["a", "b"]}
        }"#;
        let error = parse_egret_time_series(doc, None).unwrap_err().to_string();
        assert!(
            error.contains("outside the supported scalar network profile"),
            "{error}"
        );
        assert!(error.contains("`area`"), "{error}");
    }

    #[test]
    fn a_series_length_disagreement_is_refused() {
        let doc = r#"{
            "elements": {
                "bus": {"1": {"matpower_bustype": "ref", "base_kv": 1.0}},
                "load": {"load_1": {"bus": "1",
                    "p_load": {"data_type": "time_series", "values": [1.0]},
                    "q_load": 0.0}}
            },
            "system": {"baseMVA": 100.0, "time_keys": ["a", "b"]}
        }"#;
        let error = parse_egret_time_series(doc, None).unwrap_err().to_string();
        assert!(error.contains("states 1 values for 2 time keys"), "{error}");
    }
}

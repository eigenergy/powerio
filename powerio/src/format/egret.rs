//! Read and write a [`Network`] as egret `ModelData` JSON.
//!
//! egret groups the network under `elements` (bus, load, branch, generator,
//! shunt, dc_branch) with a small `system` block; values stay in MW/MVAr,
//! degrees, with the base in `system.baseMVA`. Loads and shunts are first-class
//! on the `Network`, generator cost becomes a polynomial/piecewise `cost_curve`,
//! and a branch with a nonzero raw tap or a phase shift is typed `transformer`.
//!
//! The reader takes the power flow ModelData subset: numeric bus ids (as
//! matpower- and pglib-derived files have), scalar element values. Unit
//! commitment cases (`system.time_keys`, time-series values) are rejected. A
//! same format writes return the retained source like every other format.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::{Conversion, finish, jnum};
use crate::network::{
    Branch, Bus, BusId, BusType, Extras, GenCost, Generator, Hvdc, Load, Network, Shunt,
    SourceFormat,
};
use crate::{Error, Result};

const FMT: &str = "egret JSON";

#[must_use]
pub fn write_egret_json(net: &Network) -> Conversion {
    let mut warnings = Vec::new();

    let mut bus = Map::new();
    for b in &net.buses {
        bus.insert(b.id.to_string(), bus_obj(b));
    }

    // egret keys each load/shunt; use a global running suffix (load_1, load_2, …)
    // so several loads on one bus stay distinct.
    let mut load = Map::new();
    for (i, l) in net.loads.iter().enumerate() {
        load.insert(format!("load_{}", i + 1), load_obj(l));
    }
    let mut shunt = Map::new();
    for (i, s) in net.shunts.iter().enumerate() {
        shunt.insert(format!("shunt_{}", i + 1), shunt_obj(s));
    }

    let mut branch = Map::new();
    for (i, br) in net.branches.iter().enumerate() {
        branch.insert((i + 1).to_string(), branch_obj(br));
    }

    let mut generator = Map::new();
    for (i, g) in net.generators.iter().enumerate() {
        generator.insert((i + 1).to_string(), gen_obj(g, &mut warnings));
    }

    if !net.hvdc.is_empty() {
        warnings.push(format!(
            "{} dcline(s) dropped: egret HVDC mapping not implemented",
            net.hvdc.len()
        ));
    }
    if !net.storage.is_empty() {
        warnings.push(format!(
            "{} storage unit(s) dropped: egret storage mapping not implemented",
            net.storage.len()
        ));
    }

    let mut elements = Map::new();
    elements.insert("bus".into(), Value::Object(bus));
    elements.insert("load".into(), Value::Object(load));
    elements.insert("shunt".into(), Value::Object(shunt));
    elements.insert("branch".into(), Value::Object(branch));
    elements.insert("generator".into(), Value::Object(generator));

    let mut system = Map::new();
    system.insert("baseMVA".into(), jnum(net.base_mva));
    match reference_bus(net) {
        Some(r) => {
            system.insert("reference_bus".into(), Value::String(r.id.to_string()));
            system.insert("reference_bus_angle".into(), jnum(r.va));
        }
        None => warnings
            .push("no single reference bus (BusType::Ref); system.reference_bus omitted".into()),
    }

    let mut root = Map::new();
    root.insert("elements".into(), Value::Object(elements));
    root.insert("system".into(), Value::Object(system));

    finish(root, warnings)
}

fn reference_bus(net: &Network) -> Option<&Bus> {
    let mut refs = net.buses.iter().filter(|b| b.kind == BusType::Ref);
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
    m.insert("charging_susceptance".into(), jnum(br.b));
    m.insert("in_service".into(), Value::Bool(br.in_service));
    m.insert("angle_diff_min".into(), jnum(br.angmin));
    m.insert("angle_diff_max".into(), jnum(br.angmax));
    if br.is_transformer() {
        m.insert("branch_type".into(), Value::String("transformer".into()));
        m.insert("transformer_tap_ratio".into(), jnum(br.effective_tap()));
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

fn gen_obj(g: &Generator, warnings: &mut Vec<String>) -> Value {
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
        } else {
            warnings.push(format!(
                "generator at bus {} has a cost model egret's writer can't express; cost dropped",
                g.bus
            ));
        }
    }
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

/// Parse egret `ModelData` JSON into a [`Network`].
///
/// Inverts [`write_egret_json`]: the `elements` blocks map back to the typed
/// model and `system.baseMVA`/`reference_bus` to the base and bus types. Takes
/// the power flow subset (numeric bus ids, scalar values); a unit commitment
/// case (`system.time_keys`) is rejected with a clear error.
pub fn parse_egret_json(content: &str) -> Result<Network> {
    parse_egret_source(Arc::new(content.to_owned()), None)
}

/// Owned-source entry used by the format hub: parse by borrowing `source`, then
/// move the buffer into the retained source (no copy, byte-exact round-trip).
/// `name_hint` (e.g. a file stem) names the network when the JSON has no
/// `model_name`.
pub(crate) fn parse_egret_source(source: Arc<String>, name_hint: Option<&str>) -> Result<Network> {
    let content: &str = &source;
    let root: Value = serde_json::from_str(content).map_err(|e| bad(e.to_string()))?;
    let root = root
        .as_object()
        .ok_or_else(|| bad("top level is not a JSON object"))?;

    let system = obj(root, "system").ok_or_else(|| bad("missing `system` object"))?;
    if system.contains_key("time_keys") {
        return Err(bad(
            "egret unit commitment cases (system.time_keys) are not supported; expected a power flow ModelData",
        ));
    }
    let base_mva = system
        .get("baseMVA")
        .and_then(Value::as_f64)
        .ok_or_else(|| bad("missing numeric system.baseMVA"))?;
    let elements = obj(root, "elements").ok_or_else(|| bad("missing `elements` object"))?;
    let name = root
        .get("model_name")
        .and_then(Value::as_str)
        .or(name_hint)
        .unwrap_or("case")
        .to_string();

    let mut buses = Vec::new();
    if let Some(m) = obj(elements, "bus") {
        for (k, v) in sorted_kv(m) {
            buses.push(read_bus(k, v)?);
        }
    }
    let mut loads = Vec::new();
    if let Some(m) = obj(elements, "load") {
        for v in sorted_vals(m) {
            loads.push(read_load(v)?);
        }
    }
    let mut shunts = Vec::new();
    if let Some(m) = obj(elements, "shunt") {
        for v in sorted_vals(m) {
            shunts.push(read_shunt(v)?);
        }
    }
    let mut branches = Vec::new();
    if let Some(m) = obj(elements, "branch") {
        for v in sorted_vals(m) {
            branches.push(read_branch(v)?);
        }
    }
    let mut generators = Vec::new();
    if let Some(m) = obj(elements, "generator") {
        for v in sorted_vals(m) {
            generators.push(read_gen(v)?);
        }
    }
    let mut hvdc = Vec::new();
    if let Some(m) = obj(elements, "dc_branch") {
        for v in sorted_vals(m) {
            hvdc.push(read_dc_branch(v)?);
        }
    }

    let net = Network {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        buses,
        loads,
        shunts,
        branches,
        generators,
        storage: Vec::new(),
        hvdc,
        transformers_3w: Vec::new(),
        areas: Vec::new(),
        solver: None,
        source_format: SourceFormat::EgretJson,
        source: Some(source),
    };
    net.check_references(FMT)?;
    Ok(net)
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

fn sorted_vals(map: &Map<String, Value>) -> Vec<&Value> {
    sorted_kv(map).into_iter().map(|(_, v)| v).collect()
}

/// The trailing run of digits as an integer (`"5"` → 5, `"load_10"` → 10); a key
/// with no trailing digits sorts last. Scans bytes from the end, no allocation.
fn num_key(k: &str) -> i64 {
    let start = k.len() - k.bytes().rev().take_while(u8::is_ascii_digit).count();
    k[start..].parse::<i64>().unwrap_or(i64::MAX)
}

/// A non-negative integer bus id from an f64 (egret writes some ids as numbers).
/// Rejects negative, fractional, or out-of-range values rather than truncating or
/// wrapping them onto the wrong bus.
fn id_from_f64(x: f64) -> Option<usize> {
    // Strict `<`: `usize::MAX as f64` rounds up to 2^64, so values in the gap just
    // below it would pass `<=` and then saturate on the `as usize` cast.
    (x >= 0.0 && x.fract() == 0.0 && x < usize::MAX as f64).then_some(x as usize)
}

/// A bus id from a JSON value: a numeric string (egret's convention) or a bare
/// number. `None` for a non-integer, negative, or non-numeric value (named buses
/// aren't representable in the integer `BusId` space).
fn parse_id(v: &Value) -> Option<usize> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            s.parse::<usize>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().and_then(id_from_f64))
        }
        Value::Number(n) => n
            .as_u64()
            .map(|x| x as usize)
            .or_else(|| n.as_f64().and_then(id_from_f64)),
        _ => None,
    }
}

fn id_field(v: &Value, key: &str) -> Result<BusId> {
    let raw = v
        .get(key)
        .ok_or_else(|| bad(format!("element missing `{key}`")))?;
    parse_id(raw)
        .map(BusId)
        .ok_or_else(|| bad(format!("`{key}` is not a numeric bus id: {raw}")))
}

/// Field `key` as f64, `0.0` when absent. A present-but-non-numeric value is a
/// hard error, not a silent default. The PSS/E and PowerWorld
/// readers also hold, so a garbled number can't quietly become a plausible `0.0`
/// and corrupt the matrices downstream.
fn f(v: &Value, key: &str) -> Result<f64> {
    f_or(v, key, 0.0)
}
/// Field `key` as f64: absent or null ⇒ `default`, present but not a number ⇒ error.
fn f_or(v: &Value, key: &str, default: f64) -> Result<f64> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(x) => x
            .as_f64()
            .ok_or_else(|| bad(format!("`{key}` is not a number: {x}"))),
    }
}
/// Field `key` as usize, accepting a number or a numeric string (egret writes
/// `area`/`zone` as strings; its own parser writes them as numbers). Absent ⇒
/// `default`; present but not a non-negative integer ⇒ error.
fn usize_or(v: &Value, key: &str, default: usize) -> Result<usize> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(x) => {
            parse_id(x).ok_or_else(|| bad(format!("`{key}` is not a non-negative integer: {x}")))
        }
    }
}
/// Field `key` as bool: absent or null ⇒ `default`, present but not a bool ⇒ error.
fn flag(v: &Value, key: &str, default: bool) -> Result<bool> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(x) => Err(bad(format!("`{key}` is not a boolean: {x}"))),
    }
}

fn bustype_from_str(s: &str) -> BusType {
    match s {
        "PV" => BusType::Pv,
        "ref" => BusType::Ref,
        "isolated" => BusType::Isolated,
        _ => BusType::Pq,
    }
}

fn read_bus(key: &str, v: &Value) -> Result<Bus> {
    let id = key
        .trim()
        .parse::<usize>()
        .map_err(|_| bad(format!("bus key is not a numeric id: {key:?}")))?;
    Ok(Bus {
        id: BusId(id),
        kind: bustype_from_str(
            v.get("matpower_bustype")
                .and_then(Value::as_str)
                .unwrap_or("PQ"),
        ),
        vm: f_or(v, "vm", 1.0)?,
        va: f(v, "va")?,
        base_kv: f(v, "base_kv")?,
        vmax: f_or(v, "v_max", 1.1)?,
        vmin: f_or(v, "v_min", 0.9)?,
        area: usize_or(v, "area", 0)?,
        zone: usize_or(v, "zone", 0)?,
        name: v.get("name").and_then(Value::as_str).map(str::to_string),
        extras: Extras::new(),
    })
}

fn read_load(v: &Value) -> Result<Load> {
    Ok(Load {
        bus: id_field(v, "bus")?,
        p: f(v, "p_load")?,
        q: f(v, "q_load")?,
        in_service: flag(v, "in_service", true)?,
        extras: Extras::new(),
    })
}

fn read_shunt(v: &Value) -> Result<Shunt> {
    Ok(Shunt {
        bus: id_field(v, "bus")?,
        g: f(v, "gs")?,
        b: f(v, "bs")?,
        in_service: flag(v, "in_service", true)?,
        control: None,
        extras: Extras::new(),
    })
}

fn read_branch(v: &Value) -> Result<Branch> {
    let is_xf = v.get("branch_type").and_then(Value::as_str) == Some("transformer");
    Ok(Branch {
        from: id_field(v, "from_bus")?,
        to: id_field(v, "to_bus")?,
        r: f(v, "resistance")?,
        x: f(v, "reactance")?,
        b: f(v, "charging_susceptance")?,
        rate_a: f(v, "rating_long_term")?,
        rate_b: f(v, "rating_short_term")?,
        rate_c: f(v, "rating_emergency")?,
        tap: if is_xf {
            f_or(v, "transformer_tap_ratio", 1.0)?
        } else {
            0.0
        },
        shift: f(v, "transformer_phase_shift")?,
        in_service: flag(v, "in_service", true)?,
        angmin: f_or(v, "angle_diff_min", -360.0)?,
        angmax: f_or(v, "angle_diff_max", 360.0)?,
        control: None,
        extras: Extras::new(),
    })
}

fn read_gen(v: &Value) -> Result<Generator> {
    let startup = f_or(v, "startup_cost", 0.0)?;
    let shutdown = f_or(v, "shutdown_cost", 0.0)?;
    // A present `p_cost` that doesn't parse is a hard error, not a silent drop:
    // the same stance the scalar field helpers take, so a malformed cost curve
    // can't quietly become a free generator.
    let cost = match v.get("p_cost") {
        None | Some(Value::Null) => None,
        Some(pc) => Some(read_cost(pc, startup, shutdown).ok_or_else(|| {
            bad("`p_cost` is present but has an unrecognized or malformed cost_curve")
        })?),
    };
    Ok(Generator {
        bus: id_field(v, "bus")?,
        pg: f(v, "pg")?,
        qg: f(v, "qg")?,
        pmax: f(v, "p_max")?,
        pmin: f(v, "p_min")?,
        qmax: f(v, "q_max")?,
        qmin: f(v, "q_min")?,
        vg: f_or(v, "vg", 1.0)?,
        mbase: f_or(v, "mbase", 100.0)?,
        in_service: flag(v, "in_service", true)?,
        cost,
        caps: Default::default(),
        regulated_bus: None,
    })
}

fn read_dc_branch(v: &Value) -> Result<Hvdc> {
    Ok(Hvdc {
        from: id_field(v, "from_bus")?,
        to: id_field(v, "to_bus")?,
        in_service: flag(v, "in_service", true)?,
        pf: f(v, "pf")?,
        pt: f(v, "pt")?,
        qf: f(v, "qf")?,
        qt: f(v, "qt")?,
        vf: f_or(v, "vf", 1.0)?,
        vt: f_or(v, "vt", 1.0)?,
        pmin: f(v, "pmin")?,
        pmax: f(v, "pmax")?,
        qminf: f(v, "qminf")?,
        qmaxf: f(v, "qmaxf")?,
        qmint: f(v, "qmint")?,
        qmaxt: f(v, "qmaxt")?,
        loss0: f(v, "loss0")?,
        loss1: f_or(v, "loss_factor", 0.0)?,
        extras: Extras::new(),
    })
}

/// egret `p_cost` → [`GenCost`]. Polynomial `{exp: coeff}` becomes the
/// highest-order-first coefficient vector (gaps filled with zeros); piecewise
/// `[[p, c], ...]` becomes the flat `(mw, cost)` breakpoints.
fn read_cost(p_cost: &Value, startup: f64, shutdown: f64) -> Option<GenCost> {
    let m = p_cost.as_object()?;
    match m.get("cost_curve_type").and_then(Value::as_str)? {
        "polynomial" => {
            let values = m.get("values")?.as_object()?;
            let pairs: Vec<(usize, f64)> = values
                .iter()
                .filter_map(|(k, c)| Some((k.parse().ok()?, c.as_f64()?)))
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
    use super::*;
    use crate::network::BusType;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/egret")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn reads_buses_loads_branches_and_reference() {
        let net = parse_egret_json(&fixture("case30.json")).unwrap();
        assert!((net.base_mva - 100.0).abs() < 1e-9);
        assert_eq!(net.buses.len(), 30);
        assert_eq!(net.loads.len(), 20);
        assert_eq!(net.shunts.len(), 2);
        assert_eq!(net.branches.len(), 41);
        assert_eq!(net.generators.len(), 6);
        // Exactly one reference bus, parsed from matpower_bustype.
        let refs = net.buses.iter().filter(|b| b.kind == BusType::Ref).count();
        assert_eq!(refs, 1);
    }

    #[test]
    fn inverts_transformer_and_polynomial_cost() {
        let net = parse_egret_json(&fixture("case14.json")).unwrap();
        // case14 has tap-changing transformers (raw tap != 0 ⇒ is_transformer).
        assert!(net.branches.iter().any(Branch::is_transformer));
        // Generators carry a polynomial cost, highest order first.
        let cost = net
            .generators
            .iter()
            .find_map(|g| g.cost.as_ref())
            .expect("a generator cost");
        assert_eq!(cost.model, 2);
        assert_eq!(cost.coeffs.len(), cost.ncost);
    }

    #[test]
    fn maps_dc_branch_to_hvdc() {
        let net = parse_egret_json(&fixture("dcline3.json")).unwrap();
        assert_eq!(net.hvdc.len(), 1);
        let dc = &net.hvdc[0];
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
        let h = read_dc_branch(&v).unwrap();
        assert_eq!((h.from, h.to), (BusId(1), BusId(2)));
        assert_eq!((h.pf, h.pt, h.qf, h.qt), (10.0, -9.5, 1.5, -1.0));
        assert_eq!((h.vf, h.vt), (1.02, 0.99));
        assert_eq!((h.pmin, h.pmax), (-50.0, 60.0));
        assert_eq!((h.qminf, h.qmaxf, h.qmint, h.qmaxt), (-5.0, 5.0, -4.0, 4.5));
        assert_eq!((h.loss0, h.loss1), (0.2, 0.03));
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
        assert!(matches!(read_gen(&v), Err(Error::FormatRead { .. })));
    }
}

//! Write a [`Network`] as PowerModels.jl network data JSON.
//!
//! Output is the PowerModels data model with `per_unit = false`: powers stay in
//! MW/MVAr and PowerModels per-unitizes on load, exactly as it does for the `.m`
//! it parses, so `parse_file(out.json)` and `parse_file(case.m)` land on the same
//! network. Angles stay in degrees (PowerModels' matpower loader, keyed by
//! `source_type`, converts them). Loads and shunts are already first-class on the
//! `Network`, line charging `b` splits half to each end, and a branch with a
//! nonzero raw tap or a phase shift is marked `transformer`. `hvdc`/`storage` are
//! mapped best-effort — PowerModels' `.m` parser derives loss-adjusted dcline
//! bounds that aren't reproduced here — and a warning is emitted when present.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::{finish, jnum, Conversion};
use crate::case::{BusType, GenCost};
use crate::network::{Branch, Bus, Generator, Hvdc, Load, Network, Shunt, SourceFormat, Storage};
use crate::{Error, Result};

/// PowerModels gen capability fields, in their conventional order. Emitted from
/// the generator's extras when present (a row may stop at PMIN).
const GEN_EXTRA_KEYS: [&str; 11] = [
    "pc1", "pc2", "qc1min", "qc1max", "qc2min", "qc2max", "ramp_agc", "ramp_10",
    "ramp_30", "ramp_q", "apf",
];

#[must_use]
pub fn write_powermodels_json(net: &Network) -> Conversion {
    let mut warnings = Vec::new();

    let mut bus = Map::new();
    for b in &net.buses {
        bus.insert(b.id.to_string(), bus_obj(b));
    }

    let mut branch = Map::new();
    for (i, br) in net.branches.iter().enumerate() {
        let idx = i + 1;
        branch.insert(idx.to_string(), branch_obj(br, idx));
    }

    let mut gen = Map::new();
    for (i, g) in net.generators.iter().enumerate() {
        let idx = i + 1;
        gen.insert(idx.to_string(), gen_obj(g, idx));
    }

    let mut load = Map::new();
    for (i, l) in net.loads.iter().enumerate() {
        let idx = i + 1;
        load.insert(idx.to_string(), load_obj(l, idx));
    }
    let mut shunt = Map::new();
    for (i, s) in net.shunts.iter().enumerate() {
        let idx = i + 1;
        shunt.insert(idx.to_string(), shunt_obj(s, idx));
    }

    let mut dcline = Map::new();
    for (i, dc) in net.hvdc.iter().enumerate() {
        let idx = i + 1;
        dcline.insert(idx.to_string(), dcline_obj(dc, idx));
    }
    let mut storage = Map::new();
    for (i, st) in net.storage.iter().enumerate() {
        let idx = i + 1;
        storage.insert(idx.to_string(), storage_obj(st, idx));
    }
    if !dcline.is_empty() {
        warnings.push(format!(
            "{} dcline(s) mapped best-effort; PowerModels' loss-adjusted flow bounds are not derived",
            dcline.len()
        ));
    }
    if !storage.is_empty() {
        warnings.push(format!(
            "{} storage unit(s) mapped best-effort to the PowerModels storage schema",
            storage.len()
        ));
    }

    let mut root = Map::new();
    root.insert("name".into(), Value::String(net.name.clone()));
    root.insert("baseMVA".into(), jnum(net.base_mva));
    root.insert("per_unit".into(), Value::Bool(false));
    root.insert("source_type".into(), Value::String("matpower".into()));
    root.insert("source_version".into(), Value::String("2".into()));
    root.insert("bus".into(), Value::Object(bus));
    root.insert("branch".into(), Value::Object(branch));
    root.insert("gen".into(), Value::Object(gen));
    root.insert("load".into(), Value::Object(load));
    root.insert("shunt".into(), Value::Object(shunt));
    root.insert("dcline".into(), Value::Object(dcline));
    root.insert("storage".into(), Value::Object(storage));
    root.insert("switch".into(), Value::Object(Map::new()));

    finish(root, warnings)
}

/// PowerModels back-reference `["bus"|"branch"|…, index]`.
fn source_id(kind: &str, idx: usize) -> Value {
    Value::Array(vec![Value::String(kind.into()), Value::from(idx as u64)])
}

fn status_int(in_service: bool) -> Value {
    Value::from(u64::from(in_service))
}

fn bus_obj(b: &Bus) -> Value {
    let mut m = Map::new();
    m.insert("bus_i".into(), Value::from(b.id as u64));
    m.insert("index".into(), Value::from(b.id as u64));
    m.insert("bus_type".into(), Value::from(u64::from(b.kind as u8)));
    m.insert("vm".into(), jnum(b.vm));
    m.insert("va".into(), jnum(b.va));
    m.insert("vmax".into(), jnum(b.vmax));
    m.insert("vmin".into(), jnum(b.vmin));
    m.insert("base_kv".into(), jnum(b.base_kv));
    m.insert("area".into(), Value::from(b.area as u64));
    m.insert("zone".into(), Value::from(b.zone as u64));
    if let Some(name) = &b.name {
        m.insert("name".into(), Value::String(name.clone()));
    }
    m.insert("source_id".into(), source_id("bus", b.id));
    Value::Object(m)
}

fn branch_obj(br: &Branch, idx: usize) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("f_bus".into(), Value::from(br.from as u64));
    m.insert("t_bus".into(), Value::from(br.to as u64));
    m.insert("br_r".into(), jnum(br.r));
    m.insert("br_x".into(), jnum(br.x));
    // MATPOWER's single line-charging `b` splits half to each end; no branch `g`.
    m.insert("b_fr".into(), jnum(br.b / 2.0));
    m.insert("b_to".into(), jnum(br.b / 2.0));
    m.insert("g_fr".into(), jnum(0.0));
    m.insert("g_to".into(), jnum(0.0));
    m.insert("tap".into(), jnum(br.effective_tap()));
    m.insert("shift".into(), jnum(br.shift));
    m.insert("br_status".into(), status_int(br.in_service));
    m.insert("angmin".into(), jnum(br.angmin));
    m.insert("angmax".into(), jnum(br.angmax));
    m.insert("transformer".into(), Value::Bool(br.is_transformer()));
    // PowerModels omits a rate when it is 0 (unlimited).
    if br.rate_a != 0.0 {
        m.insert("rate_a".into(), jnum(br.rate_a));
    }
    if br.rate_b != 0.0 {
        m.insert("rate_b".into(), jnum(br.rate_b));
    }
    if br.rate_c != 0.0 {
        m.insert("rate_c".into(), jnum(br.rate_c));
    }
    m.insert("source_id".into(), source_id("branch", idx));
    Value::Object(m)
}

fn gen_obj(g: &Generator, idx: usize) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("gen_bus".into(), Value::from(g.bus as u64));
    m.insert("pg".into(), jnum(g.pg));
    m.insert("qg".into(), jnum(g.qg));
    m.insert("qmax".into(), jnum(g.qmax));
    m.insert("qmin".into(), jnum(g.qmin));
    m.insert("vg".into(), jnum(g.vg));
    m.insert("mbase".into(), jnum(g.mbase));
    m.insert("gen_status".into(), status_int(g.in_service));
    m.insert("pmax".into(), jnum(g.pmax));
    m.insert("pmin".into(), jnum(g.pmin));
    // Gen capability columns, in PowerModels' field order, for those present.
    for key in GEN_EXTRA_KEYS {
        if let Some(v) = g.extras.get(key) {
            m.insert(key.into(), v.clone());
        }
    }
    if let Some(cost) = &g.cost {
        m.insert("model".into(), Value::from(u64::from(cost.model)));
        m.insert("ncost".into(), Value::from(cost.ncost as u64));
        m.insert("startup".into(), jnum(cost.startup));
        m.insert("shutdown".into(), jnum(cost.shutdown));
        m.insert(
            "cost".into(),
            Value::Array(cost.coeffs.iter().map(|&c| jnum(c)).collect()),
        );
    }
    m.insert("source_id".into(), source_id("gen", idx));
    Value::Object(m)
}

fn load_obj(l: &Load, idx: usize) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("load_bus".into(), Value::from(l.bus as u64));
    m.insert("pd".into(), jnum(l.p));
    m.insert("qd".into(), jnum(l.q));
    m.insert("status".into(), status_int(l.in_service));
    m.insert("source_id".into(), source_id("bus", l.bus));
    Value::Object(m)
}

fn shunt_obj(s: &Shunt, idx: usize) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("shunt_bus".into(), Value::from(s.bus as u64));
    m.insert("gs".into(), jnum(s.g));
    m.insert("bs".into(), jnum(s.b));
    m.insert("status".into(), status_int(s.in_service));
    m.insert("source_id".into(), source_id("bus", s.bus));
    Value::Object(m)
}

fn dcline_obj(dc: &Hvdc, idx: usize) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("f_bus".into(), Value::from(dc.from as u64));
    m.insert("t_bus".into(), Value::from(dc.to as u64));
    m.insert("br_status".into(), status_int(dc.in_service));
    m.insert("pf".into(), jnum(dc.pf));
    m.insert("pt".into(), jnum(dc.pt));
    m.insert("qf".into(), jnum(dc.qf));
    m.insert("qt".into(), jnum(dc.qt));
    m.insert("vf".into(), jnum(dc.vf));
    m.insert("vt".into(), jnum(dc.vt));
    // Original active-power bounds; PowerModels names these `mp_*` and derives
    // loss-adjusted per-end bounds we do not reproduce.
    m.insert("mp_pmin".into(), jnum(dc.pmin));
    m.insert("mp_pmax".into(), jnum(dc.pmax));
    m.insert("qminf".into(), jnum(dc.qminf));
    m.insert("qmaxf".into(), jnum(dc.qmaxf));
    m.insert("qmint".into(), jnum(dc.qmint));
    m.insert("qmaxt".into(), jnum(dc.qmaxt));
    m.insert("loss0".into(), jnum(dc.loss0));
    m.insert("loss1".into(), jnum(dc.loss1));
    m.insert("source_id".into(), source_id("dcline", idx));
    Value::Object(m)
}

fn storage_obj(st: &Storage, idx: usize) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("storage_bus".into(), Value::from(st.bus as u64));
    m.insert("ps".into(), jnum(st.ps));
    m.insert("qs".into(), jnum(st.qs));
    m.insert("energy".into(), jnum(st.energy));
    m.insert("energy_rating".into(), jnum(st.energy_rating));
    m.insert("charge_rating".into(), jnum(st.charge_rating));
    m.insert("discharge_rating".into(), jnum(st.discharge_rating));
    m.insert("charge_efficiency".into(), jnum(st.charge_efficiency));
    m.insert("discharge_efficiency".into(), jnum(st.discharge_efficiency));
    m.insert("thermal_rating".into(), jnum(st.thermal_rating));
    m.insert("qmin".into(), jnum(st.qmin));
    m.insert("qmax".into(), jnum(st.qmax));
    m.insert("r".into(), jnum(st.r));
    m.insert("x".into(), jnum(st.x));
    m.insert("p_loss".into(), jnum(st.p_loss));
    m.insert("q_loss".into(), jnum(st.q_loss));
    m.insert("status".into(), status_int(st.in_service));
    m.insert("source_id".into(), source_id("storage", idx));
    Value::Object(m)
}

// ---- Reader: PowerModels JSON → Network -------------------------------------

const FMT: &str = "PowerModels JSON";

/// Parse PowerModels.jl network data JSON into a [`Network`]. Loads and shunts
/// are read as first-class elements and the raw text is retained, so writing back
/// to PowerModels JSON is a byte-exact echo. `per_unit = true` input is converted
/// to the neutral MW/degree convention (powers ×baseMVA, angles to degrees, cost
/// coefficients un-scaled); `per_unit = false` (caseio's own output) is read as-is.
pub fn parse_powermodels_json(content: &str) -> Result<Network> {
    let root: Value = serde_json::from_str(content)
        .map_err(|e| Error::FormatRead { format: FMT, message: e.to_string() })?;
    let root = root.as_object().ok_or_else(|| Error::FormatRead {
        format: FMT,
        message: "top level is not a JSON object".into(),
    })?;

    let base_mva = root.get("baseMVA").and_then(Value::as_f64).ok_or_else(|| {
        Error::FormatRead { format: FMT, message: "missing numeric `baseMVA`".into() }
    })?;
    let per_unit = root.get("per_unit").and_then(Value::as_bool).unwrap_or(false);
    let pscale = if per_unit { base_mva } else { 1.0 };
    let ascale = if per_unit { 180.0 / std::f64::consts::PI } else { 1.0 };
    let name = root.get("name").and_then(Value::as_str).unwrap_or("case").to_string();

    Ok(Network {
        name,
        base_mva,
        buses: sorted(root, "bus", "index").iter().map(|v| read_bus(v, ascale)).collect(),
        loads: sorted(root, "load", "index").iter().map(|v| read_load(v, pscale)).collect(),
        shunts: sorted(root, "shunt", "index").iter().map(|v| read_shunt(v, pscale)).collect(),
        branches: sorted(root, "branch", "index").iter().map(|v| read_branch(v, pscale, ascale)).collect(),
        generators: sorted(root, "gen", "index").iter().map(|v| read_gen(v, pscale, base_mva, per_unit)).collect(),
        storage: sorted(root, "storage", "index").iter().map(|v| read_storage(v, pscale)).collect(),
        hvdc: sorted(root, "dcline", "index").iter().map(|v| read_hvdc(v, pscale)).collect(),
        source_format: SourceFormat::PowerModelsJson,
        source: Some(Arc::from(content)),
    })
}

/// Elements of a top-level section, ordered by their integer `idx_key` so a
/// re-emitted file assigns the same running keys.
fn sorted<'a>(root: &'a Map<String, Value>, section: &str, idx_key: &str) -> Vec<&'a Value> {
    let Some(obj) = root.get(section).and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut items: Vec<&Value> = obj.values().collect();
    items.sort_by_key(|v| v.get(idx_key).and_then(Value::as_i64).unwrap_or(0));
    items
}

fn f(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}
fn f_or(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(default)
}
fn uid(v: &Value, key: &str) -> usize {
    v.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}
/// A 0/1 status field; absent ⇒ in service.
fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_f64) != Some(0.0)
}

fn bustype(code: i64) -> BusType {
    match code {
        2 => BusType::Pv,
        3 => BusType::Ref,
        4 => BusType::Isolated,
        _ => BusType::Pq,
    }
}

/// Element keys the neutral model names directly are dropped here; whatever's left
/// is preserved as extras for round-trip and cross-format passthrough.
fn extras_excluding(v: &Value, known: &[&str]) -> crate::network::Extras {
    v.as_object().map_or_else(Default::default, |obj| {
        obj.iter()
            .filter(|(k, _)| !known.contains(&k.as_str()))
            .map(|(k, val)| (k.clone(), val.clone()))
            .collect()
    })
}

fn read_bus(v: &Value, ascale: f64) -> Bus {
    Bus {
        id: uid(v, "bus_i"),
        kind: bustype(v.get("bus_type").and_then(Value::as_i64).unwrap_or(1)),
        vm: f_or(v, "vm", 1.0),
        va: f(v, "va") * ascale,
        base_kv: f(v, "base_kv"),
        vmax: f(v, "vmax"),
        vmin: f(v, "vmin"),
        area: uid(v, "area"),
        zone: uid(v, "zone"),
        name: v.get("name").and_then(Value::as_str).map(str::to_string),
        extras: extras_excluding(
            v,
            &["bus_i", "index", "bus_type", "vm", "va", "vmax", "vmin", "base_kv", "area", "zone", "name", "source_id"],
        ),
    }
}

fn read_load(v: &Value, pscale: f64) -> Load {
    Load {
        bus: uid(v, "load_bus"),
        p: f(v, "pd") * pscale,
        q: f(v, "qd") * pscale,
        in_service: flag(v, "status"),
        extras: extras_excluding(v, &["load_bus", "pd", "qd", "status", "index", "source_id"]),
    }
}

fn read_shunt(v: &Value, pscale: f64) -> Shunt {
    Shunt {
        bus: uid(v, "shunt_bus"),
        g: f(v, "gs") * pscale,
        b: f(v, "bs") * pscale,
        in_service: flag(v, "status"),
        extras: extras_excluding(v, &["shunt_bus", "gs", "bs", "status", "index", "source_id"]),
    }
}

fn read_branch(v: &Value, pscale: f64, ascale: f64) -> Branch {
    // PowerModels stores the effective tap (1.0 for a line); the `transformer`
    // flag disambiguates an explicit-tap transformer from a line, which is what
    // the neutral raw-tap convention (0 = line) needs.
    let transformer = v.get("transformer").and_then(Value::as_bool).unwrap_or(false);
    let tap = if transformer { f_or(v, "tap", 1.0) } else { 0.0 };
    Branch {
        from: uid(v, "f_bus"),
        to: uid(v, "t_bus"),
        r: f(v, "br_r"),
        x: f(v, "br_x"),
        b: f(v, "b_fr") + f(v, "b_to"),
        rate_a: f(v, "rate_a") * pscale,
        rate_b: f(v, "rate_b") * pscale,
        rate_c: f(v, "rate_c") * pscale,
        tap,
        shift: f(v, "shift") * ascale,
        in_service: flag(v, "br_status"),
        angmin: f(v, "angmin") * ascale,
        angmax: f(v, "angmax") * ascale,
        extras: extras_excluding(
            v,
            &["f_bus", "t_bus", "br_r", "br_x", "b_fr", "b_to", "g_fr", "g_to", "tap", "shift", "br_status", "angmin", "angmax", "transformer", "rate_a", "rate_b", "rate_c", "index", "source_id"],
        ),
    }
}

fn read_gen(v: &Value, pscale: f64, base_mva: f64, per_unit: bool) -> Generator {
    let mut extras = crate::network::Extras::new();
    for key in GEN_EXTRA_KEYS {
        if let Some(val) = v.get(key).and_then(Value::as_f64) {
            // apf is dimensionless; the rest are powers.
            let scaled = if key == "apf" { val } else { val * pscale };
            extras.insert(key.to_string(), jnum(scaled));
        }
    }
    let cost = v.get("model").map(|_| read_cost(v, base_mva, per_unit));
    Generator {
        bus: uid(v, "gen_bus"),
        pg: f(v, "pg") * pscale,
        qg: f(v, "qg") * pscale,
        pmax: f(v, "pmax") * pscale,
        pmin: f(v, "pmin") * pscale,
        qmax: f(v, "qmax") * pscale,
        qmin: f(v, "qmin") * pscale,
        vg: f_or(v, "vg", 1.0),
        mbase: f_or(v, "mbase", base_mva),
        in_service: flag(v, "gen_status"),
        cost,
        extras,
    }
}

fn read_cost(v: &Value, base_mva: f64, per_unit: bool) -> GenCost {
    let coeffs_raw: Vec<f64> = v
        .get("cost")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    // PowerModels per-unit scales coeff i (of p^(k-1-i)) by baseMVA^(k-1-i);
    // undo it to land on the neutral MW basis.
    let k = coeffs_raw.len();
    let coeffs = if per_unit {
        coeffs_raw
            .iter()
            .enumerate()
            .map(|(i, &c)| c / base_mva.powf((k - 1 - i) as f64))
            .collect()
    } else {
        coeffs_raw
    };
    GenCost {
        model: v.get("model").and_then(Value::as_u64).unwrap_or(2) as u8,
        startup: f(v, "startup"),
        shutdown: f(v, "shutdown"),
        ncost: v.get("ncost").and_then(Value::as_u64).unwrap_or(k as u64) as usize,
        coeffs,
    }
}

fn read_hvdc(v: &Value, pscale: f64) -> Hvdc {
    Hvdc {
        from: uid(v, "f_bus"),
        to: uid(v, "t_bus"),
        in_service: flag(v, "br_status"),
        pf: f(v, "pf") * pscale,
        pt: f(v, "pt") * pscale,
        qf: f(v, "qf") * pscale,
        qt: f(v, "qt") * pscale,
        vf: f_or(v, "vf", 1.0),
        vt: f_or(v, "vt", 1.0),
        pmin: f(v, "mp_pmin") * pscale,
        pmax: f(v, "mp_pmax") * pscale,
        qminf: f(v, "qminf") * pscale,
        qmaxf: f(v, "qmaxf") * pscale,
        qmint: f(v, "qmint") * pscale,
        qmaxt: f(v, "qmaxt") * pscale,
        loss0: f(v, "loss0") * pscale,
        loss1: f(v, "loss1"),
        extras: extras_excluding(
            v,
            &["f_bus", "t_bus", "br_status", "pf", "pt", "qf", "qt", "vf", "vt", "mp_pmin", "mp_pmax", "qminf", "qmaxf", "qmint", "qmaxt", "loss0", "loss1", "index", "source_id"],
        ),
    }
}

fn read_storage(v: &Value, pscale: f64) -> Storage {
    Storage {
        bus: uid(v, "storage_bus"),
        ps: f(v, "ps") * pscale,
        qs: f(v, "qs") * pscale,
        energy: f(v, "energy") * pscale,
        energy_rating: f(v, "energy_rating") * pscale,
        charge_rating: f(v, "charge_rating") * pscale,
        discharge_rating: f(v, "discharge_rating") * pscale,
        charge_efficiency: f_or(v, "charge_efficiency", 1.0),
        discharge_efficiency: f_or(v, "discharge_efficiency", 1.0),
        thermal_rating: f(v, "thermal_rating") * pscale,
        qmin: f(v, "qmin") * pscale,
        qmax: f(v, "qmax") * pscale,
        r: f(v, "r"),
        x: f(v, "x"),
        p_loss: f(v, "p_loss") * pscale,
        q_loss: f(v, "q_loss") * pscale,
        in_service: flag(v, "status"),
        extras: extras_excluding(
            v,
            &["storage_bus", "ps", "qs", "energy", "energy_rating", "charge_rating", "discharge_rating", "charge_efficiency", "discharge_efficiency", "thermal_rating", "qmin", "qmax", "r", "x", "p_loss", "q_loss", "status", "index", "source_id"],
        ),
    }
}

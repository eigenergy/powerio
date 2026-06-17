//! Write a [`Network`] as PowerModels.jl network data JSON.
//!
//! Output is idiomatic PowerModels data with `per_unit = true`, the same form
//! PowerModels itself exports: powers are divided by `baseMVA`, angles are in
//! radians, and gen cost coefficients are rescaled to the per-unit basis (a
//! polynomial term `p^j` by `baseMVA^j`, a piecewise curve's MW breakpoints by
//! `1/baseMVA`). Because the data already declares per unit, `parse_file(out.json)`
//! reads it with PowerModels' default `validate = true` without rerunning
//! `make_per_unit!`, so it lands on the same network as `parse_file(case.m)`.
//! Loads and shunts are first-class on the `Network`, line charging `b` splits
//! half to each end, and `transformer` follows PowerModels' rule (raw tap `≠ 0`).
//! `hvdc`/`storage` are mapped to the closest PowerModels blocks and emit a
//! warning when present.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::{Conversion, finish, jnum};
use crate::network::{
    Branch, Bus, BusId, BusType, GEN_EXTRA_KEYS, GenCost, Generator, Hvdc, Load, Network, Shunt,
    SourceFormat, Storage,
};
use crate::normalize::{self, GEN_PU_KEYS};
use crate::{Error, Result};

#[must_use]
pub fn write_powermodels_json(net: &Network) -> Conversion {
    let mut warnings = Vec::new();

    // Per-unit write factors, the exact inverse of the reader's pscale/ascale:
    // powers ÷ baseMVA, angles degrees → radians. Cost rescale needs the base.
    let base = net.base_mva;
    let p = 1.0 / base;
    let a = normalize::DEG_TO_RAD;

    let mut bus = Map::new();
    for b in &net.buses {
        bus.insert(b.id.to_string(), bus_obj(b, a));
    }

    let mut branch = Map::new();
    for (i, br) in net.branches.iter().enumerate() {
        let idx = i + 1;
        branch.insert(idx.to_string(), branch_obj(br, idx, p, a));
    }

    let mut gen_map = Map::new();
    for (i, g) in net.generators.iter().enumerate() {
        let idx = i + 1;
        gen_map.insert(idx.to_string(), gen_obj(g, idx, p, base));
    }

    let mut load = Map::new();
    for (i, l) in net.loads.iter().enumerate() {
        let idx = i + 1;
        load.insert(idx.to_string(), load_obj(l, idx, p));
    }
    let mut shunt = Map::new();
    for (i, s) in net.shunts.iter().enumerate() {
        let idx = i + 1;
        shunt.insert(idx.to_string(), shunt_obj(s, idx, p));
    }

    let mut dcline = Map::new();
    for (i, dc) in net.hvdc.iter().enumerate() {
        let idx = i + 1;
        dcline.insert(idx.to_string(), dcline_obj(dc, idx, p));
    }
    let mut storage = Map::new();
    for (i, st) in net.storage.iter().enumerate() {
        let idx = i + 1;
        storage.insert(idx.to_string(), storage_obj(st, idx, p));
    }
    if !dcline.is_empty() {
        warnings.push(format!(
            "{} dcline(s) mapped with warnings to the PowerModels dcline schema",
            dcline.len()
        ));
    }
    if !storage.is_empty() {
        warnings.push(format!(
            "{} storage unit(s) mapped with warnings to the PowerModels storage schema",
            storage.len()
        ));
    }

    let mut root = Map::new();
    root.insert("name".into(), Value::String(net.name.clone()));
    root.insert("baseMVA".into(), jnum(net.base_mva));
    root.insert("per_unit".into(), Value::Bool(true));
    root.insert("source_type".into(), Value::String("matpower".into()));
    root.insert("source_version".into(), Value::String("2".into()));
    root.insert("bus".into(), Value::Object(bus));
    root.insert("branch".into(), Value::Object(branch));
    root.insert("gen".into(), Value::Object(gen_map));
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

fn bus_obj(b: &Bus, a: f64) -> Value {
    let mut m = Map::new();
    m.insert("bus_i".into(), Value::from(b.id.0 as u64));
    m.insert("index".into(), Value::from(b.id.0 as u64));
    m.insert("bus_type".into(), Value::from(u64::from(b.kind as u8)));
    m.insert("vm".into(), jnum(b.vm));
    m.insert("va".into(), jnum(b.va * a));
    m.insert("vmax".into(), jnum(b.vmax));
    m.insert("vmin".into(), jnum(b.vmin));
    m.insert("base_kv".into(), jnum(b.base_kv));
    m.insert("area".into(), Value::from(b.area as u64));
    m.insert("zone".into(), Value::from(b.zone as u64));
    if let Some(name) = &b.name {
        m.insert("name".into(), Value::String(name.clone()));
    }
    m.insert("source_id".into(), source_id("bus", b.id.0));
    Value::Object(m)
}

fn branch_obj(br: &Branch, idx: usize, p: f64, a: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("f_bus".into(), Value::from(br.from.0 as u64));
    m.insert("t_bus".into(), Value::from(br.to.0 as u64));
    m.insert("br_r".into(), jnum(br.r));
    m.insert("br_x".into(), jnum(br.x));
    // MATPOWER's single line-charging `b` splits half to each end; no branch `g`.
    m.insert("b_fr".into(), jnum(br.b / 2.0));
    m.insert("b_to".into(), jnum(br.b / 2.0));
    m.insert("g_fr".into(), jnum(0.0));
    m.insert("g_to".into(), jnum(0.0));
    m.insert("tap".into(), jnum(br.effective_tap()));
    m.insert("shift".into(), jnum(br.shift * a));
    m.insert("br_status".into(), status_int(br.in_service));
    m.insert("angmin".into(), jnum(br.angmin * a));
    m.insert("angmax".into(), jnum(br.angmax * a));
    // PowerModels' rule: a transformer is a branch with an off-nominal raw tap.
    // A pure phase shifter (tap 0, shift ≠ 0) is not flagged, matching matpower.jl.
    m.insert("transformer".into(), Value::Bool(br.tap != 0.0));
    // PowerModels omits a rate when it is 0 (unlimited).
    if br.rate_a != 0.0 {
        m.insert("rate_a".into(), jnum(br.rate_a * p));
    }
    if br.rate_b != 0.0 {
        m.insert("rate_b".into(), jnum(br.rate_b * p));
    }
    if br.rate_c != 0.0 {
        m.insert("rate_c".into(), jnum(br.rate_c * p));
    }
    m.insert("source_id".into(), source_id("branch", idx));
    Value::Object(m)
}

fn gen_obj(g: &Generator, idx: usize, p: f64, base: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("gen_bus".into(), Value::from(g.bus.0 as u64));
    m.insert("pg".into(), jnum(g.pg * p));
    m.insert("qg".into(), jnum(g.qg * p));
    m.insert("qmax".into(), jnum(g.qmax * p));
    m.insert("qmin".into(), jnum(g.qmin * p));
    m.insert("vg".into(), jnum(g.vg));
    m.insert("mbase".into(), jnum(g.mbase));
    m.insert("gen_status".into(), status_int(g.in_service));
    m.insert("pmax".into(), jnum(g.pmax * p));
    m.insert("pmin".into(), jnum(g.pmin * p));
    // Gen capability columns, in PowerModels' field order, for those present. Only
    // the ramp rates are per-unitized; the PQ curve points and apf stay raw.
    for (i, key) in GEN_EXTRA_KEYS.iter().enumerate() {
        if let Some(v) = g.caps[i] {
            let scaled = if GEN_PU_KEYS.contains(key) {
                jnum(v * p)
            } else {
                jnum(v)
            };
            m.insert((*key).into(), scaled);
        }
    }
    if let Some(cost) = &g.cost {
        let coeffs: Vec<Value> = normalize::cost_to_pu(cost, base)
            .into_iter()
            .map(jnum)
            .collect();
        // Emit `ncost` consistent with the coefficients actually written. The reader
        // un-scales by the array length, so a mismatched `ncost` (from a malformed
        // row that claimed more coefficients than it carried) would reconstruct the
        // wrong polynomial degree.
        let ncost = if cost.model == 1 {
            coeffs.len() / 2
        } else {
            coeffs.len()
        };
        m.insert("model".into(), Value::from(u64::from(cost.model)));
        m.insert("ncost".into(), Value::from(ncost as u64));
        m.insert("startup".into(), jnum(cost.startup));
        m.insert("shutdown".into(), jnum(cost.shutdown));
        m.insert("cost".into(), Value::Array(coeffs));
    }
    m.insert("source_id".into(), source_id("gen", idx));
    Value::Object(m)
}

fn load_obj(l: &Load, idx: usize, p: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("load_bus".into(), Value::from(l.bus.0 as u64));
    m.insert("pd".into(), jnum(l.p * p));
    m.insert("qd".into(), jnum(l.q * p));
    m.insert("status".into(), status_int(l.in_service));
    m.insert("source_id".into(), source_id("bus", l.bus.0));
    Value::Object(m)
}

fn shunt_obj(s: &Shunt, idx: usize, p: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("shunt_bus".into(), Value::from(s.bus.0 as u64));
    m.insert("gs".into(), jnum(s.g * p));
    m.insert("bs".into(), jnum(s.b * p));
    m.insert("status".into(), status_int(s.in_service));
    m.insert("source_id".into(), source_id("bus", s.bus.0));
    Value::Object(m)
}

fn dcline_obj(dc: &Hvdc, idx: usize, p: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("f_bus".into(), Value::from(dc.from.0 as u64));
    m.insert("t_bus".into(), Value::from(dc.to.0 as u64));
    m.insert("br_status".into(), status_int(dc.in_service));
    m.insert("pf".into(), jnum(dc.pf * p));
    // MATPOWER uses the opposite sign for Pt/Qf/Qt; PowerModels flips them.
    m.insert("pt".into(), jnum(-dc.pt * p));
    m.insert("qf".into(), jnum(-dc.qf * p));
    m.insert("qt".into(), jnum(-dc.qt * p));
    m.insert("vf".into(), jnum(dc.vf));
    m.insert("vt".into(), jnum(dc.vt));
    // Per-end active-power bounds, derived from the aggregate Pmin/Pmax and the
    // loss model exactly as PowerModels' matpower loader does (_mp2pm_dcline!), so
    // the line reads back through PowerModels' own correct_dclines! pass. Derived
    // in raw MW, then per-unitized like everything else.
    let (pminf, pmaxf, pmint, pmaxt) = dcline_p_bounds(dc.pmin, dc.pmax, dc.loss0, dc.loss1);
    m.insert("pminf".into(), jnum(pminf * p));
    m.insert("pmaxf".into(), jnum(pmaxf * p));
    m.insert("pmint".into(), jnum(pmint * p));
    m.insert("pmaxt".into(), jnum(pmaxt * p));
    // The original aggregate bounds, kept raw, as PowerModels does.
    m.insert("mp_pmin".into(), jnum(dc.pmin));
    m.insert("mp_pmax".into(), jnum(dc.pmax));
    m.insert("qminf".into(), jnum(dc.qminf * p));
    m.insert("qmaxf".into(), jnum(dc.qmaxf * p));
    m.insert("qmint".into(), jnum(dc.qmint * p));
    m.insert("qmaxt".into(), jnum(dc.qmaxt * p));
    m.insert("loss0".into(), jnum(dc.loss0 * p));
    m.insert("loss1".into(), jnum(dc.loss1));
    m.insert("source_id".into(), source_id("dcline", idx));
    Value::Object(m)
}

/// Per-end active-power bounds `(pminf, pmaxf, pmint, pmaxt)` for an HVDC line,
/// from the aggregate Pmin/Pmax and the loss model, branching on the bound signs
/// exactly as PowerModels' `_mp2pm_dcline!` does. Inputs and outputs are raw MW.
fn dcline_p_bounds(pmin: f64, pmax: f64, loss0: f64, loss1: f64) -> (f64, f64, f64, f64) {
    let l = 1.0 - loss1;
    if pmin >= 0.0 && pmax >= 0.0 {
        (pmin, pmax, loss0 - pmax * l, loss0 - pmin * l)
    } else if pmin >= 0.0 {
        (pmin, (-pmax + loss0) / l, pmax, loss0 - pmin * l)
    } else if pmax >= 0.0 {
        ((pmin + loss0) / l, pmax, loss0 - pmax * l, -pmin)
    } else {
        ((pmin + loss0) / l, (-pmax + loss0) / l, pmax, -pmin)
    }
}

fn storage_obj(st: &Storage, idx: usize, p: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("storage_bus".into(), Value::from(st.bus.0 as u64));
    // ps/qs are the dispatch setpoint; PowerModels' make_per_unit! leaves them raw
    // (it rescales the energy/ratings/limits below), so we do too.
    m.insert("ps".into(), jnum(st.ps));
    m.insert("qs".into(), jnum(st.qs));
    m.insert("energy".into(), jnum(st.energy * p));
    m.insert("energy_rating".into(), jnum(st.energy_rating * p));
    m.insert("charge_rating".into(), jnum(st.charge_rating * p));
    m.insert("discharge_rating".into(), jnum(st.discharge_rating * p));
    m.insert("charge_efficiency".into(), jnum(st.charge_efficiency));
    m.insert("discharge_efficiency".into(), jnum(st.discharge_efficiency));
    m.insert("thermal_rating".into(), jnum(st.thermal_rating * p));
    m.insert("qmin".into(), jnum(st.qmin * p));
    m.insert("qmax".into(), jnum(st.qmax * p));
    m.insert("r".into(), jnum(st.r));
    m.insert("x".into(), jnum(st.x));
    m.insert("p_loss".into(), jnum(st.p_loss * p));
    m.insert("q_loss".into(), jnum(st.q_loss * p));
    m.insert("status".into(), status_int(st.in_service));
    m.insert("source_id".into(), source_id("storage", idx));
    Value::Object(m)
}

// ---- Reader: PowerModels JSON → Network -------------------------------------

const FMT: &str = "PowerModels JSON";

/// Parse PowerModels.jl network data JSON into a [`Network`]. Loads and shunts
/// are read as first-class elements and the raw text is retained, so writing back
/// to PowerModels JSON is a byte-exact echo. `per_unit = true` input (powerio's own
/// output, and PowerModels' own export) is converted to the neutral MW/degree
/// convention (powers ×baseMVA, angles to degrees, cost coefficients un-scaled),
/// following PowerModels' own exceptions (storage `ps`/`qs` stay raw, dcline
/// `pt`/`qf`/`qt` flip sign); `per_unit = false` is read as-is.
pub fn parse_powermodels_json(content: &str) -> Result<Network> {
    parse_powermodels_json_source(Arc::new(content.to_owned()), None)
}

/// Owned-source entry used by the format hub: parse by borrowing `source`, then
/// move the buffer into the retained source (no copy). `name_hint` (e.g. a file
/// stem) names the network when the JSON carries no `name`.
pub(crate) fn parse_powermodels_json_source(
    source: Arc<String>,
    name_hint: Option<&str>,
) -> Result<Network> {
    let content: &str = &source;
    let root: Value = serde_json::from_str(content).map_err(|e| Error::FormatRead {
        format: FMT,
        message: e.to_string(),
    })?;
    let root = root.as_object().ok_or_else(|| Error::FormatRead {
        format: FMT,
        message: "top level is not a JSON object".into(),
    })?;

    let base_mva =
        root.get("baseMVA")
            .and_then(Value::as_f64)
            .ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: "missing numeric `baseMVA`".into(),
            })?;
    let per_unit = root
        .get("per_unit")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let pscale = if per_unit { base_mva } else { 1.0 };
    let ascale = if per_unit { normalize::RAD_TO_DEG } else { 1.0 };
    let name = root
        .get("name")
        .and_then(Value::as_str)
        .or(name_hint)
        .unwrap_or("case")
        .to_string();

    let net = Network {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        buses: sorted(root, "bus", "index")
            .iter()
            .map(|v| read_bus(v, ascale))
            .collect::<Result<Vec<_>>>()?,
        loads: sorted(root, "load", "index")
            .iter()
            .map(|v| read_load(v, pscale))
            .collect(),
        shunts: sorted(root, "shunt", "index")
            .iter()
            .map(|v| read_shunt(v, pscale))
            .collect(),
        branches: sorted(root, "branch", "index")
            .iter()
            .map(|v| read_branch(v, pscale, ascale))
            .collect(),
        generators: sorted(root, "gen", "index")
            .iter()
            .map(|v| read_gen(v, pscale, base_mva, per_unit))
            .collect(),
        storage: sorted(root, "storage", "index")
            .iter()
            .map(|v| read_storage(v, pscale))
            .collect(),
        hvdc: sorted(root, "dcline", "index")
            .iter()
            .map(|v| read_hvdc(v, pscale))
            .collect(),
        transformers_3w: Vec::new(),
        areas: Vec::new(),
        solver: None,
        source_format: SourceFormat::PowerModelsJson,
        source: Some(source),
    };
    net.check_references(FMT)?;
    Ok(net)
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

fn read_bus(v: &Value, ascale: f64) -> Result<Bus> {
    let id = v
        .get("bus_i")
        .or_else(|| v.get("index"))
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "bus record missing integer `bus_i`".into(),
        })? as usize;
    Ok(Bus {
        id: BusId(id),
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
            &[
                "bus_i",
                "index",
                "bus_type",
                "vm",
                "va",
                "vmax",
                "vmin",
                "base_kv",
                "area",
                "zone",
                "name",
                "source_id",
            ],
        ),
    })
}

fn read_load(v: &Value, pscale: f64) -> Load {
    Load {
        bus: BusId(uid(v, "load_bus")),
        p: f(v, "pd") * pscale,
        q: f(v, "qd") * pscale,
        in_service: flag(v, "status"),
        extras: extras_excluding(v, &["load_bus", "pd", "qd", "status", "index", "source_id"]),
    }
}

fn read_shunt(v: &Value, pscale: f64) -> Shunt {
    Shunt {
        bus: BusId(uid(v, "shunt_bus")),
        g: f(v, "gs") * pscale,
        b: f(v, "bs") * pscale,
        in_service: flag(v, "status"),
        control: None,
        extras: extras_excluding(
            v,
            &["shunt_bus", "gs", "bs", "status", "index", "source_id"],
        ),
    }
}

fn read_branch(v: &Value, pscale: f64, ascale: f64) -> Branch {
    // PowerModels stores the effective tap (1.0 for a line); the `transformer`
    // flag disambiguates an explicit-tap transformer from a line, which is what
    // the neutral raw-tap convention (0 = line) needs.
    let transformer = v
        .get("transformer")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tap = if transformer {
        f_or(v, "tap", 1.0)
    } else {
        0.0
    };
    Branch {
        from: BusId(uid(v, "f_bus")),
        to: BusId(uid(v, "t_bus")),
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
        control: None,
        extras: extras_excluding(
            v,
            &[
                "f_bus",
                "t_bus",
                "br_r",
                "br_x",
                "b_fr",
                "b_to",
                "g_fr",
                "g_to",
                "tap",
                "shift",
                "br_status",
                "angmin",
                "angmax",
                "transformer",
                "rate_a",
                "rate_b",
                "rate_c",
                "index",
                "source_id",
            ],
        ),
    }
}

fn read_gen(v: &Value, pscale: f64, base_mva: f64, per_unit: bool) -> Generator {
    let mut caps: crate::network::GenCaps = [None; GEN_EXTRA_KEYS.len()];
    for (i, key) in GEN_EXTRA_KEYS.iter().enumerate() {
        if let Some(val) = v.get(*key).and_then(Value::as_f64) {
            // Only the ramp rates are per-unit; the PQ curve points and apf are raw.
            caps[i] = Some(if GEN_PU_KEYS.contains(key) {
                val * pscale
            } else {
                val
            });
        }
    }
    let cost = v.get("model").map(|_| read_cost(v, base_mva, per_unit));
    Generator {
        bus: BusId(uid(v, "gen_bus")),
        pg: f(v, "pg") * pscale,
        qg: f(v, "qg") * pscale,
        // The writer emits an unbounded limit (±Inf) as JSON null; read a missing
        // limit back as unbounded, not as a binding 0.0. (±Inf · pscale stays ±Inf.)
        pmax: f_or(v, "pmax", f64::INFINITY) * pscale,
        pmin: f_or(v, "pmin", f64::NEG_INFINITY) * pscale,
        qmax: f_or(v, "qmax", f64::INFINITY) * pscale,
        qmin: f_or(v, "qmin", f64::NEG_INFINITY) * pscale,
        vg: f_or(v, "vg", 1.0),
        mbase: f_or(v, "mbase", base_mva),
        in_service: flag(v, "gen_status"),
        cost,
        caps,
        regulated_bus: None,
    }
}

fn read_cost(v: &Value, base_mva: f64, per_unit: bool) -> GenCost {
    // Keep non-numeric entries as NaN rather than dropping them: silently filtering
    // would shift every later coefficient's polynomial degree.
    let coeffs_raw: Vec<f64> = v
        .get("cost")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|c| c.as_f64().unwrap_or(f64::NAN)).collect())
        .unwrap_or_default();
    let model = v.get("model").and_then(Value::as_u64).unwrap_or(2) as u8;
    let k = coeffs_raw.len();
    // Undo PowerModels' per-unit cost scaling for the neutral MW basis (the
    // inverse of the writer's per-unit rescale); a non-per-unit source is read
    // as-is.
    let coeffs = if per_unit {
        normalize::cost_from_pu(&coeffs_raw, model, base_mva)
    } else {
        coeffs_raw
    };
    // A polynomial's ncost is its coefficient count; a piecewise curve stores
    // 2·ncost values ((mw, cost) pairs).
    let default_ncost = if model == 1 { k / 2 } else { k };
    GenCost {
        model,
        startup: f(v, "startup"),
        shutdown: f(v, "shutdown"),
        ncost: v
            .get("ncost")
            .and_then(Value::as_u64)
            .map_or(default_ncost, |n| n as usize),
        coeffs,
    }
}

fn read_hvdc(v: &Value, pscale: f64) -> Hvdc {
    // Aggregate bounds come from PowerModels' raw originals (mp_pmin/mp_pmax); fall
    // back to the from-end per-unit bounds for input that lacks them.
    let pmin = v
        .get("mp_pmin")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| f(v, "pminf") * pscale);
    let pmax = v
        .get("mp_pmax")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| f(v, "pmaxf") * pscale);
    Hvdc {
        from: BusId(uid(v, "f_bus")),
        to: BusId(uid(v, "t_bus")),
        in_service: flag(v, "br_status"),
        pf: f(v, "pf") * pscale,
        // PowerModels flips Pt/Qf/Qt vs MATPOWER; undo it for the neutral model.
        pt: -f(v, "pt") * pscale,
        qf: -f(v, "qf") * pscale,
        qt: -f(v, "qt") * pscale,
        vf: f_or(v, "vf", 1.0),
        vt: f_or(v, "vt", 1.0),
        pmin,
        pmax,
        // Unbounded reactive limits (±Inf) write as null; read them back unbounded.
        qminf: f_or(v, "qminf", f64::NEG_INFINITY) * pscale,
        qmaxf: f_or(v, "qmaxf", f64::INFINITY) * pscale,
        qmint: f_or(v, "qmint", f64::NEG_INFINITY) * pscale,
        qmaxt: f_or(v, "qmaxt", f64::INFINITY) * pscale,
        loss0: f(v, "loss0") * pscale,
        loss1: f(v, "loss1"),
        extras: extras_excluding(
            v,
            &[
                "f_bus",
                "t_bus",
                "br_status",
                "pf",
                "pt",
                "qf",
                "qt",
                "vf",
                "vt",
                "pmin",
                "pmax",
                "mp_pmin",
                "mp_pmax",
                "pminf",
                "pmaxf",
                "pmint",
                "pmaxt",
                "qminf",
                "qmaxf",
                "qmint",
                "qmaxt",
                "loss0",
                "loss1",
                "index",
                "source_id",
            ],
        ),
    }
}

fn read_storage(v: &Value, pscale: f64) -> Storage {
    Storage {
        bus: BusId(uid(v, "storage_bus")),
        ps: f(v, "ps"),
        qs: f(v, "qs"),
        energy: f(v, "energy") * pscale,
        energy_rating: f(v, "energy_rating") * pscale,
        charge_rating: f(v, "charge_rating") * pscale,
        discharge_rating: f(v, "discharge_rating") * pscale,
        charge_efficiency: f_or(v, "charge_efficiency", 1.0),
        discharge_efficiency: f_or(v, "discharge_efficiency", 1.0),
        thermal_rating: f(v, "thermal_rating") * pscale,
        // Unbounded reactive limits (±Inf) write as null; read them back unbounded.
        qmin: f_or(v, "qmin", f64::NEG_INFINITY) * pscale,
        qmax: f_or(v, "qmax", f64::INFINITY) * pscale,
        r: f(v, "r"),
        x: f(v, "x"),
        p_loss: f(v, "p_loss") * pscale,
        q_loss: f(v, "q_loss") * pscale,
        in_service: flag(v, "status"),
        extras: extras_excluding(
            v,
            &[
                "storage_bus",
                "ps",
                "qs",
                "energy",
                "energy_rating",
                "charge_rating",
                "discharge_rating",
                "charge_efficiency",
                "discharge_efficiency",
                "thermal_rating",
                "qmin",
                "qmax",
                "r",
                "x",
                "p_loss",
                "q_loss",
                "status",
                "index",
                "source_id",
            ],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn gen_pu_keys_subset_of_extra_keys() {
        // The per-unitized columns must be a subset of the emitted capability
        // columns; a key not in GEN_EXTRA_KEYS would never be written or scaled,
        // and a typo here silently mis-scales a ramp rate.
        for k in GEN_PU_KEYS {
            assert!(
                GEN_EXTRA_KEYS.contains(&k),
                "{k} is not a GEN_EXTRA_KEYS column"
            );
        }
    }

    #[test]
    fn dcline_p_bounds_four_quadrants() {
        // loss0 = 1, loss1 = 0.1 ⇒ l = 0.9. Each sign quadrant of (pmin, pmax)
        // hand-computed against PowerModels' _mp2pm_dcline!.
        let q1 = dcline_p_bounds(2.0, 10.0, 1.0, 0.1);
        assert!(
            approx(q1.0, 2.0) && approx(q1.1, 10.0) && approx(q1.2, -8.0) && approx(q1.3, -0.8)
        );

        let q2 = dcline_p_bounds(2.0, -5.0, 1.0, 0.1);
        assert!(
            approx(q2.0, 2.0)
                && approx(q2.1, 6.0 / 0.9)
                && approx(q2.2, -5.0)
                && approx(q2.3, -0.8)
        );

        let q3 = dcline_p_bounds(-3.0, 10.0, 1.0, 0.1);
        assert!(
            approx(q3.0, -2.0 / 0.9)
                && approx(q3.1, 10.0)
                && approx(q3.2, -8.0)
                && approx(q3.3, 3.0)
        );

        let q4 = dcline_p_bounds(-3.0, -5.0, 1.0, 0.1);
        assert!(
            approx(q4.0, -2.0 / 0.9)
                && approx(q4.1, 6.0 / 0.9)
                && approx(q4.2, -5.0)
                && approx(q4.3, 3.0)
        );
    }
}

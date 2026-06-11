//! Read and write pandapower `pandapowerNet` JSON.
//!
//! pandapower serializes each element table as a pandas split-oriented
//! `DataFrame` encoded inside a JSON string. This module implements that small
//! table codec directly so the Rust core stays Python-free.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde_json::{Map, Value};

use super::{Conversion, Parsed, finish, jnum};
use crate::network::{
    Branch, Bus, BusId, BusType, Extras, GenCost, Generator, Hvdc, Load, Network, Shunt,
    SourceFormat, Storage,
};
use crate::{Error, Result};

const FMT: &str = "pandapower JSON";
const F_HZ: f64 = 50.0;
const MAX_I_KA: f64 = 99_999.0;

/// Parse pandapower `pandapowerNet` JSON `content`. Returns [`Parsed`]: the
/// network plus the reader's fidelity warnings.
pub fn parse_pandapower_json(content: &str) -> Result<Parsed> {
    let mut warnings = Vec::new();
    let network = parse_pandapower_source(Arc::new(content.to_owned()), None, &mut warnings)?;
    Ok(Parsed { network, warnings })
}

#[allow(clippy::too_many_lines)] // direct table-to-Network mapper; split helpers obscure column mapping
pub(crate) fn parse_pandapower_source(
    source: Arc<String>,
    name_hint: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<Network> {
    let content: &str = &source;
    let root: Value = serde_json::from_str(content).map_err(|e| bad(e.to_string()))?;
    let root = root
        .as_object()
        .ok_or_else(|| bad("top level is not a JSON object"))?;
    if root.get("_class").and_then(Value::as_str) != Some("pandapowerNet") {
        return Err(bad("top level `_class` is not `pandapowerNet`"));
    }
    let obj = root
        .get("_object")
        .and_then(Value::as_object)
        .ok_or_else(|| bad("missing `_object` network map"))?;

    let base_mva = obj.get("sn_mva").and_then(Value::as_f64).unwrap_or(1.0);
    let f_hz = obj.get("f_hz").and_then(Value::as_f64).unwrap_or(F_HZ);
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or(name_hint)
        .unwrap_or("case")
        .to_string();

    let bus_frame = read_frame(obj, "bus")?.ok_or_else(|| bad("missing `bus` table"))?;
    let mut buses = Vec::with_capacity(bus_frame.data.len());
    let mut bus_of_pp = HashMap::with_capacity(bus_frame.data.len());
    for row in bus_frame.rows() {
        let pp_idx = row.index_usize()?;
        // pandapower bus ids are the pandas index values, 0-based; BusId is
        // 1-based, so shift by one. The writer shifts back.
        let id = BusId(pp_idx + 1);
        if bus_of_pp.insert(pp_idx, id).is_some() {
            return Err(bad(format!("`bus` table: duplicate index {pp_idx}")));
        }
        buses.push(Bus {
            id,
            kind: if row.bool_or("in_service", true) {
                BusType::Pq
            } else {
                BusType::Isolated
            },
            vm: 1.0,
            va: 0.0,
            base_kv: row.req_f("vn_kv")?,
            vmax: row.f_or("max_vm_pu", 1.1),
            vmin: row.f_or("min_vm_pu", 0.9),
            area: 1,
            zone: row.usize_or("zone", 1),
            name: row.string("name"),
            extras: Extras::default(),
        });
    }
    let bus_pos: HashMap<BusId, usize> = buses.iter().enumerate().map(|(i, b)| (b.id, i)).collect();

    let mut loads = Vec::new();
    if let Some(load_frame) = read_frame(obj, "load")? {
        let mut zip_rows = 0_usize;
        for row in load_frame.rows() {
            let scale = row.f_or("scaling", 1.0);
            if row.f_or("const_z_percent", 0.0) != 0.0 || row.f_or("const_i_percent", 0.0) != 0.0 {
                zip_rows += 1;
            }
            loads.push(Load {
                bus: bus_ref("load", &row, "bus", &bus_of_pp)?,
                p: row.f_or("p_mw", 0.0) * scale,
                q: row.f_or("q_mvar", 0.0) * scale,
                in_service: row.bool_or("in_service", true),
                extras: Extras::default(),
            });
        }
        if zip_rows > 0 {
            warnings.push(format!(
                "`load`: ZIP composition (const_z_percent/const_i_percent) nonzero on {zip_rows} rows; loads are read as constant power"
            ));
        }
    }

    let mut shunts = Vec::new();
    if let Some(shunt_frame) = read_frame(obj, "shunt")? {
        for row in shunt_frame.rows() {
            let step = row.f_or("step", 1.0);
            shunts.push(Shunt {
                bus: bus_ref("shunt", &row, "bus", &bus_of_pp)?,
                g: row.f_or("p_mw", 0.0) * step,
                b: -row.f_or("q_mvar", 0.0) * step,
                in_service: row.bool_or("in_service", true),
                extras: Extras::default(),
            });
        }
    }

    let costs = read_poly_costs(obj, warnings)?;
    let mut generators = Vec::new();
    if let Some(gen_frame) = read_frame(obj, "gen")? {
        for row in gen_frame.rows() {
            let idx = row.index_usize()?;
            let bus = bus_ref("gen", &row, "bus", &bus_of_pp)?;
            let slack = row.bool_or("slack", false);
            set_bus_kind(
                &mut buses,
                &bus_pos,
                bus,
                if slack { BusType::Ref } else { BusType::Pv },
            );
            generators.push(Generator {
                bus,
                pg: row.f_or("p_mw", 0.0) * row.f_or("scaling", 1.0),
                qg: 0.0,
                pmax: row.f_or("max_p_mw", row.f_or("p_mw", 0.0)),
                pmin: row.f_or("min_p_mw", 0.0),
                qmax: row.f_or("max_q_mvar", f64::INFINITY),
                qmin: row.f_or("min_q_mvar", f64::NEG_INFINITY),
                vg: row.f_or("vm_pu", 1.0),
                mbase: row.f_or("sn_mva", base_mva),
                in_service: row.bool_or("in_service", true),
                cost: costs.get(&("gen".to_string(), idx)).cloned(),
                caps: [None; crate::network::GEN_EXTRA_KEYS.len()],
            });
        }
    }
    if let Some(ext_grid_frame) = read_frame(obj, "ext_grid")? {
        for row in ext_grid_frame.rows() {
            let idx = row.index_usize()?;
            let bus = bus_ref("ext_grid", &row, "bus", &bus_of_pp)?;
            set_bus_kind(&mut buses, &bus_pos, bus, BusType::Ref);
            generators.push(Generator {
                bus,
                pg: 0.0,
                qg: 0.0,
                pmax: row.f_or("max_p_mw", f64::INFINITY),
                pmin: row.f_or("min_p_mw", f64::NEG_INFINITY),
                qmax: row.f_or("max_q_mvar", f64::INFINITY),
                qmin: row.f_or("min_q_mvar", f64::NEG_INFINITY),
                vg: row.f_or("vm_pu", 1.0),
                mbase: base_mva,
                in_service: row.bool_or("in_service", true),
                cost: costs.get(&("ext_grid".to_string(), idx)).cloned(),
                caps: [None; crate::network::GEN_EXTRA_KEYS.len()],
            });
        }
    }
    // Static generators read as PQ injections: the bus kind stays whatever the
    // gen/ext_grid tables made it.
    if let Some(sgen_frame) = read_frame(obj, "sgen")? {
        for row in sgen_frame.rows() {
            let idx = row.index_usize()?;
            let bus = bus_ref("sgen", &row, "bus", &bus_of_pp)?;
            let scale = row.f_or("scaling", 1.0);
            let p = row.f_or("p_mw", 0.0);
            generators.push(Generator {
                bus,
                pg: p * scale,
                qg: row.f_or("q_mvar", 0.0) * scale,
                pmax: row.f_or("max_p_mw", p),
                pmin: row.f_or("min_p_mw", 0.0),
                qmax: row.f_or("max_q_mvar", f64::INFINITY),
                qmin: row.f_or("min_q_mvar", f64::NEG_INFINITY),
                vg: 1.0,
                mbase: row.f_or("sn_mva", base_mva),
                in_service: row.bool_or("in_service", true),
                cost: costs.get(&("sgen".to_string(), idx)).cloned(),
                caps: [None; crate::network::GEN_EXTRA_KEYS.len()],
            });
        }
    }

    let mut branches = Vec::new();
    if let Some(line_frame) = read_frame(obj, "line")? {
        let mut g_rows = 0_usize;
        for row in line_frame.rows() {
            let from = bus_ref("line", &row, "from_bus", &bus_of_pp)?;
            let to = bus_ref("line", &row, "to_bus", &bus_of_pp)?;
            let v_to = bus_kv(&buses, &bus_pos, to);
            let zbase = zbase(v_to, base_mva);
            let par = parallel_or_one(&row);
            let max_i_ka = row.f_or("max_i_ka", 0.0);
            if row.f_or("g_us_per_km", 0.0) != 0.0 {
                g_rows += 1;
            }
            branches.push(Branch {
                from,
                to,
                r: row.f_or("r_ohm_per_km", 0.0) * row.f_or("length_km", 1.0) / zbase / par,
                x: row.f_or("x_ohm_per_km", 0.0) * row.f_or("length_km", 1.0) / zbase / par,
                b: row.f_or("c_nf_per_km", 0.0)
                    * row.f_or("length_km", 1.0)
                    * 1e-9
                    * 2.0
                    * std::f64::consts::PI
                    * f_hz
                    * zbase
                    * par,
                rate_a: if max_i_ka >= MAX_I_KA {
                    0.0
                } else {
                    max_i_ka * v_to * 3.0_f64.sqrt() * par
                },
                rate_b: 0.0,
                rate_c: 0.0,
                tap: 0.0,
                shift: 0.0,
                in_service: row.bool_or("in_service", true),
                angmin: -360.0,
                angmax: 360.0,
                extras: Extras::default(),
            });
        }
        if g_rows > 0 {
            warnings.push(format!(
                "`line`: g_us_per_km nonzero on {g_rows} rows; line shunt conductance is not representable and was ignored"
            ));
        }
    }
    if let Some(trafo_frame) = read_frame(obj, "trafo")? {
        let mut mag_rows = 0_usize;
        let mut lv_rows = 0_usize;
        for row in trafo_frame.rows() {
            let from = bus_ref("trafo", &row, "hv_bus", &bus_of_pp)?;
            let to = bus_ref("trafo", &row, "lv_bus", &bus_of_pp)?;
            let sn = row.f_or("sn_mva", base_mva);
            let par = parallel_or_one(&row);
            let r = row.f_or("vkr_percent", 0.0) * base_mva / (sn * 100.0);
            let z = row.f_or("vk_percent", 0.0).abs() * base_mva / (sn * 100.0);
            let x = (z * z - r * r).max(0.0).sqrt() * row.f_or("vk_percent", 0.0).signum();
            if row.f_or("i0_percent", 0.0) != 0.0 || row.f_or("pfe_kw", 0.0) != 0.0 {
                mag_rows += 1;
            }
            let tap_neutral = row.f_or("tap_neutral", 0.0);
            let tap_pos = row.f_or("tap_pos", tap_neutral);
            let tap_step_percent = row.f_or("tap_step_percent", 0.0);
            let mut tap = 1.0 + (tap_pos - tap_neutral) * tap_step_percent / 100.0;
            if row
                .string("tap_side")
                .is_some_and(|s| s.eq_ignore_ascii_case("lv"))
            {
                tap = 1.0;
                lv_rows += 1;
            }
            branches.push(Branch {
                from,
                to,
                r: r / par,
                x: x / par,
                b: 0.0,
                rate_a: sn * par,
                rate_b: 0.0,
                rate_c: 0.0,
                tap,
                shift: row.f_or("shift_degree", 0.0),
                in_service: row.bool_or("in_service", true),
                angmin: -360.0,
                angmax: 360.0,
                extras: Extras::default(),
            });
        }
        if mag_rows > 0 {
            warnings.push(format!(
                "`trafo`: i0_percent/pfe_kw nonzero on {mag_rows} rows; the magnetizing branch is not representable and was ignored"
            ));
        }
        if lv_rows > 0 {
            warnings.push(format!(
                "`trafo`: tap_side == \"lv\" on {lv_rows} rows; taps are modeled on the hv side, lv taps were ignored"
            ));
        }
    }

    let mut storage = Vec::new();
    if let Some(storage_frame) = read_frame(obj, "storage")? {
        for row in storage_frame.rows() {
            let bus = bus_ref("storage", &row, "bus", &bus_of_pp)?;
            let scale = row.f_or("scaling", 1.0);
            // Load convention: positive ps = charging. No sign flip.
            let ps = row.f_or("p_mw", 0.0) * scale;
            let qs = row.f_or("q_mvar", 0.0) * scale;
            let min_e = row.f_or("min_e_mwh", 0.0);
            let max_e = row.f_or("max_e_mwh", 0.0);
            let charge_rating = row.f_finite("max_p_mw").unwrap_or_else(|| ps.abs());
            let discharge_rating = row.f_finite("min_p_mw").map_or(ps.abs(), |v| (-v).max(0.0));
            storage.push(Storage {
                bus,
                ps,
                qs,
                energy: min_e + (max_e - min_e) * row.f_or("soc_percent", 0.0) / 100.0,
                energy_rating: max_e,
                charge_rating,
                discharge_rating,
                charge_efficiency: 1.0,
                discharge_efficiency: 1.0,
                thermal_rating: row
                    .f_finite("sn_mva")
                    .unwrap_or_else(|| charge_rating.max(discharge_rating)),
                qmin: row.f_or("min_q_mvar", f64::NEG_INFINITY),
                qmax: row.f_or("max_q_mvar", f64::INFINITY),
                r: 0.0,
                x: 0.0,
                p_loss: 0.0,
                q_loss: 0.0,
                in_service: row.bool_or("in_service", true),
                extras: Extras::default(),
            });
        }
    }

    let mut hvdc = Vec::new();
    if let Some(dcline_frame) = read_frame(obj, "dcline")? {
        for row in dcline_frame.rows() {
            let from = bus_ref("dcline", &row, "from_bus", &bus_of_pp)?;
            let to = bus_ref("dcline", &row, "to_bus", &bus_of_pp)?;
            let pf = row.f_or("p_mw", 0.0);
            let loss_mw = row.f_or("loss_mw", 0.0);
            let loss_percent = row.f_or("loss_percent", 0.0);
            hvdc.push(Hvdc {
                from,
                to,
                in_service: row.bool_or("in_service", true),
                pf,
                // MATPOWER PT = PF - (l0 + l1 * PF)
                pt: pf - loss_mw - pf * loss_percent / 100.0,
                qf: 0.0,
                qt: 0.0,
                vf: row.f_or("vm_from_pu", 1.0),
                vt: row.f_or("vm_to_pu", 1.0),
                pmin: 0.0,
                pmax: row.f_or("max_p_mw", f64::INFINITY),
                qminf: row.f_or("min_q_from_mvar", f64::NEG_INFINITY),
                qmaxf: row.f_or("max_q_from_mvar", f64::INFINITY),
                qmint: row.f_or("min_q_to_mvar", f64::NEG_INFINITY),
                qmaxt: row.f_or("max_q_to_mvar", f64::INFINITY),
                loss0: loss_mw,
                loss1: loss_percent / 100.0,
                extras: Extras::default(),
            });
        }
    }

    warn_nonempty_table(
        obj,
        "trafo3w",
        "three winding transformers are not mapped",
        warnings,
    )?;
    warn_nonempty_table(obj, "ward", "Ward equivalents are not mapped", warnings)?;
    warn_nonempty_table(
        obj,
        "xward",
        "extended Ward equivalents are not mapped",
        warnings,
    )?;
    warn_nonempty_table(
        obj,
        "impedance",
        "bus-to-bus impedance elements are not mapped",
        warnings,
    )?;
    warn_nonempty_table(obj, "motor", "motors are not mapped", warnings)?;
    warn_nonempty_table(
        obj,
        "switch",
        "switches are not modeled; open switches are not applied",
        warnings,
    )?;
    warn_nonempty_table(obj, "pwl_cost", "piecewise costs are not mapped", warnings)?;

    let net = Network {
        name,
        base_mva,
        buses,
        loads,
        shunts,
        branches,
        generators,
        storage,
        hvdc,
        source_format: SourceFormat::PandapowerJson,
        source: Some(source),
    };
    net.check_references(FMT)?;
    Ok(net)
}

/// `parallel` column, treating missing or nonpositive values as one device.
fn parallel_or_one(row: &Row<'_>) -> f64 {
    let par = row.f_or("parallel", 1.0);
    if par <= 0.0 { 1.0 } else { par }
}

fn warn_nonempty_table(
    obj: &Map<String, Value>,
    name: &str,
    reason: &str,
    warnings: &mut Vec<String>,
) -> Result<()> {
    if let Some(frame) = read_frame(obj, name)? {
        if !frame.data.is_empty() {
            warnings.push(format!(
                "`{name}` table ignored ({} rows): {reason}",
                frame.data.len()
            ));
        }
    }
    Ok(())
}

#[must_use]
pub fn write_pandapower_json(net: &Network) -> Conversion {
    if net.source_format == SourceFormat::PandapowerJson {
        if let Some(source) = &net.source {
            return Conversion {
                text: source.to_string(),
                warnings: Vec::new(),
            };
        }
    }

    let mut warnings = Vec::new();
    if !net.hvdc.is_empty() {
        warnings.push(format!(
            "{} dcline(s) dropped: pandapower JSON writer v1 does not model HVDC",
            net.hvdc.len()
        ));
    }
    if !net.storage.is_empty() {
        warnings.push(format!(
            "{} storage unit(s) dropped: pandapower JSON writer v1 does not model storage",
            net.storage.len()
        ));
    }
    let with_caps = net.generators.iter().filter(|g| g.has_caps()).count();
    if with_caps > 0 {
        warnings.push(format!("generator capability/ramp columns dropped for {with_caps} generator(s): pandapower gen tables have no MATPOWER capability columns"));
    }
    let constrained = net.branches.iter().filter(|b| b.has_angle_limits()).count();
    if constrained > 0 {
        warnings.push(format!("{constrained} branch angle limit(s) dropped: pandapower line/trafo tables do not carry MATPOWER angle limits"));
    }
    let rate_bc = net
        .branches
        .iter()
        .filter(|b| nonzero_differs(b.rate_b, b.rate_a) || nonzero_differs(b.rate_c, b.rate_a))
        .count();
    if rate_bc > 0 {
        warnings.push(format!("{rate_bc} branch rate_b/rate_c value set(s) dropped: pandapower carries one loading limit"));
    }

    let mut object = Map::new();
    object.insert("bus".into(), bus_frame(net));
    object.insert("load".into(), load_frame(net));
    object.insert("shunt".into(), shunt_frame(net));
    object.insert("gen".into(), gen_frame(net));
    object.insert("ext_grid".into(), ext_grid_frame(net));
    let (line, trafo) = branch_frames(net);
    object.insert("line".into(), line);
    object.insert("trafo".into(), trafo);
    object.insert("poly_cost".into(), poly_cost_frame(net, &mut warnings));
    object.insert("name".into(), Value::String(net.name.clone()));
    object.insert("f_hz".into(), jnum(F_HZ));
    object.insert("sn_mva".into(), jnum(net.base_mva));
    object.insert("version".into(), Value::String("3.0.0".into()));
    object.insert("format_version".into(), Value::String("3.0.0".into()));

    let mut root = Map::new();
    root.insert(
        "_module".into(),
        Value::String("pandapower.auxiliary".into()),
    );
    root.insert("_class".into(), Value::String("pandapowerNet".into()));
    root.insert("_object".into(), Value::Object(object));
    finish(root, warnings)
}

fn bus_frame(net: &Network) -> Value {
    let columns = [
        "name",
        "vn_kv",
        "type",
        "zone",
        "in_service",
        "geo",
        "min_vm_pu",
        "max_vm_pu",
    ];
    let mut index = Vec::with_capacity(net.buses.len());
    let mut data = Vec::with_capacity(net.buses.len());
    for b in &net.buses {
        index.push(pp_bus(b.id));
        data.push(vec![
            b.name.clone().map_or(Value::Null, Value::String),
            jnum(b.base_kv),
            Value::String("b".into()),
            Value::from(b.zone as u64),
            Value::Bool(b.kind != BusType::Isolated),
            Value::Null,
            jnum(b.vmin),
            jnum(b.vmax),
        ]);
    }
    frame(&columns, index, data)
}

fn load_frame(net: &Network) -> Value {
    let columns = [
        "name",
        "bus",
        "p_mw",
        "q_mvar",
        "const_z_percent",
        "const_i_percent",
        "sn_mva",
        "scaling",
        "in_service",
        "type",
    ];
    let mut index = Vec::with_capacity(net.loads.len());
    let mut data = Vec::with_capacity(net.loads.len());
    for l in &net.loads {
        index.push(Value::from(data.len() as u64));
        data.push(vec![
            Value::Null,
            pp_bus(l.bus),
            jnum(l.p),
            jnum(l.q),
            jnum(0.0),
            jnum(0.0),
            Value::Null,
            jnum(1.0),
            Value::Bool(l.in_service),
            Value::String("wye".into()),
        ]);
    }
    frame(&columns, index, data)
}

fn shunt_frame(net: &Network) -> Value {
    let columns = [
        "bus",
        "name",
        "q_mvar",
        "p_mw",
        "vn_kv",
        "step",
        "max_step",
        "in_service",
    ];
    let bus_kv: HashMap<BusId, f64> = net.buses.iter().map(|b| (b.id, b.base_kv)).collect();
    let mut index = Vec::with_capacity(net.shunts.len());
    let mut data = Vec::with_capacity(net.shunts.len());
    for s in &net.shunts {
        index.push(Value::from(data.len() as u64));
        data.push(vec![
            pp_bus(s.bus),
            Value::Null,
            jnum(-s.b),
            jnum(s.g),
            jnum(*bus_kv.get(&s.bus).unwrap_or(&0.0)),
            Value::from(1_u64),
            Value::from(1_u64),
            Value::Bool(s.in_service),
        ]);
    }
    frame(&columns, index, data)
}

fn gen_frame(net: &Network) -> Value {
    let columns = [
        "name",
        "bus",
        "p_mw",
        "vm_pu",
        "sn_mva",
        "min_q_mvar",
        "max_q_mvar",
        "scaling",
        "slack",
        "controllable",
        "in_service",
        "slack_weight",
        "type",
        "min_p_mw",
        "max_p_mw",
    ];
    let bus_kind: HashMap<BusId, BusType> = net.buses.iter().map(|b| (b.id, b.kind)).collect();
    let mut index = Vec::with_capacity(net.generators.len());
    let mut data = Vec::with_capacity(net.generators.len());
    for g in &net.generators {
        index.push(Value::from(data.len() as u64));
        data.push(vec![
            Value::Null,
            pp_bus(g.bus),
            jnum(g.pg),
            jnum(g.vg),
            jnum(g.mbase),
            jnum(g.qmin),
            jnum(g.qmax),
            jnum(1.0),
            Value::Bool(bus_kind.get(&g.bus).copied() == Some(BusType::Ref)),
            Value::Bool(true),
            Value::Bool(g.in_service),
            jnum(1.0),
            Value::Null,
            jnum(g.pmin),
            jnum(g.pmax),
        ]);
    }
    frame(&columns, index, data)
}

#[allow(clippy::too_many_lines)] // mirrors pandapower line/trafo column order in one place
fn branch_frames(net: &Network) -> (Value, Value) {
    let line_columns = [
        "name",
        "std_type",
        "from_bus",
        "to_bus",
        "length_km",
        "r_ohm_per_km",
        "x_ohm_per_km",
        "c_nf_per_km",
        "g_us_per_km",
        "max_i_ka",
        "df",
        "parallel",
        "type",
        "in_service",
        "geo",
    ];
    let trafo_columns = [
        "name",
        "std_type",
        "hv_bus",
        "lv_bus",
        "sn_mva",
        "vn_hv_kv",
        "vn_lv_kv",
        "vk_percent",
        "vkr_percent",
        "pfe_kw",
        "i0_percent",
        "shift_degree",
        "tap_side",
        "tap_neutral",
        "tap_step_percent",
        "tap_pos",
        "parallel",
        "df",
        "in_service",
    ];
    let bus_kv: HashMap<BusId, f64> = net.buses.iter().map(|b| (b.id, b.base_kv)).collect();
    let mut line_index = Vec::new();
    let mut line_data = Vec::new();
    let mut trafo_index = Vec::new();
    let mut trafo_data = Vec::new();
    for br in &net.branches {
        let v_to = *bus_kv.get(&br.to).unwrap_or(&0.0);
        let zb = zbase(v_to, net.base_mva);
        if br.is_transformer() {
            let sn = if br.rate_a > 0.0 {
                br.rate_a
            } else {
                net.base_mva
            };
            let z = (br.r * br.r + br.x * br.x).sqrt();
            let tap_delta = br.effective_tap() - 1.0;
            trafo_index.push(Value::from(trafo_data.len() as u64));
            trafo_data.push(vec![
                Value::Null,
                Value::Null,
                pp_bus(br.from),
                pp_bus(br.to),
                jnum(sn),
                jnum(*bus_kv.get(&br.from).unwrap_or(&0.0)),
                jnum(v_to),
                jnum(z * sn * 100.0 / net.base_mva),
                jnum(br.r * sn * 100.0 / net.base_mva),
                jnum(0.0),
                jnum(0.0),
                jnum(br.shift),
                Value::String("hv".into()),
                Value::from(0_i64),
                jnum(tap_delta.abs() * 100.0),
                jnum(tap_delta.signum()),
                Value::from(1_u64),
                jnum(1.0),
                Value::Bool(br.in_service),
            ]);
        } else {
            line_index.push(Value::from(line_data.len() as u64));
            line_data.push(vec![
                Value::Null,
                Value::Null,
                pp_bus(br.from),
                pp_bus(br.to),
                jnum(1.0),
                jnum(br.r * zb),
                jnum(br.x * zb),
                jnum(br.b / zb / (2.0 * std::f64::consts::PI * F_HZ) * 1e9),
                jnum(0.0),
                jnum(if br.rate_a == 0.0 {
                    0.0
                } else {
                    br.rate_a / (v_to * 3.0_f64.sqrt())
                }),
                jnum(1.0),
                Value::from(1_u64),
                Value::Null,
                Value::Bool(br.in_service),
                Value::Null,
            ]);
        }
    }
    (
        frame(&line_columns, line_index, line_data),
        frame(&trafo_columns, trafo_index, trafo_data),
    )
}

fn ext_grid_frame(net: &Network) -> Value {
    let columns = [
        "name",
        "bus",
        "vm_pu",
        "va_degree",
        "slack_weight",
        "in_service",
        "controllable",
    ];
    let mut index = Vec::new();
    let mut data = Vec::new();
    // A Ref bus with no generator gets an ext_grid row so pandapower sees a
    // slack; reading the file back materializes the row as a Ref generator.
    for b in &net.buses {
        if b.kind != BusType::Ref || net.generators.iter().any(|g| g.bus == b.id) {
            continue;
        }
        index.push(Value::from(data.len() as u64));
        data.push(vec![
            b.name.clone().map_or(Value::Null, Value::String),
            pp_bus(b.id),
            jnum(b.vm),
            jnum(b.va),
            jnum(1.0),
            Value::Bool(true),
            Value::Bool(true),
        ]);
    }
    frame(&columns, index, data)
}

fn poly_cost_frame(net: &Network, warnings: &mut Vec<String>) -> Value {
    let columns = [
        "element",
        "et",
        "cp0_eur",
        "cp1_eur_per_mw",
        "cp2_eur_per_mw2",
        "cq0_eur",
        "cq1_eur_per_mvar",
        "cq2_eur_per_mvar2",
    ];
    let mut index = Vec::new();
    let mut data = Vec::new();
    let mut dropped = 0_usize;
    let mut truncated = 0_usize;
    let mut empty = 0_usize;
    for (i, g) in net.generators.iter().enumerate() {
        let Some(cost) = &g.cost else {
            continue;
        };
        if cost.model != 2 {
            dropped += 1;
            continue;
        }
        // Coefficients are highest order first; keep the lowest order three.
        let n = cost.coeffs.len();
        let (c2, c1, c0) = match n {
            0 => {
                empty += 1;
                (0.0, 0.0, 0.0)
            }
            1 => (0.0, 0.0, cost.coeffs[0]),
            2 => (0.0, cost.coeffs[0], cost.coeffs[1]),
            _ => {
                if n > 3 {
                    truncated += 1;
                }
                (cost.coeffs[n - 3], cost.coeffs[n - 2], cost.coeffs[n - 1])
            }
        };
        index.push(Value::from(data.len() as u64));
        data.push(vec![
            Value::from(i as u64),
            Value::String("gen".into()),
            jnum(c0),
            jnum(c1),
            jnum(c2),
            jnum(0.0),
            jnum(0.0),
            jnum(0.0),
        ]);
    }
    if dropped > 0 {
        warnings.push(format!(
            "{dropped} generator costs dropped: pandapower poly_cost carries polynomial (model 2) costs only"
        ));
    }
    if truncated > 0 {
        warnings.push(format!(
            "{truncated} generator costs truncated to quadratic: poly_cost carries cp0/cp1/cp2 only"
        ));
    }
    if empty > 0 {
        warnings.push(format!(
            "{empty} generator costs had no coefficients and were written as zero"
        ));
    }
    frame(&columns, index, data)
}

/// pandapower bus column value for a 1-based [`BusId`]: pandapower indices are
/// 0-based, so shift down. The reader shifts back up.
fn pp_bus(id: BusId) -> Value {
    Value::from(id.0.saturating_sub(1) as u64)
}

#[allow(clippy::needless_pass_by_value)] // ownership emphasizes the frame consumes constructed rows
fn frame(columns: &[&str], index: Vec<Value>, data: Vec<Vec<Value>>) -> Value {
    let inner = serde_json::json!({
        "columns": columns,
        "index": index,
        "data": data,
    });
    let dtype = columns
        .iter()
        .map(|c| ((*c).to_string(), Value::String(dtype_for(c).into())))
        .collect();
    let mut m = Map::new();
    m.insert("_module".into(), Value::String("pandas.core.frame".into()));
    m.insert("_class".into(), Value::String("DataFrame".into()));
    m.insert(
        "_object".into(),
        Value::String(serde_json::to_string(&inner).expect("frame inner serializes")),
    );
    m.insert("orient".into(), Value::String("split".into()));
    m.insert("dtype".into(), Value::Object(dtype));
    m.insert("is_multiindex".into(), Value::Bool(false));
    m.insert("is_multicolumn".into(), Value::Bool(false));
    Value::Object(m)
}

fn nonzero_differs(value: f64, reference: f64) -> bool {
    value.abs() > f64::EPSILON && (value - reference).abs() > f64::EPSILON
}

fn dtype_for(column: &str) -> &'static str {
    match column {
        "bus" | "from_bus" | "to_bus" | "hv_bus" | "lv_bus" | "parallel" | "element" => "uint32",
        "in_service" | "slack" | "controllable" => "bool",
        "name" | "type" | "std_type" | "geo" | "et" | "tap_side" => "object",
        _ => "float64",
    }
}

#[derive(Debug)]
struct DataFrame {
    /// Table name, for error messages.
    name: String,
    columns: Vec<String>,
    index: Vec<Value>,
    data: Vec<Vec<Value>>,
}

impl DataFrame {
    fn rows(&self) -> impl Iterator<Item = Row<'_>> {
        (0..self.data.len()).map(|i| Row { frame: self, i })
    }
    fn col(&self, key: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == key)
    }
}

struct Row<'a> {
    frame: &'a DataFrame,
    i: usize,
}

impl Row<'_> {
    /// The pandas index value as a non-negative integer; pandapower element
    /// ids live in the index, so a bad value is an error, not a default.
    /// Values at or above `usize::MAX` are rejected so the float cast is exact
    /// and the bus loop's `+ 1` cannot overflow.
    fn index_usize(&self) -> Result<usize> {
        let v = &self.frame.index[self.i];
        value_usize(v)
            .or_else(|| {
                v.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0 && *f < usize::MAX as f64)
                    .map(|f| f as usize)
            })
            .filter(|&i| i < usize::MAX)
            .ok_or_else(|| {
                bad(format!(
                    "`{}` row at position {}: index is not a non-negative integer (`{}`)",
                    self.frame.name,
                    self.i,
                    value_repr(v)
                ))
            })
    }
    /// Row label for error messages: the pandas index value verbatim, else the
    /// row position.
    fn label(&self) -> String {
        match self.frame.index.get(self.i) {
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::String(s)) => s.clone(),
            _ => format!("position {}", self.i),
        }
    }
    fn get(&self, key: &str) -> Option<&Value> {
        self.frame
            .col(key)
            .and_then(|c| self.frame.data.get(self.i).and_then(|r| r.get(c)))
    }
    fn f_or(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(value_f64).unwrap_or(default)
    }
    /// Required numeric column: a missing, null, or non-numeric cell is an
    /// error, never a default. For columns whose default would silently change
    /// the electrical model (`vn_kv` -> zbase 1.0 reads ohms as per unit).
    fn req_f(&self, key: &str) -> Result<f64> {
        self.get(key).and_then(value_f64).ok_or_else(|| {
            bad(format!(
                "`{}` row {}: required column `{key}` is missing or not numeric",
                self.frame.name,
                self.label()
            ))
        })
    }
    fn f_finite(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(value_f64).filter(|v| v.is_finite())
    }
    fn usize_or(&self, key: &str, default: usize) -> usize {
        self.get(key).and_then(value_usize).unwrap_or(default)
    }
    fn bool_or(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(value_bool).unwrap_or(default)
    }
    fn string(&self, key: &str) -> Option<String> {
        self.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
}

fn read_frame(root: &Map<String, Value>, name: &str) -> Result<Option<DataFrame>> {
    let Some(v) = root.get(name) else {
        return Ok(None);
    };
    let obj = v
        .as_object()
        .ok_or_else(|| bad(format!("`{name}` table is not a DataFrame object")))?;
    if obj.get("is_multicolumn").and_then(Value::as_bool) == Some(true) {
        return Err(bad(format!(
            "`{name}` table: multi-column frames are unsupported"
        )));
    }
    let raw = obj
        .get("_object")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(format!("`{name}` table missing string `_object`")))?;
    let inner: Value =
        serde_json::from_str(raw).map_err(|e| bad(format!("`{name}` table: {e}")))?;
    let inner = inner
        .as_object()
        .ok_or_else(|| bad(format!("`{name}` split payload is not an object")))?;
    let columns = inner
        .get("columns")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(format!("`{name}` split payload missing columns")))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| bad(format!("`{name}` table: column names must be strings")))
        })
        .collect::<Result<Vec<_>>>()?;
    let index = inner
        .get("index")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let raw_data = inner
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(format!("`{name}` split payload missing data")))?;
    let mut data = Vec::with_capacity(raw_data.len());
    for (i, row) in raw_data.iter().enumerate() {
        data.push(
            row.as_array()
                .cloned()
                .ok_or_else(|| bad(format!("`{name}` table: row {i} is not an array")))?,
        );
    }
    if index.len() != data.len() {
        return Err(bad(format!(
            "`{name}` table: index length {} does not match data length {}",
            index.len(),
            data.len()
        )));
    }
    Ok(Some(DataFrame {
        name: name.to_string(),
        columns,
        index,
        data,
    }))
}

fn read_poly_costs(
    root: &Map<String, Value>,
    warnings: &mut Vec<String>,
) -> Result<BTreeMap<(String, usize), GenCost>> {
    let mut out = BTreeMap::new();
    let Some(frame) = read_frame(root, "poly_cost")? else {
        return Ok(out);
    };
    let mut cq_rows = 0_usize;
    for row in frame.rows() {
        let et = row.string("et").unwrap_or_else(|| "gen".into());
        let element = row.usize_or("element", 0);
        if row.f_or("cq2_eur_per_mvar2", 0.0) != 0.0
            || row.f_or("cq1_eur_per_mvar", 0.0) != 0.0
            || row.f_or("cq0_eur", 0.0) != 0.0
        {
            cq_rows += 1;
        }
        out.insert(
            (et, element),
            GenCost {
                model: 2,
                startup: 0.0,
                shutdown: 0.0,
                ncost: 3,
                coeffs: vec![
                    row.f_or("cp2_eur_per_mw2", 0.0),
                    row.f_or("cp1_eur_per_mw", 0.0),
                    row.f_or("cp0_eur", 0.0),
                ],
            },
        );
    }
    if cq_rows > 0 {
        warnings.push(format!(
            "`poly_cost`: reactive cost coefficients (cq*) nonzero on {cq_rows} rows; only active power costs are read"
        ));
    }
    Ok(out)
}

/// Resolve a bus reference cell strictly: a missing, negative, fractional, or
/// unknown value is an error, never a default. Float encoded integers are
/// accepted (pandas dtype maps make bus columns float64 routinely).
fn bus_ref(
    table: &str,
    row: &Row<'_>,
    key: &str,
    bus_of_pp: &HashMap<usize, BusId>,
) -> Result<BusId> {
    let label = row.label();
    let cell = match row.get(key) {
        None | Some(Value::Null) => {
            return Err(bad(format!(
                "`{table}` row {label}: missing bus reference `{key}`"
            )));
        }
        Some(v) => v,
    };
    let idx = decode_bus_index(cell).map_err(|e| match e {
        BusRefError::Negative => bad(format!(
            "`{table}` row {label}: bus reference `{key}` is negative ({})",
            value_repr(cell)
        )),
        BusRefError::NotInteger => bad(format!(
            "`{table}` row {label}: bus reference `{key}` is not an integer (`{}`)",
            value_repr(cell)
        )),
    })?;
    bus_of_pp.get(&idx).copied().ok_or_else(|| {
        bad(format!(
            "`{table}` row {label}: bus reference `{key}` points to unknown bus {idx}"
        ))
    })
}

enum BusRefError {
    Negative,
    NotInteger,
}

fn decode_bus_index(v: &Value) -> std::result::Result<usize, BusRefError> {
    fn from_f64(f: f64) -> std::result::Result<usize, BusRefError> {
        if f.fract() != 0.0 || !f.is_finite() {
            Err(BusRefError::NotInteger)
        } else if f < 0.0 {
            Err(BusRefError::Negative)
        } else {
            Ok(f as usize)
        }
    }
    match v {
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Ok(u as usize)
            } else if n.as_i64().is_some() {
                // as_u64 failed, so the integer is negative.
                Err(BusRefError::Negative)
            } else {
                from_f64(n.as_f64().ok_or(BusRefError::NotInteger)?)
            }
        }
        Value::String(s) => {
            let s = s.trim();
            if let Ok(u) = s.parse::<u64>() {
                Ok(u as usize)
            } else if s.parse::<i64>().is_ok() {
                Err(BusRefError::Negative)
            } else {
                from_f64(s.parse::<f64>().map_err(|_| BusRefError::NotInteger)?)
            }
        }
        _ => Err(BusRefError::NotInteger),
    }
}

/// A cell rendered for an error message: strings verbatim, everything else as
/// its JSON text.
fn value_repr(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn set_bus_kind(buses: &mut [Bus], bus_pos: &HashMap<BusId, usize>, bus: BusId, kind: BusType) {
    if let Some(&idx) = bus_pos.get(&bus) {
        if buses[idx].kind != BusType::Isolated {
            buses[idx].kind = kind;
        }
    }
}

fn bus_kv(buses: &[Bus], bus_pos: &HashMap<BusId, usize>, bus: BusId) -> f64 {
    bus_pos
        .get(&bus)
        .and_then(|&i| buses.get(i))
        .map_or(0.0, |b| b.base_kv)
}

fn zbase(v_kv: f64, base_mva: f64) -> f64 {
    if v_kv > 0.0 && base_mva > 0.0 {
        v_kv * v_kv / base_mva
    } else {
        1.0
    }
}

fn value_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(_) => v.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn value_usize(v: &Value) -> Option<usize> {
    match v {
        Value::Number(_) => v.as_u64().map(|x| x as usize),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn value_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(_) => v.as_f64().map(|x| x != 0.0),
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn bad(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FMT,
        message: message.into(),
    }
}

#[cfg(test)]
// Exact float compares are the point: a mapped value deviating from the
// fixture arithmetic means a column was misread. Helpers take `Value` by
// value for `json!` call site ergonomics.
#[allow(clippy::float_cmp, clippy::needless_pass_by_value)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A split-oriented DataFrame the way pandapower `to_json` encodes it.
    fn pp_frame_raw(columns: Value, index: Value, data: Value) -> Value {
        let inner = json!({ "columns": columns, "index": index, "data": data });
        json!({
            "_module": "pandas.core.frame",
            "_class": "DataFrame",
            "_object": serde_json::to_string(&inner).unwrap(),
            "orient": "split",
            "dtype": {},
            "is_multiindex": false,
            "is_multicolumn": false,
        })
    }

    fn pp_frame(columns: &[&str], index: Value, data: Value) -> Value {
        pp_frame_raw(json!(columns), index, data)
    }

    fn pp_net(tables: Vec<(&str, Value)>) -> String {
        let mut object = Map::new();
        object.insert("sn_mva".into(), json!(100.0));
        object.insert("f_hz".into(), json!(50.0));
        for (name, frame) in tables {
            object.insert(name.into(), frame);
        }
        serde_json::to_string(&json!({
            "_module": "pandapower.auxiliary",
            "_class": "pandapowerNet",
            "_object": object,
        }))
        .unwrap()
    }

    /// `bus` table with the given pandas index values, all 110 kV in service.
    fn bus_table(indices: Value) -> (&'static str, Value) {
        let n = indices.as_array().unwrap().len();
        let data: Vec<Value> = (0..n).map(|_| json!([null, 110.0, true])).collect();
        (
            "bus",
            pp_frame(&["name", "vn_kv", "in_service"], indices, json!(data)),
        )
    }

    fn err(text: &str) -> String {
        parse_pandapower_json(text).unwrap_err().to_string()
    }

    #[test]
    fn bus_ids_shift_pandas_index_by_one() {
        let parsed = parse_pandapower_json(&pp_net(vec![bus_table(json!([0, 1, 2]))])).unwrap();
        let ids: Vec<usize> = parsed.network.buses.iter().map(|b| b.id.0).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn duplicate_bus_index_errors() {
        let msg = err(&pp_net(vec![bus_table(json!([0, 0]))]));
        assert!(msg.contains("`bus` table: duplicate index 0"), "{msg}");
    }

    #[test]
    fn bus_index_must_be_non_negative_integer() {
        let msg = err(&pp_net(vec![bus_table(json!(["x"]))]));
        assert!(
            msg.contains("`bus` row at position 0: index is not a non-negative integer (`x`)"),
            "{msg}"
        );
    }

    fn load_with_bus(bus: Value) -> Vec<(&'static str, Value)> {
        vec![
            bus_table(json!([0, 1])),
            (
                "load",
                pp_frame(&["bus", "p_mw"], json!([0]), json!([[bus, 1.0]])),
            ),
        ]
    }

    #[test]
    fn bus_missing_vn_kv_is_an_error() {
        // vn_kv drives zbase; a default would silently read ohms as per unit.
        let msg = err(&pp_net(vec![(
            "bus",
            pp_frame(&["name", "in_service"], json!([0]), json!([[null, true]])),
        )]));
        assert!(
            msg.contains("`bus` row 0: required column `vn_kv` is missing or not numeric"),
            "{msg}"
        );
        let msg = err(&pp_net(vec![(
            "bus",
            pp_frame(&["vn_kv", "in_service"], json!([0]), json!([[null, true]])),
        )]));
        assert!(
            msg.contains("`bus` row 0: required column `vn_kv` is missing or not numeric"),
            "{msg}"
        );
    }

    #[test]
    fn bus_ref_missing_column() {
        let msg = err(&pp_net(vec![
            bus_table(json!([0])),
            ("load", pp_frame(&["p_mw"], json!([0]), json!([[1.0]]))),
        ]));
        assert!(
            msg.contains("`load` row 0: missing bus reference `bus`"),
            "{msg}"
        );
    }

    #[test]
    fn bus_ref_null_cell() {
        let msg = err(&pp_net(load_with_bus(json!(null))));
        assert!(
            msg.contains("`load` row 0: missing bus reference `bus`"),
            "{msg}"
        );
    }

    #[test]
    fn bus_ref_negative() {
        let msg = err(&pp_net(load_with_bus(json!(-1))));
        assert!(
            msg.contains("`load` row 0: bus reference `bus` is negative (-1)"),
            "{msg}"
        );
    }

    #[test]
    fn bus_ref_fractional() {
        let msg = err(&pp_net(load_with_bus(json!(1.5))));
        assert!(
            msg.contains("`load` row 0: bus reference `bus` is not an integer (`1.5`)"),
            "{msg}"
        );
    }

    #[test]
    fn bus_ref_unparsable_string() {
        let msg = err(&pp_net(load_with_bus(json!("abc"))));
        assert!(
            msg.contains("`load` row 0: bus reference `bus` is not an integer (`abc`)"),
            "{msg}"
        );
    }

    #[test]
    fn bus_ref_unknown_bus() {
        let msg = err(&pp_net(load_with_bus(json!(7))));
        assert!(
            msg.contains("`load` row 0: bus reference `bus` points to unknown bus 7"),
            "{msg}"
        );
    }

    #[test]
    fn bus_ref_accepts_float_encoded_integer() {
        let parsed = parse_pandapower_json(&pp_net(load_with_bus(json!(1.0)))).unwrap();
        assert_eq!(parsed.network.loads[0].bus, BusId(2));
    }

    #[test]
    fn read_frame_rejects_non_string_columns() {
        let frame = pp_frame_raw(json!([1, 2]), json!([0]), json!([[1.0, 2.0]]));
        let msg = err(&pp_net(vec![("bus", frame)]));
        assert!(
            msg.contains("`bus` table: column names must be strings"),
            "{msg}"
        );
    }

    #[test]
    fn read_frame_rejects_multicolumn() {
        let (_, mut frame) = bus_table(json!([0]));
        frame["is_multicolumn"] = json!(true);
        let msg = err(&pp_net(vec![("bus", frame)]));
        assert!(
            msg.contains("`bus` table: multi-column frames are unsupported"),
            "{msg}"
        );
    }

    #[test]
    fn read_frame_rejects_non_array_row() {
        let frame = pp_frame(&["vn_kv"], json!([0]), json!([42]));
        let msg = err(&pp_net(vec![("bus", frame)]));
        assert!(msg.contains("`bus` table: row 0 is not an array"), "{msg}");
    }

    #[test]
    fn read_frame_rejects_index_data_length_mismatch() {
        let frame = pp_frame(&["vn_kv"], json!([0]), json!([[110.0], [110.0]]));
        let msg = err(&pp_net(vec![("bus", frame)]));
        assert!(
            msg.contains("`bus` table: index length 1 does not match data length 2"),
            "{msg}"
        );
    }

    #[test]
    fn sgen_reads_as_pq_generator() {
        let parsed = parse_pandapower_json(&pp_net(vec![
            bus_table(json!([0])),
            (
                "sgen",
                pp_frame(
                    &["bus", "p_mw", "q_mvar", "scaling", "in_service"],
                    json!([0]),
                    json!([[0, 10.0, 2.0, 0.5, true]]),
                ),
            ),
        ]))
        .unwrap();
        let net = &parsed.network;
        assert_eq!(net.generators.len(), 1);
        let g = &net.generators[0];
        assert_eq!(g.bus, BusId(1));
        assert_eq!(g.pg, 5.0);
        assert_eq!(g.qg, 1.0);
        assert_eq!(g.pmax, 10.0);
        assert_eq!(g.pmin, 0.0);
        assert_eq!(g.qmax, f64::INFINITY);
        assert_eq!(g.qmin, f64::NEG_INFINITY);
        assert_eq!(g.vg, 1.0);
        assert_eq!(g.mbase, 100.0);
        // sgen is a PQ injection: the bus kind stays untouched.
        assert_eq!(net.buses[0].kind, BusType::Pq);
    }

    #[test]
    fn storage_maps_soc_and_ratings() {
        let parsed = parse_pandapower_json(&pp_net(vec![
            bus_table(json!([0])),
            (
                "storage",
                pp_frame(
                    &[
                        "bus",
                        "p_mw",
                        "q_mvar",
                        "scaling",
                        "min_e_mwh",
                        "max_e_mwh",
                        "soc_percent",
                        "max_p_mw",
                        "min_p_mw",
                        "sn_mva",
                        "min_q_mvar",
                        "max_q_mvar",
                        "in_service",
                    ],
                    json!([0]),
                    json!([[
                        0, 2.0, 0.5, 1.0, 10.0, 50.0, 25.0, 4.0, -3.0, 6.0, -1.0, 1.0, true
                    ]]),
                ),
            ),
        ]))
        .unwrap();
        let st = &parsed.network.storage[0];
        assert_eq!(st.bus, BusId(1));
        assert_eq!(st.ps, 2.0);
        assert_eq!(st.qs, 0.5);
        assert_eq!(st.energy, 10.0 + (50.0 - 10.0) * 25.0 / 100.0);
        assert_eq!(st.energy_rating, 50.0);
        assert_eq!(st.charge_rating, 4.0);
        assert_eq!(st.discharge_rating, 3.0);
        assert_eq!(st.thermal_rating, 6.0);
        assert_eq!(st.qmin, -1.0);
        assert_eq!(st.qmax, 1.0);
        assert_eq!(st.charge_efficiency, 1.0);
        assert_eq!(st.discharge_efficiency, 1.0);
        assert_eq!(st.r, 0.0);
        assert_eq!(st.x, 0.0);
    }

    #[test]
    fn storage_rating_fallbacks() {
        let parsed = parse_pandapower_json(&pp_net(vec![
            bus_table(json!([0])),
            (
                "storage",
                pp_frame(
                    &["bus", "p_mw", "max_e_mwh"],
                    json!([0]),
                    json!([[0, -2.5, 8.0]]),
                ),
            ),
        ]))
        .unwrap();
        let st = &parsed.network.storage[0];
        assert_eq!(st.charge_rating, 2.5);
        assert_eq!(st.discharge_rating, 2.5);
        assert_eq!(st.thermal_rating, 2.5);
        assert_eq!(st.energy, 8.0 * 0.0 / 100.0);
    }

    #[test]
    fn dcline_maps_to_hvdc() {
        let parsed = parse_pandapower_json(&pp_net(vec![
            bus_table(json!([0, 1])),
            (
                "dcline",
                pp_frame(
                    &[
                        "from_bus",
                        "to_bus",
                        "p_mw",
                        "loss_mw",
                        "loss_percent",
                        "vm_from_pu",
                        "vm_to_pu",
                        "max_p_mw",
                        "min_q_from_mvar",
                        "max_q_from_mvar",
                        "min_q_to_mvar",
                        "max_q_to_mvar",
                        "in_service",
                    ],
                    json!([0]),
                    json!([[
                        0, 1, 2.0, 0.05, 1.0, 1.01, 1.0, 3.0, -1.0, 1.0, -2.0, 2.0, true
                    ]]),
                ),
            ),
        ]))
        .unwrap();
        let d = &parsed.network.hvdc[0];
        assert_eq!(d.from, BusId(1));
        assert_eq!(d.to, BusId(2));
        assert_eq!(d.pf, 2.0);
        assert_eq!(d.pt, 2.0 - 0.05 - 2.0 * 1.0 / 100.0);
        assert_eq!(d.loss0, 0.05);
        assert_eq!(d.loss1, 0.01);
        assert_eq!(d.vf, 1.01);
        assert_eq!(d.vt, 1.0);
        assert_eq!(d.pmin, 0.0);
        assert_eq!(d.pmax, 3.0);
        assert_eq!((d.qminf, d.qmaxf), (-1.0, 1.0));
        assert_eq!((d.qmint, d.qmaxt), (-2.0, 2.0));
        assert_eq!((d.qf, d.qt), (0.0, 0.0));
    }

    #[test]
    fn dcline_defaults() {
        let parsed = parse_pandapower_json(&pp_net(vec![
            bus_table(json!([0, 1])),
            (
                "dcline",
                pp_frame(
                    &["from_bus", "to_bus", "p_mw"],
                    json!([0]),
                    json!([[0, 1, 5.0]]),
                ),
            ),
        ]))
        .unwrap();
        let d = &parsed.network.hvdc[0];
        assert_eq!(d.pt, 5.0);
        assert_eq!((d.vf, d.vt), (1.0, 1.0));
        assert_eq!(d.pmax, f64::INFINITY);
        assert_eq!(d.qminf, f64::NEG_INFINITY);
        assert_eq!(d.qmaxt, f64::INFINITY);
        assert!(d.in_service);
    }

    #[test]
    fn line_parallel_scales_impedance_and_rating() {
        let parsed = parse_pandapower_json(&pp_net(vec![
            bus_table(json!([0, 1])),
            (
                "line",
                pp_frame(
                    &[
                        "from_bus",
                        "to_bus",
                        "length_km",
                        "r_ohm_per_km",
                        "x_ohm_per_km",
                        "c_nf_per_km",
                        "max_i_ka",
                        "parallel",
                    ],
                    json!([0]),
                    json!([[0, 1, 4.0, 1.0, 2.0, 100.0, 0.5, 2.0]]),
                ),
            ),
        ]))
        .unwrap();
        // length_km = 4 scales r/x and the charging b (pandapower build_branch
        // multiplies c_nf_per_km by the line length).
        let br = &parsed.network.branches[0];
        let zb = 110.0 * 110.0 / 100.0;
        assert!((br.r - 1.0 * 4.0 / zb / 2.0).abs() < 1e-12);
        assert!((br.x - 2.0 * 4.0 / zb / 2.0).abs() < 1e-12);
        let b = 100.0e-9 * 4.0 * 2.0 * std::f64::consts::PI * 50.0 * zb * 2.0;
        assert!((br.b - b).abs() < 1e-12);
        assert!((br.rate_a - 0.5 * 110.0 * 3.0_f64.sqrt() * 2.0).abs() < 1e-9);
    }

    fn trafo_net(columns: &[&str], row: Value) -> String {
        pp_net(vec![
            bus_table(json!([0, 1])),
            ("trafo", pp_frame(columns, json!([0]), json!([row]))),
        ])
    }

    #[test]
    fn trafo_parallel_scales_impedance_and_rating() {
        let parsed = parse_pandapower_json(&trafo_net(
            &[
                "hv_bus",
                "lv_bus",
                "sn_mva",
                "vk_percent",
                "vkr_percent",
                "parallel",
            ],
            json!([0, 1, 50.0, 10.0, 4.0, 2.0]),
        ))
        .unwrap();
        let br = &parsed.network.branches[0];
        let r0: f64 = 4.0 * 100.0 / (50.0 * 100.0);
        let z0: f64 = 10.0 * 100.0 / (50.0 * 100.0);
        let x0 = (z0 * z0 - r0 * r0).sqrt();
        assert!((br.r - r0 / 2.0).abs() < 1e-12);
        assert!((br.x - x0 / 2.0).abs() < 1e-12);
        assert_eq!(br.rate_a, 100.0);
    }

    #[test]
    fn trafo_tap_uses_neutral_offset() {
        let parsed = parse_pandapower_json(&trafo_net(
            &[
                "hv_bus",
                "lv_bus",
                "vk_percent",
                "tap_neutral",
                "tap_pos",
                "tap_step_percent",
            ],
            json!([0, 1, 10.0, 1.0, 3.0, 2.0]),
        ))
        .unwrap();
        let br = &parsed.network.branches[0];
        assert!((br.tap - 1.04).abs() < 1e-12);
    }

    #[test]
    fn trafo_without_tap_columns_keeps_tap_one() {
        let parsed = parse_pandapower_json(&trafo_net(
            &["hv_bus", "lv_bus", "vk_percent"],
            json!([0, 1, 10.0]),
        ))
        .unwrap();
        assert_eq!(parsed.network.branches[0].tap, 1.0);
    }

    #[test]
    fn trafo_lv_tap_side_ignored_with_warning() {
        let parsed = parse_pandapower_json(&trafo_net(
            &[
                "hv_bus",
                "lv_bus",
                "vk_percent",
                "tap_side",
                "tap_pos",
                "tap_step_percent",
            ],
            json!([0, 1, 10.0, "LV", 3.0, 2.0]),
        ))
        .unwrap();
        assert_eq!(parsed.network.branches[0].tap, 1.0);
        assert!(
            parsed.warnings.iter().any(|w| w
                == "`trafo`: tap_side == \"lv\" on 1 rows; taps are modeled on the hv side, lv taps were ignored"),
            "{:?}",
            parsed.warnings
        );
    }

    #[test]
    fn ignored_tables_warn_with_counts() {
        let one_row = || pp_frame(&["x"], json!([0]), json!([[1]]));
        let parsed = parse_pandapower_json(&pp_net(vec![
            bus_table(json!([0])),
            ("trafo3w", one_row()),
            ("ward", one_row()),
            ("xward", one_row()),
            ("impedance", one_row()),
            ("motor", one_row()),
            ("switch", one_row()),
            ("pwl_cost", one_row()),
        ]))
        .unwrap();
        for expected in [
            "`trafo3w` table ignored (1 rows): three winding transformers are not mapped",
            "`ward` table ignored (1 rows): Ward equivalents are not mapped",
            "`xward` table ignored (1 rows): extended Ward equivalents are not mapped",
            "`impedance` table ignored (1 rows): bus-to-bus impedance elements are not mapped",
            "`motor` table ignored (1 rows): motors are not mapped",
            "`switch` table ignored (1 rows): switches are not modeled; open switches are not applied",
            "`pwl_cost` table ignored (1 rows): piecewise costs are not mapped",
        ] {
            assert!(
                parsed.warnings.iter().any(|w| w == expected),
                "missing {expected:?} in {:?}",
                parsed.warnings
            );
        }
    }

    #[test]
    fn poly_cost_cq_coefficients_warn() {
        let parsed = parse_pandapower_json(&pp_net(vec![
            bus_table(json!([0])),
            (
                "gen",
                pp_frame(&["bus", "p_mw"], json!([0]), json!([[0, 1.0]])),
            ),
            (
                "poly_cost",
                pp_frame(
                    &["et", "element", "cp1_eur_per_mw", "cq1_eur_per_mvar"],
                    json!([0]),
                    json!([["gen", 0, 2.5, 1.0]]),
                ),
            ),
        ]))
        .unwrap();
        let cost = parsed.network.generators[0].cost.as_ref().expect("cost");
        assert_eq!(cost.coeffs, vec![0.0, 2.5, 0.0]);
        assert!(
            parsed.warnings.iter().any(|w| w
                == "`poly_cost`: reactive cost coefficients (cq*) nonzero on 1 rows; only active power costs are read"),
            "{:?}",
            parsed.warnings
        );
    }

    #[test]
    fn empty_switch_table_does_not_warn() {
        let parsed = parse_pandapower_json(&pp_net(vec![
            bus_table(json!([0])),
            ("switch", pp_frame(&["bus"], json!([]), json!([]))),
        ]))
        .unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    }

    #[test]
    fn column_semantics_warn_with_counts() {
        let parsed = parse_pandapower_json(&pp_net(vec![
            bus_table(json!([0, 1])),
            (
                "load",
                pp_frame(
                    &["bus", "p_mw", "const_z_percent", "const_i_percent"],
                    json!([0, 1]),
                    json!([[0, 1.0, 20.0, 0.0], [0, 1.0, 0.0, 0.0]]),
                ),
            ),
            (
                "line",
                pp_frame(
                    &["from_bus", "to_bus", "g_us_per_km"],
                    json!([0]),
                    json!([[0, 1, 1.0]]),
                ),
            ),
            (
                "trafo",
                pp_frame(
                    &["hv_bus", "lv_bus", "vk_percent", "i0_percent", "pfe_kw"],
                    json!([0]),
                    json!([[0, 1, 10.0, 0.1, 0.0]]),
                ),
            ),
        ]))
        .unwrap();
        for expected in [
            "`load`: ZIP composition (const_z_percent/const_i_percent) nonzero on 1 rows; loads are read as constant power",
            "`line`: g_us_per_km nonzero on 1 rows; line shunt conductance is not representable and was ignored",
            "`trafo`: i0_percent/pfe_kw nonzero on 1 rows; the magnetizing branch is not representable and was ignored",
        ] {
            assert!(
                parsed.warnings.iter().any(|w| w == expected),
                "missing {expected:?} in {:?}",
                parsed.warnings
            );
        }
    }

    // --- writer ---

    fn test_bus(id: usize, kind: BusType) -> Bus {
        Bus {
            id: BusId(id),
            kind,
            vm: 1.02,
            va: 3.0,
            base_kv: 110.0,
            vmax: 1.1,
            vmin: 0.9,
            area: 1,
            zone: 1,
            name: None,
            extras: Extras::default(),
        }
    }

    fn test_net(buses: Vec<Bus>) -> Network {
        Network {
            name: "t".into(),
            base_mva: 100.0,
            buses,
            loads: Vec::new(),
            shunts: Vec::new(),
            branches: Vec::new(),
            generators: Vec::new(),
            storage: Vec::new(),
            hvdc: Vec::new(),
            source_format: SourceFormat::InMemory,
            source: None,
        }
    }

    fn test_gen(bus: usize, cost: Option<GenCost>) -> Generator {
        Generator {
            bus: BusId(bus),
            pg: 1.0,
            qg: 0.0,
            pmax: 2.0,
            pmin: 0.0,
            qmax: 1.0,
            qmin: -1.0,
            vg: 1.0,
            mbase: 100.0,
            in_service: true,
            cost,
            caps: [None; crate::network::GEN_EXTRA_KEYS.len()],
        }
    }

    fn test_branch(from: usize, to: usize, tap: f64) -> Branch {
        Branch {
            from: BusId(from),
            to: BusId(to),
            r: 0.01,
            x: 0.1,
            b: 0.0,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
            tap,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            extras: Extras::default(),
        }
    }

    fn poly(coeffs: Vec<f64>) -> GenCost {
        GenCost {
            model: 2,
            startup: 0.0,
            shutdown: 0.0,
            ncost: coeffs.len(),
            coeffs,
        }
    }

    /// Decode a frame back out of written JSON via the reader codec.
    fn written_frame(text: &str, table: &str) -> DataFrame {
        let root: Value = serde_json::from_str(text).unwrap();
        let obj = root["_object"].as_object().unwrap();
        read_frame(obj, table).unwrap().unwrap()
    }

    fn col(frame: &DataFrame, key: &str) -> Vec<Value> {
        let c = frame.col(key).unwrap();
        frame.data.iter().map(|r| r[c].clone()).collect()
    }

    #[test]
    fn writer_emits_zero_based_frames() {
        let mut net = test_net(vec![
            test_bus(1, BusType::Pq),
            test_bus(2, BusType::Pq),
            test_bus(3, BusType::Ref),
        ]);
        net.loads.push(Load {
            bus: BusId(2),
            p: 1.0,
            q: 0.0,
            in_service: true,
            extras: Extras::default(),
        });
        net.generators.push(test_gen(3, None));
        // Interleave: line, trafo, line — per table indices must stay contiguous.
        net.branches.push(test_branch(1, 2, 0.0));
        net.branches.push(test_branch(2, 3, 1.05));
        net.branches.push(test_branch(1, 3, 0.0));
        let conv = write_pandapower_json(&net);

        let bus = written_frame(&conv.text, "bus");
        assert_eq!(bus.index, vec![json!(0), json!(1), json!(2)]);
        let load = written_frame(&conv.text, "load");
        assert_eq!(load.index, vec![json!(0)]);
        assert_eq!(col(&load, "bus"), vec![json!(1)]);
        let gen_tbl = written_frame(&conv.text, "gen");
        assert_eq!(gen_tbl.index, vec![json!(0)]);
        assert_eq!(col(&gen_tbl, "bus"), vec![json!(2)]);
        let line = written_frame(&conv.text, "line");
        assert_eq!(line.index, vec![json!(0), json!(1)]);
        assert_eq!(col(&line, "from_bus"), vec![json!(0), json!(0)]);
        assert_eq!(col(&line, "to_bus"), vec![json!(1), json!(2)]);
        let trafo = written_frame(&conv.text, "trafo");
        assert_eq!(trafo.index, vec![json!(0)]);
        assert_eq!(col(&trafo, "hv_bus"), vec![json!(1)]);
        assert_eq!(col(&trafo, "lv_bus"), vec![json!(2)]);
    }

    #[test]
    fn writer_ext_grid_row_for_generator_less_ref_bus() {
        let mut net = test_net(vec![test_bus(1, BusType::Pq), test_bus(2, BusType::Ref)]);
        net.buses[1].name = Some("slack".into());
        let conv = write_pandapower_json(&net);
        let eg = written_frame(&conv.text, "ext_grid");
        assert_eq!(eg.index, vec![json!(0)]);
        assert_eq!(
            eg.data[0],
            vec![
                json!("slack"),
                json!(1),
                json!(1.02),
                json!(3.0),
                json!(1.0),
                json!(true),
                json!(true),
            ]
        );
    }

    #[test]
    fn writer_ext_grid_empty_when_ref_bus_has_generator() {
        let mut net = test_net(vec![test_bus(1, BusType::Ref)]);
        net.generators.push(test_gen(1, None));
        let conv = write_pandapower_json(&net);
        let eg = written_frame(&conv.text, "ext_grid");
        assert!(eg.data.is_empty());
        // The slack generator stays in the gen table.
        let gen_tbl = written_frame(&conv.text, "gen");
        assert_eq!(col(&gen_tbl, "slack"), vec![json!(true)]);
    }

    #[test]
    fn poly_cost_keeps_lowest_order_terms() {
        let mut net = test_net(vec![test_bus(1, BusType::Ref)]);
        net.generators
            .push(test_gen(1, Some(poly(vec![9.0, 3.0, 2.0, 1.0]))));
        let conv = write_pandapower_json(&net);
        let pc = written_frame(&conv.text, "poly_cost");
        assert_eq!(col(&pc, "cp0_eur"), vec![json!(1.0)]);
        assert_eq!(col(&pc, "cp1_eur_per_mw"), vec![json!(2.0)]);
        assert_eq!(col(&pc, "cp2_eur_per_mw2"), vec![json!(3.0)]);
        assert!(
            conv.warnings.iter().any(|w| w
                == "1 generator costs truncated to quadratic: poly_cost carries cp0/cp1/cp2 only"),
            "{:?}",
            conv.warnings
        );
    }

    #[test]
    fn poly_cost_warnings_and_zero_based_keys() {
        let mut net = test_net(vec![test_bus(1, BusType::Ref)]);
        let piecewise = GenCost {
            model: 1,
            startup: 0.0,
            shutdown: 0.0,
            ncost: 2,
            coeffs: vec![0.0, 0.0, 1.0, 1.0],
        };
        net.generators.push(test_gen(1, Some(piecewise)));
        net.generators
            .push(test_gen(1, Some(poly(vec![4.0, 3.0, 2.0, 1.0]))));
        net.generators.push(test_gen(1, Some(poly(Vec::new()))));
        let conv = write_pandapower_json(&net);
        let pc = written_frame(&conv.text, "poly_cost");
        // gen 0 (piecewise) dropped; gens 1 and 2 written with 0-based
        // element = generator position and a contiguous 0-based index.
        assert_eq!(pc.index, vec![json!(0), json!(1)]);
        assert_eq!(col(&pc, "element"), vec![json!(1), json!(2)]);
        for expected in [
            "1 generator costs dropped: pandapower poly_cost carries polynomial (model 2) costs only",
            "1 generator costs truncated to quadratic: poly_cost carries cp0/cp1/cp2 only",
            "1 generator costs had no coefficients and were written as zero",
        ] {
            assert!(
                conv.warnings.iter().any(|w| w == expected),
                "missing {expected:?} in {:?}",
                conv.warnings
            );
        }
    }
}

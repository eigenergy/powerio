//! Read and write pandapower `pandapowerNet` JSON.
//!
//! pandapower serializes each element table as a pandas split-oriented
//! `DataFrame` encoded inside a JSON string. This module implements that small
//! table codec directly so the Rust core stays Python-free.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde_json::{Map, Value};

use super::{Conversion, finish, jnum};
use crate::network::{
    Branch, Bus, BusId, BusType, Extras, GenCost, Generator, Load, Network, Shunt, SourceFormat,
};
use crate::{Error, Result};

const FMT: &str = "pandapower JSON";
const F_HZ: f64 = 50.0;
const MAX_I_KA: f64 = 99_999.0;

pub fn parse_pandapower_json(content: &str) -> Result<Network> {
    parse_pandapower_source(Arc::new(content.to_owned()), None)
}

#[allow(clippy::too_many_lines)] // direct table-to-Network mapper; split helpers obscure column mapping
pub(crate) fn parse_pandapower_source(
    source: Arc<String>,
    name_hint: Option<&str>,
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
        let pp_idx = row.index_usize();
        let id = BusId(pp_idx);
        bus_of_pp.insert(pp_idx, id);
        buses.push(Bus {
            id,
            kind: if row.bool_or("in_service", true) {
                BusType::Pq
            } else {
                BusType::Isolated
            },
            vm: 1.0,
            va: 0.0,
            base_kv: row.f_or("vn_kv", 0.0),
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
        for row in load_frame.rows() {
            let scale = row.f_or("scaling", 1.0);
            loads.push(Load {
                bus: bus_ref(&row, "bus", &bus_of_pp),
                p: row.f_or("p_mw", 0.0) * scale,
                q: row.f_or("q_mvar", 0.0) * scale,
                in_service: row.bool_or("in_service", true),
                extras: Extras::default(),
            });
        }
    }

    let mut shunts = Vec::new();
    if let Some(shunt_frame) = read_frame(obj, "shunt")? {
        for row in shunt_frame.rows() {
            let step = row.f_or("step", 1.0);
            shunts.push(Shunt {
                bus: bus_ref(&row, "bus", &bus_of_pp),
                g: row.f_or("p_mw", 0.0) * step,
                b: -row.f_or("q_mvar", 0.0) * step,
                in_service: row.bool_or("in_service", true),
                extras: Extras::default(),
            });
        }
    }

    let costs = read_poly_costs(obj)?;
    let mut generators = Vec::new();
    if let Some(gen_frame) = read_frame(obj, "gen")? {
        for row in gen_frame.rows() {
            let idx = row.index_usize();
            let bus = bus_ref(&row, "bus", &bus_of_pp);
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
            let idx = row.index_usize();
            let bus = bus_ref(&row, "bus", &bus_of_pp);
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

    let mut branches = Vec::new();
    if let Some(line_frame) = read_frame(obj, "line")? {
        for row in line_frame.rows() {
            let from = bus_ref(&row, "from_bus", &bus_of_pp);
            let to = bus_ref(&row, "to_bus", &bus_of_pp);
            let v_to = bus_kv(&buses, &bus_pos, to);
            let zbase = zbase(v_to, base_mva);
            let rate_a = row.f_or("max_i_ka", 0.0) * v_to * 3.0_f64.sqrt();
            branches.push(Branch {
                from,
                to,
                r: row.f_or("r_ohm_per_km", 0.0) * row.f_or("length_km", 1.0) / zbase,
                x: row.f_or("x_ohm_per_km", 0.0) * row.f_or("length_km", 1.0) / zbase,
                b: row.f_or("c_nf_per_km", 0.0) * 1e-9 * 2.0 * std::f64::consts::PI * f_hz * zbase,
                rate_a: if rate_a >= MAX_I_KA * v_to {
                    0.0
                } else {
                    rate_a
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
    }
    if let Some(trafo_frame) = read_frame(obj, "trafo")? {
        for row in trafo_frame.rows() {
            let from = bus_ref(&row, "hv_bus", &bus_of_pp);
            let to = bus_ref(&row, "lv_bus", &bus_of_pp);
            let sn = row.f_or("sn_mva", base_mva);
            let r = row.f_or("vkr_percent", 0.0) * base_mva / (sn * 100.0);
            let z = row.f_or("vk_percent", 0.0).abs() * base_mva / (sn * 100.0);
            let x = (z * z - r * r).max(0.0).sqrt() * row.f_or("vk_percent", 0.0).signum();
            let tap_step = row.f_or("tap_step_percent", 0.0) / 100.0;
            let tap_pos = row.f_or("tap_pos", 0.0);
            branches.push(Branch {
                from,
                to,
                r,
                x,
                b: 0.0,
                rate_a: sn,
                rate_b: 0.0,
                rate_c: 0.0,
                tap: if tap_step == 0.0 {
                    1.0
                } else {
                    1.0 + tap_pos * tap_step
                },
                shift: row.f_or("shift_degree", 0.0),
                in_service: row.bool_or("in_service", true),
                angmin: -360.0,
                angmax: 360.0,
                extras: Extras::default(),
            });
        }
    }

    let net = Network {
        name,
        base_mva,
        buses,
        loads,
        shunts,
        branches,
        generators,
        storage: Vec::new(),
        hvdc: Vec::new(),
        source_format: SourceFormat::PandapowerJson,
        source: Some(source),
    };
    net.check_references(FMT)?;
    Ok(net)
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
    object.insert(
        "ext_grid".into(),
        empty_frame(&[
            "name",
            "bus",
            "vm_pu",
            "va_degree",
            "slack_weight",
            "in_service",
            "controllable",
        ]),
    );
    let (line, trafo) = branch_frames(net);
    object.insert("line".into(), line);
    object.insert("trafo".into(), trafo);
    object.insert("poly_cost".into(), poly_cost_frame(net));
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
        index.push(Value::from(b.id.0 as u64));
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
    for (i, l) in net.loads.iter().enumerate() {
        index.push(Value::from((i + 1) as u64));
        data.push(vec![
            Value::Null,
            Value::from(l.bus.0 as u64),
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
    for (i, s) in net.shunts.iter().enumerate() {
        index.push(Value::from((i + 1) as u64));
        data.push(vec![
            Value::from(s.bus.0 as u64),
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
    for (i, g) in net.generators.iter().enumerate() {
        index.push(Value::from((i + 1) as u64));
        data.push(vec![
            Value::Null,
            Value::from(g.bus.0 as u64),
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
    for (i, br) in net.branches.iter().enumerate() {
        let idx = i + 1;
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
            trafo_index.push(Value::from(idx as u64));
            trafo_data.push(vec![
                Value::Null,
                Value::Null,
                Value::from(br.from.0 as u64),
                Value::from(br.to.0 as u64),
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
            line_index.push(Value::from(idx as u64));
            line_data.push(vec![
                Value::Null,
                Value::Null,
                Value::from(br.from.0 as u64),
                Value::from(br.to.0 as u64),
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

fn poly_cost_frame(net: &Network) -> Value {
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
    for (i, g) in net.generators.iter().enumerate() {
        let Some(cost) = &g.cost else {
            continue;
        };
        if cost.model != 2 {
            continue;
        }
        let (c2, c1, c0) = match cost.coeffs.as_slice() {
            [c2, c1, c0, ..] => (*c2, *c1, *c0),
            [c1, c0] => (0.0, *c1, *c0),
            [c0] => (0.0, 0.0, *c0),
            _ => (0.0, 0.0, 0.0),
        };
        index.push(Value::from((data.len() + 1) as u64));
        data.push(vec![
            Value::from((i + 1) as u64),
            Value::String("gen".into()),
            jnum(c0),
            jnum(c1),
            jnum(c2),
            jnum(0.0),
            jnum(0.0),
            jnum(0.0),
        ]);
    }
    frame(&columns, index, data)
}

fn empty_frame(columns: &[&str]) -> Value {
    frame(columns, Vec::new(), Vec::new())
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
    fn index_usize(&self) -> usize {
        value_usize(&self.frame.index[self.i]).unwrap_or(self.i + 1)
    }
    fn get(&self, key: &str) -> Option<&Value> {
        self.frame
            .col(key)
            .and_then(|c| self.frame.data.get(self.i).and_then(|r| r.get(c)))
    }
    fn f_or(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(value_f64).unwrap_or(default)
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
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect();
    let index = inner
        .get("index")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let data = inner
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(format!("`{name}` split payload missing data")))?
        .iter()
        .map(|row| row.as_array().cloned().unwrap_or_default())
        .collect();
    Ok(Some(DataFrame {
        columns,
        index,
        data,
    }))
}

fn read_poly_costs(root: &Map<String, Value>) -> Result<BTreeMap<(String, usize), GenCost>> {
    let mut out = BTreeMap::new();
    let Some(frame) = read_frame(root, "poly_cost")? else {
        return Ok(out);
    };
    for row in frame.rows() {
        let et = row.string("et").unwrap_or_else(|| "gen".into());
        let element = row.usize_or("element", 0);
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
    Ok(out)
}

fn bus_ref(row: &Row<'_>, key: &str, bus_of_pp: &HashMap<usize, BusId>) -> BusId {
    let raw = row.usize_or(key, 0);
    bus_of_pp.get(&raw).copied().unwrap_or(BusId(raw))
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

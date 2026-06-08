//! Read and write Surge native `surge-json` network documents.
//!
//! This module deliberately depends only on `serde_json` and the `Network` hub.
//! Surge's native JSON envelope is versioned and the network body is richer than
//! powerio's neutral model, so the reader maps the electrical core and the writer
//! emits a network profile document that Surge can load. Same-format byte-exact
//! writes still use the retained source in the format hub.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use super::{Conversion, finish, jnum};
use crate::network::{
    Branch, Bus, BusId, BusType, Extras, GEN_EXTRA_KEYS, GenCaps, GenCost, Generator, Hvdc, Load,
    Network, Shunt, SourceFormat, Storage,
};
use crate::{Error, Result};

const FMT: &str = "Surge JSON";
const FORMAT_VALUE: &str = "surge-json";
const SCHEMA_VERSION: &str = "0.1.0";
const DEG_PER_RAD: f64 = 180.0 / std::f64::consts::PI;
const EPS: f64 = 1e-12;

#[must_use]
pub fn write_surge_json(net: &Network) -> Conversion {
    let mut warnings = Vec::new();
    let mut network = Map::new();

    network.insert("name".into(), Value::String(net.name.clone()));
    network.insert("base_mva".into(), jnum(net.base_mva));
    network.insert("freq_hz".into(), jnum(60.0));

    let buses = net.buses.iter().map(bus_obj).collect();
    network.insert("buses".into(), Value::Array(buses));

    let loads = net.loads.iter().enumerate().map(load_obj).collect();
    network.insert("loads".into(), Value::Array(loads));

    let shunts = net.shunts.iter().enumerate().map(shunt_obj).collect();
    network.insert("fixed_shunts".into(), Value::Array(shunts));

    let branches = net.branches.iter().enumerate().map(branch_obj).collect();
    network.insert("branches".into(), Value::Array(branches));

    let mut gen_counts: BTreeMap<BusId, usize> = BTreeMap::new();
    let mut generators = Vec::new();
    for g in &net.generators {
        generators.push(gen_obj(g, &mut gen_counts, &mut warnings));
    }
    for st in &net.storage {
        generators.push(storage_gen_obj(st, &mut gen_counts));
    }
    network.insert("generators".into(), Value::Array(generators));

    if !net.hvdc.is_empty() {
        let links = net
            .hvdc
            .iter()
            .enumerate()
            .map(|(i, dc)| hvdc_link_obj(dc, i, &mut warnings))
            .collect();
        let mut hvdc = Map::new();
        hvdc.insert("links".into(), Value::Array(links));
        network.insert("hvdc".into(), Value::Object(hvdc));
    }

    network.insert("metadata".into(), Value::Object(Map::new()));
    network.insert("market_data".into(), Value::Object(Map::new()));
    network.insert("controls".into(), Value::Object(Map::new()));
    network.insert("cim".into(), Value::Object(Map::new()));

    let mut meta = Map::new();
    meta.insert("producer".into(), Value::String("surge".into()));
    meta.insert("profile".into(), Value::String("network".into()));

    let mut root = Map::new();
    root.insert("format".into(), Value::String(FORMAT_VALUE.into()));
    root.insert(
        "schema_version".into(),
        Value::String(SCHEMA_VERSION.into()),
    );
    root.insert("meta".into(), Value::Object(meta));
    root.insert("network".into(), Value::Object(network));

    finish(root, warnings)
}

fn bus_type(kind: BusType) -> &'static str {
    match kind {
        BusType::Pq => "PQ",
        BusType::Pv => "PV",
        BusType::Ref => "Slack",
        BusType::Isolated => "Isolated",
    }
}

fn bus_obj(b: &Bus) -> Value {
    let mut m = Map::new();
    m.insert("number".into(), Value::from(b.id.0 as u64));
    m.insert(
        "name".into(),
        Value::String(b.name.clone().unwrap_or_default()),
    );
    m.insert("bus_type".into(), Value::String(bus_type(b.kind).into()));
    m.insert("base_kv".into(), jnum(b.base_kv));
    m.insert("voltage_magnitude_pu".into(), jnum(b.vm));
    m.insert("voltage_angle_rad".into(), jnum(b.va.to_radians()));
    m.insert("voltage_min_pu".into(), jnum(b.vmin));
    m.insert("voltage_max_pu".into(), jnum(b.vmax));
    m.insert("shunt_conductance_mw".into(), jnum(0.0));
    m.insert("shunt_susceptance_mvar".into(), jnum(0.0));
    m.insert("area".into(), Value::from(b.area as u64));
    m.insert("zone".into(), Value::from(b.zone as u64));
    m.insert("island_id".into(), Value::from(0_u64));
    Value::Object(m)
}

fn load_obj((i, l): (usize, &Load)) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), Value::String(format!("load_{}", i + 1)));
    m.insert("bus".into(), Value::from(l.bus.0 as u64));
    m.insert("active_power_demand_mw".into(), jnum(l.p));
    m.insert("reactive_power_demand_mvar".into(), jnum(l.q));
    m.insert("in_service".into(), Value::Bool(l.in_service));
    m.insert("conforming".into(), Value::Bool(true));
    m.insert("connection".into(), Value::String("WyeGrounded".into()));
    m.insert("zip_p_impedance_frac".into(), jnum(0.0));
    m.insert("zip_p_current_frac".into(), jnum(0.0));
    m.insert("zip_p_power_frac".into(), jnum(1.0));
    m.insert("zip_q_impedance_frac".into(), jnum(0.0));
    m.insert("zip_q_current_frac".into(), jnum(0.0));
    m.insert("zip_q_power_frac".into(), jnum(1.0));
    Value::Object(m)
}

fn shunt_obj((i, s): (usize, &Shunt)) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), Value::String(format!("shunt_{}", i + 1)));
    m.insert("bus".into(), Value::from(s.bus.0 as u64));
    m.insert("g_mw".into(), jnum(s.g));
    m.insert("b_mvar".into(), jnum(s.b));
    m.insert("in_service".into(), Value::Bool(s.in_service));
    m.insert(
        "shunt_type".into(),
        Value::String(if s.b < 0.0 { "Reactor" } else { "Capacitor" }.into()),
    );
    Value::Object(m)
}

fn branch_obj((_i, br): (usize, &Branch)) -> Value {
    let mut m = Map::new();
    m.insert("from_bus".into(), Value::from(br.from.0 as u64));
    m.insert("to_bus".into(), Value::from(br.to.0 as u64));
    m.insert("circuit".into(), Value::String("1".into()));
    m.insert("r".into(), jnum(br.r));
    m.insert("x".into(), jnum(br.x));
    m.insert("b".into(), jnum(br.b));
    m.insert("tap".into(), jnum(br.effective_tap()));
    m.insert("phase_shift_rad".into(), jnum(br.shift.to_radians()));
    m.insert("rating_a_mva".into(), jnum(br.rate_a));
    m.insert("rating_b_mva".into(), jnum(br.rate_b));
    m.insert("rating_c_mva".into(), jnum(br.rate_c));
    m.insert("in_service".into(), Value::Bool(br.in_service));
    m.insert(
        "branch_type".into(),
        Value::String(
            if br.is_transformer() {
                "Transformer"
            } else {
                "Line"
            }
            .into(),
        ),
    );
    m.insert("angle_diff_min_rad".into(), jnum(br.angmin.to_radians()));
    m.insert("angle_diff_max_rad".into(), jnum(br.angmax.to_radians()));
    m.insert("g_pi".into(), jnum(0.0));
    m.insert("g_mag".into(), jnum(0.0));
    m.insert("b_mag".into(), jnum(0.0));
    Value::Object(m)
}

fn next_id(prefix: &str, counts: &mut BTreeMap<BusId, usize>, bus: BusId) -> String {
    let count = counts.entry(bus).or_insert(0);
    *count += 1;
    format!("{prefix}_{}_{}", bus.0, *count)
}

fn gen_obj(
    g: &Generator,
    counts: &mut BTreeMap<BusId, usize>,
    warnings: &mut Vec<String>,
) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), Value::String(next_id("gen", counts, g.bus)));
    m.insert("bus".into(), Value::from(g.bus.0 as u64));
    m.insert("p".into(), jnum(g.pg));
    m.insert("q".into(), jnum(g.qg));
    m.insert("pmax".into(), jnum(g.pmax));
    m.insert("pmin".into(), jnum(g.pmin));
    m.insert("qmax".into(), jnum(g.qmax));
    m.insert("qmin".into(), jnum(g.qmin));
    m.insert("voltage_setpoint_pu".into(), jnum(g.vg));
    m.insert("machine_base_mva".into(), jnum(g.mbase));
    m.insert("in_service".into(), Value::Bool(g.in_service));
    m.insert("gen_type".into(), Value::String("Synchronous".into()));
    m.insert("pfr_eligible".into(), Value::Bool(true));
    m.insert("quick_start".into(), Value::Bool(false));
    m.insert("voltage_regulated".into(), Value::Bool(true));
    if let Some(cost) = &g.cost {
        if let Some(cost) = cost_obj(cost, warnings) {
            m.insert("cost".into(), cost);
        }
    }
    if g.has_caps() {
        warnings.push(format!(
            "generator at bus {} has MATPOWER capability/ramp columns not represented in Surge JSON",
            g.bus
        ));
    }
    Value::Object(m)
}

fn cost_obj(cost: &GenCost, warnings: &mut Vec<String>) -> Option<Value> {
    match cost.model {
        2 => {
            let want = cost.ncost.min(cost.coeffs.len());
            let coeffs = cost.coeffs[..want].iter().copied().map(jnum).collect();
            let mut curve = Map::new();
            curve.insert("coeffs".into(), Value::Array(coeffs));
            curve.insert("startup".into(), jnum(cost.startup));
            curve.insert("shutdown".into(), jnum(cost.shutdown));

            let mut wrapper = Map::new();
            wrapper.insert("Polynomial".into(), Value::Object(curve));
            Some(Value::Object(wrapper))
        }
        1 => {
            let want = (cost.ncost * 2).min(cost.coeffs.len());
            if want % 2 != 0 {
                warnings.push(
                    "piecewise generator cost has an odd coefficient count; cost dropped".into(),
                );
                return None;
            }
            let mut points = Vec::new();
            for pair in cost.coeffs[..want].chunks(2) {
                points.push(Value::Array(vec![jnum(pair[0]), jnum(pair[1])]));
            }
            let mut curve = Map::new();
            curve.insert("points".into(), Value::Array(points));
            curve.insert("startup".into(), jnum(cost.startup));
            curve.insert("shutdown".into(), jnum(cost.shutdown));

            let mut wrapper = Map::new();
            wrapper.insert("PiecewiseLinear".into(), Value::Object(curve));
            Some(Value::Object(wrapper))
        }
        _ => {
            warnings.push(format!(
                "unsupported generator cost model {} dropped in Surge JSON",
                cost.model
            ));
            None
        }
    }
}

fn storage_gen_obj(st: &Storage, counts: &mut BTreeMap<BusId, usize>) -> Value {
    let mut m = Map::new();
    m.insert(
        "id".into(),
        Value::String(next_id("storage", counts, st.bus)),
    );
    m.insert("bus".into(), Value::from(st.bus.0 as u64));
    m.insert("p".into(), jnum(st.ps));
    m.insert("q".into(), jnum(st.qs));
    m.insert("pmax".into(), jnum(st.discharge_rating));
    m.insert("pmin".into(), jnum(-st.charge_rating));
    m.insert("qmax".into(), jnum(st.qmax));
    m.insert("qmin".into(), jnum(st.qmin));
    m.insert("voltage_setpoint_pu".into(), jnum(1.0));
    m.insert("machine_base_mva".into(), jnum(st.thermal_rating.max(1.0)));
    m.insert("in_service".into(), Value::Bool(st.in_service));
    m.insert("gen_type".into(), Value::String("Synchronous".into()));
    m.insert("pfr_eligible".into(), Value::Bool(true));
    m.insert("quick_start".into(), Value::Bool(false));
    m.insert("voltage_regulated".into(), Value::Bool(false));

    let mut storage = Map::new();
    storage.insert("energy_capacity_mwh".into(), jnum(st.energy_rating));
    storage.insert("soc_initial_mwh".into(), jnum(st.energy));
    storage.insert("soc_min_mwh".into(), jnum(0.0));
    storage.insert("soc_max_mwh".into(), jnum(st.energy_rating));
    storage.insert("charge_efficiency".into(), jnum(st.charge_efficiency));
    storage.insert("discharge_efficiency".into(), jnum(st.discharge_efficiency));
    storage.insert("variable_cost_per_mwh".into(), jnum(0.0));
    storage.insert("degradation_cost_per_mwh".into(), jnum(0.0));
    storage.insert(
        "dispatch_mode".into(),
        Value::String("CostMinimization".into()),
    );
    m.insert("storage".into(), Value::Object(storage));

    Value::Object(m)
}

fn hvdc_link_obj(dc: &Hvdc, i: usize, warnings: &mut Vec<String>) -> Value {
    if dc.qf != 0.0
        || dc.qt != 0.0
        || dc.qminf != 0.0
        || dc.qmaxf != 0.0
        || dc.qmint != 0.0
        || dc.qmaxt != 0.0
        || dc.loss0 != 0.0
        || dc.loss1 != 0.0
    {
        warnings.push(format!(
            "dcline {} reactive limits or loss model mapped best-effort in Surge JSON",
            i + 1
        ));
    }

    let mut m = Map::new();
    m.insert("technology".into(), Value::String("lcc".into()));
    m.insert("name".into(), Value::String(format!("dcl_{}", i + 1)));
    m.insert(
        "mode".into(),
        Value::String(
            if dc.in_service {
                "PowerControl"
            } else {
                "Blocked"
            }
            .into(),
        ),
    );
    m.insert("rectifier".into(), lcc_terminal_obj(dc.from, dc.in_service));
    m.insert("inverter".into(), lcc_terminal_obj(dc.to, dc.in_service));
    m.insert("scheduled_setpoint".into(), jnum(dc.pf));
    m.insert("p_dc_min_mw".into(), jnum(dc.pmin));
    m.insert("p_dc_max_mw".into(), jnum(dc.pmax));
    m.insert("scheduled_voltage_kv".into(), jnum(0.0));
    m.insert("resistance_ohm".into(), jnum(0.0));
    Value::Object(m)
}

fn lcc_terminal_obj(bus: BusId, in_service: bool) -> Value {
    let mut m = Map::new();
    m.insert("bus".into(), Value::from(bus.0 as u64));
    m.insert("in_service".into(), Value::Bool(in_service));
    m.insert("n_bridges".into(), Value::from(1_u64));
    m.insert("alpha_min".into(), jnum(5.0));
    m.insert("alpha_max".into(), jnum(90.0));
    m.insert("base_voltage_kv".into(), jnum(0.0));
    m.insert("commutation_reactance_ohm".into(), jnum(0.0));
    m.insert("commutation_resistance_ohm".into(), jnum(0.0));
    m.insert("tap".into(), jnum(1.0));
    m.insert("tap_min".into(), jnum(0.9));
    m.insert("tap_max".into(), jnum(1.1));
    m.insert("tap_step".into(), jnum(0.00625));
    m.insert("turns_ratio".into(), jnum(1.0));
    Value::Object(m)
}

pub fn parse_surge_json(content: &str) -> Result<Network> {
    parse_surge_json_source(Arc::new(content.to_owned()), None)
}

pub(crate) fn parse_surge_json_source(
    source: Arc<String>,
    name_hint: Option<&str>,
) -> Result<Network> {
    let content: &str = &source;
    let root_value: Value = serde_json::from_str(content).map_err(|e| Error::FormatRead {
        format: FMT,
        message: e.to_string(),
    })?;
    let root = object(&root_value, "top level")?;
    validate_envelope(root)?;
    let network = object_field(root, "network")?;

    let mut buses = Vec::new();
    let mut shunts = Vec::new();
    for value in array_field(network, "buses", true)? {
        let (bus, bus_shunt) = read_bus(value)?;
        buses.push(bus);
        if let Some(shunt) = bus_shunt {
            shunts.push(shunt);
        }
    }

    shunts.extend(
        array_field(network, "fixed_shunts", false)?
            .into_iter()
            .map(read_fixed_shunt)
            .collect::<Result<Vec<_>>>()?,
    );

    let mut generators = Vec::new();
    let mut storage = Vec::new();
    for value in array_field(network, "generators", false)? {
        let (generator, storage_record) = read_generator(value)?;
        generators.push(generator);
        if let Some(storage_record) = storage_record {
            storage.push(storage_record);
        }
    }

    let name = string_map(network, "name")
        .filter(|name| !name.is_empty())
        .or(name_hint)
        .unwrap_or("case")
        .to_string();

    let net = Network {
        name,
        base_mva: f_map_or(network, "base_mva", 100.0)?,
        buses,
        loads: array_field(network, "loads", false)?
            .into_iter()
            .map(read_load)
            .collect::<Result<Vec<_>>>()?,
        shunts,
        branches: array_field(network, "branches", false)?
            .into_iter()
            .map(read_branch)
            .collect::<Result<Vec<_>>>()?,
        generators,
        storage,
        hvdc: read_hvdc(network)?,
        source_format: SourceFormat::Surge,
        source: Some(source),
    };
    net.check_references(FMT)?;
    Ok(net)
}

fn validate_envelope(root: &Map<String, Value>) -> Result<()> {
    let format = required_string_map(root, "format")?;
    if format != FORMAT_VALUE {
        return Err(format_error(format!(
            "unsupported `format` value `{format}`; expected `{FORMAT_VALUE}`"
        )));
    }
    let schema_version = required_string_map(root, "schema_version")?;
    if schema_version != SCHEMA_VERSION {
        return Err(format_error(format!(
            "unsupported `schema_version` value `{schema_version}`; expected `{SCHEMA_VERSION}`"
        )));
    }
    let meta = object_field(root, "meta")?;
    if let Some(producer) = string_map(meta, "producer") {
        if producer != "surge" {
            return Err(format_error(format!(
                "unsupported `meta.producer` value `{producer}`"
            )));
        }
    }
    if let Some(profile) = string_map(meta, "profile") {
        if !matches!(profile, "network" | "dispatch" | "results") {
            return Err(format_error(format!(
                "unsupported `meta.profile` value `{profile}`"
            )));
        }
    }
    if !root.contains_key("network") {
        return Err(format_error("missing object `network`"));
    }
    Ok(())
}

fn read_bus(value: &Value) -> Result<(Bus, Option<Shunt>)> {
    let obj = object(value, "bus record")?;
    let id = BusId(required_usize(obj, "number")?);
    let g = f_map_or(obj, "shunt_conductance_mw", 0.0)?;
    let b = f_map_or(obj, "shunt_susceptance_mvar", 0.0)?;
    let shunt = if g != 0.0 || b != 0.0 {
        Some(Shunt {
            bus: id,
            g,
            b,
            in_service: true,
            extras: Extras::new(),
        })
    } else {
        None
    };
    let bus = Bus {
        id,
        kind: read_bus_type(string_map(obj, "bus_type").unwrap_or("PQ"))?,
        vm: f_map_or(obj, "voltage_magnitude_pu", 1.0)?,
        va: f_map_or(obj, "voltage_angle_rad", 0.0)? * DEG_PER_RAD,
        base_kv: f_map_or(obj, "base_kv", 0.0)?,
        vmax: f_map_or(obj, "voltage_max_pu", 1.1)?,
        vmin: f_map_or(obj, "voltage_min_pu", 0.9)?,
        area: usize_map_or(obj, "area", 1)?,
        zone: usize_map_or(obj, "zone", 1)?,
        name: string_map(obj, "name")
            .filter(|name| !name.is_empty())
            .map(str::to_string),
        extras: Extras::new(),
    };
    Ok((bus, shunt))
}

fn read_bus_type(value: &str) -> Result<BusType> {
    match value {
        "PQ" => Ok(BusType::Pq),
        "PV" => Ok(BusType::Pv),
        "Slack" | "REF" | "Ref" => Ok(BusType::Ref),
        "Isolated" => Ok(BusType::Isolated),
        other => Err(format_error(format!("unknown bus_type `{other}`"))),
    }
}

fn read_load(value: &Value) -> Result<Load> {
    let obj = object(value, "load record")?;
    Ok(Load {
        bus: BusId(required_usize(obj, "bus")?),
        p: f_map_or(obj, "active_power_demand_mw", 0.0)?,
        q: f_map_or(obj, "reactive_power_demand_mvar", 0.0)?,
        in_service: bool_map_or(obj, "in_service", true)?,
        extras: Extras::new(),
    })
}

fn read_fixed_shunt(value: &Value) -> Result<Shunt> {
    let obj = object(value, "fixed_shunt record")?;
    Ok(Shunt {
        bus: BusId(required_usize(obj, "bus")?),
        g: f_map_alias_or(obj, &["g_mw", "conductance_mw"], 0.0)?,
        b: f_map_alias_or(obj, &["b_mvar", "susceptance_mvar"], 0.0)?,
        in_service: bool_map_or(obj, "in_service", true)?,
        extras: Extras::new(),
    })
}

fn read_branch(value: &Value) -> Result<Branch> {
    let obj = object(value, "branch record")?;
    let branch_type = string_map(obj, "branch_type").unwrap_or("Line");
    let tap_value = f_map_or(obj, "tap", 1.0)?;
    let shift = f_map_or(obj, "phase_shift_rad", 0.0)? * DEG_PER_RAD;
    let tap = if branch_type == "Line" && (tap_value - 1.0).abs() < EPS && shift.abs() < EPS {
        0.0
    } else {
        tap_value
    };
    Ok(Branch {
        from: BusId(required_usize(obj, "from_bus")?),
        to: BusId(required_usize(obj, "to_bus")?),
        r: f_map_or(obj, "r", 0.0)?,
        x: f_map_or(obj, "x", 0.0)?,
        b: f_map_or(obj, "b", 0.0)?,
        rate_a: f_map_or(obj, "rating_a_mva", 0.0)?,
        rate_b: f_map_or(obj, "rating_b_mva", 0.0)?,
        rate_c: f_map_or(obj, "rating_c_mva", 0.0)?,
        tap,
        shift,
        in_service: bool_map_or(obj, "in_service", true)?,
        angmin: f_map_or(obj, "angle_diff_min_rad", -std::f64::consts::TAU)? * DEG_PER_RAD,
        angmax: f_map_or(obj, "angle_diff_max_rad", std::f64::consts::TAU)? * DEG_PER_RAD,
        extras: Extras::new(),
    })
}

fn read_generator(value: &Value) -> Result<(Generator, Option<Storage>)> {
    let obj = object(value, "generator record")?;
    let mut caps: GenCaps = [None; GEN_EXTRA_KEYS.len()];
    if let Some(apf) = obj.get("agc_participation_factor").and_then(Value::as_f64) {
        if let Some(slot) = GEN_EXTRA_KEYS.iter().position(|key| *key == "apf") {
            caps[slot] = Some(apf);
        }
    }

    let bus = BusId(required_usize(obj, "bus")?);
    let pg = f_map_alias_or(obj, &["p", "pg"], 0.0)?;
    let qg = f_map_alias_or(obj, &["q", "qg"], 0.0)?;
    let pmax = f_map_or(obj, "pmax", 0.0)?;
    let pmin = f_map_or(obj, "pmin", 0.0)?;
    let qmax = f_map_or(obj, "qmax", 0.0)?;
    let qmin = f_map_or(obj, "qmin", 0.0)?;
    let in_service = bool_map_or(obj, "in_service", true)?;

    let generator = Generator {
        bus,
        pg,
        qg,
        pmax,
        pmin,
        qmax,
        qmin,
        vg: f_map_or(obj, "voltage_setpoint_pu", 1.0)?,
        mbase: f_map_or(obj, "machine_base_mva", 0.0)?,
        in_service,
        cost: match obj.get("cost") {
            Some(Value::Null) | None => None,
            Some(value) => Some(read_cost(value)?),
        },
        caps,
    };

    let storage = match obj.get("storage") {
        Some(Value::Null) | None => None,
        Some(value) => Some(read_storage(
            obj, value, bus, pg, qg, pmax, pmin, qmax, qmin, in_service,
        )?),
    };

    Ok((generator, storage))
}

fn read_cost(value: &Value) -> Result<GenCost> {
    let obj = object(value, "generator cost")?;
    if let Some(poly) = obj.get("Polynomial") {
        let poly = object(poly, "Polynomial cost")?;
        let coeffs = number_array(poly, "coeffs")?;
        return Ok(GenCost {
            model: 2,
            startup: f_map_or(poly, "startup", 0.0)?,
            shutdown: f_map_or(poly, "shutdown", 0.0)?,
            ncost: coeffs.len(),
            coeffs,
        });
    }
    if let Some(piecewise) = obj.get("PiecewiseLinear").or_else(|| obj.get("Piecewise")) {
        let piecewise = object(piecewise, "PiecewiseLinear cost")?;
        let points = array_field(piecewise, "points", true)?;
        let ncost = points.len();
        let mut coeffs = Vec::with_capacity(points.len() * 2);
        for point in &points {
            let pair = point
                .as_array()
                .ok_or_else(|| format_error("piecewise cost point must be a two-element array"))?;
            if pair.len() != 2 {
                return Err(format_error("piecewise cost point must have two elements"));
            }
            coeffs.push(value_to_f64(&pair[0], "piecewise cost MW")?);
            coeffs.push(value_to_f64(&pair[1], "piecewise cost value")?);
        }
        return Ok(GenCost {
            model: 1,
            startup: f_map_or(piecewise, "startup", 0.0)?,
            shutdown: f_map_or(piecewise, "shutdown", 0.0)?,
            ncost,
            coeffs,
        });
    }
    Err(format_error("unsupported generator cost curve"))
}

#[allow(clippy::too_many_arguments)]
fn read_storage(
    _generator: &Map<String, Value>,
    storage: &Value,
    bus: BusId,
    pg: f64,
    qg: f64,
    pmax: f64,
    pmin: f64,
    qmax: f64,
    qmin: f64,
    in_service: bool,
) -> Result<Storage> {
    let obj = object(storage, "storage params")?;
    let efficiency = f_map_or(obj, "efficiency", 1.0)?;
    let split_efficiency = if efficiency >= 0.0 {
        efficiency.sqrt()
    } else {
        1.0
    };
    let energy_rating = f_map_alias_or(obj, &["energy_capacity_mwh", "soc_max_mwh"], 0.0)?;
    Ok(Storage {
        bus,
        ps: pg,
        qs: qg,
        energy: f_map_or(obj, "soc_initial_mwh", 0.0)?,
        energy_rating,
        charge_rating: if pmin < 0.0 { -pmin } else { 0.0 },
        discharge_rating: pmax.max(0.0),
        charge_efficiency: f_map_or(obj, "charge_efficiency", split_efficiency)?,
        discharge_efficiency: f_map_or(obj, "discharge_efficiency", split_efficiency)?,
        thermal_rating: pmax.abs().max(pmin.abs()),
        qmin,
        qmax,
        r: 0.0,
        x: 0.0,
        p_loss: 0.0,
        q_loss: 0.0,
        in_service,
        extras: Extras::new(),
    })
}

fn read_hvdc(network: &Map<String, Value>) -> Result<Vec<Hvdc>> {
    let Some(hvdc) = network.get("hvdc") else {
        return Ok(Vec::new());
    };
    if hvdc.is_null() {
        return Ok(Vec::new());
    }
    let hvdc = object(hvdc, "hvdc")?;
    let mut out = Vec::new();
    for link in array_field(hvdc, "links", false)? {
        out.push(read_hvdc_link(link)?);
    }
    Ok(out)
}

fn read_hvdc_link(value: &Value) -> Result<Hvdc> {
    let obj = object(value, "hvdc link")?;
    let tech = string_map(obj, "technology").unwrap_or("lcc");
    let (from_terminal, to_terminal) = match tech {
        "lcc" | "Lcc" | "LCC" => (
            object_field(obj, "rectifier")?,
            object_field(obj, "inverter")?,
        ),
        "vsc" | "Vsc" | "VSC" => (
            object_field(obj, "converter1")?,
            object_field(obj, "converter2")?,
        ),
        other => {
            return Err(format_error(format!(
                "unsupported hvdc technology `{other}`"
            )));
        }
    };
    let from = BusId(required_usize(from_terminal, "bus")?);
    let to = BusId(required_usize(to_terminal, "bus")?);
    let setpoint = f_map_alias_or(
        obj,
        &["scheduled_setpoint", "scheduled_setpoint_mw"],
        f_map_or(from_terminal, "dc_setpoint", 0.0)?,
    )?;
    let pmin = f_map_or(obj, "p_dc_min_mw", setpoint.min(0.0))?;
    let pmax = f_map_or(obj, "p_dc_max_mw", setpoint.max(0.0))?;
    let in_service = string_map(obj, "mode").unwrap_or("PowerControl") != "Blocked"
        && bool_map_or(from_terminal, "in_service", true)?
        && bool_map_or(to_terminal, "in_service", true)?;

    Ok(Hvdc {
        from,
        to,
        in_service,
        pf: setpoint,
        pt: -setpoint,
        qf: 0.0,
        qt: 0.0,
        vf: f_map_or(from_terminal, "ac_setpoint", 1.0)?,
        vt: f_map_or(to_terminal, "ac_setpoint", 1.0)?,
        pmin,
        pmax,
        qminf: f_map_or(from_terminal, "q_min_mvar", 0.0)?,
        qmaxf: f_map_or(from_terminal, "q_max_mvar", 0.0)?,
        qmint: f_map_or(to_terminal, "q_min_mvar", 0.0)?,
        qmaxt: f_map_or(to_terminal, "q_max_mvar", 0.0)?,
        loss0: f_map_or(from_terminal, "loss_constant_mw", 0.0)?
            + f_map_or(to_terminal, "loss_constant_mw", 0.0)?,
        loss1: f_map_or(from_terminal, "loss_linear", 0.0)?
            + f_map_or(to_terminal, "loss_linear", 0.0)?,
        extras: Extras::new(),
    })
}

pub(crate) fn source_loss_warnings(net: &Network) -> Vec<String> {
    if !matches!(net.source_format, SourceFormat::Surge) {
        return Vec::new();
    }
    let Some(source) = &net.source else {
        return Vec::new();
    };
    let Ok(root_value) = serde_json::from_str::<Value>(source) else {
        return vec!["surge source could not be inspected for source loss warnings".into()];
    };
    let Some(root) = root_value.as_object() else {
        return Vec::new();
    };
    let Some(network) = root.get("network").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut warnings = Vec::new();

    let profile = root
        .get("meta")
        .and_then(Value::as_object)
        .and_then(|meta| string_map(meta, "profile"));
    if matches!(profile, Some("dispatch" | "results")) || has_nonempty(root, "dispatch") {
        warnings.push("surge dispatch profile data ignored by powerio's Network hub".into());
    }
    if matches!(profile, Some("results")) || has_nonempty(root, "solution") {
        warnings.push("surge solution profile data ignored by powerio's Network hub".into());
    }

    if num_not_default(network, "freq_hz", 60.0) {
        warnings.push("surge system frequency dropped".into());
    }

    let top = [
        "facts_devices",
        "topology",
        "controls",
        "area_schedules",
        "interfaces",
        "flowgates",
        "market_data",
        "pumped_hydro_units",
        "combined_cycle_plants",
        "dispatchable_loads",
        "induction_machines",
        "power_injections",
        "breaker_ratings",
        "conditional_limits",
        "nomograms",
        "cim",
        "metadata",
    ];
    let dropped_top: Vec<&str> = top
        .into_iter()
        .filter(|key| has_nonempty(network, key))
        .collect();
    if !dropped_top.is_empty() {
        warnings.push(format!(
            "surge network sections dropped: {}",
            dropped_top.join(", ")
        ));
    }

    warn_count(
        &mut warnings,
        network,
        "loads",
        "load ZIP, composition, frequency, classification, or ownership fields dropped",
        load_has_source_only_fields,
    );
    warn_count(
        &mut warnings,
        network,
        "branches",
        "branch control, phase-shifter bounds, sequence, thermal, cost, or circuit metadata dropped",
        branch_has_source_only_fields,
    );
    warn_count(
        &mut warnings,
        network,
        "generators",
        "generator commitment, ramping, fuel, market, reserve, emission, classification, or richer storage fields dropped",
        generator_has_source_only_fields,
    );
    if has_nonempty(network, "hvdc") {
        warnings.push(
            "surge HVDC converter, reactive, loss, and control details mapped best-effort".into(),
        );
    }

    warnings
}

fn warn_count(
    warnings: &mut Vec<String>,
    network: &Map<String, Value>,
    section: &str,
    message: &str,
    predicate: fn(&Map<String, Value>) -> bool,
) {
    let count = network
        .get(section)
        .and_then(Value::as_array)
        .map_or(0, |items| {
            items
                .iter()
                .filter_map(Value::as_object)
                .filter(|item| predicate(item))
                .count()
        });
    if count > 0 {
        warnings.push(format!("{count} surge {message}"));
    }
}

fn load_has_source_only_fields(load: &Map<String, Value>) -> bool {
    num_not_default(load, "zip_p_impedance_frac", 0.0)
        || num_not_default(load, "zip_p_current_frac", 0.0)
        || num_not_default(load, "zip_p_power_frac", 1.0)
        || num_not_default(load, "zip_q_impedance_frac", 0.0)
        || num_not_default(load, "zip_q_current_frac", 0.0)
        || num_not_default(load, "zip_q_power_frac", 1.0)
        || num_not_default(load, "freq_sensitivity_p_pct_per_hz", 0.0)
        || num_not_default(load, "freq_sensitivity_q_pct_per_hz", 0.0)
        || num_not_default(load, "frac_static", 1.0)
        || num_not_default(load, "frac_motor_a", 0.0)
        || num_not_default(load, "frac_motor_b", 0.0)
        || num_not_default(load, "frac_motor_c", 0.0)
        || num_not_default(load, "frac_motor_d", 0.0)
        || num_not_default(load, "frac_electronic", 0.0)
        || bool_not_default(load, "conforming", true)
        || string_not_default(load, "connection", "WyeGrounded")
        || has_nonempty(load, "owners")
        || has_nonempty(load, "load_class")
        || has_nonempty(load, "classification")
}

fn branch_has_source_only_fields(branch: &Map<String, Value>) -> bool {
    [
        "g_pi",
        "g_mag",
        "b_mag",
        "g_shunt_from",
        "b_shunt_from",
        "g_shunt_to",
        "b_shunt_to",
        "bi0",
        "bj0",
        "gi0",
        "gj0",
        "r_temp_coeff",
        "skin_effect_alpha",
        "cost_startup",
        "cost_shutdown",
        "tap_step",
        "phase_step_rad",
    ]
    .into_iter()
    .any(|key| num_not_default(branch, key, 0.0))
        || has_nonempty(branch, "phase_min_rad")
        || has_nonempty(branch, "phase_max_rad")
        || num_not_default(branch, "tap_min", 1.0)
        || num_not_default(branch, "tap_max", 1.0)
        || bool_not_default(branch, "bypassed", false)
        || bool_not_default(branch, "delta_connected", false)
        || string_not_default(branch, "phase_mode", "fixed")
        || string_not_default(branch, "tap_mode", "fixed")
        || string_not_default(branch, "circuit", "1")
        || has_nonempty(branch, "opf_control")
        || has_nonempty(branch, "owners")
        || has_nonempty(branch, "zero_sequence")
}

fn generator_has_source_only_fields(generator: &Map<String, Value>) -> bool {
    [
        "commitment",
        "ramping",
        "market",
        "reserve_offers",
        "qualifications",
        "emission_rates",
        "fuel_type",
        "machine_id",
        "commitment_status",
        "ramp_down_curve",
        "ramp_up_curve",
        "min_down_time_hr",
        "min_up_time_hr",
        "hours_offline",
        "hours_online",
        "reg_bus",
    ]
    .into_iter()
    .any(|key| has_nonempty(generator, key))
        || bool_not_default(generator, "quick_start", false)
        || bool_not_default(generator, "grid_forming", false)
        || bool_not_default(generator, "curtailable", false)
        || bool_not_default(generator, "voltage_regulated", true)
        || generator
            .get("gen_type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "Synchronous")
        || generator
            .get("storage")
            .and_then(Value::as_object)
            .is_some_and(storage_has_source_only_fields)
}

fn storage_has_source_only_fields(storage: &Map<String, Value>) -> bool {
    num_not_default(storage, "variable_cost_per_mwh", 0.0)
        || num_not_default(storage, "degradation_cost_per_mwh", 0.0)
        || num_not_default(storage, "self_schedule_mw", 0.0)
        || has_nonempty(storage, "chemistry")
        || string_not_default(storage, "dispatch_mode", "CostMinimization")
}

fn format_error(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FMT,
        message: message.into(),
    }
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format_error(format!("{context} is not a JSON object")))
}

fn object_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a Map<String, Value>> {
    let value = obj
        .get(key)
        .ok_or_else(|| format_error(format!("missing object `{key}`")))?;
    object(value, key)
}

fn array_field<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    required: bool,
) -> Result<Vec<&'a Value>> {
    match obj.get(key) {
        Some(Value::Array(items)) => Ok(items.iter().collect()),
        Some(Value::Null) | None if !required => Ok(Vec::new()),
        None => Err(format_error(format!("missing array `{key}`"))),
        Some(_) => Err(format_error(format!("`{key}` must be an array"))),
    }
}

fn required_string_map<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    string_map(obj, key).ok_or_else(|| format_error(format!("missing string `{key}`")))
}

fn string_map<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

fn required_usize(obj: &Map<String, Value>, key: &str) -> Result<usize> {
    let value = obj
        .get(key)
        .ok_or_else(|| format_error(format!("missing integer `{key}`")))?;
    value_to_usize(value, key)
}

fn usize_map_or(obj: &Map<String, Value>, key: &str, default: usize) -> Result<usize> {
    match obj.get(key) {
        Some(Value::Null) | None => Ok(default),
        Some(value) => value_to_usize(value, key),
    }
}

fn f_map_or(obj: &Map<String, Value>, key: &str, default: f64) -> Result<f64> {
    match obj.get(key) {
        Some(Value::Null) | None => Ok(default),
        Some(value) => value_to_f64(value, key),
    }
}

fn f_map_alias_or(obj: &Map<String, Value>, keys: &[&str], default: f64) -> Result<f64> {
    for key in keys {
        if let Some(value) = obj.get(*key) {
            return if value.is_null() {
                Ok(default)
            } else {
                value_to_f64(value, key)
            };
        }
    }
    Ok(default)
}

fn bool_map_or(obj: &Map<String, Value>, key: &str, default: bool) -> Result<bool> {
    match obj.get(key) {
        Some(Value::Null) | None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(Value::Number(value)) => value
            .as_f64()
            .map(|value| value != 0.0)
            .ok_or_else(|| format_error(format!("`{key}` is not a finite bool-like number"))),
        Some(Value::String(value)) => match value.as_str() {
            "true" | "True" | "1" => Ok(true),
            "false" | "False" | "0" => Ok(false),
            _ => Err(format_error(format!("`{key}` is not a bool"))),
        },
        Some(_) => Err(format_error(format!("`{key}` is not a bool"))),
    }
}

fn number_array(obj: &Map<String, Value>, key: &str) -> Result<Vec<f64>> {
    let values = array_field(obj, key, true)?;
    values
        .iter()
        .enumerate()
        .map(|(i, value)| value_to_f64(value, &format!("{key}[{i}]")))
        .collect()
}

fn value_to_f64(value: &Value, key: &str) -> Result<f64> {
    match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| format_error(format!("`{key}` is not a finite f64"))),
        Value::String(value) => value
            .parse::<f64>()
            .map_err(|_| format_error(format!("`{key}` string is not a f64"))),
        Value::Object(obj) if obj.contains_key("$surge_float") => Err(format_error(format!(
            "`{key}` uses Surge tagged non-finite float values, which powerio does not support"
        ))),
        _ => Err(format_error(format!("`{key}` is not a number"))),
    }
}

fn value_to_usize(value: &Value, key: &str) -> Result<usize> {
    match value {
        Value::Number(number) => {
            if let Some(value) = number.as_u64() {
                usize::try_from(value)
                    .map_err(|_| format_error(format!("`{key}` integer is too large")))
            } else if let Some(value) = number.as_i64() {
                if value >= 0 {
                    usize::try_from(value as u64)
                        .map_err(|_| format_error(format!("`{key}` integer is too large")))
                } else {
                    Err(format_error(format!("`{key}` must be nonnegative")))
                }
            } else if let Some(value) = number.as_f64() {
                if value >= 0.0 && value.fract() == 0.0 {
                    Ok(value as usize)
                } else {
                    Err(format_error(format!("`{key}` must be an integer")))
                }
            } else {
                Err(format_error(format!("`{key}` is not an integer")))
            }
        }
        Value::String(value) => value
            .parse::<usize>()
            .map_err(|_| format_error(format!("`{key}` string is not an integer"))),
        _ => Err(format_error(format!("`{key}` is not an integer"))),
    }
}

fn has_nonempty(obj: &Map<String, Value>, key: &str) -> bool {
    obj.get(key).is_some_and(value_nonempty)
}

fn value_nonempty(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
    }
}

fn num_not_default(obj: &Map<String, Value>, key: &str, default: f64) -> bool {
    obj.get(key)
        .and_then(|value| value_to_f64(value, key).ok())
        .is_some_and(|value| (value - default).abs() > EPS)
}

fn bool_not_default(obj: &Map<String, Value>, key: &str, default: bool) -> bool {
    obj.get(key)
        .and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::Number(number) => number.as_f64().map(|value| value != 0.0),
            _ => None,
        })
        .is_some_and(|value| value != default)
}

fn string_not_default(obj: &Map<String, Value>, key: &str, default: &str) -> bool {
    obj.get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| value != default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_envelope() {
        let err = parse_surge_json(
            r#"{"format":"surge-json","schema_version":"9","meta":{},"network":{}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::FormatRead { .. }));
    }

    #[test]
    fn bus_type_mapping() {
        assert_eq!(read_bus_type("PQ").unwrap(), BusType::Pq);
        assert_eq!(read_bus_type("PV").unwrap(), BusType::Pv);
        assert_eq!(read_bus_type("Slack").unwrap(), BusType::Ref);
        assert_eq!(read_bus_type("Isolated").unwrap(), BusType::Isolated);
    }

    #[test]
    fn cost_mapping() {
        let cost = read_cost(&serde_json::json!({
            "Polynomial": {"coeffs": [1.0, 2.0, 3.0], "startup": 4.0, "shutdown": 5.0}
        }))
        .unwrap();
        assert_eq!(cost.model, 2);
        assert_eq!(cost.coeffs, vec![1.0, 2.0, 3.0]);

        let cost = read_cost(&serde_json::json!({
            "PiecewiseLinear": {"points": [[0.0, 0.0], [10.0, 20.0]]}
        }))
        .unwrap();
        assert_eq!(cost.model, 1);
        assert_eq!(cost.coeffs, vec![0.0, 0.0, 10.0, 20.0]);
    }

    #[test]
    fn branch_tap_convention() {
        let branch = read_branch(&serde_json::json!({
            "from_bus": 1,
            "to_bus": 2,
            "branch_type": "Line",
            "tap": 1.0
        }))
        .unwrap();
        assert!(branch.tap.abs() < EPS);

        let branch = read_branch(&serde_json::json!({
            "from_bus": 1,
            "to_bus": 2,
            "branch_type": "Transformer",
            "tap": 1.0
        }))
        .unwrap();
        assert!((branch.tap - 1.0).abs() < EPS);
    }
}

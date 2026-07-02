//! Read and write Surge native `surge-json` network documents.
//!
//! Surge JSON is a versioned envelope around a richer network body. The reader
//! maps the electrical core into `Network`, retains the original source for byte
//! exact same format writes, and reports source sections that stay only in the
//! retained document.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use super::{Conversion, Parsed, finish, jnum, warn_extra_branch_rating_sets};
use crate::network::{
    Branch, BranchCharging, BranchCurrentRatings, BranchSolution, Bus, BusId, BusType, Extras,
    GEN_EXTRA_KEYS, GenCaps, GenCost, Generator, Hvdc, Load, LoadVoltageModel, Network, Shunt,
    SourceFormat, Storage,
};
use crate::normalize;
use crate::{Error, Result};

const FMT: &str = "Surge JSON";
const FORMAT_VALUE: &str = "surge-json";
const SCHEMA_VERSION: &str = "0.1.0";
const EPS: f64 = 1e-12;

#[must_use]
pub fn write_surge_json(net: &Network) -> Conversion {
    let mut warnings = Vec::new();
    let mut network = Map::new();

    network.insert("name".into(), Value::String(net.name.clone()));
    network.insert("base_mva".into(), jnum(net.base_mva));
    network.insert("freq_hz".into(), jnum(net.base_frequency));

    network.insert(
        "buses".into(),
        Value::Array(net.buses.iter().map(bus_obj).collect()),
    );
    network.insert(
        "loads".into(),
        Value::Array(net.loads.iter().enumerate().map(load_obj).collect()),
    );
    network.insert(
        "fixed_shunts".into(),
        Value::Array(net.shunts.iter().enumerate().map(shunt_obj).collect()),
    );
    network.insert(
        "branches".into(),
        Value::Array(net.branches.iter().enumerate().map(branch_obj).collect()),
    );

    let mut gen_counts: BTreeMap<BusId, usize> = BTreeMap::new();
    let mut generators = Vec::new();
    for generator in &net.generators {
        generators.push(gen_obj(generator, &mut gen_counts, &mut warnings));
    }
    for storage in &net.storage {
        generators.push(storage_gen_obj(storage, &mut gen_counts));
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

    warn_extra_branch_rating_sets(FMT, net, &mut warnings);
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

fn bus_obj(bus: &Bus) -> Value {
    let mut obj = Map::new();
    obj.insert("number".into(), Value::from(bus.id.0 as u64));
    obj.insert(
        "name".into(),
        Value::String(bus.name.clone().unwrap_or_default()),
    );
    obj.insert("bus_type".into(), Value::String(bus_type(bus.kind).into()));
    obj.insert("base_kv".into(), jnum(bus.base_kv));
    obj.insert("voltage_magnitude_pu".into(), jnum(bus.vm));
    obj.insert("voltage_angle_rad".into(), jnum(bus.va.to_radians()));
    obj.insert("voltage_min_pu".into(), jnum(bus.vmin));
    obj.insert("voltage_max_pu".into(), jnum(bus.vmax));
    obj.insert("shunt_conductance_mw".into(), jnum(0.0));
    obj.insert("shunt_susceptance_mvar".into(), jnum(0.0));
    obj.insert("area".into(), Value::from(bus.area as u64));
    obj.insert("zone".into(), Value::from(bus.zone as u64));
    obj.insert("island_id".into(), Value::from(0_u64));
    Value::Object(obj)
}

fn frac(value: f64, total: f64, default: f64) -> f64 {
    if total.abs() > EPS {
        value / total
    } else {
        default
    }
}

fn load_obj((i, load): (usize, &Load)) -> Value {
    let mut obj = Map::new();
    obj.insert("id".into(), Value::String(format!("load_{}", i + 1)));
    obj.insert("bus".into(), Value::from(load.bus.0 as u64));
    obj.insert("active_power_demand_mw".into(), jnum(load.p));
    obj.insert("reactive_power_demand_mvar".into(), jnum(load.q));
    obj.insert("in_service".into(), Value::Bool(load.in_service));
    obj.insert("conforming".into(), Value::Bool(true));
    obj.insert("connection".into(), Value::String("WyeGrounded".into()));

    let (pz, pi, pp, qz, qi, qp) = match &load.voltage_model {
        Some(LoadVoltageModel::Zip {
            p_constant_power,
            q_constant_power,
            p_constant_current,
            q_constant_current,
            p_constant_impedance,
            q_constant_impedance,
            ..
        }) => (
            frac(*p_constant_impedance, load.p, 0.0),
            frac(*p_constant_current, load.p, 0.0),
            frac(*p_constant_power, load.p, 1.0),
            frac(*q_constant_impedance, load.q, 0.0),
            frac(*q_constant_current, load.q, 0.0),
            frac(*q_constant_power, load.q, 1.0),
        ),
        _ => (0.0, 0.0, 1.0, 0.0, 0.0, 1.0),
    };
    obj.insert("zip_p_impedance_frac".into(), jnum(pz));
    obj.insert("zip_p_current_frac".into(), jnum(pi));
    obj.insert("zip_p_power_frac".into(), jnum(pp));
    obj.insert("zip_q_impedance_frac".into(), jnum(qz));
    obj.insert("zip_q_current_frac".into(), jnum(qi));
    obj.insert("zip_q_power_frac".into(), jnum(qp));
    Value::Object(obj)
}

fn shunt_obj((i, shunt): (usize, &Shunt)) -> Value {
    let mut obj = Map::new();
    obj.insert("id".into(), Value::String(format!("shunt_{}", i + 1)));
    obj.insert("bus".into(), Value::from(shunt.bus.0 as u64));
    obj.insert("g_mw".into(), jnum(shunt.g));
    obj.insert("b_mvar".into(), jnum(shunt.b));
    obj.insert("in_service".into(), Value::Bool(shunt.in_service));
    obj.insert(
        "shunt_type".into(),
        Value::String(
            if shunt.b < 0.0 {
                "Reactor"
            } else {
                "Capacitor"
            }
            .into(),
        ),
    );
    Value::Object(obj)
}

fn branch_obj((_i, branch): (usize, &Branch)) -> Value {
    let charging = branch.terminal_charging();
    let mut obj = Map::new();
    obj.insert("from_bus".into(), Value::from(branch.from.0 as u64));
    obj.insert("to_bus".into(), Value::from(branch.to.0 as u64));
    obj.insert("circuit".into(), Value::String("1".into()));
    obj.insert("r".into(), jnum(branch.r));
    obj.insert("x".into(), jnum(branch.x));
    obj.insert("b".into(), jnum(branch.legacy_total_charging_b()));
    obj.insert("g_shunt_from".into(), jnum(charging.g_fr));
    obj.insert("b_shunt_from".into(), jnum(charging.b_fr));
    obj.insert("g_shunt_to".into(), jnum(charging.g_to));
    obj.insert("b_shunt_to".into(), jnum(charging.b_to));
    obj.insert("tap".into(), jnum(branch.effective_tap()));
    obj.insert("phase_shift_rad".into(), jnum(branch.shift.to_radians()));
    obj.insert("rating_a_mva".into(), jnum(branch.rate_a));
    obj.insert("rating_b_mva".into(), jnum(branch.rate_b));
    obj.insert("rating_c_mva".into(), jnum(branch.rate_c));
    if let Some(ratings) = branch.current_ratings {
        obj.insert("current_rating_a".into(), jnum(ratings.c_rating_a));
        obj.insert("current_rating_b".into(), jnum(ratings.c_rating_b));
        obj.insert("current_rating_c".into(), jnum(ratings.c_rating_c));
    }
    obj.insert("in_service".into(), Value::Bool(branch.in_service));
    obj.insert(
        "branch_type".into(),
        Value::String(
            if branch.is_transformer() {
                "Transformer"
            } else {
                "Line"
            }
            .into(),
        ),
    );
    obj.insert(
        "angle_diff_min_rad".into(),
        jnum(branch.angmin.to_radians()),
    );
    obj.insert(
        "angle_diff_max_rad".into(),
        jnum(branch.angmax.to_radians()),
    );
    if let Some(solution) = branch.solution {
        obj.insert("pf_mw".into(), jnum(solution.pf));
        obj.insert("qf_mvar".into(), jnum(solution.qf));
        obj.insert("pt_mw".into(), jnum(solution.pt));
        obj.insert("qt_mvar".into(), jnum(solution.qt));
    }
    obj.insert("g_pi".into(), jnum(0.0));
    obj.insert("g_mag".into(), jnum(0.0));
    obj.insert("b_mag".into(), jnum(0.0));
    Value::Object(obj)
}

fn next_id(prefix: &str, counts: &mut BTreeMap<BusId, usize>, bus: BusId) -> String {
    let count = counts.entry(bus).or_insert(0);
    *count += 1;
    format!("{prefix}_{}_{}", bus.0, *count)
}

fn gen_obj(
    generator: &Generator,
    counts: &mut BTreeMap<BusId, usize>,
    warnings: &mut Vec<String>,
) -> Value {
    let mut obj = Map::new();
    obj.insert(
        "id".into(),
        Value::String(next_id("gen", counts, generator.bus)),
    );
    obj.insert("bus".into(), Value::from(generator.bus.0 as u64));
    if let Some(regulated_bus) = generator.regulated_bus {
        obj.insert("reg_bus".into(), Value::from(regulated_bus.0 as u64));
    }
    obj.insert("p".into(), jnum(generator.pg));
    obj.insert("q".into(), jnum(generator.qg));
    obj.insert("pmax".into(), jnum(generator.pmax));
    obj.insert("pmin".into(), jnum(generator.pmin));
    obj.insert("qmax".into(), jnum(generator.qmax));
    obj.insert("qmin".into(), jnum(generator.qmin));
    obj.insert("voltage_setpoint_pu".into(), jnum(generator.vg));
    obj.insert("machine_base_mva".into(), jnum(generator.mbase));
    obj.insert("in_service".into(), Value::Bool(generator.in_service));
    obj.insert("gen_type".into(), Value::String("Synchronous".into()));
    obj.insert("pfr_eligible".into(), Value::Bool(true));
    obj.insert("quick_start".into(), Value::Bool(false));
    obj.insert("voltage_regulated".into(), Value::Bool(true));
    if let Some(cost) = &generator.cost {
        if let Some(cost) = cost_obj(cost, warnings) {
            obj.insert("cost".into(), cost);
        }
    }
    if generator.has_caps() {
        warnings.push(format!(
            "generator at bus {} has MATPOWER capability or ramp columns not represented in Surge JSON",
            generator.bus
        ));
    }
    Value::Object(obj)
}

fn cost_obj(cost: &GenCost, warnings: &mut Vec<String>) -> Option<Value> {
    match cost.model {
        2 => {
            let count = cost.ncost.min(cost.coeffs.len());
            let coeffs = cost.coeffs[..count].iter().copied().map(jnum).collect();
            let mut curve = Map::new();
            curve.insert("coeffs".into(), Value::Array(coeffs));
            curve.insert("startup".into(), jnum(cost.startup));
            curve.insert("shutdown".into(), jnum(cost.shutdown));

            let mut wrapper = Map::new();
            wrapper.insert("Polynomial".into(), Value::Object(curve));
            Some(Value::Object(wrapper))
        }
        1 => {
            let count = (cost.ncost * 2).min(cost.coeffs.len());
            if count % 2 != 0 {
                warnings.push(
                    "piecewise generator cost has an odd coefficient count; cost dropped".into(),
                );
                return None;
            }
            let mut points = Vec::new();
            for pair in cost.coeffs[..count].chunks(2) {
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

fn storage_gen_obj(storage: &Storage, counts: &mut BTreeMap<BusId, usize>) -> Value {
    let mut obj = storage
        .extras
        .get("surge_generator")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if !obj.contains_key("id") {
        obj.insert(
            "id".into(),
            Value::String(next_id("storage", counts, storage.bus)),
        );
    }
    obj.insert("bus".into(), Value::from(storage.bus.0 as u64));
    obj.insert("p".into(), jnum(storage.ps));
    obj.insert("q".into(), jnum(storage.qs));
    obj.insert("pmax".into(), jnum(storage.discharge_rating));
    obj.insert("pmin".into(), jnum(-storage.charge_rating));
    obj.insert("qmax".into(), jnum(storage.qmax));
    obj.insert("qmin".into(), jnum(storage.qmin));
    obj.insert("voltage_setpoint_pu".into(), jnum(1.0));
    obj.insert(
        "machine_base_mva".into(),
        jnum(storage.thermal_rating.max(1.0)),
    );
    obj.insert("in_service".into(), Value::Bool(storage.in_service));
    obj.entry("gen_type")
        .or_insert_with(|| Value::String("Synchronous".into()));
    obj.entry("pfr_eligible").or_insert(Value::Bool(true));
    obj.entry("quick_start").or_insert(Value::Bool(false));
    obj.entry("voltage_regulated").or_insert(Value::Bool(false));

    let mut storage_obj = storage
        .extras
        .get("surge_storage")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    storage_obj.insert("energy_capacity_mwh".into(), jnum(storage.energy_rating));
    storage_obj.insert("soc_initial_mwh".into(), jnum(storage.energy));
    storage_obj.insert("soc_min_mwh".into(), jnum(0.0));
    storage_obj.insert("soc_max_mwh".into(), jnum(storage.energy_rating));
    storage_obj.insert("charge_efficiency".into(), jnum(storage.charge_efficiency));
    storage_obj.insert(
        "discharge_efficiency".into(),
        jnum(storage.discharge_efficiency),
    );
    storage_obj
        .entry("variable_cost_per_mwh")
        .or_insert_with(|| jnum(0.0));
    storage_obj
        .entry("degradation_cost_per_mwh")
        .or_insert_with(|| jnum(0.0));
    storage_obj
        .entry("dispatch_mode")
        .or_insert_with(|| Value::String("CostMinimization".into()));
    obj.insert("storage".into(), Value::Object(storage_obj));

    Value::Object(obj)
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
        || dc.cost.is_some()
    {
        warnings.push(format!(
            "dcline {} reactive limits, loss model, or cost mapped best effort in Surge JSON",
            i + 1
        ));
    }

    let mut obj = Map::new();
    obj.insert("technology".into(), Value::String("lcc".into()));
    obj.insert("name".into(), Value::String(format!("dcl_{}", i + 1)));
    obj.insert(
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
    obj.insert("rectifier".into(), lcc_terminal_obj(dc.from, dc.in_service));
    obj.insert("inverter".into(), lcc_terminal_obj(dc.to, dc.in_service));
    obj.insert("scheduled_setpoint".into(), jnum(dc.pf));
    obj.insert("p_dc_min_mw".into(), jnum(dc.pmin));
    obj.insert("p_dc_max_mw".into(), jnum(dc.pmax));
    obj.insert("scheduled_voltage_kv".into(), jnum(0.0));
    obj.insert("resistance_ohm".into(), jnum(0.0));
    Value::Object(obj)
}

fn lcc_terminal_obj(bus: BusId, in_service: bool) -> Value {
    let mut obj = Map::new();
    obj.insert("bus".into(), Value::from(bus.0 as u64));
    obj.insert("in_service".into(), Value::Bool(in_service));
    obj.insert("n_bridges".into(), Value::from(1_u64));
    obj.insert("alpha_min".into(), jnum(5.0));
    obj.insert("alpha_max".into(), jnum(90.0));
    obj.insert("base_voltage_kv".into(), jnum(0.0));
    obj.insert("commutation_reactance_ohm".into(), jnum(0.0));
    obj.insert("commutation_resistance_ohm".into(), jnum(0.0));
    obj.insert("tap".into(), jnum(1.0));
    obj.insert("tap_min".into(), jnum(0.9));
    obj.insert("tap_max".into(), jnum(1.1));
    obj.insert("tap_step".into(), jnum(0.00625));
    obj.insert("turns_ratio".into(), jnum(1.0));
    Value::Object(obj)
}

pub fn parse_surge_json(content: &str) -> Result<Parsed> {
    let mut warnings = Vec::new();
    let network = parse_surge_source(Arc::new(content.to_owned()), None, &mut warnings)?;
    Ok(Parsed { network, warnings })
}

pub(crate) fn parse_surge_source(
    source: Arc<String>,
    name_hint: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<Network> {
    let root_value: Value = serde_json::from_str(&source).map_err(|e| Error::FormatRead {
        format: FMT,
        message: e.to_string(),
    })?;
    let root = object(&root_value, "top level")?;
    validate_envelope(root)?;
    let network = object_field(root, "network")?;

    warnings.extend(source_loss_warnings_from_root(root, network));

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
        if let Some(generator) = generator {
            generators.push(generator);
        }
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
        base_frequency: f_map_or(network, "freq_hz", crate::network::DEFAULT_BASE_FREQUENCY)?,
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
        switches: Vec::new(),
        generators,
        storage,
        hvdc: read_hvdc(network)?,
        transformers_3w: Vec::new(),
        areas: Vec::new(),
        solver: None,
        source_format: SourceFormat::SurgeJson,
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
    if let Some(producer) = string_map(meta, "producer")
        && producer != "surge"
    {
        return Err(format_error(format!(
            "unsupported `meta.producer` value `{producer}`"
        )));
    }
    if let Some(profile) = string_map(meta, "profile")
        && !matches!(profile, "network" | "dispatch" | "results")
    {
        return Err(format_error(format!(
            "unsupported `meta.profile` value `{profile}`"
        )));
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
            control: None,
            uid: None,
            extras: Extras::new(),
        })
    } else {
        None
    };
    let bus = Bus {
        id,
        kind: read_bus_type(string_map(obj, "bus_type").unwrap_or("PQ"))?,
        vm: f_map_or(obj, "voltage_magnitude_pu", 1.0)?,
        va: f_map_or(obj, "voltage_angle_rad", 0.0)? * normalize::RAD_TO_DEG,
        base_kv: f_map_or(obj, "base_kv", 0.0)?,
        vmax: f_map_or(obj, "voltage_max_pu", 1.1)?,
        vmin: f_map_or(obj, "voltage_min_pu", 0.9)?,
        evhi: None,
        evlo: None,
        area: usize_map_or(obj, "area", 1)?,
        zone: usize_map_or(obj, "zone", 1)?,
        name: string_map(obj, "name")
            .filter(|name| !name.is_empty())
            .map(str::to_string),
        uid: None,
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
    let p = f_map_or(obj, "active_power_demand_mw", 0.0)?;
    let q = f_map_or(obj, "reactive_power_demand_mvar", 0.0)?;
    Ok(Load {
        bus: BusId(required_usize(obj, "bus")?),
        p,
        q,
        voltage_model: read_load_voltage_model(obj, p, q)?,
        in_service: bool_map_or(obj, "in_service", true)?,
        uid: None,
        extras: Extras::new(),
    })
}

fn read_load_voltage_model(
    obj: &Map<String, Value>,
    p: f64,
    q: f64,
) -> Result<Option<LoadVoltageModel>> {
    let pz = f_map_or(obj, "zip_p_impedance_frac", 0.0)?;
    let pi = f_map_or(obj, "zip_p_current_frac", 0.0)?;
    let pp = f_map_or(obj, "zip_p_power_frac", 1.0)?;
    let qz = f_map_or(obj, "zip_q_impedance_frac", 0.0)?;
    let qi = f_map_or(obj, "zip_q_current_frac", 0.0)?;
    let qp = f_map_or(obj, "zip_q_power_frac", 1.0)?;
    let is_default = (pz.abs() <= EPS)
        && (pi.abs() <= EPS)
        && ((pp - 1.0).abs() <= EPS)
        && (qz.abs() <= EPS)
        && (qi.abs() <= EPS)
        && ((qp - 1.0).abs() <= EPS);
    if is_default {
        Ok(None)
    } else {
        Ok(Some(LoadVoltageModel::Zip {
            p_constant_power: p * pp,
            q_constant_power: q * qp,
            p_constant_current: p * pi,
            q_constant_current: q * qi,
            p_constant_impedance: p * pz,
            q_constant_impedance: q * qz,
            v_nom: None,
            load_type: None,
            scaling: None,
        }))
    }
}

fn read_fixed_shunt(value: &Value) -> Result<Shunt> {
    let obj = object(value, "fixed_shunt record")?;
    Ok(Shunt {
        bus: BusId(required_usize(obj, "bus")?),
        g: f_map_alias_or(obj, &["g_mw", "conductance_mw"], 0.0)?,
        b: f_map_alias_or(obj, &["b_mvar", "susceptance_mvar"], 0.0)?,
        in_service: bool_map_or(obj, "in_service", true)?,
        control: None,
        uid: None,
        extras: Extras::new(),
    })
}

fn read_branch(value: &Value) -> Result<Branch> {
    let obj = object(value, "branch record")?;
    let branch_type = string_map(obj, "branch_type").unwrap_or("Line");
    let tap_value = f_map_or(obj, "tap", 1.0)?;
    let shift = f_map_or(obj, "phase_shift_rad", 0.0)? * normalize::RAD_TO_DEG;
    let tap = if branch_type == "Line" && (tap_value - 1.0).abs() < EPS {
        0.0
    } else {
        tap_value
    };
    let b = f_map_or(obj, "b", 0.0)?;
    Ok(Branch {
        from: BusId(required_usize(obj, "from_bus")?),
        to: BusId(required_usize(obj, "to_bus")?),
        r: f_map_or(obj, "r", 0.0)?,
        x: f_map_or(obj, "x", 0.0)?,
        b,
        charging: read_branch_charging(obj, b)?,
        rate_a: f_map_or(obj, "rating_a_mva", 0.0)?,
        rate_b: f_map_or(obj, "rating_b_mva", 0.0)?,
        rate_c: f_map_or(obj, "rating_c_mva", 0.0)?,
        rating_sets: Vec::new(),
        current_ratings: read_current_ratings(obj)?,
        tap,
        shift,
        in_service: bool_map_or(obj, "in_service", true)?,
        angmin: f_map_or(obj, "angle_diff_min_rad", -std::f64::consts::TAU)?
            * normalize::RAD_TO_DEG,
        angmax: f_map_or(obj, "angle_diff_max_rad", std::f64::consts::TAU)? * normalize::RAD_TO_DEG,
        control: None,
        solution: read_branch_solution(obj)?,
        uid: None,
        extras: Extras::new(),
    })
}

fn read_branch_charging(obj: &Map<String, Value>, b: f64) -> Result<Option<BranchCharging>> {
    let has_terminal = [
        "g_shunt_from",
        "b_shunt_from",
        "g_shunt_to",
        "b_shunt_to",
        "g_fr",
        "b_fr",
        "g_to",
        "b_to",
    ]
    .iter()
    .any(|key| obj.contains_key(*key));
    if !has_terminal {
        return Ok(None);
    }
    Ok(Some(BranchCharging {
        g_fr: f_map_alias_or(obj, &["g_shunt_from", "g_fr"], 0.0)?,
        b_fr: f_map_alias_or(obj, &["b_shunt_from", "b_fr"], b / 2.0)?,
        g_to: f_map_alias_or(obj, &["g_shunt_to", "g_to"], 0.0)?,
        b_to: f_map_alias_or(obj, &["b_shunt_to", "b_to"], b / 2.0)?,
    }))
}

fn read_current_ratings(obj: &Map<String, Value>) -> Result<Option<BranchCurrentRatings>> {
    let has_rating = [
        "current_rating_a",
        "current_rating_b",
        "current_rating_c",
        "c_rating_a",
        "c_rating_b",
        "c_rating_c",
    ]
    .iter()
    .any(|key| obj.contains_key(*key));
    if !has_rating {
        return Ok(None);
    }
    Ok(Some(BranchCurrentRatings {
        c_rating_a: f_map_alias_or(obj, &["current_rating_a", "c_rating_a"], 0.0)?,
        c_rating_b: f_map_alias_or(obj, &["current_rating_b", "c_rating_b"], 0.0)?,
        c_rating_c: f_map_alias_or(obj, &["current_rating_c", "c_rating_c"], 0.0)?,
    }))
}

fn read_branch_solution(obj: &Map<String, Value>) -> Result<Option<BranchSolution>> {
    let has_solution = [
        "pf_mw", "qf_mvar", "pt_mw", "qt_mvar", "pf", "qf", "pt", "qt",
    ]
    .iter()
    .any(|key| obj.contains_key(*key));
    if !has_solution {
        return Ok(None);
    }
    Ok(Some(BranchSolution {
        pf: f_map_alias_or(obj, &["pf_mw", "pf"], 0.0)?,
        qf: f_map_alias_or(obj, &["qf_mvar", "qf"], 0.0)?,
        pt: f_map_alias_or(obj, &["pt_mw", "pt"], 0.0)?,
        qt: f_map_alias_or(obj, &["qt_mvar", "qt"], 0.0)?,
    }))
}

fn read_generator(value: &Value) -> Result<(Option<Generator>, Option<Storage>)> {
    let obj = object(value, "generator record")?;
    let mut caps: GenCaps = [None; GEN_EXTRA_KEYS.len()];
    if let Some(apf) = obj.get("agc_participation_factor").and_then(Value::as_f64)
        && let Some(slot) = GEN_EXTRA_KEYS.iter().position(|key| *key == "apf")
    {
        caps[slot] = Some(apf);
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
        regulated_bus: optional_usize(obj, "reg_bus")?.map(BusId),
        uid: None,
    };

    let storage = match obj.get("storage") {
        Some(Value::Null) | None => None,
        Some(value) => {
            let mut storage = read_storage(value, bus, pg, qg, pmax, pmin, qmax, qmin, in_service)?;
            retain_storage_generator_metadata(&mut storage, obj);
            Some(storage)
        }
    };

    if storage.is_some() {
        Ok((None, storage))
    } else {
        Ok((Some(generator), None))
    }
}

fn retain_storage_generator_metadata(storage: &mut Storage, generator: &Map<String, Value>) {
    let mut metadata = generator.clone();
    metadata.remove("storage");
    if !metadata.is_empty() {
        storage
            .extras
            .insert("surge_generator".to_owned(), Value::Object(metadata));
    }
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
                .ok_or_else(|| format_error("piecewise cost point must be a two element array"))?;
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
    let mut out = Storage {
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
        current_rating: f_map_opt(obj, "current_rating")?,
        qmin,
        qmax,
        r: 0.0,
        x: 0.0,
        p_loss: 0.0,
        q_loss: 0.0,
        in_service,
        uid: None,
        extras: Extras::new(),
    };
    out.extras
        .insert("surge_storage".to_owned(), Value::Object(obj.clone()));
    Ok(out)
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
        cost: None,
        uid: None,
        extras: Extras::new(),
    })
}

fn source_loss_warnings_from_root(
    root: &Map<String, Value>,
    network: &Map<String, Value>,
) -> Vec<String> {
    let mut warnings = Vec::new();

    let profile = root
        .get("meta")
        .and_then(Value::as_object)
        .and_then(|meta| string_map(meta, "profile"));
    if matches!(profile, Some("dispatch" | "results")) || has_nonempty(root, "dispatch") {
        warnings.push("Surge dispatch profile data retained only in source text".into());
    }
    if matches!(profile, Some("results")) || has_nonempty(root, "solution") {
        warnings.push("Surge solution profile data retained only in source text".into());
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
    let retained_top: Vec<&str> = top
        .into_iter()
        .filter(|key| has_nonempty(network, key))
        .collect();
    if !retained_top.is_empty() {
        warnings.push(format!(
            "Surge network sections retained only in source text: {}",
            retained_top.join(", ")
        ));
    }

    warn_count(
        &mut warnings,
        network,
        "loads",
        "load composition, frequency, classification, or ownership fields retained only in source text",
        load_has_source_only_fields,
    );
    warn_count(
        &mut warnings,
        network,
        "branches",
        "branch control, phase shifter bounds, sequence, thermal, cost, or circuit metadata retained only in source text",
        branch_has_source_only_fields,
    );
    warn_count(
        &mut warnings,
        network,
        "generators",
        "generator commitment, ramping, fuel, market, reserve, emission, classification, or richer storage fields retained only in source text",
        generator_has_source_only_fields,
    );
    if has_nonempty(network, "hvdc") {
        warnings.push(
            "Surge HVDC converter, reactive, loss, and control details mapped best effort".into(),
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
        warnings.push(format!("{count} Surge {message}"));
    }
}

fn load_has_source_only_fields(load: &Map<String, Value>) -> bool {
    num_not_default(load, "freq_sensitivity_p_pct_per_hz", 0.0)
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

fn optional_usize(obj: &Map<String, Value>, key: &str) -> Result<Option<usize>> {
    match obj.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value_to_usize(value, key).map(Some),
    }
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

fn f_map_opt(obj: &Map<String, Value>, key: &str) -> Result<Option<f64>> {
    match obj.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value_to_f64(value, key).map(Some),
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
            .filter(|value| value.is_finite())
            .ok_or_else(|| format_error(format!("`{key}` is not a finite f64"))),
        Value::String(value) => {
            let parsed = value
                .parse::<f64>()
                .map_err(|_| format_error(format!("`{key}` string is not a f64")))?;
            if parsed.is_finite() {
                Ok(parsed)
            } else {
                Err(format_error(format!("`{key}` string is not a finite f64")))
            }
        }
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
            "branch_type": "Line",
            "tap": 1.0,
            "phase_shift_rad": 0.1
        }))
        .unwrap();
        assert!(branch.tap.abs() < EPS);
        assert!((branch.shift - 0.1 * normalize::RAD_TO_DEG).abs() < EPS);

        let branch = read_branch(&serde_json::json!({
            "from_bus": 1,
            "to_bus": 2,
            "branch_type": "Transformer",
            "tap": 1.0
        }))
        .unwrap();
        assert!((branch.tap - 1.0).abs() < EPS);
    }

    #[test]
    fn preserves_branch_terminal_charging() {
        let branch = read_branch(&serde_json::json!({
            "from_bus": 1,
            "to_bus": 2,
            "g_shunt_from": 0.1,
            "b_shunt_from": 0.2,
            "g_shunt_to": 0.3,
            "b_shunt_to": 0.4
        }))
        .unwrap();
        let charging = branch.charging.unwrap();
        assert!((charging.g_fr - 0.1).abs() < EPS);
        assert!((charging.b_fr - 0.2).abs() < EPS);
        assert!((charging.g_to - 0.3).abs() < EPS);
        assert!((charging.b_to - 0.4).abs() < EPS);
    }

    #[test]
    fn rejects_nonfinite_numeric_strings() {
        let err = parse_surge_json(
            r#"{
              "format": "surge-json",
              "schema_version": "0.1.0",
              "meta": {},
              "network": {
                "buses": [
                  {"number": 1, "voltage_angle_rad": "NaN"}
                ]
              }
            }"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::FormatRead { .. }));
    }
}

//! Write a [`Network`] as EGRET `ModelData` JSON.
//!
//! EGRET groups the network under `elements` (bus, load, branch, generator,
//! shunt) with a small `system` block; values stay in MW/MVAr, degrees, with the
//! base in `system.baseMVA`. Loads and shunts are first-class on the `Network`,
//! generator cost becomes a polynomial/piecewise `cost_curve`, and a branch with
//! a nonzero raw tap or a phase shift is typed `transformer`. Schema-faithful;
//! validate against EGRET's own loader when it is in the toolchain.

use serde_json::{Map, Value};

use super::{finish, jnum, Conversion};
use crate::network::{BusType, GenCost};
use crate::network::{Branch, Bus, Generator, Load, Network, Shunt};

#[must_use]
pub fn write_egret_json(net: &Network) -> Conversion {
    let mut warnings = Vec::new();

    let mut bus = Map::new();
    for b in &net.buses {
        bus.insert(b.id.to_string(), bus_obj(b));
    }

    // EGRET keys each load/shunt; use a global running suffix (load_1, load_2, …)
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
            "{} dcline(s) dropped: EGRET HVDC mapping not implemented",
            net.hvdc.len()
        ));
    }
    if !net.storage.is_empty() {
        warnings.push(format!(
            "{} storage unit(s) dropped: EGRET storage mapping not implemented",
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
    m.insert("matpower_bustype".into(), Value::String(bustype(b.kind).into()));
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
    // EGRET treats a zero rating as "unset"; emit only nonzero limits.
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
                "generator at bus {} has a cost model EGRET's writer can't express; cost dropped",
                g.bus
            ));
        }
    }
    Value::Object(m)
}

/// EGRET `cost_curve`. MATPOWER model 2 (polynomial) maps to a degree→coefficient
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

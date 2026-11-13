//! SiennaSchemas SystemDocument electrical exchange, versions 0.1 through 0.4.
//!
//! Component IDs and bus numbers are separate namespaces. All component-base
//! parameters are converted to the network's 100 MVA base on input. The module
//! extension retains source fields which have no electrical interpretation.

use super::TextEmission;
use crate::diagnostics::{Diagnostics, codes};
use crate::network::{
    BalancedNetwork, BalancedNetworkTables, Branch, BranchCharging, Bus, BusId, BusType, GenCost,
    Generator, Load, LoadVoltageModel, Shunt, SourceFormat, Storage,
};
use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const EXTENSION: &str = "org.eigenergy.powerio.sienna";
mod areas;
mod control;
mod generators;
mod hvdc;
mod merge;
mod metadata;
pub(super) mod powersystems;
mod powersystems_three_winding;
mod shunts;
mod three_winding;
const BASE: f64 = 100.0;
const FMT: &str = "SiennaSchemas JSON";
const ELECTRICAL_KINDS: &[&str] = &[
    "ACBus",
    "Area",
    "LoadZone",
    "Arc",
    "Line",
    "TransformerCircuit",
    "TwoWindingTransformer",
    "ThreeWindingTransformer",
    "PowerLoad",
    "StandardLoad",
    "InterruptiblePowerLoad",
    "InterruptibleStandardLoad",
    "FixedAdmittance",
    "SwitchedAdmittance",
    "ThermalStandard",
    "ThermalMultiStart",
    "RenewableDispatch",
    "RenewableNonDispatch",
    "HydroDispatch",
    "Source",
    "SynchronousCondenser",
    "EnergyReservoirStorage",
    "TwoTerminalGenericHVDCLine",
];

pub(super) fn token(version: u8) -> &'static str {
    match version {
        1 => "sienna-0-1-json",
        2 => "sienna-0-2-json",
        3 => "sienna-0-3-json",
        4 => "sienna-0-4-json",
        _ => "sienna-json",
    }
}
pub(super) fn error(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FMT,
        message: message.into(),
    }
}
pub(super) fn version(root: &Value) -> Result<u8> {
    match root.get("schema_version") {
        None => Ok(1),
        Some(Value::String(s)) => match s.as_str() {
            "0.1.0" => Ok(1),
            "0.2.0" => Ok(2),
            "0.3.0" => Ok(3),
            "0.4.0" => Ok(4),
            _ => Err(error(format!(
                "unsupported schema_version {s:?}; supported releases: 0.1.0, 0.2.0, 0.3.0, 0.4.0"
            ))),
        },
        _ => Err(error("schema_version must be a string")),
    }
}
pub(super) fn document_version(text: &str) -> Result<u8> {
    version(&serde_json::from_str::<Value>(text).map_err(|e| error(e.to_string()))?)
}
fn integer(v: &Value, k: &str) -> Result<i64> {
    v.get(k)
        .and_then(Value::as_i64)
        .ok_or_else(|| error(format!("{k} must be an integer in component {}", v["id"])))
}
fn number(v: &Value, k: &str) -> Result<f64> {
    v.get(k)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .ok_or_else(|| {
            error(format!(
                "{k} must be a finite number in component {}",
                v["id"]
            ))
        })
}
fn optional(v: &Value, k: &str, default: f64) -> Result<f64> {
    if v.get(k).is_some_and(|x| !x.is_null()) {
        number(v, k)
    } else {
        Ok(default)
    }
}

/// PowerSystems' `INFINITE_BOUND`: the per-unit rating its parsers state for
/// an unbounded branch or winding (a zero or absent PSS/E rating). JSON cannot
/// state infinity and PowerSystems requires a numeric `Line.rating` and winding
/// ratings, so an unlimited rating is written as this bound and read back as
/// unlimited. PowerSystems' own validation replaces it with the rate it
/// derives from the line's parameters.
pub(super) const INFINITE_BOUND: f64 = 1e6;

/// Whether a per-unit rating is PowerSystems' unbounded marker, allowing the
/// rounding a natural-unit value picks up through its component base.
pub(super) fn is_infinite_bound(per_unit: f64) -> bool {
    (per_unit - INFINITE_BOUND).abs() <= INFINITE_BOUND * 1e-12
}

/// Writes a branch's MVA ratings on a row whose base is `base_mva`.
/// SiennaSchemas 0.1–0.4 and PowerSystems 5.x require a numeric primary
/// `rating` and allow `rating_b` and `rating_c` to be absent, which reads as
/// unlimited. An unlimited primary rating is PowerSystems' `INFINITE_BOUND`
/// on the row's base, never zero, which is a real bound.
pub(super) fn put_ratings(row: &mut Value, [a, b, c]: [f64; 3], base_mva: f64) {
    row["rating"] = json!(if a.is_finite() {
        a
    } else {
        INFINITE_BOUND * base_mva
    });
    for (key, rating) in [("rating_b", b), ("rating_c", c)] {
        if rating.is_finite() {
            row[key] = json!(rating);
        }
    }
}

/// A branch rating in MVA: absent, null, or the unbounded marker is unlimited.
pub(super) fn branch_rating(row: &Value, k: &str, scale: f64) -> Result<f64> {
    let rating = thermal_limit(row, k, scale)?;
    Ok(
        if rating.is_finite() && is_infinite_bound(rating / base(row)?) {
            f64::INFINITY
        } else {
            rating
        },
    )
}
fn thermal_limit(v: &Value, k: &str, scale: f64) -> Result<f64> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(f64::INFINITY),
        Some(_) => crate::thermal_limit::scaled(number(v, k)?, scale),
    }
}
fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| error(format!("{k} must be a string in component {}", v["id"])))
}
fn available(v: &Value) -> Result<bool> {
    v["available"].as_bool().ok_or_else(|| {
        error(format!(
            "available must be boolean in component {}",
            v["id"]
        ))
    })
}
fn rows<'a>(root: &'a Value, kind: &str) -> Result<&'a [Value]> {
    match root["components"].get(kind) {
        None => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        _ => Err(error(format!("components.{kind} must be an array"))),
    }
}
fn uid(kind: &str, row: &Value) -> Result<Option<String>> {
    Ok(Some(format!("sienna:{kind}:{}", integer(row, "id")?)))
}
fn positive(v: f64, what: &str) -> Result<f64> {
    if v > 0.0 && v.is_finite() {
        Ok(v)
    } else {
        Err(error(format!("{what} must be finite and positive")))
    }
}
fn base(row: &Value) -> Result<f64> {
    positive(optional(row, "base_power", BASE)?, "base_power")
}
fn power_scale(row: &Value) -> Result<f64> {
    match string(row, "power_units")? {
        "NATURAL_UNITS" => Ok(1.0),
        "COMPONENT_BASE" => base(row),
        other => Err(error(format!("unsupported power_units {other}"))),
    }
}
fn parameter_scale(row: &Value, kv: f64) -> Result<f64> {
    match row
        .get("parameter_units")
        .and_then(Value::as_str)
        .unwrap_or("COMPONENT_BASE")
    {
        "COMPONENT_BASE" => Ok(BASE / base(row)?),
        "NATURAL_UNITS" => Ok(BASE / positive(kv, "base_voltage")?.powi(2)),
        other => Err(error(format!("unsupported parameter_units {other}"))),
    }
}
fn limits(row: &Value, key: &str, default: (f64, f64)) -> Result<(f64, f64)> {
    match row.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => Ok((number(v, "min")?, number(v, "max")?)),
    }
}
fn end(row: &Value, key: &str, side: &str) -> Result<f64> {
    match row.get(key) {
        None => Ok(0.0),
        Some(v) => number(v, side),
    }
}
fn bus_ref(row: &Value, field: &str, ids: &BTreeMap<i64, BusId>) -> Result<BusId> {
    let id = integer(row, field)?;
    ids.get(&id)
        .copied()
        .ok_or_else(|| error(format!("{field} references missing ACBus id {id}")))
}
fn retained(warnings: &mut Diagnostics, message: impl Into<String>) {
    warnings.push(&codes::READ_SIENNA_RETAINED_SOURCE_ONLY, message);
}

#[allow(clippy::too_many_lines, clippy::float_cmp)] // Table mapping keeps exact source distinctions.
pub(super) fn read(
    root: &Value,
    name_hint: Option<&str>,
    requested: u8,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let actual = version(root)?;
    if requested != 0 && requested != actual {
        return Err(error(format!(
            "requested 0.{requested}.0, document is 0.{actual}.0"
        )));
    }
    let components = root["components"]
        .as_object()
        .ok_or_else(|| error("components must be an object"))?;
    let mut all_ids = BTreeSet::new();
    for (kind, list) in components {
        for row in list
            .as_array()
            .ok_or_else(|| error(format!("components.{kind} must be an array")))?
        {
            let id = integer(row, "id")?;
            if !all_ids.insert(id) {
                return Err(error(format!("duplicate component id {id}")));
            }
        }
    }
    let (areas, area_numbers, zone_numbers) = areas::read(root)?;
    let mut buses = Vec::new();
    let mut ids = BTreeMap::new();
    let mut numbers = BTreeSet::new();
    for row in rows(root, "ACBus")? {
        let id = integer(row, "id")?;
        let n = usize::try_from(integer(row, "number")?)
            .map_err(|_| error("bus number must be nonnegative"))?;
        if !numbers.insert(n) {
            return Err(error(format!("duplicate bus number {n}")));
        }
        let kind = match row.get("bustype").and_then(Value::as_str).unwrap_or("PQ") {
            "PQ" => BusType::Pq,
            "PV" => BusType::Pv,
            "REF" | "SLACK" => BusType::Ref,
            "ISOLATED" => BusType::Isolated,
            other => return Err(error(format!("unknown bustype {other}"))),
        };
        let mut bus = Bus::new(BusId(n), kind, optional(row, "base_voltage", 0.0)?);
        bus.name = Some(string(row, "name")?.into());
        bus.uid = uid("ACBus", row)?;
        if row.get("area").is_some_and(|v| !v.is_null()) {
            bus.area = *area_numbers
                .get(&integer(row, "area")?)
                .ok_or_else(|| error("bus references missing Area"))?;
        }
        if row.get("load_zone").is_some_and(|v| !v.is_null()) {
            bus.zone = *zone_numbers
                .get(&integer(row, "load_zone")?)
                .ok_or_else(|| error("bus references missing LoadZone"))?;
        }
        bus.vm = optional(row, "magnitude", 1.0)?;
        bus.va = optional(row, "angle", 0.0)?.to_degrees();
        (bus.vmin, bus.vmax) = limits(row, "voltage_limits", (0.9, 1.1))?;
        if !available(row)? {
            bus.kind = BusType::Isolated;
        }
        for field in [
            "bustype",
            "base_voltage",
            "magnitude",
            "angle",
            "voltage_limits",
        ] {
            if row.get(field).is_none() {
                warnings.push(
                    &codes::READ_SIENNA_VALUE_DEFAULTED,
                    format!("ACBus {id}: {field} absent; using the electrical profile default"),
                );
            }
        }
        ids.insert(id, bus.id);
        metadata::restore(root, row, &mut bus, &["uid", "name", "extras"])?;
        buses.push(bus);
    }
    let by_bus: BTreeMap<BusId, &Bus> = buses.iter().map(|b| (b.id, b)).collect();
    let mut arcs = BTreeMap::new();
    for row in rows(root, "Arc")? {
        arcs.insert(
            integer(row, "id")?,
            (bus_ref(row, "from_id", &ids)?, bus_ref(row, "to_id", &ids)?),
        );
    }
    let mut branches = Vec::new();
    for kind in ["Line", "TransformerCircuit"] {
        for row in rows(root, kind)? {
            let (from, to) = *arcs
                .get(&integer(row, "arc")?)
                .ok_or_else(|| error("branch references missing Arc"))?;
            let kv_from = by_bus[&from].base_kv;
            let mut z = parameter_scale(row, kv_from)?;
            let primary_kv = optional(row, "base_voltage_primary", kv_from)?;
            if kind == "TransformerCircuit"
                && row["parameter_units"].as_str() != Some("NATURAL_UNITS")
                && row.get("base_voltage_primary").is_some()
            {
                z *= (positive(primary_kv, "base_voltage_primary")?
                    / positive(kv_from, "bus base_voltage")?)
                .powi(2);
            }
            let p = power_scale(row)?;
            let mut branch = Branch::new(
                from,
                to,
                optional(row, "r", 0.0)? * z,
                optional(row, "x", 0.0)? * z,
            );
            branch.uid = uid(kind, row)?;
            branch.name = row.get("name").and_then(Value::as_str).map(str::to_owned);
            branch.in_service = available(row)?;
            branch.rate_a = branch_rating(row, "rating", p)?;
            branch.rate_b = branch_rating(row, "rating_b", p)?;
            branch.rate_c = branch_rating(row, "rating_c", p)?;
            if kind == "Line" {
                let ch = BranchCharging::new(
                    end(row, "g", "from")? / z,
                    end(row, "b", "from")? / z,
                    end(row, "g", "to")? / z,
                    end(row, "b", "to")? / z,
                );
                branch.b = ch.b_fr + ch.b_to;
                if ch.g_fr != 0.0 || ch.g_to != 0.0 || ch.b_fr != ch.b_to {
                    branch.charging = Some(ch);
                }
                let (lo, hi) = limits(
                    row,
                    "angle_limits",
                    (-std::f64::consts::TAU, std::f64::consts::TAU),
                )?;
                branch.angmin = lo.to_degrees();
                branch.angmax = hi.to_degrees();
            } else {
                branch.tap = optional(row, "tap", 1.0)?;
                if row.get("base_voltage_primary").is_some()
                    && row.get("base_voltage_secondary").is_some()
                {
                    let secondary_kv = positive(
                        number(row, "base_voltage_secondary")?,
                        "base_voltage_secondary",
                    )?;
                    branch.tap *= primary_kv / secondary_kv
                        * positive(by_bus[&to].base_kv, "bus base_voltage")?
                        / positive(kv_from, "bus base_voltage")?;
                }
                positive(branch.tap, "tap")?;
                branch.shift = optional(row, "alpha", 0.0)?.to_degrees();
                branch.control = control::read(row, actual)?;
            }
            metadata::restore(
                root,
                row,
                &mut branch,
                &["uid", "name", "extras", "rating_sets"],
            )?;
            if kind == "TransformerCircuit" {
                metadata::restore(root, row, &mut branch, &["angmin", "angmax"])?;
            }
            metadata::restore_branch_solution(root, row, &mut branch, warnings)?;
            branches.push(branch);
        }
    }
    let line_count = rows(root, "Line")?.len();
    let circuits = rows(root, "TransformerCircuit")?;
    // Circuit rows by id, built once: each transformer and secondary shunt
    // finds its circuit without scanning the table per component.
    let circuit_index: BTreeMap<i64, usize> = circuits
        .iter()
        .enumerate()
        .filter_map(|(index, r)| r["id"].as_i64().map(|id| (id, index)))
        .collect();
    for row in rows(root, "TwoWindingTransformer")? {
        let id = integer(row, "circuit")?;
        let index = *circuit_index
            .get(&id)
            .ok_or_else(|| error("missing transformer circuit"))?;
        let b = branches
            .get_mut(line_count + index)
            .ok_or_else(|| error(format!("missing TransformerCircuit {id}")))?;
        b.name = Some(string(row, "name")?.into());
        let circuit = &circuits[index];
        metadata::restore(root, circuit, b, &["uid", "name"])?;
        if let Some(y) = row.get("magnetizing_shunt") {
            let scale = match row
                .get("admittance_units")
                .and_then(Value::as_str)
                .unwrap_or("COMPONENT_BASE")
            {
                "COMPONENT_BASE" => {
                    let ratio = if circuit.get("base_voltage_primary").is_some() {
                        positive(by_bus[&b.from].base_kv, "bus base_voltage")?
                            / positive(
                                number(circuit, "base_voltage_primary")?,
                                "base_voltage_primary",
                            )?
                    } else {
                        1.0
                    };
                    base(circuit)? / BASE * ratio.powi(2)
                }
                "COMPONENT_MVAR" => 1.0 / BASE,
                "NATURAL_UNITS" => {
                    positive(by_bus[&b.from].base_kv, "base_voltage")?.powi(2) / BASE
                }
                other => return Err(error(format!("unsupported admittance_units {other}"))),
            };
            let g = number(y, "real")? * scale;
            let susceptance = number(y, "imag")? * scale;
            let loc = row
                .get("shunt_location")
                .and_then(Value::as_str)
                .unwrap_or("PRIMARY");
            b.charging = Some(match loc {
                "PRIMARY" => BranchCharging::new(
                    g * b.calc_effective_tap().powi(2),
                    susceptance * b.calc_effective_tap().powi(2),
                    0.0,
                    0.0,
                ),
                "SECONDARY" => BranchCharging::new(0.0, 0.0, g, susceptance),
                "SPLIT" => BranchCharging::new(
                    g * b.calc_effective_tap().powi(2),
                    susceptance * b.calc_effective_tap().powi(2),
                    g,
                    susceptance,
                ),
                other => return Err(error(format!("unsupported shunt_location {other}"))),
            });
            b.b = b.charging.expect("set above").calc_total_b();
        }
    }
    let mut loads = Vec::new();
    for kind in [
        "PowerLoad",
        "StandardLoad",
        "InterruptiblePowerLoad",
        "InterruptibleStandardLoad",
    ] {
        for row in rows(root, kind)? {
            let p = power_scale(row)?;
            let bus = bus_ref(row, "bus", &ids)?;
            let mut load = if kind.ends_with("StandardLoad") {
                let pp = optional(row, "constant_active_power", 0.0)? * p;
                let qp = optional(row, "constant_reactive_power", 0.0)? * p;
                let pi = optional(row, "current_active_power", 0.0)? * p;
                let qi = optional(row, "current_reactive_power", 0.0)? * p;
                let pz = optional(row, "impedance_active_power", 0.0)? * p;
                let qz = optional(row, "impedance_reactive_power", 0.0)? * p;
                let mut l = Load::new(bus, pp + pi + pz, qp + qi + qz);
                l.voltage_model = Some(LoadVoltageModel::Zip {
                    p_constant_power: pp,
                    q_constant_power: qp,
                    p_constant_current: pi,
                    q_constant_current: qi,
                    p_constant_impedance: pz,
                    q_constant_impedance: qz,
                    v_nom: None,
                    load_type: None,
                    scaling: None,
                });
                l
            } else {
                Load::new(
                    bus,
                    number(row, "active_power")? * p,
                    number(row, "reactive_power")? * p,
                )
            };
            load.in_service = available(row)?;
            load.uid = uid(kind, row)?;
            metadata::restore(root, row, &mut load, &["uid", "extras"])?;
            loads.push(load);
        }
    }
    let mut shunts = Vec::new();
    for row in rows(root, "FixedAdmittance")? {
        let bus = bus_ref(row, "bus", &ids)?;
        let scale = shunts::scale(row, by_bus[&bus].base_kv)?;
        // The writer states a transformer's secondary-terminal charging as a
        // FixedAdmittance on the to bus, since the schema carries one
        // magnetizing shunt per two-winding transformer. The ext marker folds
        // it back into the branch instead of adding a bus shunt the source
        // never had.
        let id = integer(row, "id")?;
        let extra = metadata::ext(root, id);
        if let Some(marker) = extra
            .get("transformer_circuit")
            .or_else(|| extra.get("transformer_uuid"))
        {
            // The marker folds the admittance back only while the shunt and
            // its transformer share one status. Once a native edit switches
            // one of them, the shunt stays a bus shunt with its own status,
            // so a disabled secondary shunt no longer charges a live branch.
            if let Some(branch) = marker
                .as_i64()
                .and_then(|circuit| circuit_index.get(&circuit))
                .and_then(|index| branches.get_mut(line_count + index))
                && branch.to == bus
                && available(row)? == branch.in_service
            {
                let mut charging = branch
                    .charging
                    .unwrap_or_else(|| BranchCharging::from_total_b(branch.b));
                charging.g_to += number(&row["Y"], "real")? * scale / BASE;
                charging.b_to += number(&row["Y"], "imag")? * scale / BASE;
                branch.charging = Some(charging);
                branch.b = charging.calc_total_b();
                continue;
            }
            retained(
                warnings,
                format!(
                    "FixedAdmittance {id}: secondary-shunt marker does not match a live transformer at the same bus and service state; preserved as an ordinary bus shunt"
                ),
            );
        }
        let mut sh = Shunt::new(
            bus,
            number(&row["Y"], "real")? * scale,
            number(&row["Y"], "imag")? * scale,
        );
        sh.uid = uid("FixedAdmittance", row)?;
        sh.in_service = available(row)?;
        metadata::restore(
            root,
            row,
            &mut sh,
            &["uid", "extras", "section_count", "control"],
        )?;
        shunts.push(sh);
    }
    for row in rows(root, "SwitchedAdmittance")? {
        shunts.push(shunts::read(root, row, &ids, &by_bus, actual)?);
    }
    let mut generators = Vec::new();
    for kind in generators::KNOWN {
        for row in rows(root, kind)? {
            let p = power_scale(row)?;
            let bus = bus_ref(row, "bus", &ids)?;
            let mut g = Generator::new(bus);
            g.uid = uid(kind, row)?;
            g.pg = if kind == "SynchronousCondenser" {
                -optional(row, "active_power_losses", 0.0)? * p
            } else {
                optional(row, "active_power", 0.0)? * p
            };
            g.qg = optional(row, "reactive_power", 0.0)? * p;
            let (lo, hi) = limits(
                row,
                "active_power_limits",
                (0.0, optional(row, "rating", g.pg / p)?),
            )?;
            g.pmin = lo * p;
            g.pmax = hi * p;
            if kind == "SynchronousCondenser" {
                g.pmin = g.pg;
                g.pmax = g.pg;
            }
            g.energy_source = generators::energy_source(kind, row);
            let q_default = if kind == "Source" {
                (0.0, 0.0)
            } else {
                (f64::NEG_INFINITY, f64::INFINITY)
            };
            let (lo, hi) = limits(row, "reactive_power_limits", q_default)?;
            g.qmin = lo * p;
            g.qmax = hi * p;
            g.mbase = base(row)?;
            g.vg = by_bus[&bus].vm;
            g.in_service = available(row)?
                && row
                    .get("status")
                    .and_then(Value::as_str)
                    .is_none_or(|s| s == "ONLINE");
            let cost_findings = warnings.len();
            if let Some(c) = row.get("operation_cost") {
                g.cost = read_cost(c, g.mbase, warnings)?;
            }
            metadata::restore(root, row, &mut g, &["uid", "vg", "caps"])?;
            if metadata::ext(root, integer(row, "id")?)["operation_cost_absent"] == true
                && generators::absent_placeholder(g.cost.as_ref(), warnings.len() == cost_findings)
            {
                g.cost = None;
            }
            generators.push(g);
        }
    }
    let mut storage = Vec::new();
    for row in rows(root, "EnergyReservoirStorage")? {
        let p = power_scale(row)?;
        let mut s = Storage::new(bus_ref(row, "bus", &ids)?);
        s.uid = uid("EnergyReservoirStorage", row)?;
        let e = match row
            .get("energy_units")
            .and_then(Value::as_str)
            .unwrap_or("MWH")
        {
            "MWH" => 1.0,
            "MWMIN" => 1.0 / 60.0,
            other => return Err(error(format!("unsupported energy_units {other}"))),
        };
        s.energy_rating = number(row, "storage_capacity")?
            * e
            * positive(
                optional(row, "conversion_factor", 1.0)?,
                "storage conversion_factor",
            )?;
        s.energy = number(row, "initial_storage_capacity_level")? * s.energy_rating;
        s.ps = -number(row, "active_power")? * p;
        s.qs = -number(row, "reactive_power")? * p;
        s.charge_rating = number(&row["input_active_power_limits"], "max")? * p;
        s.discharge_rating = number(&row["output_active_power_limits"], "max")? * p;
        s.charge_efficiency = number(&row["efficiency"], "in")?;
        s.discharge_efficiency = number(&row["efficiency"], "out")?;
        s.thermal_rating = number(row, "rating")? * p;
        let (lo, hi) = limits(row, "reactive_power_limits", (0.0, 0.0))?;
        s.qmin = -hi * p;
        s.qmax = -lo * p;
        s.in_service = available(row)?;
        metadata::restore(root, row, &mut s, &["uid"])?;
        storage.push(s);
    }
    let hvdc = hvdc::read_hvdc(root, &arcs, &by_bus, warnings)?;
    for (kind, list) in components {
        if !ELECTRICAL_KINDS.contains(&kind.as_str())
            && list.as_array().is_some_and(|v| !v.is_empty())
        {
            retained(
                warnings,
                format!(
                    "components.{kind} is retained in {EXTENSION}; excluded from the electrical value"
                ),
            );
        }
    }
    for key in [
        "time_series_associations",
        "supplemental_attributes",
        "service_associations",
        "plant_associations",
        "combined_cycle_associations",
        "voltage_control_associations",
    ] {
        if root
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|v| !v.is_empty())
        {
            retained(
                warnings,
                format!("{key} is retained in {EXTENSION}; excluded from the electrical value"),
            );
        }
    }
    if root
        .get("time_series_storage_file")
        .is_some_and(|v| !v.is_null())
    {
        retained(
            warnings,
            "time_series_storage_file requires the companion bundle; this electrical document does not decode its arrays",
        );
    }
    let transformers_3w = three_winding::read(root, &mut buses, &mut branches, &ids)?;
    let frequency = optional(root, "frequency", 60.0)?;
    positive(frequency, "frequency")?;
    let net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name: root
            .get("name")
            .and_then(Value::as_str)
            .or(name_hint)
            .unwrap_or("case")
            .into(),
        base_mva: BASE,
        base_frequency: frequency,
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
        storage: storage.into(),
        hvdc: hvdc.into(),
        transformers_3w: transformers_3w.into(),
        areas: areas.into(),
        solver: None,
        source_format: SourceFormat::SiennaJson,
    });
    net.check_references(FMT)?;
    Ok(net)
}

#[allow(clippy::too_many_lines)] // Normalize supported curve families and their cost additions together.
fn read_cost(cost: &Value, base: f64, warnings: &mut Diagnostics) -> Result<Option<GenCost>> {
    let c = &cost["variable_operation_cost"];
    if !matches!(c["variable_cost_type"].as_str(), Some("COST" | "FUEL"))
        || c["value_curve"]["curve_type"] != "INPUT_OUTPUT"
    {
        retained(
            warnings,
            "operation_cost outside input-output cost/fuel curves is retained without a typed generator cost",
        );
        return Ok(None);
    }
    let multiplier = if c["variable_cost_type"] == "FUEL" {
        if c["fuel_cost_time_series"].is_number() || !c["fuel_cost"].is_number() {
            retained(
                warnings,
                "time-varying fuel prices require a temporal calculation and remain in the retained document",
            );
            return Ok(None);
        }
        number(c, "fuel_cost")?
    } else {
        1.0
    };
    let scale = match c["power_units"].as_str() {
        Some("COMPONENT_BASE") => base,
        Some("NATURAL_UNITS") => 1.0,
        _ => return Err(error("invalid cost power_units")),
    };
    let f = &c["value_curve"]["function_data"];
    let mut out = match string(f, "function_type")? {
        "LINEAR" => GenCost::new(
            2,
            0.0,
            0.0,
            vec![
                number(f, "proportional_term")? / scale,
                number(f, "constant_term")?,
            ],
        ),
        "QUADRATIC" => GenCost::new(
            2,
            0.0,
            0.0,
            vec![
                number(f, "quadratic_term")? / scale.powi(2),
                number(f, "proportional_term")? / scale,
                number(f, "constant_term")?,
            ],
        ),
        "PIECEWISE_LINEAR" => {
            let mut coeffs = Vec::new();
            for pt in f["points"]
                .as_array()
                .ok_or_else(|| error("cost points must be an array"))?
            {
                coeffs.extend([number(pt, "x")? * scale, number(pt, "y")?]);
            }
            GenCost::new(1, 0.0, 0.0, coeffs)
        }
        _ => {
            retained(warnings, "unmapped cost function retained");
            return Ok(None);
        }
    };
    out.startup = if cost.get("start_up").is_some_and(Value::is_object) {
        retained(
            warnings,
            "staged startup costs are retained outside the scalar startup cost",
        );
        0.0
    } else {
        optional(cost, "start_up", 0.0)?
    };
    out.shutdown = optional(cost, "shut_down", 0.0)?;
    let fixed = optional(cost, "fixed", 0.0)?;
    let vom = &c["vom_cost"]["function_data"];
    let (slope, constant) = if vom.is_null() {
        (0.0, 0.0)
    } else if vom["function_type"] == "LINEAR" {
        (
            number(vom, "proportional_term")? / scale,
            number(vom, "constant_term")?,
        )
    } else {
        retained(
            warnings,
            "nonlinear variable operation and maintenance cost remains outside the typed generator cost",
        );
        (0.0, 0.0)
    };
    if out.model == 1 {
        for point in out.coeffs.as_chunks_mut::<2>().0 {
            point[1] = point[1] * multiplier + fixed + slope * point[0] + constant;
        }
    } else {
        for coefficient in &mut out.coeffs {
            *coefficient *= multiplier;
        }
        let n = out.coeffs.len();
        out.coeffs[n - 1] += fixed + constant;
        out.coeffs[n - 2] += slope;
    }
    if c.get("startup_fuel_offtake").is_some_and(|v| {
        v["function_data"]["proportional_term"]
            .as_f64()
            .is_some_and(|n| n != 0.0)
            || v["function_data"]["constant_term"]
                .as_f64()
                .is_some_and(|n| n != 0.0)
    }) {
        retained(
            warnings,
            "startup fuel offtake remains outside the scalar startup cost",
        );
    }
    Ok(Some(out))
}

fn cost_json(cost: Option<&GenCost>, warnings: &mut Diagnostics) -> Value {
    let zero = json!({"curve_type":"INPUT_OUTPUT","function_data":{"function_type":"LINEAR","constant_term":0.0,"proportional_term":0.0}});
    let mut function =
        json!({"function_type":"LINEAR","constant_term":0.0,"proportional_term":0.0});
    if let Some(c) = cost {
        if let Some((hessian, c1, c0)) = c.calc_quadratic_with_constant_tol(0.0) {
            function = json!({"function_type":"QUADRATIC","constant_term":c0,"proportional_term":c1,"quadratic_term":hessian / 2.0});
        } else if c.model == 1 && c.coeffs.len() % 2 == 0 {
            function = json!({"function_type":"PIECEWISE_LINEAR","points":c.coeffs.as_chunks::<2>().0.iter().map(|p|json!({"x":p[0],"y":p[1]})).collect::<Vec<_>>()});
        } else {
            warnings.push(
                &codes::EMIT_SIENNA.field_dropped,
                "generator cost outside linear/quadratic/piecewise-linear curves is omitted",
            );
        }
    }
    json!({"cost_type":"THERMAL","fixed":0.0,"start_up":cost.map_or(0.0,|c|c.startup),"shut_down":cost.map_or(0.0,|c|c.shutdown),"variable_operation_cost":{"variable_cost_type":"COST","power_units":"NATURAL_UNITS","value_curve":{"curve_type":"INPUT_OUTPUT","function_data":function},"vom_cost":zero}})
}

#[derive(Default)]
struct Ids {
    used: BTreeSet<i64>,
    reserved: BTreeSet<i64>,
    next: i64,
    original: BTreeMap<String, i64>,
    related: BTreeMap<(&'static str, i64), i64>,
    area_numbers: BTreeMap<(String, usize), i64>,
}
impl Ids {
    fn original_id(&self, uid: &str) -> Option<i64> {
        self.original.get(uid).copied().or_else(|| sienna_id(uid))
    }

    fn secondary_shunts(&mut self, original: &Value, net: &BalancedNetwork) -> Result<()> {
        let independent: BTreeSet<_> = net
            .shunts()
            .iter()
            .filter_map(|shunt| shunt.uid.as_deref().and_then(|uid| self.original_id(uid)))
            .collect();
        let arcs: BTreeMap<_, _> = rows(original, "Arc")?
            .iter()
            .map(|row| Ok((integer(row, "id")?, integer(row, "to_id")?)))
            .collect::<Result<_>>()?;
        let circuits: BTreeMap<_, _> = rows(original, "TransformerCircuit")?
            .iter()
            .map(|row| Ok((integer(row, "id")?, row)))
            .collect::<Result<_>>()?;
        for row in rows(original, "FixedAdmittance")? {
            let id = integer(row, "id")?;
            if !independent.contains(&id)
                && let Some(circuit) = metadata::ext(original, id)["transformer_circuit"].as_i64()
                && let Some(owner) = circuits.get(&circuit)
                && arcs.get(&integer(owner, "arc")?) == Some(&integer(row, "bus")?)
                && available(row)? == available(owner)?
            {
                self.related
                    .entry(("secondary_shunt", circuit))
                    .or_insert(id);
            }
        }
        Ok(())
    }

    fn related(&mut self, field: &'static str, owner: i64) -> i64 {
        let known = self.related.get(&(field, owner)).copied();
        self.claim(known)
    }
    fn get(&mut self, uid: Option<&str>) -> i64 {
        let known = uid.and_then(|u| self.original_id(u));
        self.claim(known)
    }
    /// A known id while it is still free, otherwise the next unreserved one.
    fn claim(&mut self, known: Option<i64>) -> i64 {
        if let Some(id) = known
            && self.used.insert(id)
        {
            return id;
        }
        loop {
            self.next += 1;
            if !self.reserved.contains(&self.next) && self.used.insert(self.next) {
                return self.next;
            }
        }
    }
}
/// The Sienna id a PowerIO uid of the form `sienna:<kind>:<id>` names.
fn sienna_id(uid: &str) -> Option<i64> {
    uid.strip_prefix("sienna:")?
        .rsplit(':')
        .next()?
        .parse()
        .ok()
}
fn push(doc: &mut Value, kind: &str, row: Value) {
    doc["components"]
        .as_object_mut()
        .expect("constructed object")
        .entry(kind)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("constructed array")
        .push(row);
}
fn minmax(lo: f64, hi: f64) -> Value {
    json!({"min":lo,"max":hi})
}

pub(super) fn write(
    net: &BalancedNetwork,
    requested: u8,
    original: Option<&Value>,
) -> Result<TextEmission> {
    let (doc, warnings) = write_document(net, requested, original)?;
    let text = serde_json::to_string_pretty(&doc).map_err(|e| error(e.to_string()))?;
    Ok(TextEmission::new(text, warnings))
}

/// The SystemDocument as a JSON value, for a writer that transforms it further.
#[allow(clippy::too_many_lines, clippy::float_cmp)] // One block for each emitted component table.
pub(super) fn write_document(
    net: &BalancedNetwork,
    requested: u8,
    original: Option<&Value>,
) -> Result<(Value, Diagnostics)> {
    net.check_base_mva()?;
    net.check_references(FMT)?;
    let selected = if requested == 0 {
        original.map(version).transpose()?.unwrap_or(4)
    } else {
        requested
    };
    if !(1..=4).contains(&selected) {
        return Err(error("unsupported Sienna output version"));
    }
    let mut warnings = Diagnostics::new();
    let mut ids = Ids::default();
    let mut bus_ids = BTreeMap::new();
    for u in net
        .buses()
        .iter()
        .filter_map(|x| x.uid.as_deref())
        .chain(net.branches().iter().filter_map(|x| x.uid.as_deref()))
        .chain(net.loads().iter().filter_map(|x| x.uid.as_deref()))
        .chain(net.generators().iter().filter_map(|x| x.uid.as_deref()))
        .chain(net.shunts().iter().filter_map(|x| x.uid.as_deref()))
        .chain(net.storage().iter().filter_map(|x| x.uid.as_deref()))
        .chain(net.hvdc().iter().filter_map(|x| x.uid.as_deref()))
    {
        if let Some(id) = sienna_id(u) {
            ids.reserved.insert(id);
        }
    }
    if let Some(attributes) = original.and_then(|o| o["supplemental_attributes"].as_array()) {
        for row in attributes {
            if let Some(id) = row["id"].as_i64() {
                ids.reserved.insert(id);
            }
        }
    }
    if let Some(c) = original.and_then(|o| o["components"].as_object()) {
        for list in c.values() {
            if let Some(rows) = list.as_array() {
                for r in rows {
                    if let Some(id) = r["id"].as_i64() {
                        ids.reserved.insert(id);
                        if let Some(name) =
                            original.and_then(|o| metadata::ext(o, id)["uid"].as_str())
                        {
                            ids.original.insert(name.into(), id);
                        }
                        for key in [
                            "arc",
                            "star_bus",
                            "primary_circuit",
                            "secondary_circuit",
                            "tertiary_circuit",
                        ] {
                            if let Some(target) = r[key].as_i64() {
                                ids.related.insert((key, id), target);
                            }
                        }
                        if let Some(circuit) = r["circuit"].as_i64() {
                            ids.related.insert(("two_winding", circuit), id);
                        }
                    }
                }
            }
        }
    }
    if let Some(original) = original {
        ids.secondary_shunts(original, net)?;
        for kind in ["Area", "LoadZone"] {
            for row in rows(original, kind)? {
                let id = integer(row, "id")?;
                let number = original["ext"][id.to_string()][metadata::KEY]["number"]
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .or_else(|| usize::try_from(id).ok());
                if let Some(number) = number {
                    ids.area_numbers.entry((kind.into(), number)).or_insert(id);
                }
            }
        }
    }
    let mut doc = json!({"name":net.name(),"frequency":net.base_frequency(),"components":{},"supplemental_attributes":[],"supplemental_attribute_associations":[],"plant_associations":[],"combined_cycle_associations":[],"service_associations":[],"time_series_associations":[],"time_series_storage_file":null});
    if selected != 1 {
        doc["schema_version"] = json!(format!("0.{selected}.0"));
    }
    let [area_ids, zone_ids] = areas::write(net, &mut doc, &mut ids, &mut warnings, selected)?;
    for b in net.buses() {
        let id = ids.get(b.uid.as_deref());
        bus_ids.insert(b.id, id);
        metadata::record(&mut doc, id, b, &["uid", "name", "extras"])?;
        push(
            &mut doc,
            "ACBus",
            json!({"id":id,"number":b.id.0,"name":b.name.clone().unwrap_or_else(||b.id.0.to_string()),"available":b.kind!=BusType::Isolated,"area":area_ids[&b.area],"load_zone":zone_ids[&b.zone],"bustype":match b.kind {BusType::Pq=>"PQ",BusType::Pv=>"PV",BusType::Ref=>"REF",BusType::Isolated=>"ISOLATED"},"base_voltage":b.base_kv,"angle":b.va.to_radians(),"magnitude":b.vm,"voltage_limits":minmax(b.vmin,b.vmax)}),
        );
    }
    let bus_by_id: BTreeMap<BusId, &Bus> = net.buses().iter().map(|b| (b.id, b)).collect();
    for b in net.branches() {
        let id = ids.get(b.uid.as_deref());
        metadata::record(&mut doc, id, b, &["uid", "name", "extras", "rating_sets"])?;
        if b.is_transformer() {
            metadata::ext_mut(&mut doc, id)["angmin"] = json!(b.angmin);
            metadata::ext_mut(&mut doc, id)["angmax"] = json!(b.angmax);
        }
        metadata::record_branch_solution(&mut doc, id, b, &mut warnings);
        let arc = ids.related("arc", id);
        push(
            &mut doc,
            "Arc",
            json!({"id":arc,"from_id":bus_ids[&b.from],"to_id":bus_ids[&b.to]}),
        );
        let ch = b
            .charging
            .unwrap_or_else(|| BranchCharging::from_total_b(b.b));
        let mut row = json!({"id":id,"name":b.name.clone().unwrap_or_else(||format!("branch_{id}")),"available":b.in_service,"arc":arc,"r":b.r,"x":b.x,"base_power":net.base_mva(),"power_units":"NATURAL_UNITS","parameter_units":"COMPONENT_BASE",});
        put_ratings(&mut row, [b.rate_a, b.rate_b, b.rate_c], net.base_mva());
        if let Some(solution) = &b.solution {
            row["active_power_flow"] = json!(solution.pf);
            row["reactive_power_flow"] = json!(solution.qf);
        } else if !b.is_transformer() {
            row["active_power_flow"] = json!(0.0);
            row["reactive_power_flow"] = json!(0.0);
            metadata::ext_mut(&mut doc, id)["branch_flow_absent"] = json!(true);
        }
        if b.is_transformer() {
            row.as_object_mut().expect("object").remove("name");
            row["tap"] = json!(b.calc_effective_tap());
            row["alpha"] = json!(b.shift.to_radians());
            let from_kv = bus_by_id[&b.from].base_kv;
            let to_kv = bus_by_id[&b.to].base_kv;
            if from_kv > 0.0 && to_kv > 0.0 {
                row["base_voltage_primary"] = json!(from_kv);
                row["base_voltage_secondary"] = json!(to_kv);
            }
            if let Some(c) = &b.control {
                control::write(&mut row, c, selected)?;
            }
            push(&mut doc, "TransformerCircuit", row);
            let tr = ids.related("two_winding", id);
            push(
                &mut doc,
                "TwoWindingTransformer",
                json!({"id":tr,"name":b.name.clone().unwrap_or_else(||format!("transformer_{tr}")),"circuit":id,"admittance_units":"COMPONENT_BASE","magnetizing_shunt":{"real":ch.g_fr/b.calc_effective_tap().powi(2),"imag":ch.b_fr/b.calc_effective_tap().powi(2)},"shunt_location":"PRIMARY"}),
            );
            if ch.g_to != 0.0 || ch.b_to != 0.0 {
                let sh = ids.related("secondary_shunt", id);
                push(
                    &mut doc,
                    "FixedAdmittance",
                    json!({"id":sh,"name":format!("transformer_{tr}_secondary_shunt"),"available":b.in_service,"bus":bus_ids[&b.to],"Y":{"real":ch.g_to*net.base_mva(),"imag":ch.b_to*net.base_mva()},"base_power":net.base_mva(),"admittance_units":"COMPONENT_MVAR"}),
                );
                // The reader folds this admittance back into the branch.
                metadata::ext_mut(&mut doc, sh)["transformer_circuit"] = json!(id);
            }
            if b.angmin != -360.0 || b.angmax != 360.0 {
                warnings.push(&codes::EMIT_SIENNA.field_dropped,format!("transformer {id}: terminal angle difference limits have no TransformerCircuit field"));
            }
        } else {
            row["b"] = json!({"from":ch.b_fr,"to":ch.b_to});
            row["g"] = json!({"from":ch.g_fr,"to":ch.g_to});
            row["angle_limits"] = minmax(b.angmin.to_radians(), b.angmax.to_radians());
            push(&mut doc, "Line", row);
        }
    }
    for l in net.loads() {
        let id = ids.get(l.uid.as_deref());
        metadata::record(&mut doc, id, l, &["uid", "extras"])?;
        let mut row = json!({"id":id,"name":format!("load_{id}"),"available":l.in_service,"bus":bus_ids[&l.bus],"base_power":net.base_mva(),"power_units":"NATURAL_UNITS"});
        if let Some(LoadVoltageModel::Zip {
            p_constant_power,
            q_constant_power,
            p_constant_current,
            q_constant_current,
            p_constant_impedance,
            q_constant_impedance,
            ..
        }) = l.voltage_model
        {
            for (k, v) in [
                ("constant_active_power", p_constant_power),
                ("constant_reactive_power", q_constant_power),
                ("current_active_power", p_constant_current),
                ("current_reactive_power", q_constant_current),
                ("impedance_active_power", p_constant_impedance),
                ("impedance_reactive_power", q_constant_impedance),
            ] {
                row[k] = json!(v);
                row[format!("max_{k}")] = json!(v);
            }
            push(&mut doc, "StandardLoad", row);
        } else {
            row["active_power"] = json!(l.p);
            row["reactive_power"] = json!(l.q);
            row["max_active_power"] = json!(l.p);
            row["max_reactive_power"] = json!(l.q);
            push(&mut doc, "PowerLoad", row);
            if l.voltage_model.is_some() {
                warnings.push(
                    &codes::EMIT_SIENNA.field_dropped,
                    format!("load {id}: voltage model has no PowerLoad mapping"),
                );
            }
        }
    }
    for s in net.shunts() {
        let id = ids.get(s.uid.as_deref());
        if s.control.is_some() {
            let row = shunts::write(&mut doc, id, bus_ids[&s.bus], s, selected, &mut warnings)?;
            push(&mut doc, "SwitchedAdmittance", row);
            continue;
        }
        metadata::record(
            &mut doc,
            id,
            s,
            &["uid", "extras", "section_count", "control"],
        )?;
        push(
            &mut doc,
            "FixedAdmittance",
            json!({"id":id,"name":format!("shunt_{id}"),"available":s.in_service,"bus":bus_ids[&s.bus],"Y":{"real":s.g,"imag":s.b},"base_power":net.base_mva(),"admittance_units":"COMPONENT_MVAR"}),
        );
    }
    let original_generators = generators::by_id(original);
    for g in net.generators() {
        let id = ids.get(g.uid.as_deref());
        metadata::record(&mut doc, id, g, &["uid", "vg", "caps"])?;
        if (g.vg - bus_by_id[&g.bus].vm).abs() > 1e-12 || g.caps.iter().flatten().any(|x| *x != 0.0)
        {
            warnings.push(&codes::EMIT_SIENNA.field_dropped, format!("generator {id}: voltage setpoint and capability columns without native fields are retained in ext; a Sienna consumer must explicitly handle those extension keys"));
        }
        let cost = cost_json(g.cost.as_ref(), &mut warnings);
        let mut row = json!({"id":id,"name":format!("generator_{id}"),"available":g.in_service,"status":if g.in_service {"ONLINE"}else{"OFFLINE"},"bus":bus_ids[&g.bus],"active_power":g.pg,"reactive_power":g.qg,"rating":g.mbase.max(g.pmax.abs()).max(g.pg.abs()),"active_power_limits":minmax(g.pmin,g.pmax),"reactive_power_limits":minmax(g.qmin,g.qmax),"base_power":if g.mbase>0.0 {g.mbase}else{net.base_mva()},"power_units":"NATURAL_UNITS","operation_cost":cost});
        if g.qmin.is_nan()
            || g.qmax.is_nan()
            || g.qmin == f64::INFINITY
            || g.qmax == f64::NEG_INFINITY
            || g.qmin > g.qmax
        {
            return Err(error("invalid generator reactive bounds"));
        }
        if !g.qmin.is_finite() || !g.qmax.is_finite() {
            row.as_object_mut()
                .expect("constructed object")
                .remove("reactive_power_limits");
            if g.qmin.is_finite() || g.qmax.is_finite() {
                warnings.push(&codes::EMIT_SIENNA.field_dropped, format!("generator {id}: one-sided reactive bounds have no finite MinMax representation"));
            }
        }
        let (kind, retained_cost) = generators::prepare(
            &mut row,
            g,
            original_generators.get(&id).copied(),
            id,
            &mut warnings,
        );
        if g.cost.is_none() && !retained_cost {
            metadata::ext_mut(&mut doc, id)["operation_cost_absent"] = json!(true);
            warnings.push(&codes::EMIT_SIENNA.value_defaulted, format!("generator {id}: required operation_cost defaults to zero because the source has no cost"));
        }
        push(&mut doc, kind, row);
    }
    for s in net.storage() {
        let id = ids.get(s.uid.as_deref());
        metadata::record(&mut doc, id, s, &["uid"])?;
        push(
            &mut doc,
            "EnergyReservoirStorage",
            json!({"id":id,"name":format!("storage_{id}"),"available":s.in_service,"bus":bus_ids[&s.bus],"prime_mover_type":"BA","storage_technology_type":"OTHER_CHEM","storage_capacity":s.energy_rating,"conversion_factor":1.0,"energy_units":"MWH","storage_level_limits":minmax(0.0,1.0),"initial_storage_capacity_level":if s.energy_rating>0.0 {s.energy/s.energy_rating}else{0.0},"rating":s.thermal_rating,"active_power":-s.ps,"reactive_power":-s.qs,"input_active_power_limits":minmax(0.0,s.charge_rating),"output_active_power_limits":minmax(0.0,s.discharge_rating),"efficiency":{"in":s.charge_efficiency,"out":s.discharge_efficiency},"reactive_power_limits":minmax(-s.qmax,-s.qmin),"base_power":net.base_mva(),"power_units":"NATURAL_UNITS","operation_cost":{"cost_type":"STORAGE","fixed":0.0,"start_up":0.0,"shut_down":0.0}}),
        );
    }
    hvdc::write_hvdc(net, &mut doc, &mut ids, &bus_ids, selected, &mut warnings)?;
    three_winding::write(net, &mut doc, &mut ids, &bus_ids, selected)?;
    if net
        .branches()
        .iter()
        .any(|b| !b.is_transformer() && b.solution.is_none())
    {
        warnings.push(&codes::EMIT_SIENNA.value_defaulted,"required Line sending-terminal flow fields use zero when no solved flow is supplied; ext marks those values as unavailable");
    }
    for b in net.branches() {
        if !b.rating_sets.is_empty() {
            warnings.push(&codes::EMIT_SIENNA.field_dropped, "named branch rating sets are retained in ext without native Sienna constraint semantics");
        }
    }
    for transformer in net.transformers_3w() {
        if (0..3).any(|i| !transformer.winding_rating_sets(i).is_empty()) {
            warnings.push(&codes::EMIT_SIENNA.field_dropped,
                "named three-winding rating sets are retained in ext without native Sienna constraint semantics");
        }
    }
    let extras = net
        .transformers_3w()
        .iter()
        .filter(|t| !t.extras.is_empty())
        .count()
        + net.buses().iter().filter(|b| !b.extras.is_empty()).count()
        + net
            .branches()
            .iter()
            .filter(|b| !b.extras.is_empty())
            .count()
        + net.loads().iter().filter(|l| !l.extras.is_empty()).count();
    if extras > 0 {
        warnings.push(&codes::EMIT_SIENNA.extras_dropped, format!("{extras} component extras are retained in ext and require explicit consumer handling"));
    }
    for (kind, n) in [
        ("switches", net.switches().len()),
        (
            "static var compensators",
            net.static_var_compensators().len(),
        ),
    ] {
        if n > 0 {
            warnings.push(
                &codes::EMIT_SIENNA.record_dropped,
                format!("{n} {kind} have no implemented Sienna electrical mapping"),
            );
        }
    }
    for (kind, list) in doc["components"].as_object().expect("constructed object") {
        for (index, row) in list
            .as_array()
            .expect("constructed array")
            .iter()
            .enumerate()
        {
            if has_null(row)
                && let Some(path) = null_path(row, &format!("components.{kind}[{index}]"))
            {
                return Err(error(format!("nonfinite numeric output at {path}")));
            }
        }
    }
    if let Some(source) = original {
        if version(source)? == selected {
            merge::preserve_unmapped(&mut doc, source, &mut warnings)?;
        } else {
            warnings.push(&codes::EMIT_SIENNA.extras_dropped,"cross-version write retains the typed electrical value; source-only fields require a version-specific migration");
        }
    }
    Ok((doc, warnings))
}

/// The path to the first null in `v`, built only once one is found: the walk
/// covers every emitted row, and almost none holds a null.
fn null_path(v: &Value, path: &str) -> Option<String> {
    match v {
        Value::Null => Some(path.into()),
        Value::Object(o) => o
            .iter()
            .find_map(|(k, v)| has_null(v).then(|| null_path(v, &format!("{path}.{k}")))?),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .find_map(|(i, v)| has_null(v).then(|| null_path(v, &format!("{path}[{i}]")))?),
        _ => None,
    }
}
fn has_null(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(o) => o.values().any(has_null),
        Value::Array(a) => a.iter().any(has_null),
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    fn document() -> Value {
        json!({"schema_version":"0.4.0","name":"units","frequency":50.0,"components":{"ACBus":[{"id":91,"number":7,"name":"source","available":true,"bustype":"REF","angle":0.2,"magnitude":1.02,"base_voltage":100.0,"voltage_limits":{"min":0.9,"max":1.1}},{"id":92,"number":43,"name":"sink","available":true,"bustype":"PQ","angle":-0.1,"magnitude":0.99,"base_voltage":100.0,"voltage_limits":{"min":0.9,"max":1.1}}],"Arc":[{"id":10,"from_id":91,"to_id":92}],"Line":[{"id":11,"name":"link","available":true,"arc":10,"r":0.01,"x":0.1,"base_power":50.0,"power_units":"COMPONENT_BASE","parameter_units":"COMPONENT_BASE","b":{"from":0.02,"to":0.03},"g":{"from":0.001,"to":0.002},"rating":2.0,"active_power_flow":0.0,"reactive_power_flow":0.0,"angle_limits":{"min":-0.4,"max":0.4}}]},"supplemental_attributes":[],"supplemental_attribute_associations":[],"plant_associations":[],"combined_cycle_associations":[],"service_associations":[],"time_series_associations":[],"time_series_storage_file":null})
    }
    fn decode(v: &Value) -> BalancedNetwork {
        read(v, None, 0, &mut Diagnostics::new()).unwrap()
    }
    #[test]
    fn component_bases_and_natural_units_agree() {
        let mut doc = document();
        let a = decode(&doc);
        let b = &a.branches()[0];
        assert_eq!((b.from, b.to), (BusId(7), BusId(43)));
        assert!((b.r - 0.02).abs() < 1e-14);
        assert!((b.x - 0.2).abs() < 1e-14);
        assert_eq!(b.rate_a, (100.0));
        let ch = b.charging.unwrap();
        assert!((ch.b_fr - 0.01).abs() < 1e-14);
        assert!((ch.b_to - 0.015).abs() < 1e-14);
        assert!((a.buses()[0].va - 0.2_f64.to_degrees()).abs() < 1e-12);
        let line = &mut doc["components"]["Line"][0];
        line["parameter_units"] = json!("NATURAL_UNITS");
        line["r"] = json!(2.0);
        line["x"] = json!(20.0);
        line["b"] = json!({"from":0.0001,"to":0.00015});
        line["g"] = json!({"from":0.000_005,"to":0.00001});
        let natural = decode(&doc);
        let n = &natural.branches()[0];
        assert!((n.r - b.r).abs() < 1e-14);
        assert!((n.x - b.x).abs() < 1e-14);
        assert!((n.charging.unwrap().b_fr - ch.b_fr).abs() < 1e-14);
    }
    #[test]
    fn transformer_voltage_bases_and_terminal_shunts() {
        let mut doc = document();
        doc["components"]["ACBus"][1]["base_voltage"] = json!(10.0);
        doc["components"].as_object_mut().unwrap().remove("Line");
        doc["components"]["TransformerCircuit"] = json!([{"id":11,"available":true,"arc":10,"r":0.01,"x":0.1,"base_power":50.0,"power_units":"NATURAL_UNITS","parameter_units":"COMPONENT_BASE","base_voltage_primary":120.0,"base_voltage_secondary":12.0,"rating":80.0,"tap":1.1}]);
        doc["components"]["TwoWindingTransformer"] = json!([{"id":12,"name":"transformer","circuit":11,"admittance_units":"COMPONENT_BASE","magnetizing_shunt":{"real":0.01,"imag":-0.02},"shunt_location":"SPLIT"}]);
        let net = decode(&doc);
        let b = &net.branches()[0];
        assert!((b.r - 0.0288).abs() < 1e-12);
        assert!((b.tap - 1.1).abs() < 1e-12);
        let y = 0.01 * 0.5 / 1.44;
        let ch = b.charging.unwrap();
        assert!((ch.g_fr / b.tap.powi(2) - y).abs() < 1e-12);
        assert!((ch.g_to - y).abs() < 1e-12);
    }

    #[test]
    fn transformer_secondary_charging_round_trips_without_a_bus_shunt() {
        let mut doc = document();
        doc["components"].as_object_mut().unwrap().remove("Line");
        doc["components"]["TransformerCircuit"] = json!([{"id":11,"available":true,"arc":10,"r":0.01,"x":0.1,"base_power":100.0,"power_units":"NATURAL_UNITS","parameter_units":"COMPONENT_BASE","rating":80.0,"tap":1.0}]);
        doc["components"]["TwoWindingTransformer"] = json!([{"id":12,"name":"transformer","circuit":11,"admittance_units":"COMPONENT_BASE","magnetizing_shunt":{"real":0.0,"imag":0.03},"shunt_location":"PRIMARY"}]);
        let mut net = decode(&doc);
        assert_eq!(net.shunts().as_slice(), []);
        net.branches_mut()[0].charging = Some(BranchCharging::new(0.0, 0.03, 0.001, 0.05));
        net.branches_mut()[0].b = 0.08;
        let output = write(&net, 4, None).unwrap();
        let written: Value = serde_json::from_str(&output.text).unwrap();
        assert_eq!(
            written["components"]["FixedAdmittance"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let recovered = decode(&written);
        assert_eq!(recovered.shunts().as_slice(), []);
        let ch = recovered.branches()[0].charging.unwrap();
        assert!((ch.b_fr - 0.03).abs() < 1e-12);
        assert!((ch.g_to - 0.001).abs() < 1e-12);
        assert!((ch.b_to - 0.05).abs() < 1e-12);
        assert!((recovered.branches()[0].b - 0.08).abs() < 1e-12);
        let again = write(&recovered, 4, None).unwrap();
        assert_eq!(again.text, output.text);
    }

    #[test]
    fn an_unstated_phase_band_keeps_the_default_in_degrees() {
        let mut doc = document();
        doc["components"].as_object_mut().unwrap().remove("Line");
        doc["components"]["TransformerCircuit"] = json!([{"id":11,"available":true,"arc":10,"r":0.01,"x":0.1,"base_power":100.0,"power_units":"NATURAL_UNITS","parameter_units":"COMPONENT_BASE","rating":80.0,"tap":1.0,"control_objective":"ACTIVE_POWER_FLOW"}]);
        let control = decode(&doc).branches()[0].control.clone().unwrap();
        assert_eq!((control.tap_min, control.tap_max), (0.9, 1.1));
        doc["components"]["TransformerCircuit"][0]["phase_angle_limits"] =
            json!({"min":-0.5,"max":0.5});
        let stated = decode(&doc).branches()[0].control.clone().unwrap();
        assert!((stated.tap_min - (-0.5_f64).to_degrees()).abs() < 1e-12);
        assert!((stated.tap_max - 0.5_f64.to_degrees()).abs() < 1e-12);
    }

    #[test]
    fn an_edited_unlimited_rating_does_not_restore_the_retained_value() {
        // The writer omits an unlimited optional rating; merging the source
        // row brought its finite rating back.
        let mut doc = document();
        doc["components"]["Line"][0]["rating_b"] = json!(2.4);
        let mut net = decode(&doc);
        assert_eq!(net.branches()[0].rate_b, 120.0);
        net.branches_mut()[0].rate_b = f64::INFINITY;
        let written: Value =
            serde_json::from_str(&write(&net, 4, Some(&doc)).unwrap().text).unwrap();
        assert!(written["components"]["Line"][0].get("rating_b").is_none());
        assert_eq!(decode(&written).branches()[0].rate_b, f64::INFINITY);

        let mut doc = document();
        doc["components"].as_object_mut().unwrap().remove("Line");
        doc["components"]["TransformerCircuit"] = json!([{"id":11,"available":true,"arc":10,"r":0.01,"x":0.1,"base_power":100.0,"power_units":"NATURAL_UNITS","parameter_units":"COMPONENT_BASE","rating":77.0,"tap":1.0}]);
        let mut net = decode(&doc);
        assert_eq!(net.branches()[0].rate_a, 77.0);
        net.branches_mut()[0].rate_a = f64::INFINITY;
        let written: Value =
            serde_json::from_str(&write(&net, 4, Some(&doc)).unwrap().text).unwrap();
        // The schema requires a transformer rating, so an unlimited one is the
        // unbounded marker, as a line's is.
        let row = &written["components"]["TransformerCircuit"][0];
        assert_eq!(
            row["rating"],
            INFINITE_BOUND * number(row, "base_power").unwrap()
        );
        assert_eq!(decode(&written).branches()[0].rate_a, f64::INFINITY);
        let native: Value =
            serde_json::from_str(&powersystems::write(&net, None).unwrap().text).unwrap();
        let transformer = native["data"]["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["__metadata__"]["type"] == "TapTransformer")
            .unwrap();
        assert_eq!(transformer["rating"], INFINITE_BOUND);
        let back = powersystems::read(&native, None, &mut Diagnostics::new()).unwrap();
        assert_eq!(back.branches()[0].rate_a, f64::INFINITY);
    }

    #[test]
    fn zero_is_a_bound_and_an_unlimited_line_is_the_unbounded_marker() {
        let mut net = decode(&document());
        net.branches_mut()[0].rate_a = 0.0;
        net.branches_mut()[0].rate_b = f64::INFINITY;
        net.branches_mut()[0].rate_c = 0.0;
        for version in 1..=4 {
            let output = write(&net, version, None).unwrap();
            let written: Value = serde_json::from_str(&output.text).unwrap();
            let row = &written["components"]["Line"][0];
            assert_eq!(row["rating"], 0.0);
            assert_eq!(row["rating_c"], 0.0);
            assert!(row.get("rating_b").is_none());
            let back = decode(&written);
            assert_eq!(back.branches()[0].rate_a, (0.0));
            assert_eq!(back.branches()[0].rate_b, f64::INFINITY);
            assert!(back.branches()[0].in_service);
        }
        // An unlimited required rating is PowerSystems' unbounded marker, on
        // the row's base, and reads back as unlimited.
        net.branches_mut()[0].rate_a = f64::INFINITY;
        for version in 1..=4 {
            let written: Value =
                serde_json::from_str(&write(&net, version, None).unwrap().text).unwrap();
            let row = &written["components"]["Line"][0];
            assert_eq!(
                row["rating"],
                INFINITE_BOUND * number(row, "base_power").unwrap()
            );
            assert_eq!(decode(&written).branches()[0].rate_a, f64::INFINITY);
        }
        let native: Value =
            serde_json::from_str(&powersystems::write(&net, None).unwrap().text).unwrap();
        let line = native["data"]["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["__metadata__"]["type"] == "Line")
            .unwrap();
        assert_eq!(line["rating"], INFINITE_BOUND);
        let back = powersystems::read(&native, None, &mut Diagnostics::new()).unwrap();
        assert_eq!(back.branches()[0].rate_a, f64::INFINITY);
        net.branches_mut()[0].rate_a = 100.0;
        net.branches_mut()[0].r = f64::NAN;
        assert!(write_document(&net, 4, None).is_err());
    }

    #[test]
    fn a_retained_cost_curve_is_not_marked_absent() {
        let mut doc = document();
        doc["components"]["ThermalStandard"] = json!([{
            "id":12,"name":"unit","bus":91,"available":true,"status":"ONLINE",
            "base_power":50.0,"power_units":"COMPONENT_BASE","active_power":0.5,"reactive_power":0.0,
            "active_power_limits":{"min":0.0,"max":1.0},
            "operation_cost":{"cost_type":"THERMAL","fixed":3.0,"start_up":2.0,"shut_down":1.0,
            "variable_operation_cost":{"variable_cost_type":"COST","power_units":"NATURAL_UNITS","value_curve":{"curve_type":"INPUT_OUTPUT","function_data":{"function_type":"PIECEWISE_STEP","x_coords":[0.0,50.0],"y_coords":[10.0]}}}}
        }]);
        let net = decode(&doc);
        assert!(net.generators()[0].cost.is_none());
        let output = write(&net, 4, Some(&doc)).unwrap();
        let written: Value = serde_json::from_str(&output.text).unwrap();
        assert_eq!(
            written["components"]["ThermalStandard"][0]["operation_cost"],
            doc["components"]["ThermalStandard"][0]["operation_cost"]
        );
        assert!(written["ext"]["12"]["org.eigenergy.powerio"]["operation_cost_absent"].is_null());
        assert!(
            !output
                .diagnostics
                .iter()
                .any(|d| d.message().contains("operation_cost defaults to zero"))
        );
    }

    #[test]
    fn nan_values_are_rejected_but_unbounded_q_is_omitted() {
        let mut net = decode(&document());
        let mut g = Generator::new(BusId(7));
        g.mbase = 100.0;
        g.qmin = f64::NEG_INFINITY;
        g.qmax = f64::INFINITY;
        net.generators_mut().push(g);
        let output = write(&net, 4, None).unwrap();
        let recovered = decode(&serde_json::from_str(&output.text).unwrap());
        assert_eq!(recovered.generators()[0].qmin, f64::NEG_INFINITY);
        assert_eq!(recovered.generators()[0].qmax, f64::INFINITY);
        net.branches_mut()[0].r = f64::NAN;
        assert!(write(&net, 4, None).is_err());
    }

    #[test]
    fn every_release_writes_fresh_and_reads_with_matching_version() {
        let net = decode(&document());
        for version in 1..=4 {
            let out = write(&net, version, None).unwrap();
            let mut warnings = Diagnostics::new();
            let reread = read(
                &serde_json::from_str(&out.text).unwrap(),
                None,
                version,
                &mut warnings,
            )
            .unwrap();
            assert_eq!(reread.buses().len(), 2);
            assert_eq!(reread.branches().len(), 1);
            assert!((reread.branches()[0].x - net.branches()[0].x).abs() < 1e-12);
            let j: Value = serde_json::from_str(&out.text).unwrap();
            assert_eq!(j.get("schema_version").is_none(), version == 1);
        }
    }
    #[test]
    fn rejects_duplicate_ids_dangling_references_and_unsupported_versions() {
        let mut doc = document();
        doc["schema_version"] = json!("0.5.0");
        assert!(read(&doc, None, 0, &mut Diagnostics::new()).is_err());
        doc = document();
        doc["components"]["ACBus"][1]["id"] = json!(91);
        assert!(read(&doc, None, 0, &mut Diagnostics::new()).is_err());
        doc = document();
        doc["components"]["Arc"][0]["to_id"] = json!(43);
        assert!(read(&doc, None, 0, &mut Diagnostics::new()).is_err());
        assert!(read(&document(), None, 2, &mut Diagnostics::new()).is_err());
    }
    #[test]
    fn edited_fields_override_retained_fields() {
        let doc = document();
        let mut net = decode(&doc);
        std::sync::Arc::make_mut(&mut net.tables_mut().branches)[0].x = 0.3;
        let out = write(&net, 0, Some(&doc)).unwrap();
        let reread = read(
            &serde_json::from_str(&out.text).unwrap(),
            None,
            0,
            &mut Diagnostics::new(),
        )
        .unwrap();
        assert!((reread.branches()[0].x - 0.3).abs() < 1e-12);
    }
    #[test]
    fn edited_document_rebases_retained_fields_and_prunes_deleted_references() {
        let mut doc = document();
        doc["components"]["Line"][0]["operational_flow_limit"] = json!({"min":-0.5,"max":1.5});
        doc["components"]["ThermalStandard"] = json!([{
            "id":12,"name":"unit","bus":91,"available":true,"status":"ONLINE",
            "base_power":50.0,"power_units":"COMPONENT_BASE","active_power":0.5,"reactive_power":0.0,
            "active_power_limits":{"min":0.0,"max":1.0},"ramp_limits":{"up":0.2,"down":0.1},
            "operation_cost":{"cost_type":"THERMAL","fixed":3.0,"start_up":{"hot":2.0,"warm":3.0,"cold":4.0},"shut_down":1.0,
            "variable_operation_cost":{"variable_cost_type":"COST","power_units":"NATURAL_UNITS","value_curve":{"curve_type":"INPUT_OUTPUT","function_data":{"function_type":"LINEAR","proportional_term":4.0,"constant_term":1.0}}}}
        }]);
        doc["service_associations"] = json!([{"component_id":12,"service_id":13}]);
        doc["components"]["OnlineReserve"] = json!([{"id":13,"name":"reserve"}]);
        let mut net = decode(&doc);
        net.generators_mut()[0].pg = 30.0;
        let output = write(&net, 4, Some(&doc)).unwrap();
        let written: Value = serde_json::from_str(&output.text).unwrap();
        assert_eq!(
            written["components"]["ThermalStandard"][0]["operation_cost"],
            doc["components"]["ThermalStandard"][0]["operation_cost"]
        );
        assert_eq!(
            written["components"]["ThermalStandard"][0]["ramp_limits"]["up"],
            10.0
        );
        assert_eq!(
            written["components"]["Line"][0]["operational_flow_limit"]["max"],
            75.0
        );
        net.generators_mut().clear();
        let output = write(&net, 4, Some(&doc)).unwrap();
        let written: Value = serde_json::from_str(&output.text).unwrap();
        assert_eq!(written["service_associations"], json!([]));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.code().ends_with("RECORD_DROPPED"))
        );
    }
    #[test]
    fn fuel_price_and_maintenance_cost_use_the_declared_power_base() {
        let cost = json!({"fixed":2.0,"start_up":7.0,"shut_down":8.0,"variable_operation_cost":{
            "variable_cost_type":"FUEL","fuel_cost":3.0,"power_units":"COMPONENT_BASE",
            "value_curve":{"curve_type":"INPUT_OUTPUT","function_data":{"function_type":"QUADRATIC","quadratic_term":1100.0,"proportional_term":500.0,"constant_term":150.0}},
            "vom_cost":{"curve_type":"INPUT_OUTPUT","function_data":{"function_type":"LINEAR","proportional_term":300.0,"constant_term":4.0}}}});
        let mut warnings = Diagnostics::new();
        let cost = read_cost(&cost, 100.0, &mut warnings).unwrap().unwrap();
        for (a, e) in cost.coeffs.iter().zip([0.33, 18.0, 456.0]) {
            assert!((a - e).abs() < 1e-12);
        }
        assert!(warnings.is_empty());
        assert!((cost.startup - 7.0).abs() < 1e-12);
    }
}

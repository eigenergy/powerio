//! Normalize per-coil capacitor arrays into the existing terminal shunt model.
//! No source-only metadata carries electrical coefficients into downstream solvers.
use crate::{
    Configuration,
    collect::Diagnostics,
    diagnostics::codes,
    error::{Error, Result},
};
use serde_json::{Map, Value, json};

fn invalid(name: &str) -> Error {
    Error::Json {
        format: "BMOPF",
        message: format!(
            "capacitor {name}: expected finite nonnegative per-coil ratings, positive coil voltage, and matching WYE/DELTA/SINGLE_PHASE terminals"
        ),
    }
}

/// A WYE coil returns through the bank's one mapped terminal that is not a
/// phase of its bus (the one WYE neutral rule over the bus's
/// `terminal_names` and the document's `terminal_conventions`, the map
/// itself standing in for an undeclared bus), or, when every mapped terminal
/// is a phase, through the last terminal.
fn wye_neutral(
    doc: &Map<String, Value>,
    capacitor: &Value,
    map: &[String],
) -> std::result::Result<Option<usize>, String> {
    let bus_terminals: Vec<String> = capacitor
        .get("bus")
        .and_then(Value::as_str)
        .and_then(|bus| doc.get("bus")?.get(bus)?.get("terminal_names"))
        .and_then(Value::as_array)
        .map_or_else(
            || map.to_vec(),
            |names| {
                names
                    .iter()
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect()
            },
        );
    crate::connection::calc_mapped_neutral_position(
        &bus_terminals,
        map,
        doc.get("terminal_conventions"),
    )
}

pub(super) fn lower(doc: &mut Map<String, Value>, diagnostics: &mut Diagnostics) -> Result<()> {
    let Some(Value::Object(mut capacitors)) = doc.remove("capacitor") else {
        return Ok(());
    };
    for name in capacitors.keys().cloned().collect::<Vec<_>>() {
        let capacitor = &capacitors[&name];
        if !capacitor.get("q_rated").is_some_and(Value::is_array) {
            continue;
        }
        let q: Vec<f64> =
            serde_json::from_value(capacitor["q_rated"].clone()).map_err(|_| invalid(&name))?;
        let terminals = capacitor["terminal_map"]
            .as_array()
            .ok_or_else(|| invalid(&name))?;
        if terminals.iter().any(|t| !t.is_string()) {
            return Err(invalid(&name));
        }
        let n = terminals.len();
        if n > 64
            || terminals
                .iter()
                .enumerate()
                .any(|(i, t)| terminals[..i].contains(t))
        {
            return Err(invalid(&name));
        }
        let configuration = match capacitor["configuration"].as_str() {
            Some("WYE") => Configuration::Wye,
            Some("DELTA") => Configuration::Delta,
            Some("SINGLE_PHASE") => Configuration::SinglePhase,
            _ => return Err(invalid(&name)),
        };
        let map: Vec<String> = terminals
            .iter()
            .filter_map(|t| t.as_str().map(str::to_owned))
            .collect();
        let neutral = if configuration == Configuration::Wye {
            wye_neutral(doc, capacitor, &map).map_err(|_| invalid(&name))?
        } else {
            None
        };
        let coils = crate::connection::calc_connection_channels(configuration, n, q.len(), neutral)
            .map_err(|_| invalid(&name))?;
        let voltage = capacitor["v_nom"].as_f64().ok_or_else(|| invalid(&name))?;
        if !voltage.is_finite() || voltage <= 0.0 || q.iter().any(|q| !q.is_finite() || *q < 0.0) {
            return Err(invalid(&name));
        }
        let susceptance: Vec<f64> = q
            .iter()
            .map(|&q| crate::electrical::calc_rated_admittance(q, voltage))
            .collect();
        if susceptance.iter().any(|value| !value.is_finite()) {
            return Err(invalid(&name));
        }
        let b = crate::connection::calc_coil_susceptance_matrix(n, &coils, &susceptance);
        if b.iter().flatten().any(|value| !value.is_finite()) {
            return Err(invalid(&name));
        }
        let mut shunt = capacitor.as_object().ok_or_else(|| invalid(&name))?.clone();
        for key in ["q_rated", "v_nom", "configuration"] {
            shunt.remove(key);
        }
        for (i, row) in b.iter().enumerate() {
            for (j, value) in row.iter().enumerate() {
                shunt.insert(format!("G_{}_{}", i + 1, j + 1), json!(0.0));
                shunt.insert(format!("B_{}_{}", i + 1, j + 1), json!(value));
            }
        }
        let table = doc
            .entry("shunt")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| invalid(&name))?;
        let prefix = format!("__bmopf_capacitor_{name}");
        let mut identity = prefix.clone();
        let mut suffix = 1;
        while table.keys().any(|key| key.eq_ignore_ascii_case(&identity)) {
            identity = format!("{prefix}_{suffix}");
            suffix += 1;
        }
        table.insert(identity.clone(), Value::Object(shunt));
        capacitors.remove(&name);
        diagnostics.push(&codes::READ_BMOPF_CAPACITOR_LOWERED, format!("capacitor {name}: per-coil ratings represented exactly by shunt {identity} in terminal order"));
    }
    doc.insert("capacitor".into(), Value::Object(capacitors));
    Ok(())
}

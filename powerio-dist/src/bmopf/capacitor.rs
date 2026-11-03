//! Normalize per-coil capacitor arrays into the existing terminal shunt model.
//! No source-only metadata carries electrical coefficients into downstream solvers.
use crate::{
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
        let pairs: Vec<(usize, Option<usize>)> = match (capacitor["configuration"].as_str(), n) {
            (Some("WYE"), 1) => vec![(0, None)],
            (Some("WYE"), 2..) => (0..n - 1).map(|k| (k, Some(n - 1))).collect(),
            (Some("SINGLE_PHASE" | "DELTA"), 2) => vec![(0, Some(1))],
            (Some("DELTA"), 3) => (0..3).map(|k| (k, Some((k + 1) % 3))).collect(),
            _ => return Err(invalid(&name)),
        };
        let voltage = capacitor["v_nom"].as_f64().ok_or_else(|| invalid(&name))?;
        if !voltage.is_finite()
            || voltage <= 0.0
            || q.len() != pairs.len()
            || q.iter().any(|q| !q.is_finite() || *q < 0.0)
        {
            return Err(invalid(&name));
        }
        let mut b = vec![vec![0.0; n]; n];
        for (q, (p, m)) in q.iter().zip(pairs) {
            let value = q / voltage.powi(2);
            if !value.is_finite() {
                return Err(invalid(&name));
            }
            b[p][p] += value;
            if let Some(m) = m {
                b[m][m] += value;
                b[p][m] -= value;
                b[m][p] -= value;
            }
        }
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

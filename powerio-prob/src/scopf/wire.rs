//! Versioned Julia compatibility wire format.

use serde::Serialize;
use serde_json::{Map, Value};

use super::{ScopfInstance, ScopfResult};

pub const SCOPF_WIRE_SCHEMA: &str = "powerio.scopf.julia";
pub const SCOPF_WIRE_VERSION: &str = "1.0.0";

#[derive(Serialize)]
struct WireEnvelope {
    schema: &'static str,
    schema_version: &'static str,
    index_base: usize,
    instance: Value,
}

/// Convert an internal instance to the versioned 1-based Julia wire format.
pub fn to_wire_value(instance: &ScopfInstance) -> ScopfResult<Value> {
    let mut value = serde_json::to_value(instance)?;
    rename_fields(&mut value);
    increment_indices(&mut value);
    Ok(serde_json::to_value(WireEnvelope {
        schema: SCOPF_WIRE_SCHEMA,
        schema_version: SCOPF_WIRE_VERSION,
        index_base: 1,
        instance: value,
    })?)
}

fn rename_fields(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                rename_fields(value);
            }
        }
        Value::Object(object) => {
            let is_lengths = object.contains_key("l_j_xf") && object.contains_key("l_t");
            for (source, target) in [
                ("static_data", "static"),
                ("sigma_rgu", "σ_rgu"),
                ("sigma_rgd", "σ_rgd"),
                ("sigma_scr", "σ_scr"),
                ("sigma_nsc", "σ_nsc"),
                ("w_en_max_pr", "W_en_max_pr"),
                ("w_en_max_cs", "W_en_max_cs"),
                ("w_en_min_pr", "W_en_min_pr"),
                ("w_en_min_cs", "W_en_min_cs"),
                ("t_w_en_max_pr", "T_w_en_max_pr"),
                ("t_w_en_max_cs", "T_w_en_max_cs"),
                ("t_w_en_min_pr", "T_w_en_min_pr"),
                ("t_w_en_min_cs", "T_w_en_min_cs"),
            ] {
                rename_key(object, source, target);
            }
            if is_lengths {
                for (source, target) in [
                    ("l_j_xf", "L_J_xf"),
                    ("l_j_ln", "L_J_ln"),
                    ("l_j_ac", "L_J_ac"),
                    ("l_j_dc", "L_J_dc"),
                    ("l_j_br", "L_J_br"),
                    ("l_j_cs", "L_J_cs"),
                    ("l_j_pr", "L_J_pr"),
                    ("l_j_cspr", "L_J_cspr"),
                    ("l_j_sh", "L_J_sh"),
                    ("i", "I"),
                    ("l_t", "L_T"),
                    ("l_n_p", "L_N_p"),
                    ("l_n_q", "L_N_q"),
                ] {
                    rename_key(object, source, target);
                }
            }
            for value in object.values_mut() {
                rename_fields(value);
            }
        }
        _ => {}
    }
}

fn rename_key(object: &mut Map<String, Value>, source: &str, target: &str) {
    if let Some(value) = object.remove(source) {
        object.insert(target.to_owned(), value);
    }
}

/// Serialize an internal instance as the versioned 1-based Julia wire format.
pub fn to_wire_json(instance: &ScopfInstance) -> ScopfResult<String> {
    Ok(serde_json::to_string(&to_wire_value(instance)?)?)
}

fn increment_indices(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                increment_indices(value);
            }
        }
        Value::Object(object) => increment_object(object),
        _ => {}
    }
}

fn increment_object(object: &mut Map<String, Value>) {
    for (key, value) in object {
        if is_internal_index(key)
            && let Some(index) = value.as_u64()
        {
            *value = Value::from(index + 1);
            continue;
        }
        increment_indices(value);
    }
}

fn is_internal_index(key: &str) -> bool {
    matches!(
        key,
        "j_ln"
            | "j_xf"
            | "j_dc"
            | "n_p"
            | "n_q"
            | "t"
            | "m"
            | "ctg"
            | "flat_k"
            | "flat_jtk_dc"
            | "w_en_max_pr_ind"
            | "w_en_max_cs_ind"
            | "w_en_min_pr_ind"
            | "w_en_min_cs_ind"
    )
}

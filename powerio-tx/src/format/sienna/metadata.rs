//! Only fields without a Sienna schema counterpart belong in `ext`.
//! Electrical fields with a native counterpart always come from that field.
use super::{Result, Value, error, integer};
use crate::network::{Area, Branch, Bus, Generator, Hvdc, Load, Shunt, Storage, Transformer3W};
use powerio_core::__implementation::nonfinite::{NonFiniteDe, NonFiniteSer};
use serde::{Deserialize, Serialize};

pub(super) const KEY: &str = "org.eigenergy.powerio";

/// A component's PowerIO namespace in a document's `ext` map, or null.
pub(super) fn ext(doc: &Value, id: impl std::fmt::Display) -> &Value {
    &doc["ext"][id.to_string()][KEY]
}

/// A component's PowerIO namespace for writing, created on first use.
pub(super) fn ext_mut(doc: &mut Value, id: impl std::fmt::Display) -> &mut Value {
    &mut doc["ext"][id.to_string()][KEY]
}

// These describe the current typed value or generated equipment. Never restore
// an old value merely because the current writer deliberately omitted it.
pub(super) const WRITTEN_FIELDS: &[&str] = &[
    "uid",
    "name",
    "extras",
    "rating_sets",
    "angmin",
    "angmax",
    "section_count",
    "control",
    "vg",
    "caps",
    "number",
    "slack_bus",
    "tolerance",
    "area_type",
    "cost",
    "vf",
    "vt",
    "qf",
    "qt",
    "pt",
    "operation_cost_absent",
    "branch_flow_absent",
    "to_terminal_power",
    "transformer_circuit",
    "transformer_uuid",
    "implicit",
    "rmpct",
    "regulating_terminal",
    "solved_conductance",
    "switched_control_fields",
    "voltage_band_offsets",
    "control_fields",
    "winding_control_fields",
    "winding_rating_fields",
];

pub(super) fn record<T: Serialize>(
    doc: &mut Value,
    id: i64,
    value: &T,
    fields: &[&str],
) -> Result<()> {
    let serialized = value
        .serialize(NonFiniteSer(serde_json::value::Serializer))
        .map_err(|e| error(e.to_string()))?;
    // Durable fail-closed wrappers are not physical component fields.
    let object = match serialized.get("type").and_then(Value::as_str) {
        Some("powerio.PartialTransformer3W") => &serialized["value"],
        _ => &serialized,
    };
    let mut extra = serde_json::Map::new();
    for key in fields {
        if let Some(v) = object.get(key) {
            // Required Sienna names are synthesized only when the source lacks
            // one. A supplied name is carried by the native schema field. An
            // empty collection restores the same default as an absent one.
            let empty = v.as_object().is_some_and(serde_json::Map::is_empty)
                || v.as_array().is_some_and(Vec::is_empty);
            if (*key != "name" || v.is_null()) && !empty {
                extra.insert((*key).into(), v.clone());
            }
        } else if matches!(*key, "uid" | "name") {
            extra.insert((*key).into(), Value::Null);
        }
    }
    if !extra.is_empty() {
        doc["ext"][id.to_string()][KEY] = Value::Object(extra);
    }
    Ok(())
}

pub(super) fn restore<T: Restore>(
    root: &Value,
    row: &Value,
    value: &mut T,
    fields: &[&str],
) -> Result<()> {
    let id = integer(row, "id")?;
    let Some(extra) = root["ext"][id.to_string()][KEY].as_object() else {
        return Ok(());
    };
    for key in fields {
        if let Some(v) = extra.get(*key) {
            if *key == "name" && !v.is_null() {
                return Err(error("ext must not override a native Sienna name"));
            }
            value
                .restore_field(key, v.clone())
                .map_err(|e| error(format!("invalid PowerIO metadata on component {id}: {e}")))?;
        }
    }
    Ok(())
}

// Deserialize only extension fields. Serializing the complete electrical value
// here would turn valid unbounded limits into JSON nulls.
pub(super) trait Restore {
    fn restore_field(&mut self, key: &str, value: Value) -> serde_json::Result<()>;
}
macro_rules! fields {
    ($ty:ty, $($field:ident),+ $(,)?) => {
        impl Restore for $ty {
            fn restore_field(&mut self, key: &str, value: Value) -> serde_json::Result<()> {
                match key {
                    $(stringify!($field) => self.$field = Deserialize::deserialize(NonFiniteDe(value))?,)+
                    _ => unreachable!("extension field is not in the allowlist"),
                }
                Ok(())
            }
        }
    };
}
fields!(Bus, uid, name, extras);
fields!(Branch, uid, name, extras, rating_sets, angmin, angmax);
fields!(Load, uid, extras);
fields!(Shunt, uid, extras, section_count, control);
fields!(Storage, uid);
fields!(Transformer3W, uid, name, extras);
fields!(Area, number, uid, name, slack_bus, tolerance, area_type);
fields!(Hvdc, uid, cost, vf, vt, qf, qt, pt);
impl Restore for Generator {
    fn restore_field(&mut self, key: &str, value: Value) -> serde_json::Result<()> {
        match key {
            "uid" => self.uid = serde_json::from_value(value)?,
            "vg" => self.vg = serde_json::from_value(value)?,
            "caps" => {
                // GenCaps uses the network's named-column serde representation.
                let mut shell = serde_json::to_value(Generator::new(self.bus))?;
                shell["caps"] = value;
                self.caps = serde_json::from_value::<Generator>(shell)?.caps;
            }
            _ => unreachable!("extension field is not in the allowlist"),
        }
        Ok(())
    }
}

pub(super) fn record_branch_solution(
    doc: &mut Value,
    id: i64,
    branch: &Branch,
    warnings: &mut super::Diagnostics,
) {
    if let Some(solution) = &branch.solution {
        doc["ext"][id.to_string()][KEY]["to_terminal_power"] =
            serde_json::json!({"active":solution.pt, "reactive":solution.qt});
        warnings.push(&super::codes::EMIT_SIENNA.field_dropped, "branch receiving-terminal results are retained in ext; native fields carry sending-terminal results");
    }
}

pub(super) fn restore_branch_solution(
    root: &Value,
    row: &Value,
    branch: &mut Branch,
    warnings: &mut super::Diagnostics,
) -> Result<()> {
    let id = integer(row, "id")?;
    if root["ext"][id.to_string()][KEY]["branch_flow_absent"] == true
        && row["active_power_flow"] == 0.0
        && row["reactive_power_flow"] == 0.0
    {
        return Ok(());
    }
    if let Some(power) = root["ext"][id.to_string()][KEY].get("to_terminal_power") {
        let scale = super::power_scale(row)?;
        branch.solution = Some(crate::network::BranchSolution {
            pf: super::number(row, "active_power_flow")? * scale,
            qf: super::number(row, "reactive_power_flow")? * scale,
            pt: super::number(power, "active")?,
            qt: super::number(power, "reactive")?,
        });
    } else if row.get("active_power_flow").is_some() || row.get("reactive_power_flow").is_some() {
        super::retained(
            warnings,
            "branch sending-terminal results are retained without a complete two-terminal solution",
        );
    }
    Ok(())
}

use super::{
    BTreeMap, BTreeSet, Diagnostics, ELECTRICAL_KINDS, Result, Value, codes, json, power_scale,
};

// Power-family fields that can remain outside the balanced value. Their units
// follow the enclosing component, including nested MinMax and ramp-rate pairs.
const POWER_FIELDS: &[&str] = &[
    "ramp_limits",
    "power_trajectory",
    "standing_loss",
    "operational_flow_limit",
    "rating_from",
    "rating_to",
    "peak_active_power",
    "peak_reactive_power",
    "active_power_flow",
    "reactive_power_flow",
];
const REFERENCES: &[&str] = &[
    "bus",
    "arc",
    "area",
    "load_zone",
    "from_id",
    "to_id",
    "circuit",
    "star_bus",
    "primary_circuit",
    "secondary_circuit",
    "tertiary_circuit",
    "dynamic_injector",
    "entity_id",
    "owner_id",
    "component_id",
    "service_id",
    "plant_id",
    "control_id",
    "attribute_id",
    "trading_hub_id",
];

fn scale(value: &mut Value, factor: f64) {
    match value {
        Value::Number(n) => {
            if let Some(v) = n.as_f64() {
                *value = json!(v * factor);
            }
        }
        Value::Object(o) => {
            for v in o.values_mut() {
                scale(v, factor);
            }
        }
        Value::Array(a) => {
            for v in a {
                scale(v, factor);
            }
        }
        _ => {}
    }
}

pub(super) fn preserve_unmapped(
    doc: &mut Value,
    original: &Value,
    warnings: &mut Diagnostics,
) -> Result<()> {
    let Some(source) = original.as_object() else {
        return Ok(());
    };
    for (key, value) in source {
        if !matches!(
            key.as_str(),
            "components" | "schema_version" | "name" | "frequency" | "ext"
        ) {
            doc[key] = value.clone();
        }
    }
    if let Some(old) = original["ext"].as_object() {
        for (id, fields) in old {
            if let Some(fields) = fields.as_object() {
                for (key, value) in fields {
                    if doc["ext"][id].get(key).is_none() {
                        doc["ext"][id][key] = value.clone();
                    }
                }
            }
        }
    }
    if let Some(components) = original["components"].as_object() {
        for (kind, rows) in components {
            if doc["components"].get(kind).is_none() {
                if !ELECTRICAL_KINDS.contains(&kind.as_str()) {
                    doc["components"][kind] = rows.clone();
                }
                continue;
            }
            if let (Some(old), Some(new)) =
                (rows.as_array(), doc["components"][kind].as_array_mut())
            {
                // Retained rows by id, built once per kind, so emitting an
                // edited document stays linear in the component count.
                let previous_by_id: BTreeMap<i64, &Value> = old
                    .iter()
                    .filter_map(|r| r["id"].as_i64().map(|id| (id, r)))
                    .collect();
                for row in new {
                    if let Some(previous) = row["id"]
                        .as_i64()
                        .and_then(|id| previous_by_id.get(&id).copied())
                    {
                        let mut merged = previous.clone();
                        // Required/defaulted power units on old and new rows may
                        // differ. Retained ramp rates must follow the new basis.
                        if previous.get("power_units").is_some() && row.get("power_units").is_some()
                        {
                            let factor = power_scale(previous)? / power_scale(row)?;
                            for key in POWER_FIELDS {
                                if row.get(*key).is_none()
                                    && let Some(v) = merged.get_mut(*key)
                                {
                                    scale(v, factor);
                                }
                            }
                        }
                        if let Some(o) = merged.as_object_mut() {
                            if row.get("reactive_power_limits").is_none() {
                                o.remove("reactive_power_limits");
                            }
                            if kind == "TransformerCircuit" {
                                for key in [
                                    "control_objective",
                                    "control_limits",
                                    "controlled_quantity_limits",
                                    "tap_ratio_limits",
                                    "phase_angle_limits",
                                    "controlled_voltage_limits",
                                    "controlled_active_power_flow_limits",
                                    "controlled_reactive_power_flow_limits",
                                    "regulated_bus_number",
                                    "number_of_tap_positions",
                                ] {
                                    if row.get(key).is_none() {
                                        o.remove(key);
                                    }
                                }
                            }
                        }
                        if let Some(o) = row.as_object() {
                            for (k, v) in o {
                                merged[k] = v.clone();
                            }
                        }
                        *row = merged;
                    }
                }
            }
        }
    }
    prune_references(doc, warnings);
    Ok(())
}

fn prune_references(doc: &mut Value, warnings: &mut Diagnostics) {
    // Deleting a typed component also invalidates its memberships and opaque
    // dependents. Remove them transitively with explicit loss diagnostics.
    loop {
        let mut ids = BTreeSet::new();
        for rows in doc["components"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
        {
            for row in rows.as_array().into_iter().flatten() {
                if let Some(id) = row["id"].as_i64() {
                    ids.insert(id);
                }
            }
        }
        for row in doc["supplemental_attributes"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(id) = row["id"].as_i64() {
                ids.insert(id);
            }
        }
        let missing = |row: &Value| {
            REFERENCES.iter().any(|key| {
                row.get(*key)
                    .and_then(Value::as_i64)
                    .is_some_and(|id| !ids.contains(&id))
            })
        };
        let mut removed = 0;
        for rows in doc["components"]
            .as_object_mut()
            .into_iter()
            .flat_map(|m| m.values_mut())
        {
            if let Some(rows) = rows.as_array_mut() {
                let before = rows.len();
                rows.retain(|r| !missing(r));
                removed += before - rows.len();
            }
        }
        for (key, rows) in doc.as_object_mut().expect("document object") {
            if key.ends_with("_associations")
                && let Some(rows) = rows.as_array_mut()
            {
                let before = rows.len();
                rows.retain(|r| !missing(r));
                removed += before - rows.len();
            }
        }
        if removed == 0 {
            break;
        }
        warnings.push(&codes::EMIT_SIENNA.record_dropped, format!("{removed} retained components or associations reference deleted components and were omitted"));
    }
}

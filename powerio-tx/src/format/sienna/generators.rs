use super::{Diagnostics, Value, codes, json};
use crate::network::{Generator, GeneratorEnergySource};

pub(super) fn energy_source(kind: &str, row: &Value) -> GeneratorEnergySource {
    match row["prime_mover_type"].as_str() {
        Some("WT" | "WS") => return GeneratorEnergySource::Wind,
        Some("PVe") => return GeneratorEnergySource::Solar,
        Some("HY" | "PS" | "HA" | "HB" | "HK") => return GeneratorEnergySource::Hydro,
        _ => {}
    }
    if kind == "HydroDispatch" {
        return GeneratorEnergySource::Hydro;
    }
    match row["fuel"].as_str() {
        Some("NUCLEAR") => GeneratorEnergySource::Nuclear,
        Some(fuel) if fuel != "OTHER" => GeneratorEnergySource::Thermal,
        _ => GeneratorEnergySource::Other,
    }
}

/// Shape the generator row for its schema component. Returns the component
/// kind and whether a retained source cost curve replaced the row's cost.
#[allow(clippy::too_many_lines)] // Each schema component has distinct required fields.
pub(super) fn prepare(
    row: &mut Value,
    g: &Generator,
    original: Option<&Value>,
    id: i64,
    warnings: &mut Diagnostics,
) -> (&'static str, bool) {
    let mut retained_cost = false;
    let mut kind = match g.energy_source {
        GeneratorEnergySource::Wind | GeneratorEnergySource::Solar => "RenewableDispatch",
        GeneratorEnergySource::Hydro => "HydroDispatch",
        _ => "ThermalStandard",
    };
    let known = [
        "ThermalStandard",
        "ThermalMultiStart",
        "RenewableDispatch",
        "RenewableNonDispatch",
        "HydroDispatch",
        "Source",
        "SynchronousCondenser",
    ];
    let previous = original.and_then(|doc| {
        known.into_iter().find_map(|k| {
            doc["components"][k]
                .as_array()?
                .iter()
                .find(|r| r["id"] == id)
                .map(|r| (k, r))
        })
    });
    if let Some((k, old)) = previous {
        if energy_source(k, old) == g.energy_source {
            kind = k;
        } else {
            warnings.push(&codes::EMIT_SIENNA.field_dropped, format!("generator {id}: changed energy source requires a new component type; original type-specific fields are omitted"));
        }
        // A schema name is not represented on the balanced generator row.
        row["name"] = old["name"].clone();
    }
    if matches!(
        g.energy_source,
        GeneratorEnergySource::Wind | GeneratorEnergySource::Solar | GeneratorEnergySource::Hydro
    ) {
        row["prime_mover_type"] = json!(match g.energy_source {
            GeneratorEnergySource::Wind => "WT",
            GeneratorEnergySource::Solar => "PVe",
            _ => "HY",
        });
    }
    if g.energy_source == GeneratorEnergySource::Nuclear {
        row["fuel"] = json!("NUCLEAR");
    }
    if matches!(kind, "RenewableDispatch" | "RenewableNonDispatch") {
        row["prime_mover_type"] =
            previous.map_or(json!("OT"), |(_, old)| old["prime_mover_type"].clone());
        if row["prime_mover_type"].is_null() || previous.is_none() {
            row["prime_mover_type"] = json!(match g.energy_source {
                GeneratorEnergySource::Wind => "WT",
                GeneratorEnergySource::Solar => "PVe",
                _ => "OT",
            });
        }
        row["rating"] = json!(g.pmax);
        row["power_factor"] = previous.map_or(json!(1.0), |(_, old)| old["power_factor"].clone());
        let object = row.as_object_mut().expect("generated object");
        object.remove("active_power_limits");
        object.remove("status");
        if g.pmin != 0.0 {
            warnings.push(&codes::EMIT_SIENNA.field_dropped, format!("generator {id}: renewable component uses rating; independent active-power bounds have no schema field"));
        }
    }
    match kind {
        "RenewableDispatch" | "HydroDispatch" => {
            let cost = row["operation_cost"]
                .as_object_mut()
                .expect("generated cost");
            cost.insert(
                "cost_type".into(),
                json!(if kind == "HydroDispatch" {
                    "HYDRO_GEN"
                } else {
                    "RENEWABLE"
                }),
            );
            cost.remove("start_up");
            cost.remove("shut_down");
            if g.cost
                .as_ref()
                .is_some_and(|c| c.startup != 0.0 || c.shutdown != 0.0)
            {
                warnings.push(
                    &codes::EMIT_SIENNA.field_dropped,
                    format!(
                        "generator {id}: startup and shutdown costs have no {kind} cost fields"
                    ),
                );
            }
            if kind == "HydroDispatch" {
                row.as_object_mut().expect("object").remove("status");
            }
        }
        "RenewableNonDispatch" | "SynchronousCondenser" => {
            let object = row.as_object_mut().expect("generated object");
            object.remove("operation_cost");
            object.remove("status");
            object.remove("active_power_limits");
            if kind == "RenewableNonDispatch" {
                object.remove("reactive_power_limits");
            } else {
                object.remove("active_power");
                object.insert("active_power_losses".into(), json!(-g.pg));
            }
            if g.cost.is_some() {
                warnings.push(
                    &codes::EMIT_SIENNA.field_dropped,
                    format!("generator {id}: {kind} has no operation_cost field"),
                );
            }
        }
        "Source" => {
            let object = row.as_object_mut().expect("object");
            object.remove("status");
            object.remove("rating");
            if let Some((_, old)) = previous {
                object.insert("operation_cost".into(), old["operation_cost"].clone());
            }
            if g.cost.is_some() {
                warnings.push(&codes::EMIT_SIENNA.field_dropped, format!("source {id}: a single generator cost cannot represent separate import/export offers"));
            }
        }
        _ => {}
    }
    if let Some((old_kind, old)) = previous
        && old_kind == kind
        && let Some(old_cost) = old.get("operation_cost")
        && row.get("operation_cost").is_some()
    {
        let projection = super::read_cost(
            old_cost,
            super::base(old).unwrap_or(super::BASE),
            &mut Diagnostics::new(),
        );
        if projection.as_ref().is_ok_and(|cost| cost == &g.cost) && same_cost_basis(old, row) {
            row["operation_cost"] = old_cost.clone();
            retained_cost = g.cost.is_none();
        } else if old_cost["start_up"].is_object() {
            warnings.push(
                &codes::EMIT_SIENNA.field_dropped,
                format!(
                    "generator {id}: edited scalar cost replaces retained staged startup costs"
                ),
            );
        }
    }
    // A retained curve with no balanced cost interpretation remains authoritative
    // until the caller supplies a typed replacement.
    if g.cost.is_none()
        && let Some((old_kind, old)) = previous
        && old_kind == kind
        && old.get("operation_cost").is_some()
        && row.get("operation_cost").is_some()
    {
        if same_cost_basis(old, row) {
            row["operation_cost"] = old["operation_cost"].clone();
            retained_cost = true;
        } else {
            warnings.push(&codes::EMIT_SIENNA.field_dropped, format!("generator {id}: changing the component base requires a typed replacement for the retained cost curve"));
        }
    }
    (kind, retained_cost)
}

fn same_cost_basis(old: &Value, new: &Value) -> bool {
    fn component_basis(value: &Value) -> bool {
        value.as_object().is_some_and(|o| {
            o.get("power_units").is_some_and(|v| v == "COMPONENT_BASE")
                || o.values().any(component_basis)
        }) || value
            .as_array()
            .is_some_and(|a| a.iter().any(component_basis))
    }
    !component_basis(&old["operation_cost"]) || old["base_power"] == new["base_power"]
}

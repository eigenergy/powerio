use super::{BTreeMap, Diagnostics, Value, codes, json};
use crate::network::{Generator, GeneratorEnergySource};

/// The schema components a balanced generator can come from, in the order a
/// retained row is looked up.
pub(super) const KNOWN: [&str; 7] = [
    "ThermalStandard",
    "ThermalMultiStart",
    "RenewableDispatch",
    "RenewableNonDispatch",
    "HydroDispatch",
    "Source",
    "SynchronousCondenser",
];

/// The retained document's generator rows by id, built once per emission so
/// each generator finds its source row without scanning every component.
pub(super) fn by_id(original: Option<&Value>) -> BTreeMap<i64, (&'static str, &Value, bool)> {
    let mut rows = BTreeMap::new();
    for kind in KNOWN {
        for row in original
            .and_then(|doc| doc["components"][kind].as_array())
            .into_iter()
            .flatten()
        {
            if let Some(id) = row["id"].as_i64() {
                let absent = original.is_some_and(|doc| {
                    super::metadata::ext(doc, id)["operation_cost_absent"] == true && {
                        let mut findings = Diagnostics::new();
                        super::read_cost(
                            &row["operation_cost"],
                            super::base(row).unwrap_or(super::BASE),
                            &mut findings,
                        )
                        .is_ok_and(|cost| absent_placeholder(cost.as_ref(), findings.is_empty()))
                    }
                });
                rows.entry(id).or_insert((kind, row, absent));
            }
        }
    }
    rows
}

pub(super) fn absent_placeholder(cost: Option<&crate::network::GenCost>, complete: bool) -> bool {
    complete
        && cost.is_some_and(|cost| {
            cost.startup == 0.0 && cost.shutdown == 0.0 && cost.coeffs.iter().all(|v| *v == 0.0)
        })
}

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

/// Shape the generator row for its schema component. `previous` is the
/// retained source row from [`by_id`]. Returns the component kind and whether
/// a retained source cost curve replaced the row's cost.
#[allow(clippy::too_many_lines)] // Each schema component has distinct required fields.
pub(super) fn prepare(
    row: &mut Value,
    g: &Generator,
    previous: Option<(&'static str, &Value, bool)>,
    id: i64,
    warnings: &mut Diagnostics,
) -> (&'static str, bool) {
    let mut retained_cost = false;
    let mut kind = match g.energy_source {
        GeneratorEnergySource::Wind | GeneratorEnergySource::Solar => "RenewableDispatch",
        GeneratorEnergySource::Hydro => "HydroDispatch",
        _ => "ThermalStandard",
    };
    // Type-specific fields of the retained row still describe this generator
    // only while its energy source is unchanged.
    let retained = previous.filter(|(k, old, _)| energy_source(k, old) == g.energy_source);
    if let Some((k, old, _)) = previous {
        if retained.is_some() {
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
        row["prime_mover_type"] = retained
            .map(|(_, old, _)| old["prime_mover_type"].clone())
            .filter(|value| !value.is_null())
            .unwrap_or_else(|| {
                json!(match g.energy_source {
                    GeneratorEnergySource::Wind => "WT",
                    GeneratorEnergySource::Solar => "PVe",
                    _ => "OT",
                })
            });
        row["rating"] = json!(g.pmax);
        row["power_factor"] =
            retained.map_or(json!(1.0), |(_, old, _)| old["power_factor"].clone());
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
            if let Some((_, old, false)) = previous {
                object.insert("operation_cost".into(), old["operation_cost"].clone());
            }
            if g.cost.is_some() {
                warnings.push(&codes::EMIT_SIENNA.field_dropped, format!("source {id}: a single generator cost cannot represent separate import/export offers"));
            }
        }
        _ => {}
    }
    if let Some((old_kind, old, false)) = previous
        && old_kind == kind
        && let Some(old_cost) = old.get("operation_cost")
        && row.get("operation_cost").is_some()
    {
        let projection = super::read_cost(
            old_cost,
            super::base(old).unwrap_or(super::BASE),
            &mut Diagnostics::new(),
        );
        let unchanged = projection.as_ref().is_ok_and(|cost| cost == &g.cost);
        // An unprojectable curve survives until the caller supplies a
        // replacement. Removing a projected cost must not restore it.
        if unchanged && same_cost_basis(old, row) {
            row["operation_cost"] = old_cost.clone();
            retained_cost = g.cost.is_none();
        } else {
            if unchanged && g.cost.is_none() {
                warnings.push(&codes::EMIT_SIENNA.field_dropped, format!("generator {id}: changing the component base requires a typed replacement for the retained cost curve"));
            }
            if old_cost["start_up"].is_object() {
                warnings.push(
                    &codes::EMIT_SIENNA.field_dropped,
                    format!(
                        "generator {id}: edited scalar cost replaces retained staged startup costs"
                    ),
                );
            }
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

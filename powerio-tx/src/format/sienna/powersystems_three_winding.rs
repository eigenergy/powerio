//! Conversion between explicit transformer circuits and PowerSystems winding fields.
use super::powersystems::{clock_angle, rename, scale};
use super::{
    BTreeMap, BTreeSet, Diagnostics, Result, Value, codes, error, integer, json, number, optional,
    push, rows,
};
const SIDES: [&str; 3] = ["primary", "secondary", "tertiary"];

pub(super) fn decode(row: &mut Value, doc: &mut Value, next: &mut i64, base: f64) -> Result<()> {
    for (side, pair) in SIDES.into_iter().zip(["12", "23", "13"]) {
        let basis = optional(row, &format!("base_power_{pair}"), base)?;
        let id = *next;
        *next += 1;
        let mut circuit = json!({
            "id":id,"arc":row[format!("{side}_star_arc")],
            "available": row["available"] == true && row.get(format!("available_{side}")).is_none_or(|v|*v == true),
            "r":row[format!("r_{side}")],"x":row[format!("x_{side}")],
            "tap":optional(row,&format!("{side}_turns_ratio"),1.0)?,
            "alpha":optional(row,&format!("α_{side}"),clock_angle(row,&format!("{side}_group_number"))?)?,
            "rating":if row.get(format!("rating_{side}")).is_none() {
                json!(0.0) // PowerSystems' constructor default is a finite zero.
            } else {
                let rating = number(row, &format!("rating_{side}"))?;
                if super::is_infinite_bound(rating) { Value::Null } else { json!(rating) }
            },
            "active_power_flow":optional(row,&format!("active_power_flow_{side}"),0.0)?,
            "reactive_power_flow":optional(row,&format!("reactive_power_flow_{side}"),0.0)?,
            "control_objective":row.get(format!("control_objective_{side}")).cloned().unwrap_or(json!("UNDEFINED")),
            "base_power":basis,"power_units":"COMPONENT_BASE","parameter_units":"COMPONENT_BASE"
        });
        if let Some(extra) =
            super::metadata::ext(doc, &row["id"])["winding_control_fields"][side].as_object()
        {
            for (field, value) in extra {
                circuit[field] = value.clone();
            }
        }
        // PowerSystems has one native winding rating. Additional ratings are
        // explicit metadata, expressed on this winding's component base.
        let extra = &super::metadata::ext(doc, &row["id"])["winding_rating_fields"][side];
        for field in ["rating_b", "rating_c"] {
            if extra.get(field).is_some() {
                circuit[field] = json!(super::thermal_limit(extra, field, 1.0)?);
            }
        }
        // The circuit uses the system base so the neutral control base remains consistent.
        let factor = base / basis;
        for field in ["r", "x"] {
            scale(&mut circuit[field], factor);
        }
        for field in [
            "rating",
            "rating_b",
            "rating_c",
            "active_power_flow",
            "reactive_power_flow",
        ] {
            scale(&mut circuit[field], 1.0 / factor);
        }
        circuit["base_power"] = json!(base);
        push(doc, "TransformerCircuit", circuit);
        row[format!("{side}_circuit")] = json!(id);
    }
    for field in ["r", "x", "base_power"] {
        rename(row, &format!("{field}_13"), &format!("{field}_31"));
    }
    row["magnetizing_shunt"] = json!({"real":optional(row,"g",0.0)?,"imag":optional(row,"b",0.0)?});
    let factor = optional(row, "base_power_12", base)? / base;
    scale(&mut row["magnetizing_shunt"], factor);
    row["admittance_units"] = json!("COMPONENT_BASE");
    row["shunt_location"] = json!("STAR");
    Ok(())
}

#[allow(clippy::too_many_lines)] // A parent and its three circuits are converted together.
pub(super) fn prepare(doc: &mut Value, base: f64, warnings: &mut Diagnostics) -> Result<()> {
    let parents = rows(doc, "ThreeWindingTransformer")?.to_vec();
    let circuits: BTreeMap<_, _> = rows(doc, "TransformerCircuit")?
        .iter()
        .map(|r| Ok((integer(r, "id")?, r.clone())))
        .collect::<Result<_>>()?;
    let arcs: BTreeMap<_, _> = rows(doc, "Arc")?
        .iter()
        .map(|r| Ok((integer(r, "id")?, r.clone())))
        .collect::<Result<_>>()?;
    let mut removed = BTreeSet::new();
    let mut next = doc["components"]
        .as_object()
        .expect("components")
        .values()
        .flat_map(|r| r.as_array().into_iter().flatten())
        .filter_map(|r| r["id"].as_i64())
        .max()
        .unwrap_or(0)
        + 1;
    for mut row in parents {
        let mut phase = false;
        let mut any_available = false;
        for (side, pair) in SIDES.into_iter().zip(["12", "23", "31"]) {
            let basis = optional(&row, &format!("base_power_{pair}"), base)?;
            let id = integer(&row, &format!("{side}_circuit"))?;
            removed.insert(id);
            let c = circuits
                .get(&id)
                .ok_or_else(|| error("missing three-winding circuit"))?;
            row[format!("{side}_star_arc")] = c["arc"].clone();
            for field in ["r", "x"] {
                row[format!("{field}_{side}")] = json!(number(c, field)? * basis / base);
            }
            let angle = optional(c, "alpha", 0.0)?;
            phase |= angle != 0.0;
            row[format!("α_{side}")] = json!(angle);
            row[format!("{side}_turns_ratio")] = c["tap"].clone();
            row[format!("available_{side}")] = c["available"].clone();
            any_available |= c["available"] == true;
            // Native winding ratings are Float64, unlike the parent's optional
            // aggregate rating. An unlimited circuit, stated by absence or by
            // the marker on the circuit's own base, is PowerSystems' unbounded
            // marker, never a finite zero bound.
            let rating = super::thermal_limit(c, "rating", 1.0 / basis)?;
            let marked = match c.get("rating").and_then(Value::as_f64) {
                Some(mva) => super::is_infinite_bound(mva / super::base(c)?),
                None => false,
            };
            row[format!("rating_{side}")] = json!(if rating.is_finite() && !marked {
                rating
            } else {
                super::INFINITE_BOUND
            });
            for field in ["rating_b", "rating_c"] {
                let rating = super::thermal_limit(c, field, 1.0 / basis)?;
                if !rating.is_infinite() {
                    super::metadata::ext_mut(doc, &row["id"])["winding_rating_fields"][side]
                        [field] = json!(rating);
                    warnings.push(&codes::EMIT_SIENNA.field_dropped, format!(
                        "PowerSystems {side} {field} remains in ext without native winding constraint semantics"
                    ));
                }
            }
            for field in ["active_power_flow", "reactive_power_flow"] {
                row[format!("{field}_{side}")] = json!(optional(c, field, 0.0)? / basis);
            }
            if let Some(control) = c.get("control_objective") {
                row[format!("control_objective_{side}")] = control.clone();
                warnings.push(&codes::EMIT_SIENNA.field_dropped,"PowerSystems three-winding control carries its objective; actuator settings and target bounds remain in ext");
                for field in [
                    "tap_ratio_limits",
                    "phase_angle_limits",
                    "controlled_voltage_limits",
                    "controlled_active_power_flow_limits",
                    "controlled_reactive_power_flow_limits",
                    "number_of_tap_positions",
                    "regulated_bus_number",
                ] {
                    if let Some(value) = c.get(field) {
                        super::metadata::ext_mut(doc, &row["id"])["winding_control_fields"][side]
                            [field] = value.clone();
                    }
                }
            }
        }
        row["available"] = json!(any_available);
        row["rating"] = Value::Null;
        if row["shunt_location"] == "PRIMARY" {
            let c = &circuits[&integer(&row, "primary_circuit")?];
            let arc = &arcs[&integer(c, "arc")?];
            let mut y = row["magnetizing_shunt"].clone();
            scale(&mut y, base);
            if y["real"] != 0.0 || y["imag"] != 0.0 {
                push(
                    doc,
                    "FixedAdmittance",
                    json!({"id":next,"name":format!("{}_magnetizing",row["name"].as_str().unwrap_or("transformer")),"available":any_available,"bus":arc["from_id"],"Y":y,"base_power":base,"admittance_units":"COMPONENT_MVAR","power_units":"NATURAL_UNITS"}),
                );
                next += 1;
                warnings.push(&codes::EMIT_SIENNA.field_dropped,"PowerSystems represents primary-terminal magnetizing admittance as a separate fixed shunt");
            }
            row["g"] = json!(0.0);
            row["b"] = json!(0.0);
        } else {
            row["g"] = row["magnetizing_shunt"]["real"].clone();
            row["b"] = row["magnetizing_shunt"]["imag"].clone();
            let factor = base / optional(&row, "base_power_12", base)?;
            scale(&mut row["g"], factor);
            scale(&mut row["b"], factor);
        }
        for field in ["r", "x", "base_power"] {
            rename(&mut row, &format!("{field}_31"), &format!("{field}_13"));
        }
        push(
            doc,
            if phase {
                "PhaseShiftingTransformer3W"
            } else {
                "Transformer3W"
            },
            row,
        );
    }
    doc["components"]
        .as_object_mut()
        .expect("components")
        .remove("ThreeWindingTransformer");
    if let Some(rows) = doc["components"]
        .get_mut("TransformerCircuit")
        .and_then(Value::as_array_mut)
    {
        rows.retain(|r| !removed.contains(&r["id"].as_i64().unwrap_or(0)));
    }
    Ok(())
}

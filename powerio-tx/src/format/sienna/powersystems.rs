//! PowerSystems.jl's serialized System layout. Stored electrical values use
//! device bases independently of the display units in `units_settings`.
use super::{
    BTreeMap, BalancedNetwork, Diagnostics, ELECTRICAL_KINDS, Result, SourceFormat, TextEmission,
    Value, base, codes, error, integer, json, limits, minmax, number, optional, positive,
    power_scale, push, retained, rows,
};
use powerio_core::cim::deterministic_mrid;

pub(in crate::format) const EXTENSION: &str = "org.eigenergy.powerio.powersystems";
/// Fields that refer to another component: its UUID in PowerSystems output
/// and its Sienna id in the schema document.
const REFERENCE_FIELDS: &[&str] = &[
    "bus",
    "arc",
    "area",
    "load_zone",
    "from",
    "to",
    "dynamic_injector",
    "primary_star_arc",
    "secondary_star_arc",
    "tertiary_star_arc",
    "star_bus",
];
const POWER_FIELDS: &[&str] = &[
    "active_power",
    "reactive_power",
    "active_power_flow",
    "reactive_power_flow",
    "rating",
    "rating_b",
    "rating_c",
    "active_power_limits",
    "reactive_power_limits",
    "input_active_power_limits",
    "output_active_power_limits",
    "active_power_limits_from",
    "active_power_limits_to",
    "reactive_power_limits_from",
    "reactive_power_limits_to",
    "operational_flow_limit",
    "max_active_power",
    "max_reactive_power",
    "peak_active_power",
    "peak_reactive_power",
    "ramp_limits",
    "constant_active_power",
    "constant_reactive_power",
    "current_active_power",
    "current_reactive_power",
    "impedance_active_power",
    "impedance_reactive_power",
    "max_constant_active_power",
    "max_constant_reactive_power",
    "max_current_active_power",
    "max_current_reactive_power",
    "max_impedance_active_power",
    "max_impedance_reactive_power",
];
pub(super) fn scale(v: &mut Value, factor: f64) {
    if let Some(n) = v.as_f64() {
        *v = json!(n * factor);
    } else if let Some(o) = v.as_object_mut() {
        for value in o.values_mut() {
            scale(value, factor);
        }
    }
}
fn type_name(v: &Value) -> Result<&str> {
    v["__metadata__"]["type"]
        .as_str()
        .ok_or_else(|| error("PowerSystems component lacks type metadata"))
}
fn internal(id: &str) -> Value {
    json!({"uuid":{"value":id},"units_info":null,"ext":null})
}
fn uuid(kind: &str, id: &str) -> String {
    deterministic_mrid("https://powerio.org/powersystems", kind, id)
}

fn component_refs(doc: &Value) -> Result<BTreeMap<i64, String>> {
    let mut refs = BTreeMap::new();
    let mut used = super::BTreeSet::new();
    let mut fresh = Vec::new();
    for rows in doc["components"].as_object().expect("components").values() {
        for row in rows.as_array().expect("rows") {
            let id = integer(row, "id")?;
            if let Some(identity) = super::metadata::ext(doc, id)["native_uuid"].as_str() {
                if !used.insert(identity.to_owned()) {
                    return Err(error("duplicate retained native component UUID"));
                }
                refs.insert(id, identity.to_owned());
            } else {
                fresh.push(id);
            }
        }
    }
    for id in fresh {
        let mut identity = uuid("component", &id.to_string());
        let mut attempt = 0;
        while !used.insert(identity.clone()) {
            attempt += 1;
            identity = uuid("component", &format!("{id}:{attempt}"));
        }
        refs.insert(id, identity);
    }
    Ok(refs)
}
fn metadata(v: &mut Value, kind: &str, module: &str) {
    v["__metadata__"] = json!({"type":kind,"module":module});
}
pub(super) fn rename(v: &mut Value, from: &str, to: &str) {
    if let Some(x) = v.as_object_mut().and_then(|o| o.remove(from)) {
        v[to] = x;
    }
}
fn strip_metadata(v: &mut Value) {
    match v {
        Value::Object(o) => {
            o.remove("__metadata__");
            for value in o.values_mut() {
                strip_metadata(value);
            }
        }
        Value::Array(a) => {
            for value in a {
                strip_metadata(value);
            }
        }
        _ => {}
    }
}
fn decode_curve(v: &mut Value) {
    let kind = v["__metadata__"]["type"].as_str().unwrap_or("").to_owned();
    let (key, tag) = match kind.as_str() {
        "CostCurve" => ("variable_cost_type", "COST"),
        "FuelCurve" => ("variable_cost_type", "FUEL"),
        "InputOutputCurve" => ("curve_type", "INPUT_OUTPUT"),
        "IncrementalCurve" => ("curve_type", "INCREMENTAL"),
        "AverageRateCurve" => ("curve_type", "AVERAGE_RATE"),
        "LinearFunctionData" => ("function_type", "LINEAR"),
        "QuadraticFunctionData" => ("function_type", "QUADRATIC"),
        "PiecewiseLinearData" => ("function_type", "PIECEWISE_LINEAR"),
        "PiecewiseStepData" => ("function_type", "PIECEWISE_STEP"),
        _ => ("", ""),
    };
    if !key.is_empty() {
        v[key] = json!(tag);
    }
    if v["power_units"] == "DEVICE_BASE" {
        v["power_units"] = json!("COMPONENT_BASE");
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("__metadata__");
        for value in o.values_mut() {
            decode_curve(value);
        }
    }
}
fn encode_curve(v: &mut Value) {
    let mut kind = None;
    for (field, names) in [
        (
            "variable_cost_type",
            &[("COST", "CostCurve"), ("FUEL", "FuelCurve")][..],
        ),
        (
            "curve_type",
            &[
                ("INPUT_OUTPUT", "InputOutputCurve"),
                ("INCREMENTAL", "IncrementalCurve"),
                ("AVERAGE_RATE", "AverageRateCurve"),
            ][..],
        ),
        (
            "function_type",
            &[
                ("LINEAR", "LinearFunctionData"),
                ("QUADRATIC", "QuadraticFunctionData"),
                ("PIECEWISE_LINEAR", "PiecewiseLinearData"),
                ("PIECEWISE_STEP", "PiecewiseStepData"),
            ][..],
        ),
    ] {
        if let Some(tag) = v.get(field).and_then(Value::as_str) {
            kind = names.iter().find(|(name, _)| *name == tag).map(|(_, k)| *k);
        }
        if let Some(o) = v.as_object_mut() {
            o.remove(field);
        }
    }
    if v["power_units"] == "COMPONENT_BASE" {
        v["power_units"] = json!("DEVICE_BASE");
    }
    if let Some(o) = v.as_object_mut() {
        for value in o.values_mut() {
            encode_curve(value);
        }
    }
    if let Some(kind) = kind {
        metadata(v, kind, "InfrastructureSystems");
    }
}
fn decode_cost(v: &mut Value) -> Result<()> {
    let kind = type_name(v)?.to_owned();
    let tag = match kind.as_str() {
        "ThermalGenerationCost" => "THERMAL",
        "HydroGenerationCost" => "HYDRO_GEN",
        "RenewableGenerationCost" => "RENEWABLE",
        "StorageCost" => "STORAGE",
        "ImportExportCost" => "IMPORT_EXPORT",
        _ => "UNMAPPED",
    };
    rename(v, "variable", "variable_operation_cost");
    decode_curve(v);
    v["cost_type"] = json!(tag);
    Ok(())
}
fn encode_cost(v: &mut Value) -> Result<()> {
    let kind = match v["cost_type"].as_str() {
        Some("THERMAL") => "ThermalGenerationCost",
        Some("HYDRO_GEN") => "HydroGenerationCost",
        Some("RENEWABLE") => "RenewableGenerationCost",
        Some("STORAGE") => "StorageCost",
        Some("IMPORT_EXPORT") => "ImportExportCost",
        _ => return Err(error("unsupported PowerSystems cost type")),
    };
    v.as_object_mut().expect("cost object").remove("cost_type");
    rename(v, "variable_operation_cost", "variable");
    encode_curve(v);
    metadata(v, kind, "PowerSystems");
    Ok(())
}

#[allow(clippy::too_many_lines)] // Native component variants share the UUID and unit conversion pass.
pub(in crate::format) fn to_schema(root: &Value) -> Result<Value> {
    match root["data_format_version"].as_str() {
        Some("4.0.0" | "5.0.0") => {}
        other => {
            return Err(error(format!(
                "unsupported PowerSystems data_format_version {other:?}; supported: 4.0.0, 5.0.0"
            )));
        }
    }
    let system_base = positive(
        number(&root["units_settings"], "base_value")?,
        "system base",
    )?;
    let components = root["data"]["components"]
        .as_array()
        .ok_or_else(|| error("PowerSystems data.components must be an array"))?;
    let mut ids = BTreeMap::new();
    for (i, row) in components.iter().enumerate() {
        let key = row["internal"]["uuid"]["value"]
            .as_str()
            .ok_or_else(|| error("component requires internal UUID"))?;
        if ids
            .insert(
                key,
                i64::try_from(i + 1).map_err(|_| error("too many components"))?,
            )
            .is_some()
        {
            return Err(error("duplicate component UUID"));
        }
    }
    let mut doc = json!({"schema_version":"0.4.0","name":root["metadata"]["name"],"frequency":root["frequency"],"components":{},"supplemental_attributes":[],"supplemental_attribute_associations":[],"plant_associations":[],"combined_cycle_associations":[],"service_associations":[],"time_series_associations":[],"time_series_storage_file":null});
    if doc["frequency"].is_null() {
        doc["frequency"] = json!(60.0);
    }
    if doc["name"].is_null() {
        doc["name"] = json!("PowerSystems");
    }
    let mut next = i64::try_from(components.len() + 1).map_err(|_| error("too many components"))?;
    for original in components {
        let mut row = original.clone();
        let mut kind = type_name(&row)?.to_owned();
        if kind == "TwoTerminalHVDCLine" {
            kind = "TwoTerminalGenericHVDCLine".into();
        }
        let key = original["internal"]["uuid"]["value"]
            .as_str()
            .expect("validated UUID");
        let id = ids[key];
        row["id"] = json!(id);
        for &field in REFERENCE_FIELDS {
            if row.get(field).is_some_and(|v| !v.is_null()) {
                let key = row[field]["value"]
                    .as_str()
                    .ok_or_else(|| error(format!("invalid {field} UUID reference")))?;
                row[field] = json!(
                    ids.get(key)
                        .ok_or_else(|| error(format!("missing component UUID {key}")))?
                );
            } else if let Some(o) = row.as_object_mut() {
                o.remove(field);
            }
        }
        if let Some(cost) = row.get_mut("operation_cost") {
            decode_cost(cost)?;
        }
        if let Some(loss) = row.get_mut("loss") {
            decode_curve(loss);
        }
        if row["status"].is_boolean() {
            row["status"] = json!(if row["status"] == true {
                "ONLINE"
            } else {
                "OFFLINE"
            });
        }
        strip_metadata(&mut row);
        row.as_object_mut()
            .expect("component object")
            .remove("internal");
        doc["ext"][id.to_string()] = row
            .as_object_mut()
            .expect("object")
            .remove("ext")
            .unwrap_or(json!({}));
        if doc["ext"][id.to_string()].is_null() {
            doc["ext"][id.to_string()] = json!({});
        }
        if !doc["ext"][id.to_string()].is_object()
            || doc["ext"][id.to_string()]
                .get("org.eigenergy.powerio")
                .is_some_and(|v| !v.is_object())
        {
            return Err(error("PowerSystems component ext must contain objects"));
        }
        if super::metadata::ext(&doc, id).get("uid").is_none() {
            super::metadata::ext_mut(&mut doc, id)["uid"] = json!(format!("powersystems:{key}"));
        }
        super::metadata::ext_mut(&mut doc, id)["native_uuid"] = json!(key);
        // A transformer's secondary-terminal shunt names its transformer by
        // UUID in PowerSystems output; the Sienna reader expects the circuit id.
        if let Some(uuid) = super::metadata::ext(&doc, id)["transformer_uuid"]
            .as_str()
            .map(str::to_owned)
            && let Some(circuit) = ids.get(uuid.as_str())
        {
            let extra = super::metadata::ext_mut(&mut doc, id)
                .as_object_mut()
                .expect("ext object");
            extra.remove("transformer_uuid");
            extra.insert("transformer_circuit".into(), json!(circuit));
        }
        // An unresolved extension marker is diagnosed by the shared reader;
        // the native shunt and its valid bus reference remain authoritative.
        for field in [
            "base_voltage_primary",
            "base_voltage_secondary",
            "base_voltage_tertiary",
        ] {
            if row.get(field).is_some_and(|v| v.is_null() || *v == 0.0) {
                row.as_object_mut().expect("row").remove(field);
            }
        }
        if super::metadata::ext(&doc, id)["branch_flow_absent"] == true
            && row["active_power_flow"] == 0.0
            && row["reactive_power_flow"] == 0.0
        {
            row.as_object_mut()
                .expect("row")
                .remove("active_power_flow");
            row.as_object_mut()
                .expect("row")
                .remove("reactive_power_flow");
        }
        row["base_power"] = json!(if matches!(
            kind.as_str(),
            "Line" | "MonitoredLine" | "TwoTerminalGenericHVDCLine"
        ) {
            system_base
        } else {
            optional(&row, "base_power", system_base)?
        });
        row["power_units"] = json!("COMPONENT_BASE");
        row["parameter_units"] = json!("COMPONENT_BASE");
        match kind.as_str() {
            "ACBus" => {
                if row.get("available").is_none() {
                    row["available"] = json!(row["bustype"] != "ISOLATED");
                }
            }
            "Arc" => {
                rename(&mut row, "from", "from_id");
                rename(&mut row, "to", "to_id");
            }
            "TwoTerminalGenericHVDCLine" => {
                let (lo, hi) = limits(&row, "active_power_limits_from", (0.0, 0.0))?;
                row["rating"] = json!(lo.abs().max(hi.abs()));
                row["operational_flow_limit"] = json!({"from_to_min":lo.max(0.0),"from_to_max":hi.max(0.0),"to_from_min":(-hi).max(0.0),"to_from_max":(-lo).max(0.0)});
                let curve = row["loss"].clone();
                row["loss"] = json!({"power_units":"COMPONENT_BASE","value_curve":curve});
            }
            "Line" => {
                super::metadata::ext_mut(&mut doc, id)
                    .as_object_mut()
                    .expect("validated extension object")
                    .remove("native_kind");
            }
            "MonitoredLine" => {
                super::metadata::ext_mut(&mut doc, id)["native_kind"] = json!("MonitoredLine");
                kind = "Line".into();
            }
            "SwitchedAdmittance" => {
                // Older format-5 runtimes omit these fields on serialization.
                // Explicit native fields always take precedence over the fallback.
                for field in ["control_mode", "regulated_bus_number"] {
                    if row.get(field).is_none()
                        && let Some(value) =
                            super::metadata::ext(&doc, id)["switched_control_fields"].get(field)
                    {
                        row[field] = value.clone();
                    }
                }
                scale(&mut row["Y"], system_base);
                let y = row["Y"].clone();
                row["solved_admittance"] = y["imag"].clone();
                super::metadata::ext_mut(&mut doc, id)["solved_conductance"] = y["real"].clone();
                if let Some(values) = row["Y_increase"].as_array_mut() {
                    for y in values {
                        scale(y, system_base);
                    }
                }
                let n = row["number_of_steps"].as_array().map_or(0, Vec::len);
                row["number_engaged"] = json!(vec![0; n]);
                row["admittance_units"] = json!("COMPONENT_MVAR");
                let key = if matches!(
                    row["control_mode"].as_str(),
                    Some(
                        "DISCRETE_REACTIVE_PLANT"
                            | "DISCRETE_REACTIVE_VSC"
                            | "DISCRETE_ADMITTANCE_REMOTE"
                            | "DISCRETE_REACTIVE_FACTS"
                    )
                ) {
                    "reactive_power_range_limits"
                } else {
                    "voltage_limits"
                };
                row[key] = row
                    .get("admittance_limits")
                    .cloned()
                    .unwrap_or_else(|| minmax(1.0, 1.0));
            }
            "FixedAdmittance" => {
                scale(&mut row["Y"], system_base);
                row["admittance_units"] = json!("COMPONENT_MVAR");
            }
            "Transformer2W" | "TapTransformer" | "PhaseShiftingTransformer" => {
                let mut shunt = row
                    .as_object_mut()
                    .expect("object")
                    .remove("primary_shunt")
                    .unwrap_or(json!({"real":0.0,"imag":0.0}));
                if let Some(susceptance) = shunt.as_f64() {
                    // Native 4.x stores positive inductive shunt susceptance.
                    shunt = json!({"real":0.0,"imag":-susceptance});
                }
                let name = row["name"].clone();
                rename(&mut row, "α", "alpha");
                if row.get("alpha").is_none() {
                    row["alpha"] = json!(clock_angle(&row, "winding_group_number")?);
                }
                rename(&mut row, "tap_limits", "tap_ratio_limits");
                if let Some(setpoint) = row.get("voltage_setpoint").and_then(Value::as_f64) {
                    let offset = &super::metadata::ext(&doc, id)["voltage_band_offsets"];
                    row["controlled_voltage_limits"] = minmax(
                        setpoint + optional(offset, "min", 0.0)?,
                        setpoint + optional(offset, "max", 0.0)?,
                    );
                }
                if let Some(extra) = super::metadata::ext(&doc, id)["control_fields"].as_object() {
                    for (field, value) in extra {
                        if row.get(field).is_none() {
                            row[field] = value.clone();
                        }
                    }
                }
                row["tap"] = json!(optional(&row, "tap", 1.0)?);
                let parent = json!({"id":next,"name":name,"circuit":id,"magnetizing_shunt":shunt,"admittance_units":"COMPONENT_BASE","shunt_location":"PRIMARY"});
                next += 1;
                push(&mut doc, "TwoWindingTransformer", parent);
                kind = "TransformerCircuit".into();
            }
            "Transformer3W" | "PhaseShiftingTransformer3W" => {
                super::powersystems_three_winding::decode(
                    &mut row,
                    &mut doc,
                    &mut next,
                    system_base,
                )?;
                kind = "ThreeWindingTransformer".into();
            }
            "EnergyReservoirStorage" => {
                let capacity = number(&row, "storage_capacity")? * base(&row)?;
                row["storage_capacity"] = json!(capacity);
                row["energy_units"] = json!("MWH");
            }
            _ => {}
        }
        push(&mut doc, &kind, row);
    }
    Ok(doc)
}

pub(in crate::format) fn read(
    root: &Value,
    name: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let doc = to_schema(root)?;
    if root["data"]["components"].as_array().is_some_and(|rows| {
        rows.iter().any(|row| {
            row["__metadata__"]["type"] == "SwitchedAdmittance"
                && ["control_mode", "regulated_bus_number"]
                    .iter()
                    .any(|field| {
                        row.get(*field).is_none()
                            && row["ext"][super::metadata::KEY]["switched_control_fields"]
                                .get(*field)
                                .is_some()
                    })
        })
    }) {
        retained(
            warnings,
            "switched-shunt controls restored from ext; the native consumer did not serialize these control fields",
        );
    }
    let mut net = super::read(&doc, name, 4, warnings)?;
    if rows(&doc, "Line")?
        .iter()
        .any(|r| r.get("flow_limits").is_some())
    {
        retained(
            warnings,
            "MonitoredLine directional flow limits remain in the native extension; the electrical branch retains its thermal rating",
        );
    }
    net.tables_mut().source_format = SourceFormat::PowerSystemsJson;
    if root["data"].get("time_series_storage_file").is_some()
        || root["data"]["masked_components"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    {
        retained(
            warnings,
            "PowerSystems companion data and masked components remain in the retained document outside the electrical projection",
        );
    }
    Ok(net)
}

#[allow(clippy::too_many_lines)] // Serialize component variants after common reference resolution.
pub(in crate::format) fn write(
    net: &BalancedNetwork,
    original: Option<&Value>,
) -> Result<TextEmission> {
    let schema = original.map(to_schema).transpose()?;
    let (mut doc, warnings) = super::write_document(net, 4, schema.as_ref())?;
    let mut output = TextEmission::new(String::new(), warnings);
    let mut winding_warnings = Diagnostics::new();
    super::powersystems_three_winding::prepare(&mut doc, net.base_mva(), &mut winding_warnings)?;
    output.diagnostics.extend(winding_warnings.into_records());
    deduplicate_arcs(&mut doc)?;
    let mut components = Vec::new();
    let refs = component_refs(&doc)?;
    let parents: BTreeMap<_, _> = rows(&doc, "TwoWindingTransformer")?
        .iter()
        .map(|r| Ok((integer(r, "circuit")?, r)))
        .collect::<Result<_>>()?;
    for (kind, records) in doc["components"].as_object().expect("components") {
        if kind == "TwoWindingTransformer" {
            continue;
        }
        if kind == "ThreeWindingTransformer" {
            return Err(error(
                "PowerSystems emission of three-winding equipment requires explicit star conversion",
            ));
        }
        if !ELECTRICAL_KINDS.contains(&kind.as_str())
            && !matches!(
                kind.as_str(),
                "Transformer3W" | "PhaseShiftingTransformer3W"
            )
        {
            output.push(
                &codes::EMIT_SIENNA.record_dropped,
                format!("PowerSystems: retained {kind} components have no native writer"),
            );
            continue;
        }
        for source in records.as_array().expect("rows") {
            let mut row = source.clone();
            let id = integer(&row, "id")?;
            let mut native_kind = kind.as_str();
            let component_base = if matches!(
                kind.as_str(),
                "Line"
                    | "TransformerCircuit"
                    | "Area"
                    | "LoadZone"
                    | "FixedAdmittance"
                    | "TwoTerminalGenericHVDCLine"
            ) {
                net.base_mva()
            } else {
                base(&row)?
            };
            let factor = power_scale(&row).unwrap_or(1.0) / component_base;
            for key in POWER_FIELDS {
                if let Some(value) = row.get_mut(*key) {
                    scale(value, factor);
                }
            }
            // PowerSystems recognizes its unbounded marker by exact equality,
            // so undo the rounding the base change may add.
            if matches!(kind.as_str(), "Line" | "TransformerCircuit") {
                for key in ["rating", "rating_b", "rating_c"] {
                    if let Some(rating) = row.get_mut(key)
                        && rating.as_f64().is_some_and(super::is_infinite_bound)
                    {
                        *rating = json!(super::INFINITE_BOUND);
                    }
                }
            }
            if kind == "Line"
                && super::metadata::ext(&doc, id)["native_kind"] == "MonitoredLine"
                && row.get("flow_limits").is_some()
            {
                native_kind = "MonitoredLine";
            }
            if kind == "SwitchedAdmittance" {
                row["Y"] = json!({"real":optional(super::metadata::ext(&doc, id),"solved_conductance",0.0)?/net.base_mva(),"imag":number(&row,"solved_admittance")?/net.base_mva()});
                if let Some(values) = row["Y_increase"].as_array_mut() {
                    for y in values {
                        scale(y, 1.0 / net.base_mva());
                    }
                }
                let n = row["number_of_steps"].as_array().map_or(0, Vec::len);
                row["initial_status"] = json!(vec![0; n]);
                row["admittance_limits"] = row
                    .get("voltage_limits")
                    .or_else(|| row.get("reactive_power_range_limits"))
                    .cloned()
                    .unwrap_or_else(|| minmax(1.0, 1.0));
                output.push(&codes::EMIT_SIENNA.field_dropped,"PowerSystems 5.9 omits switched-shunt control mode and remote regulation; ext retains those fields. Consumers whose Ybus excludes switched admittances must include the initial injection separately");
            }
            if kind == "EnergyReservoirStorage" {
                scale(&mut row["storage_capacity"], 1.0 / component_base);
            }
            if kind == "FixedAdmittance" {
                scale(&mut row["Y"], 1.0 / net.base_mva());
            }
            if kind == "TwoTerminalGenericHVDCLine" {
                let limit = &row["operational_flow_limit"];
                let lo = if number(limit, "to_from_max")? > 0.0 {
                    -number(limit, "to_from_max")?
                } else {
                    number(limit, "from_to_min")?
                };
                let hi = if number(limit, "from_to_max")? > 0.0 {
                    number(limit, "from_to_max")?
                } else {
                    -number(limit, "to_from_min")?
                };
                row["active_power_limits_from"] = minmax(lo, hi);
                let loss_base = if row["loss"]["power_units"] == "NATURAL_UNITS" {
                    1.0 / net.base_mva()
                } else {
                    1.0
                };
                let mut curve = row["loss"]["value_curve"].clone();
                scale(&mut curve["function_data"]["constant_term"], loss_base);
                let constant = number(&curve["function_data"], "constant_term")?;
                let rate = number(&curve["function_data"], "proportional_term")?;
                row["active_power_limits_to"] =
                    minmax(-hi * (1.0 - rate) + constant, -lo * (1.0 - rate) + constant);
                row["loss"] = curve;
                row.as_object_mut()
                    .expect("row")
                    .remove("operational_flow_limit");
                row.as_object_mut().expect("row").remove("rating");
            }
            if kind == "TransformerCircuit" {
                let parent = parents
                    .get(&id)
                    .ok_or_else(|| error("PowerSystems transformer circuit lacks parent"))?;
                native_kind = if optional(&row, "alpha", 0.0)? == 0.0 {
                    "TapTransformer"
                } else {
                    "PhaseShiftingTransformer"
                };
                row["name"] = parent["name"].clone();
                row["primary_shunt"] = parent["magnetizing_shunt"].clone();
                rename(&mut row, "alpha", "α");
                if native_kind == "PhaseShiftingTransformer" && row.get("α").is_none() {
                    row["α"] = json!(0.0);
                }
                row["active_power_flow"] = json!(optional(&row, "active_power_flow", 0.0)?);
                row["reactive_power_flow"] = json!(optional(&row, "reactive_power_flow", 0.0)?);
                if row.get("control_objective").is_none() {
                    row["control_objective"] = json!("UNDEFINED");
                }
                rename(&mut row, "tap_ratio_limits", "tap_limits");
                if let Some(band) = row.get("controlled_voltage_limits") {
                    row["voltage_setpoint"] =
                        json!(f64::midpoint(number(band, "min")?, number(band, "max")?));
                }
                if source.get("control_objective").is_some() {
                    output.push(&codes::EMIT_SIENNA.field_dropped, "PowerSystems native transformer control carries its objective and available actuator settings; additional target bounds remain in ext");
                }
            }
            if kind == "Arc" {
                rename(&mut row, "from_id", "from");
                rename(&mut row, "to_id", "to");
            }
            if kind == "Line" {
                row["active_power_flow"] = json!(optional(&row, "active_power_flow", 0.0)?);
                row["reactive_power_flow"] = json!(optional(&row, "reactive_power_flow", 0.0)?);
            }
            if matches!(kind.as_str(), "ThermalStandard" | "ThermalMultiStart") {
                row["status"] = json!(row["status"] == "ONLINE");
                if row.get("ramp_limits").is_none() {
                    row["ramp_limits"] = Value::Null;
                }
                if row.get("reactive_power_limits").is_none() {
                    row["reactive_power_limits"] = Value::Null;
                }
            }
            if let Some(cost) = row.get_mut("operation_cost") {
                encode_cost(cost)?;
            }
            if let Some(loss) = row.get_mut("loss") {
                encode_curve(loss);
            }
            for &field in REFERENCE_FIELDS {
                if let Some(target) = row.get(field).and_then(Value::as_i64) {
                    row[field] = json!({"value":refs.get(&target).ok_or_else(||error("missing PowerSystems reference"))?});
                }
            }
            row["internal"] = internal(&refs[&id]);
            row["ext"] = doc["ext"][id.to_string()]
                .as_object()
                .cloned()
                .map_or(json!({}), Value::Object);
            // PowerSystems components have no Sienna ids: a secondary-terminal
            // shunt names its transformer by the UUID that circuit is written with.
            if kind == "FixedAdmittance"
                && let Some(circuit) =
                    row["ext"][super::metadata::KEY]["transformer_circuit"].as_i64()
            {
                let uuid = refs.get(&circuit).ok_or_else(|| {
                    error("secondary shunt references a missing transformer circuit")
                })?;
                let extra = row["ext"][super::metadata::KEY]
                    .as_object_mut()
                    .expect("ext object");
                extra.remove("transformer_circuit");
                extra.insert("transformer_uuid".into(), json!(uuid));
            }
            if matches!(kind.as_str(), "Line" | "TransformerCircuit")
                && source.get("active_power_flow").is_none()
                && source.get("reactive_power_flow").is_none()
            {
                row["ext"][super::metadata::KEY]["branch_flow_absent"] = json!(true);
            }
            if kind == "SwitchedAdmittance" {
                row["ext"][super::metadata::KEY]["switched_control_fields"] = json!({
                    "control_mode": row["control_mode"],
                    "regulated_bus_number": row["regulated_bus_number"]
                });
            }
            if kind == "TransformerCircuit" {
                if let Some(band) = source.get("controlled_voltage_limits") {
                    let lo = number(band, "min")?;
                    let hi = number(band, "max")?;
                    let mid = lo.midpoint(hi);
                    row["ext"][super::metadata::KEY]["voltage_band_offsets"] =
                        minmax(lo - mid, hi - mid);
                }
                let mut extra = serde_json::Map::new();
                for field in [
                    "controlled_voltage_limits",
                    "controlled_reactive_power_flow_limits",
                    "controlled_active_power_flow_limits",
                    "number_of_tap_positions",
                    "regulated_bus_number",
                    "tap_ratio_limits",
                ] {
                    if let Some(value) = source.get(field) {
                        extra.insert(field.into(), value.clone());
                    }
                }
                row["ext"][super::metadata::KEY]["control_fields"] = Value::Object(extra);
            }
            for key in [
                "id",
                "power_units",
                "parameter_units",
                "admittance_units",
                "energy_units",
            ] {
                row.as_object_mut().expect("row").remove(key);
            }
            metadata(&mut row, native_kind, "PowerSystems");
            components.push(row);
        }
    }
    let mut root = json!({"data_format_version":"5.0.0","frequency":net.base_frequency(),"runchecks":true,"metadata":{"name":net.name(),"description":null},"units_settings":{"base_value":net.base_mva(),"unit_system":"SYSTEM_BASE"},"internal":internal(&uuid("system",net.name())),"data":{"components":components,"masked_components":[],"subsystems":{},"internal":internal(&uuid("data",net.name())),"time_series_compression_enabled":false,"time_series_in_memory":false}});
    if original.is_some() {
        output.push(&codes::EMIT_SIENNA.field_dropped,"PowerSystems fresh output contains the edited electrical model; external time-series, services and supplemental associations require separate emission");
    }
    metadata(
        &mut root["units_settings"],
        "SystemUnitsSettings",
        "InfrastructureSystems",
    );
    output.text = serde_json::to_string_pretty(&root).map_err(|e| error(e.to_string()))?;
    Ok(output)
}

// PowerSystems derives Arc names from endpoints, so parallel devices share one.
fn deduplicate_arcs(doc: &mut Value) -> Result<()> {
    let mut by_pair = BTreeMap::new();
    let mut replacements = BTreeMap::new();
    for row in rows(doc, "Arc")? {
        let id = integer(row, "id")?;
        let canonical = *by_pair
            .entry((integer(row, "from_id")?, integer(row, "to_id")?))
            .or_insert(id);
        replacements.insert(id, canonical);
    }
    for records in doc["components"]
        .as_object_mut()
        .expect("components")
        .values_mut()
    {
        for row in records
            .as_array_mut()
            .ok_or_else(|| error("component table must be an array"))?
        {
            for field in [
                "arc",
                "primary_star_arc",
                "secondary_star_arc",
                "tertiary_star_arc",
            ] {
                if let Some(id) = row.get(field).and_then(Value::as_i64)
                    && let Some(canonical) = replacements.get(&id)
                {
                    row[field] = json!(canonical);
                }
            }
        }
    }
    if let Some(arcs) = doc["components"]
        .get_mut("Arc")
        .and_then(Value::as_array_mut)
    {
        arcs.retain(|a| a["id"].as_i64().is_some_and(|id| replacements[&id] == id));
    }
    Ok(())
}

pub(super) fn clock_angle(row: &Value, field: &str) -> Result<f64> {
    let Some(group) = row.get(field).and_then(Value::as_str) else {
        return Ok(0.0);
    };
    if group == "UNDEFINED" {
        return Ok(0.0);
    }
    let clock = group
        .strip_prefix("GROUP_")
        .and_then(|s| s.parse::<u8>().ok())
        .filter(|n| *n <= 12)
        .ok_or_else(|| error("invalid winding clock group"))?;
    Ok(-f64::from(clock) * std::f64::consts::PI / 6.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_component_uuid_does_not_collide_with_a_retained_uuid() {
        let retained = uuid("component", "1");
        let doc = json!({"components":{"ACBus":[{"id":1},{"id":2}]},
            "ext":{"2":{"org.eigenergy.powerio":{"native_uuid":retained}}}});
        let refs = component_refs(&doc).unwrap();
        assert_eq!(refs[&2], retained);
        assert_ne!(refs[&1], refs[&2]);
        assert_eq!(refs, component_refs(&doc).unwrap());
    }

    #[test]
    fn duplicate_retained_uuids_are_rejected() {
        let doc = json!({"components":{"ACBus":[{"id":1},{"id":2}]},
            "ext":{"1":{"org.eigenergy.powerio":{"native_uuid":"same"}},
                "2":{"org.eigenergy.powerio":{"native_uuid":"same"}}}});
        assert!(component_refs(&doc).is_err());
    }
}

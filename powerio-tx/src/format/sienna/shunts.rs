use super::{
    BTreeMap, Bus, BusId, Diagnostics, Result, Shunt, Value, available, bus_ref, codes, error,
    integer, json, limits, metadata, minmax, number, optional, positive, uid,
};
use crate::network::{ShuntBlock, SwitchedShuntControl, SwitchedShuntMode};

pub(super) fn scale(row: &Value, kv: f64) -> Result<f64> {
    match row["admittance_units"].as_str().unwrap_or("COMPONENT_MVAR") {
        "COMPONENT_MVAR" => Ok(1.0),
        "NATURAL_UNITS" => Ok(positive(kv, "base_voltage")?.powi(2)),
        other => Err(error(format!("unsupported shunt admittance_units {other}"))),
    }
}
fn mode(tag: &str) -> Result<(SwitchedShuntMode, i64)> {
    Ok(match tag {
        "UNDEFINED" | "FIXED" => (SwitchedShuntMode::Locked, 0),
        "DISCRETE_VOLTAGE" => (SwitchedShuntMode::Discrete, 1),
        "CONTINUOUS_VOLTAGE" => (SwitchedShuntMode::Continuous, 2),
        "DISCRETE_REACTIVE_PLANT" => (SwitchedShuntMode::Discrete, 3),
        "DISCRETE_REACTIVE_VSC" => (SwitchedShuntMode::Discrete, 4),
        "DISCRETE_ADMITTANCE_REMOTE" => (SwitchedShuntMode::Discrete, 5),
        "DISCRETE_REACTIVE_FACTS" => (SwitchedShuntMode::Discrete, 6),
        _ => return Err(error(format!("unknown switched shunt control_mode {tag}"))),
    })
}
fn band(code: i64, version: u8) -> &'static str {
    if version == 1 {
        "admittance_limits"
    } else if code >= 3 {
        "reactive_power_range_limits"
    } else {
        "voltage_limits"
    }
}
pub(super) fn read(
    root: &Value,
    row: &Value,
    ids: &BTreeMap<i64, BusId>,
    buses: &BTreeMap<BusId, &Bus>,
    version: u8,
) -> Result<Shunt> {
    let bus = bus_ref(row, "bus", ids)?;
    let factor = scale(row, buses[&bus].base_kv)?;
    let steps = row["number_of_steps"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let values = row["Y_increase"].as_array().map_or(&[][..], Vec::as_slice);
    let engaged = row["number_engaged"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if steps.len() != values.len() || (!engaged.is_empty() && engaged.len() != steps.len()) {
        return Err(error("switched shunt block arrays have different lengths"));
    }
    let mut blocks = Vec::new();
    let (mut g, mut b) = (0.0, 0.0);
    for (i, (n, y)) in steps.iter().zip(values).enumerate() {
        let n = n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| error("shunt steps must be nonnegative integers"))?;
        let count = engaged
            .get(i)
            .map_or(Some(0), Value::as_u64)
            .filter(|c| *c <= u64::from(n))
            .ok_or_else(|| error("number_engaged exceeds available steps"))?;
        let block = ShuntBlock::with_admittance(
            n,
            number(y, "real")? * factor,
            number(y, "imag")? * factor,
        );
        #[allow(clippy::cast_precision_loss)] // count is bounded by a u32 step count.
        let count = count as f64;
        g += block.g * count;
        b += block.b * count;
        blocks.push(block);
    }
    b = optional(row, "solved_admittance", b / factor)? * factor;
    let mut shunt = Shunt::new(bus, g, b);
    shunt.uid = uid("SwitchedAdmittance", row)?;
    shunt.in_service = available(row)?;
    metadata::restore(root, row, &mut shunt, &["uid", "extras", "section_count"])?;
    let (mode, code) = mode(row["control_mode"].as_str().unwrap_or("FIXED"))?;
    let (low, high) = limits(row, band(code, version), (1.0, 1.0))?;
    let mut control = SwitchedShuntControl::new(mode, high, low, blocks);
    let remote = row
        .get("regulated_bus_number")
        .map_or(Ok(0), |_| integer(row, "regulated_bus_number"))?;
    if remote != 0 {
        let id = BusId(
            usize::try_from(remote).map_err(|_| error("negative shunt regulated_bus_number"))?,
        );
        if !buses.contains_key(&id) {
            return Err(error("unknown shunt regulated bus"));
        }
        control.control_bus = Some(id);
    }
    let extra = super::metadata::ext(root, integer(row, "id")?);
    control.rmpct = optional(extra, "rmpct", 100.0)?;
    if let Some(value) = extra.get("regulating_terminal") {
        control.regulating_terminal =
            serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
    }
    shunt.g = optional(extra, "solved_conductance", shunt.g)?;
    if code >= 3 {
        shunt.extras.insert("psse_modsw".into(), json!(code));
    } else {
        shunt.extras.remove("psse_modsw");
    }
    shunt.control = Some(control);
    Ok(shunt)
}
#[allow(clippy::float_cmp)] // Exact default determines whether participation metadata is needed.
pub(super) fn write(
    doc: &mut Value,
    id: i64,
    bus: i64,
    shunt: &Shunt,
    version: u8,
    warnings: &mut Diagnostics,
) -> Result<Value> {
    let control = shunt.control.as_ref().expect("switched shunt control");
    let code = match control.mode {
        SwitchedShuntMode::Locked => 0,
        SwitchedShuntMode::Continuous => 2,
        SwitchedShuntMode::Discrete => shunt
            .extras
            .get("psse_modsw")
            .and_then(Value::as_i64)
            .filter(|v| (3..=6).contains(v))
            .unwrap_or(1),
    };
    let tag = [
        "FIXED",
        "DISCRETE_VOLTAGE",
        "CONTINUOUS_VOLTAGE",
        "DISCRETE_REACTIVE_PLANT",
        "DISCRETE_REACTIVE_VSC",
        "DISCRETE_ADMITTANCE_REMOTE",
        "DISCRETE_REACTIVE_FACTS",
    ][usize::try_from(code).expect("control code")];
    metadata::record(doc, id, shunt, &["uid", "extras", "section_count"])?;
    let extra = super::metadata::ext_mut(doc, id);
    extra["rmpct"] = json!(control.rmpct);
    extra["regulating_terminal"] =
        serde_json::to_value(&control.regulating_terminal).map_err(|e| error(e.to_string()))?;
    extra["solved_conductance"] = json!(shunt.g);
    if shunt.g != 0.0 || control.rmpct != 100.0 || control.regulating_terminal.is_some() {
        warnings.push(&codes::EMIT_SIENNA.field_dropped,"switched shunt conductance, reactive participation, and exact regulating terminal require the PowerIO extension when supplied");
    }
    let mut row = json!({"id":id,"name":format!("shunt_{id}"),"bus":bus,"available":shunt.in_service,"admittance_units":"COMPONENT_MVAR","control_mode":tag,"regulated_bus_number":control.control_bus.map_or(0,|b|b.0),"number_of_steps":control.blocks.iter().map(|b|b.steps).collect::<Vec<_>>(),"number_engaged":vec![0;control.blocks.len()],"Y_increase":control.blocks.iter().map(|b|json!({"real":b.g,"imag":b.b})).collect::<Vec<_>>(),"solved_admittance":shunt.b});
    row[band(code, version)] = minmax(control.vlow, control.vhigh);
    Ok(row)
}

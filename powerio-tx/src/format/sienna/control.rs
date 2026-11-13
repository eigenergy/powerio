use super::{Result, Value, base, error, integer, json, limits, minmax, power_scale};
use crate::network::{BusId, TransformerControl, TransformerControlMode};

fn keys(mode: TransformerControlMode, version: u8) -> (&'static str, &'static str, bool) {
    let phase = matches!(
        mode,
        TransformerControlMode::ActiveFlow | TransformerControlMode::AsymmetricActiveFlow
    );
    if version == 1 {
        return ("control_limits", "controlled_quantity_limits", phase);
    }
    (
        if phase {
            "phase_angle_limits"
        } else {
            "tap_ratio_limits"
        },
        match mode {
            TransformerControlMode::ReactiveFlow => "controlled_reactive_power_flow_limits",
            TransformerControlMode::ActiveFlow
            | TransformerControlMode::AsymmetricActiveFlow
            | TransformerControlMode::DcLineQuantity => "controlled_active_power_flow_limits",
            _ => "controlled_voltage_limits",
        },
        phase,
    )
}

pub(super) fn read(row: &Value, version: u8) -> Result<Option<TransformerControl>> {
    let objective = row
        .get("control_objective")
        .and_then(Value::as_str)
        .unwrap_or("UNDEFINED");
    if objective == "UNDEFINED" {
        return Ok(None);
    }
    let enabled = !objective.ends_with("_DISABLED") && objective != "FIXED";
    let mode = match objective.strip_suffix("_DISABLED").unwrap_or(objective) {
        "FIXED" => TransformerControlMode::Fixed,
        "VOLTAGE" => TransformerControlMode::Voltage,
        "REACTIVE_POWER_FLOW" => TransformerControlMode::ReactiveFlow,
        "ACTIVE_POWER_FLOW" => TransformerControlMode::ActiveFlow,
        "CONTROL_OF_DC_LINE" => TransformerControlMode::DcLineQuantity,
        "ASYMMETRIC_ACTIVE_POWER_FLOW" => TransformerControlMode::AsymmetricActiveFlow,
        _ => {
            return Err(error(format!(
                "unknown transformer control objective {objective}"
            )));
        }
    };
    let mut control = TransformerControl::new(mode);
    control.enabled = enabled;
    let (actuator, target, phase) = keys(mode, version);
    // An absent actuator band keeps the control's own defaults, which a phase
    // shifter states in degrees; only a stated band is in radians.
    if row.get(actuator).is_some_and(|v| !v.is_null()) {
        (control.tap_min, control.tap_max) = limits(row, actuator, (0.9, 1.1))?;
        if phase {
            control.tap_min = control.tap_min.to_degrees();
            control.tap_max = control.tap_max.to_degrees();
        }
    }
    (control.band_min, control.band_max) = limits(row, target, (0.9, 1.1))?;
    if version >= 2
        && !matches!(
            mode,
            TransformerControlMode::Voltage | TransformerControlMode::Fixed
        )
    {
        let scale = power_scale(row)?;
        control.band_min *= scale;
        control.band_max *= scale;
    }
    let bus = if row.get("regulated_bus_number").is_some() {
        integer(row, "regulated_bus_number")?
    } else {
        0
    };
    if bus != 0 {
        control.controlled_bus = Some(BusId(
            usize::try_from(bus.unsigned_abs())
                .map_err(|_| error("regulated bus number out of range"))?,
        ));
        control.controlled_bus_on_winding_side = bus < 0;
    }
    control.ntp = if row.get("number_of_tap_positions").is_some() {
        u32::try_from(integer(row, "number_of_tap_positions")?)
            .map_err(|_| error("invalid number_of_tap_positions"))?
    } else {
        33
    };
    control.mva_base = base(row)?;
    Ok(Some(control))
}

pub(super) fn write(row: &mut Value, control: &TransformerControl, version: u8) -> Result<()> {
    let objective = match control.mode {
        TransformerControlMode::Fixed => "FIXED",
        TransformerControlMode::Voltage => "VOLTAGE",
        TransformerControlMode::ReactiveFlow => "REACTIVE_POWER_FLOW",
        TransformerControlMode::ActiveFlow => "ACTIVE_POWER_FLOW",
        TransformerControlMode::DcLineQuantity => "CONTROL_OF_DC_LINE",
        TransformerControlMode::AsymmetricActiveFlow => "ASYMMETRIC_ACTIVE_POWER_FLOW",
    };
    row["control_objective"] = json!(if !control.enabled
        && control.mode != TransformerControlMode::Fixed
    {
        format!("{objective}_DISABLED")
    } else {
        objective.into()
    });
    let (actuator, target, phase) = keys(control.mode, version);
    row[actuator] = if phase {
        minmax(control.tap_min.to_radians(), control.tap_max.to_radians())
    } else {
        minmax(control.tap_min, control.tap_max)
    };
    row[target] = minmax(control.band_min, control.band_max);
    row["number_of_tap_positions"] = json!(control.ntp);
    let number = control.controlled_bus.map_or(Ok(0), |bus| {
        i64::try_from(bus.0).map_err(|_| error("regulated bus number out of range"))
    })?;
    row["regulated_bus_number"] = json!(if control.controlled_bus_on_winding_side {
        -number
    } else {
        number
    });
    Ok(())
}

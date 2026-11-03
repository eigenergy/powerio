//! Internal battery attached to a constant-PF inverter: no DC input is supplied.
use super::Reader;
use crate::glm::{GlmQuantityKind, ObjectKey};
use powerio_core::{Diagnostic, Error};

pub(super) fn quantity_spec(
    class: &str,
    name: &str,
) -> Option<(GlmQuantityKind, Option<&'static str>)> {
    if class != "battery" {
        return None;
    }
    let unit = match name {
        "nominal_voltage" => "V",
        "rated_power" => "W",
        "battery_capacity" => "Wh",
        "round_trip_efficiency" | "state_of_charge" => "pu",
        _ => return None,
    };
    Some((GlmQuantityKind::Real, Some(unit)))
}
pub(super) fn allowed(class: &str, name: &str) -> bool {
    class == "battery"
        && (quantity_spec(class, name).is_some()
            || matches!(name, "use_internal_battery_model" | "battery_type"))
}
impl Reader<'_> {
    pub(super) fn idle_battery(&mut self, key: ObjectKey) -> Result<(), Error> {
        if !self
            .text(key, "use_internal_battery_model")?
            .is_some_and(|v| matches!(v.as_str(), "true" | "TRUE" | "1"))
            || self.text(key, "battery_type")?.as_deref() != Some("LI_ION")
        {
            return Err(self.error(key, "battery snapshot requires the internal LI_ION model; legacy and other battery profiles need additional mapping"));
        }
        for field in ["nominal_voltage", "rated_power", "battery_capacity"] {
            if !self.real(key, field).is_some_and(|v| v > 0.0) {
                return Err(self.error(key, format!("battery requires explicit positive {field}")));
            }
        }
        if !self
            .real(key, "state_of_charge")
            .is_some_and(|v| (0.0..=1.0).contains(&v))
            || !self
                .real(key, "round_trip_efficiency")
                .is_some_and(|v| v > 0.0 && v <= 1.0)
        {
            return Err(self.error(key, "battery requires explicit state_of_charge in 0..1 and round_trip_efficiency in (0,1]"));
        }
        // Both pinned internal models consume their inverter's power_in; they
        // do not set V_In/I_In. CONSTANT_PF therefore initializes at zero, even
        // when P_Out requests charging. No time integration is represented.
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_BATTERY_IDLE,
            format!("battery {}: internal model supplies no DC input to CONSTANT_PF; fixed AC injection is zero; declared inverter P_Out/Q_Out, load-following thresholds, energy capacity, state of charge and DC parameters remain source-only", self.names[&key])));
        Ok(())
    }
}

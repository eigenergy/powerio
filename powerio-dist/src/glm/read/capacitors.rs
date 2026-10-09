//! Fixed wye capacitor coils represented by their terminal admittance.
use super::{Reader, allocate_name, class, parse_phases, phase_name};
use crate::{DistLocationTarget, DistShunt};
use powerio_core::{Diagnostic, Error};

pub(super) fn allowed(class: &str, name: &str) -> bool {
    class == "capacitor"
        && (matches!(
            name,
            "phases_connected"
                | "control"
                | "control_level"
                | "pt_phase"
                | "remote_sense"
                | "remote_sense_B"
                | "switchA"
                | "switchB"
                | "switchC"
        ) || unit(class, name).is_some())
}

pub(super) fn unit(class: &str, name: &str) -> Option<&'static str> {
    match (class, name) {
        (
            "capacitor",
            "cap_nominal_voltage" | "voltage_set_high" | "voltage_set_low" | "voltage_deadband",
        ) => Some("V"),
        (
            "capacitor",
            "VAr_set_high" | "VAr_set_low" | "VAr_deadband" | "capacitor_A" | "capacitor_B"
            | "capacitor_C",
        ) => Some("VAr"),
        ("capacitor", "current_set_high" | "current_set_low" | "current_deadband") => Some("A"),
        ("capacitor", "time_delay" | "dwell_time" | "lockout_time") => Some("s"),
        _ => None,
    }
}

impl Reader<'_> {
    pub(super) fn capacitors(&mut self) -> Result<(), Error> {
        let keys = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "capacitor")
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        for key in keys {
            let root = self.roots[&key];
            let declared = self.text(key, "phases")?.unwrap_or_default();
            let phases = parse_phases(&declared).map_err(|m| self.error(key, m))?;
            self.capacitor_control(key, &phases)?;
            let connected = self.text(key, "phases_connected")?.unwrap_or(declared);
            let connected = parse_phases(&connected).map_err(|m| self.error(key, m))?;
            if connected.iter().any(|p| !phases.contains(p)) {
                return Err(self.error(key, "capacitor connection is outside its node phases"));
            }
            let voltage = self
                .real(key, "cap_nominal_voltage")
                .filter(|v| *v != 0.0)
                // Native node initialization copies the parent's nominal
                // voltage before the capacitor derives its omitted rating.
                .unwrap_or(self.nominal[&root]);
            if voltage <= 0.0 {
                return Err(self.error(key, "capacitor nominal voltage must be positive"));
            }
            let single = phases.len() == 1 && key != root;
            for phase in phases {
                let p = phase_name(phase);
                let closed = match self.text(key, &format!("switch{p}"))?.as_deref() {
                    None | Some("OPEN") => false,
                    Some("CLOSED") => true,
                    _ => {
                        return Err(
                            self.error(key, "capacitor switch state must be OPEN or CLOSED")
                        );
                    }
                };
                let rating = self.real(key, &format!("capacitor_{p}")).unwrap_or(0.0);
                if rating < 0.0 {
                    return Err(self.error(key, "negative capacitor ratings are not qualified"));
                }
                let b = if closed && connected.contains(&phase) {
                    rating / voltage / voltage
                } else {
                    0.0
                };
                if !b.is_finite()
                    || (closed && connected.contains(&phase) && rating > 0.0 && b == 0.0)
                {
                    return Err(self.error(key, "capacitor admittance overflows or underflows"));
                }
                let name = if single {
                    self.names[&key].clone()
                } else {
                    allocate_name(
                        &mut self.identities,
                        &format!("{}__glm_{p}", self.names[&key]),
                    )
                };
                self.net.shunts_mut().push(DistShunt::new(
                    &name,
                    &self.names[&root],
                    vec![(phase + 1).to_string()],
                    vec![vec![0.0]],
                    vec![vec![b]],
                ));
                self.attach(key, DistLocationTarget::Shunt, name);
            }
            self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_SOURCE_ONLY,
                format!("capacitor {}: typed phase shunts preserve fixed admittance; bank ratings and switch declarations remain in source", self.names[&key])));
        }
        Ok(())
    }
}

impl Reader<'_> {
    fn capacitor_control(
        &mut self,
        key: crate::glm::ObjectKey,
        phases: &[usize],
    ) -> Result<(), Error> {
        let control = self
            .text(key, "control")?
            .unwrap_or_else(|| "MANUAL".into());
        if self.real(key, "nominal_voltage").is_some_and(|v| v <= 0.0) {
            return Err(self.error(key, "capacitor node nominal voltage must be positive"));
        }
        if self.text(key, "control_level")?.as_deref() == Some("BANK")
            && control == "MANUAL"
            && phases.len() == 1
            && self
                .text(key, "pt_phase")?
                .is_some_and(|v| v == phase_name(phases[0]).to_string())
        {
            // A one-phase bank sensing that phase cannot change its declared
            // switch state during native bank initialization.
            return Ok(());
        }
        if !matches!(
            self.text(key, "control_level")?.as_deref(),
            None | Some("INDIVIDUAL")
        ) {
            return Err(self.error(
                key,
                "banked capacitor initialization requires a qualified snapshot",
            ));
        }
        if control == "MANUAL" {
            return Ok(());
        }
        if !matches!(control.as_str(), "VAR" | "VOLT" | "VARVOLT" | "CURRENT") {
            return Err(self.error(key, "unknown capacitor control"));
        }
        for &phase in phases {
            if self
                .text(key, &format!("switch{}", phase_name(phase)))?
                .is_none()
            {
                return Err(self.error(
                    key,
                    "active capacitor control requires an explicit switch state on every phase",
                ));
            }
        }
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_CONTROL_FROZEN,
            format!("capacitor {}: preserve declared phase switch states; {control} control execution and sensing are excluded from this static snapshot", self.names[&key])));
        Ok(())
    }
}

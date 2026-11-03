//! Standalone static sequence-voltage sources in both pinned forks.
use super::{Reader, class};
use crate::glm::{GlmQuantityKind, ObjectKey};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

pub(super) fn quantity_spec(class: &str, name: &str) -> Option<(GlmQuantityKind, &'static str)> {
    if class != "substation" {
        return None;
    }
    match name {
        "zero_sequence_voltage" | "positive_sequence_voltage" | "negative_sequence_voltage" => {
            Some((GlmQuantityKind::Complex, "V"))
        }
        "base_power" | "power_convergence_value" => Some((GlmQuantityKind::Real, "VA")),
        _ => None,
    }
}
pub(super) fn allowed(class: &str, name: &str) -> bool {
    class == "substation" && (name == "reference_phase" || quantity_spec(class, name).is_some())
}
impl Reader<'_> {
    pub(super) fn substation_voltages(
        &mut self,
        key: ObjectKey,
    ) -> Result<Option<[Complex64; 3]>, Error> {
        if class(&self.index.objects()[key.ordinal()]) != "substation" {
            return Ok(None);
        }
        if self.index.parent(key).is_some() {
            return Err(self.error(
                key,
                "parented substation coupling requires an additional static mapping",
            ));
        }
        let angle = match self
            .text(key, "reference_phase")?
            .as_deref()
            .unwrap_or("PHASE_A")
        {
            "PHASE_A" => 0.0,
            "PHASE_B" => 2.0 * std::f64::consts::PI / 3.0,
            "PHASE_C" => -2.0 * std::f64::consts::PI / 3.0,
            _ => {
                return Err(self.error(
                    key,
                    "substation reference_phase must be PHASE_A, PHASE_B or PHASE_C",
                ));
            }
        };
        let seq = [
            "zero_sequence_voltage",
            "positive_sequence_voltage",
            "negative_sequence_voltage",
        ]
        .map(|n| self.quantity(key, n).unwrap_or_default());
        if self.real(key, "base_power").is_some()
            || self.real(key, "power_convergence_value").is_some()
        {
            self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_SOURCE_ONLY,
                format!("substation {}: base_power and power_convergence_value govern external parent power reporting; standalone source voltages are unchanged; these settings remain in source", self.names[&key])));
        }
        if seq.iter().all(|v| *v == Complex64::default()) {
            return Ok(None);
        }
        if self.text(key, "bustype")?.as_deref() != Some("SWING") || self.phases[&key] != [0, 1, 2]
        {
            return Err(self.error(
                key,
                "sequence-voltage substation requires an explicit three-phase SWING bus",
            ));
        }
        let a = Complex64::from_polar(1.0, 2.0 * std::f64::consts::PI / 3.0);
        let rotation = Complex64::from_polar(1.0, angle);
        let values = [
            seq[0] + seq[1] + seq[2],
            seq[0] + a.conj() * seq[1] + a * seq[2],
            seq[0] + a * seq[1] + a.conj() * seq[2],
        ]
        .map(|v| v * rotation);
        if values
            .iter()
            .any(|v| !v.re.is_finite() || !v.im.is_finite())
        {
            return Err(self.error(key, "substation sequence-to-phase conversion overflows"));
        }
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_SUBSTATION_EQUIVALENT,
            format!("substation {}: static sequence voltages and reference phase determine the fixed phase voltages, overriding phase voltage declarations", self.names[&key])));
        Ok(Some(values))
    }
}

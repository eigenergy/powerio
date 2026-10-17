//! Ordinary transformer coils on the existing winding model.
use super::{Reader, class, parse_phases};
use crate::glm::GlmQuantityKind;
use crate::{DistLocationTarget, DistTransformer, DistWinding, DistWindingConn};
use powerio_core::Error;

pub(super) fn allowed(class: &str, name: &str) -> bool {
    match class {
        "transformer" => matches!(name, "phases" | "from" | "to" | "configuration"),
        "transformer_configuration" => {
            matches!(name, "connect_type" | "install_type") || quantity_spec(class, name).is_some()
        }
        _ => false,
    }
}

pub(super) fn quantity_spec(class: &str, name: &str) -> Option<(GlmQuantityKind, &'static str)> {
    use GlmQuantityKind::{Complex, Real};
    if class != "transformer_configuration" {
        return None;
    }
    match name {
        "primary_voltage" | "secondary_voltage" => Some((Real, "V")),
        "power_rating" | "powerA_rating" | "powerB_rating" | "powerC_rating" => Some((Real, "kVA")),
        "resistance" | "reactance" => Some((Real, "pu*Ohm")),
        "impedance" => Some((Complex, "pu*Ohm")),
        _ => None,
    }
}

impl Reader<'_> {
    pub(super) fn transformers(&mut self) -> Result<(), Error> {
        let keys = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "transformer")
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        for key in keys {
            let cfg = self.geometry_reference(key, "configuration", "transformer_configuration")?;
            let phases = parse_phases(
                self.text(key, "phases")?
                    .as_deref()
                    .ok_or_else(|| self.error(key, "transformer requires explicit phases"))?,
            )
            .map_err(|m| self.error(key, m))?;
            let connection = self.text(cfg, "connect_type")?.unwrap_or_default();
            if !matches!(
                (connection.as_str(), phases.len()),
                ("WYE_WYE" | "1" | "DELTA_DELTA" | "2", 3) | ("SINGLE_PHASE", 1)
            ) {
                return Err(self.error(
                    key,
                    "transformer connection/phase profile is not qualified yet",
                ));
            }
            if !self.newton_raphson && matches!(connection.as_str(), "DELTA_DELTA" | "2") {
                return Err(self.error(
                    key,
                    "FBS delta transformer equations need a separate mapping",
                ));
            }
            let primary = self.positive_geometry(cfg, "primary_voltage")?;
            let secondary = self.positive_geometry(cfg, "secondary_voltage")?;
            let rating = self.transformer_rating(cfg)?;
            let impedance = self.quantity(cfg, "impedance").ok_or_else(|| {
                self.error(
                    cfg,
                    "transformer requires positive series resistance and reactance",
                )
            })?;
            if impedance.re <= 0.0 || impedance.im <= 0.0 {
                return Err(self.error(
                    cfg,
                    "native transformer requires positive series resistance and reactance",
                ));
            }
            let mut windings = Vec::new();
            for (field, voltage, resistance) in [
                ("from", primary, 0.0),
                ("to", secondary, 100.0 * impedance.re),
            ] {
                let object = self
                    .index
                    .resolve_property(key, field)?
                    .ok_or_else(|| self.error(key, format!("transformer requires {field}")))?;
                let root = self.roots.get(&object).ok_or_else(|| {
                    self.error(key, "transformer endpoint is not an electrical bus")
                })?;
                if phases.iter().any(|p| !self.phases[root].contains(p)) {
                    return Err(self.error(key, "transformer phase is absent at an endpoint"));
                }
                let mut terminals = phases
                    .iter()
                    .map(|p| (p + 1).to_string())
                    .collect::<Vec<_>>();
                terminals.push("n".into());
                let mut winding = DistWinding::new(
                    &self.names[root],
                    terminals,
                    DistWindingConn::Wye,
                    voltage,
                    rating,
                );
                winding.r_pct = resistance;
                windings.push(winding);
            }
            let name = self.names[&key].clone();
            let mut transformer =
                DistTransformer::new(&name, windings, vec![100.0 * impedance.im], phases.len());
            // Native ordinary transformers have no artificial grounding
            // admittance. Prevent OpenDSS's default anti-floating shunts from
            // adding a reactive current to this circuit on cross emission.
            transformer
                .extras
                .insert("ppm_antifloat".into(), serde_json::json!(0.0));
            if !self.newton_raphson {
                self.default_fbs_core(cfg, &mut transformer)?;
            }
            if matches!(connection.as_str(), "DELTA_DELTA" | "2") {
                // Both pinned NR engines stamp three independent grounded
                // branches here, not a physical delta winding primitive.
                // The effective circuit lives in typed windings; extras retain
                // only the source declaration's provenance across IR.
                transformer.extras.insert(
                    "gridlabd_declared_connection".into(),
                    serde_json::json!("DELTA_DELTA"),
                );
                self.diagnostics.push(powerio_core::Diagnostic::of(
                    &crate::diagnostics::codes::READ_GLM_TRANSFORMER_EQUIVALENT,
                    format!("{name}: native NR DELTA_DELTA uses grounded-wye port equations, including zero-sequence transfer; winding records preserve that engine behavior, not physical delta isolation"),
                ));
            }
            self.net.transformers_mut().push(transformer);
            self.attach(key, DistLocationTarget::Transformer, name);
        }
        Ok(())
    }
}

impl Reader<'_> {
    fn default_fbs_core(
        &self,
        cfg: crate::glm::ObjectKey,
        transformer: &mut DistTransformer,
    ) -> Result<(), Error> {
        // FBS includes the native finite default core at the secondary
        // terminals, after the series impedance. NR omits this branch.
        let secondary = &transformer.windings[1];
        let g = secondary.s_rating / secondary.v_ref.powi(2) / 999_999_999.0;
        if !g.is_finite() || g <= 0.0 {
            return Err(self.error(cfg, "FBS core admittance overflows or underflows"));
        }
        transformer.extras.insert(
            "no_load_shunt".into(),
            serde_json::json!({"winding": 2, "g": g, "b": -g}),
        );
        Ok(())
    }

    fn transformer_rating(&mut self, cfg: crate::glm::ObjectKey) -> Result<f64, Error> {
        let mut phase_sum = 0.0;
        let mut declared = false;
        for field in ["powerA_rating", "powerB_rating", "powerC_rating"] {
            if let Some(value) = self.real(cfg, field) {
                if value < 0.0 {
                    return Err(self.error(cfg, "negative transformer phase rating is unsupported"));
                }
                declared = true;
                phase_sum += value;
            }
        }
        let total = self
            .real(cfg, "power_rating")
            .filter(|v| *v != 0.0)
            .unwrap_or(phase_sum);
        if !total.is_finite() || total <= 0.0 || !(total * 1000.0).is_finite() {
            return Err(self.error(cfg, "transformer requires a positive finite total rating"));
        }
        if phase_sum != 0.0 && ((total - phase_sum) / total).abs() > 0.01 {
            return Err(self.error(
                cfg,
                "native transformer total/phase rating mismatch exceeds 1%",
            ));
        }
        if declared {
            self.diagnostics.push(powerio_core::Diagnostic::of(
                &crate::diagnostics::codes::READ_GLM_SOURCE_ONLY,
                format!("{}: ordinary transformer impedance uses total power_rating; individual phase nameplate ratings remain in source", self.names[&cfg]),
            ));
        }
        Ok(total * 1000.0)
    }
}

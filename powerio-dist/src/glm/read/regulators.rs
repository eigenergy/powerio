//! Declared regulator tap states; this module never executes controls.
use super::{Reader, allocate_name, class, parse_phases, phase_name};
use crate::diagnostics::codes;
use crate::glm::{GlmQuantityKind, ObjectKey};
use crate::{DistLocationTarget, DistTransformer, DistWinding, DistWindingConn};
use powerio_core::{Diagnostic, Error};

pub(super) fn allowed(class: &str, name: &str) -> bool {
    match class {
        "regulator" => {
            matches!(
                name,
                "phases" | "from" | "to" | "configuration" | "tap_A" | "tap_B" | "tap_C"
            ) || quantity_spec(class, name).is_some()
        }
        "regulator_configuration" => {
            matches!(
                name,
                "connect_type"
                    | "Control"
                    | "Type"
                    | "raise_taps"
                    | "lower_taps"
                    | "tap_pos_A"
                    | "tap_pos_B"
                    | "tap_pos_C"
                    | "CT_phase"
                    | "PT_phase"
                    | "control_level"
                    | "reverse_flow_control"
            ) || quantity_spec(class, name).is_some()
        }
        _ => false,
    }
}

pub(super) fn quantity_spec(
    class: &str,
    name: &str,
) -> Option<(GlmQuantityKind, Option<&'static str>)> {
    // Native regulation has no published unit; a suffix is not a percent conversion.
    if class == "regulator_configuration" && name == "regulation" {
        return Some((GlmQuantityKind::Real, None));
    }
    let unit = match (class, name) {
        ("regulator", "regulator_resistance") => "Ohm",
        ("regulator_configuration", "current_transducer_ratio" | "power_transducer_ratio") => "pu",
        (
            "regulator_configuration",
            "band_center"
            | "band_width"
            | "compensator_r_setting_A"
            | "compensator_r_setting_B"
            | "compensator_r_setting_C"
            | "compensator_x_setting_A"
            | "compensator_x_setting_B"
            | "compensator_x_setting_C",
        ) => "V",
        ("regulator_configuration", "time_delay" | "dwell_time") => "s",
        _ => return None,
    };
    Some((GlmQuantityKind::Real, Some(unit)))
}

impl Reader<'_> {
    fn regulator_integer(&self, key: ObjectKey, field: &str) -> Result<Option<i16>, Error> {
        self.text(key, field)?
            .map(|v| {
                v.parse::<i16>().map_err(|_| {
                    self.error(key, format!("{field} requires a signed 16-bit integer"))
                })
            })
            .transpose()
    }

    pub(super) fn regulators(&mut self) -> Result<(), Error> {
        let keys = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "regulator")
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        for key in keys {
            self.regulator(key)?;
        }
        Ok(())
    }

    fn regulator(&mut self, key: ObjectKey) -> Result<(), Error> {
        let cfg = self.geometry_reference(key, "configuration", "regulator_configuration")?;
        if !matches!(
            self.text(cfg, "connect_type")?.as_deref(),
            Some("WYE_WYE" | "1")
        ) {
            return Err(self.error(cfg, "only WYE_WYE regulators are qualified"));
        }
        if !matches!(
            self.text(cfg, "control_level")?.as_deref(),
            None | Some("INDIVIDUAL")
        ) {
            return Err(self.error(cfg, "banked regulator initialization is not qualified"));
        }
        let control = self
            .text(cfg, "Control")?
            .unwrap_or_else(|| "MANUAL".into());
        if !matches!(
            control.as_str(),
            "MANUAL" | "OUTPUT_VOLTAGE" | "LINE_DROP_COMP" | "REMOTE_NODE"
        ) {
            return Err(self.error(cfg, "unknown regulator control"));
        }
        let kind = self.text(cfg, "Type")?.unwrap_or_else(|| "B".into());
        if !matches!(kind.as_str(), "A" | "B") {
            return Err(self.error(cfg, "regulator Type must be A or B"));
        }
        let raise = self.regulator_integer(cfg, "raise_taps")?.unwrap_or(0);
        let lower = self.regulator_integer(cfg, "lower_taps")?.unwrap_or(0);
        if raise <= 0 || lower <= 0 {
            return Err(self.error(cfg, "raise_taps and lower_taps must be positive"));
        }
        let step = self.real(cfg, "regulation").unwrap_or(0.0) / f64::from(raise);
        let declared_r = self.real(key, "regulator_resistance").unwrap_or(-1.0);
        let resistance = if !self.newton_raphson {
            0.0
        } else if declared_r < 0.0 {
            self.switch_default_resistance
        } else {
            declared_r
        };
        if self.newton_raphson && resistance <= 0.0 {
            return Err(self.error(
                key,
                "zero regulator_resistance produces a singular native NR circuit",
            ));
        }
        let phases = parse_phases(
            self.text(key, "phases")?
                .as_deref()
                .ok_or_else(|| self.error(key, "regulator requires explicit phases"))?,
        )
        .map_err(|m| self.error(key, m))?;
        let mut endpoints = Vec::new();
        for field in ["from", "to"] {
            let object = self
                .index
                .resolve_property(key, field)?
                .ok_or_else(|| self.error(key, format!("missing regulator {field}")))?;
            let root = *self
                .roots
                .get(&object)
                .ok_or_else(|| self.error(key, "regulator endpoint must be a bus"))?;
            if self.triplex.contains_key(&root)
                || phases.iter().any(|p| !self.phases[&root].contains(p))
            {
                return Err(self.error(key, "regulator phase absent at endpoint"));
            }
            endpoints.push(root);
        }
        let nominal = self.nominal[&endpoints[0]];
        let r_pct = 100.0 * resistance / nominal.powi(2);
        if !r_pct.is_finite() || (resistance > 0.0 && r_pct == 0.0) {
            return Err(self.error(
                key,
                "regulator winding normalization overflows or underflows",
            ));
        }
        for phase in phases.iter().copied() {
            let ratio = self.regulator_ratio(key, cfg, phase, &control, &kind, step)?;
            self.regulator_phase(
                key,
                &endpoints,
                phase,
                nominal,
                ratio,
                resistance,
                phases.len() == 1,
            );
        }
        let circuit = if self.newton_raphson {
            "NR series resistance"
        } else {
            "FBS ideal constraint (declared resistance is unused)"
        };
        self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_REGULATOR_EQUIVALENT, format!("regulator {}: phase windings preserve declared fixed ratios and {resistance} ohm {circuit}; 1 VA is a normalization base, not a source nameplate rating; control configuration remains in source",self.names[&key])));
        if control != "MANUAL" {
            self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_CONTROL_FROZEN,format!("regulator {}: {control} execution is excluded; stated taps are used without settling",self.names[&key])));
        }
        Ok(())
    }

    fn regulator_ratio(
        &self,
        key: ObjectKey,
        cfg: ObjectKey,
        phase: usize,
        control: &str,
        kind: &str,
        step: f64,
    ) -> Result<f64, Error> {
        let p = phase_name(phase);
        let object_tap = self
            .regulator_integer(key, &format!("tap_{p}"))?
            .filter(|t| *t != -999);
        let config_tap = self
            .regulator_integer(cfg, &format!("tap_pos_{p}"))?
            .filter(|t| *t != 999);
        let tap = object_tap.or(config_tap);
        if tap.is_none() && control != "MANUAL" {
            return Err(self.error(
                key,
                "active regulator requires an explicitly declared fixed tap on every active phase",
            ));
        }
        let amount = f64::from(tap.unwrap_or(0)) * step;
        let ratio = if kind == "A" {
            1.0 / (1.0 + amount)
        } else {
            1.0 - amount
        };
        if !ratio.is_finite() || ratio <= 0.0 {
            return Err(self.error(
                key,
                "regulator tap produces a nonpositive or nonfinite turns ratio",
            ));
        }
        Ok(ratio)
    }

    #[allow(clippy::too_many_arguments)] // One phase of the already validated regulator.
    fn regulator_phase(
        &mut self,
        key: ObjectKey,
        endpoints: &[ObjectKey],
        phase: usize,
        nominal: f64,
        ratio: f64,
        resistance: f64,
        single: bool,
    ) {
        let name = if single {
            self.names[&key].clone()
        } else {
            allocate_name(
                &mut self.identities,
                &format!("{}__{}", self.names[&key], phase_name(phase)),
            )
        };
        let terminals = vec![(phase + 1).to_string(), "n".into()];
        let mut primary = DistWinding::new(
            &self.names[&endpoints[0]],
            terminals.clone(),
            DistWindingConn::Wye,
            nominal,
            1.0,
        );
        let mut secondary = DistWinding::new(
            &self.names[&endpoints[1]],
            terminals,
            DistWindingConn::Wye,
            nominal,
            1.0,
        );
        primary.tap = ratio;
        secondary.r_pct = 100.0 * resistance / nominal.powi(2);
        let mut t = DistTransformer::new(&name, vec![primary, secondary], vec![0.0], 1);
        t.extras
            .insert("ppm_antifloat".into(), serde_json::json!(0.0));
        // Keep the absence of a nameplate rating with the value across IR,
        // even when module diagnostics/source-default records are unavailable.
        t.extras.insert(
            "rating_is_normalization_base".into(),
            serde_json::json!(true),
        );
        self.net.transformers_mut().push(t);
        self.net
            .defaulted_mut()
            .entry(format!("transformer.{name}"))
            .or_default()
            .push("s_rating: 1 VA normalization base; no source nameplate rating");
        self.attach(key, DistLocationTarget::Transformer, name);
    }
}

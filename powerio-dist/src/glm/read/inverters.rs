//! Fixed four-quadrant inverter dispatch in the pinned profiles.
use super::{Reader, allocate_name, class, parse_phases, phase_name};
use crate::glm::{GlmQuantityKind, ObjectKey};
use crate::{DistIbr, DistLocationTarget, IbrPrimeMover, IbrTopology};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

pub(super) fn quantity_spec(
    class: &str,
    name: &str,
) -> Option<(GlmQuantityKind, Option<&'static str>)> {
    if class != "inverter" {
        return None;
    }
    let unit = match name {
        "rated_power" | "P_Out" => Some("VA"),
        "Q_Out" => Some("VAr"),
        "V_base" => Some("V"),
        "power_factor" => Some("unit"),
        "inverter_efficiency" => None,
        "max_charge_rate"
        | "max_discharge_rate"
        | "charge_on_threshold"
        | "charge_off_threshold"
        | "discharge_on_threshold"
        | "discharge_off_threshold" => Some("W"),
        "charge_lockout_time" | "discharge_lockout_time" => Some("s"),
        _ => return None,
    };
    Some((GlmQuantityKind::Real, unit))
}
pub(super) fn allowed(class: &str, name: &str) -> bool {
    class == "inverter"
        && (quantity_spec(class, name).is_some()
            || matches!(
                name,
                "phases"
                    | "generator_status"
                    | "generator_mode"
                    | "inverter_type"
                    | "use_multipoint_efficiency"
                    | "four_quadrant_control_mode"
                    | "sense_object"
            ))
}
impl Reader<'_> {
    pub(super) fn fixed_inverters(&mut self) -> Result<(), Error> {
        let keys = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "inverter")
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        for key in keys {
            self.fixed_inverter(key)?;
        }
        for object in self
            .index
            .objects()
            .iter()
            .filter(|o| matches!(class(o), "solar" | "battery"))
        {
            if self
                .index
                .parent(object.key())
                .is_none_or(|k| class(&self.index.objects()[k.ordinal()]) != "inverter")
            {
                return Err(self.error(
                    object.key(),
                    "DC device requires a supported inverter parent",
                ));
            }
        }
        Ok(())
    }
    fn fixed_inverter(&mut self, key: ObjectKey) -> Result<(), Error> {
        let online = self.fixed_inverter_mode(key)?;
        let (dc, prime_mover, dc_power) = self.inverter_input(key)?;
        let maps = self.inverter_terminals(key)?;
        let parent = self.index.parent(key).expect("parent validated");
        let root = self.roots[&parent];
        let eta = self
            .real(key, "inverter_efficiency")
            .filter(|v| *v != 0.0)
            .unwrap_or(0.9);
        if eta <= 0.0 || eta > 1.0 {
            return Err(self.error(
                key,
                "inverter efficiency must lie in (0,1] after native defaults",
            ));
        }
        let rating = self
            .real(key, "rated_power")
            .filter(|v| *v != 0.0)
            .unwrap_or(25000.0);
        if rating <= 0.0 {
            return Err(self.error(key, "inverter rated_power must be positive"));
        }
        let available = dc_power * eta;
        let pf = self.real(key, "power_factor").unwrap_or(1.0);
        if !(-1.0..=1.0).contains(&pf) {
            return Err(self.error(key, "inverter power_factor must lie in -1..1"));
        }
        let mut power = if pf == 0.0 {
            Complex64::new(0.0, available)
        } else {
            Complex64::new(
                available,
                -(available / pf.abs() * (1.0 - pf * pf).sqrt()).copysign(pf),
            )
        };
        let maximum = rating * maps.len() as f64;
        if power.norm() > maximum {
            power = Complex64::new(
                (maximum * pf).abs(),
                (maximum * (1.0 - pf * pf).sqrt()).copysign(power.im),
            );
        }
        if !online {
            power = Complex64::default();
        }
        if !available.is_finite()
            || !maximum.is_finite()
            || !power.re.is_finite()
            || !power.im.is_finite()
        {
            return Err(self.error(key, "inverter initialized power overflows"));
        }
        let original = self.names[&key].clone();
        let n = maps.len() as f64;
        for (suffix, map) in maps {
            let name = allocate_name(&mut self.identities, &format!("{original}__{suffix}"));
            let mut ibr = DistIbr::new(
                &name,
                &self.names[&root],
                map,
                IbrTopology::SinglePhase,
                prime_mover,
                vec![rating],
            );
            ibr.p_avail = (prime_mover == IbrPrimeMover::Pv).then_some(available / n);
            ibr.p_min = Some(vec![power.re / n]);
            ibr.p_max.clone_from(&ibr.p_min);
            ibr.q_min = Some(vec![power.im / n]);
            ibr.q_max.clone_from(&ibr.q_min);
            self.net.ibrs_mut().push(ibr);
            self.attach(key, DistLocationTarget::Ibr, name.clone());
            self.attach(dc, DistLocationTarget::Ibr, name);
        }
        if prime_mover == IbrPrimeMover::Pv {
            self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_SOLAR_EQUIVALENT,format!("inverter {original}: fixed solar irradiance/temperature, simple efficiency, constant power factor and native clipping produce {power} VA; scheduled P_Out/Q_Out and V_base do not set this mode's dispatch; DC/thermal metadata and inactive load-following settings remain in source")));
        }

        Ok(())
    }
    fn inverter_input(&mut self, key: ObjectKey) -> Result<(ObjectKey, IbrPrimeMover, f64), Error> {
        let children = self
            .index
            .objects()
            .iter()
            .filter(|o| {
                matches!(class(o), "solar" | "battery") && self.index.parent(o.key()) == Some(key)
            })
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        let [child] = children.as_slice() else {
            return Err(self.error(
                key,
                "inverter snapshot requires exactly one supported solar or battery child",
            ));
        };
        // LOAD_FOLLOWING fields cannot affect the required CONSTANT_PF mode.
        // Resolve object references even when the native controller is inactive.
        self.index.resolve_property(key, "sense_object")?;
        if class(&self.index.objects()[child.ordinal()]) == "solar" {
            Ok((*child, IbrPrimeMover::Pv, self.solar_dc_power(*child)?))
        } else {
            self.idle_battery(*child)?;
            Ok((*child, IbrPrimeMover::Battery, 0.0))
        }
    }
    fn fixed_inverter_mode(&self, key: ObjectKey) -> Result<bool, Error> {
        if self.text(key, "inverter_type")?.as_deref() != Some("FOUR_QUADRANT")
            || self
                .text(key, "four_quadrant_control_mode")?
                .is_some_and(|v| v != "CONSTANT_PF")
        {
            return Err(self.error(key,"fixed inverter requires FOUR_QUADRANT CONSTANT_PF; other static/control modes remain under implementation"));
        }
        if self
            .text(key, "use_multipoint_efficiency")?
            .is_some_and(|v| !matches!(v.as_str(), "FALSE" | "false" | "0"))
        {
            return Err(self.error(
                key,
                "multipoint inverter efficiency needs a verified static mapping",
            ));
        }
        if self.text(key, "generator_mode")?.is_some_and(|v| {
            !matches!(
                v.as_str(),
                "UNKNOWN" | "CONSTANT_V" | "CONSTANT_PQ" | "CONSTANT_PF" | "SUPPLY_DRIVEN"
            )
        }) {
            return Err(self.error(key, "invalid inverter generator_mode"));
        }
        let online = match self
            .text(key, "generator_status")?
            .as_deref()
            .unwrap_or("ONLINE")
        {
            "ONLINE" => true,
            "OFFLINE" => false,
            _ => return Err(self.error(key, "inverter generator_status must be ONLINE or OFFLINE")),
        };
        Ok(online)
    }

    fn inverter_terminals(&self, key: ObjectKey) -> Result<Vec<(String, Vec<String>)>, Error> {
        let parent = self
            .index
            .parent(key)
            .ok_or_else(|| self.error(key, "inverter requires an electrical parent"))?;
        let root = self
            .roots
            .get(&parent)
            .ok_or_else(|| self.error(key, "inverter parent is not an electrical bus"))?;
        let flags = self
            .text(key, "phases")?
            .ok_or_else(|| self.error(key, "inverter requires explicit phases"))?;
        let parent_class = class(&self.index.objects()[parent.ordinal()]);
        if self.project.dialect() == crate::glm::GlmDialect::Arras4_3
            && !matches!(parent_class, "meter" | "triplex_meter")
        {
            return Err(self.error(
                key,
                "pinned Arras inverter requires a meter or triplex_meter parent",
            ));
        }
        if let Some(anchor) = self.triplex.get(root) {
            if !matches!(
                parent_class,
                "triplex_node" | "triplex_meter" | "triplex_load"
            ) || super::split_phase::anchor(&flags).ok().as_ref() != Some(anchor)
            {
                return Err(
                    self.error(key, "triplex inverter phases must match its parent service")
                );
            }
            return Ok(vec![("12".into(), vec!["1".into(), "2".into()])]);
        }
        if !matches!(parent_class, "node" | "meter" | "load") {
            return Err(self.error(
                key,
                "inverter requires an ordinary node/meter/load or triplex parent",
            ));
        }
        let phases = parse_phases(&flags).map_err(|m| self.error(key, m))?;
        let parent_flags = self
            .text(parent, "phases")?
            .ok_or_else(|| self.error(key, "inverter parent requires explicit phases"))?;
        let parent_phases = parse_phases(&parent_flags).map_err(|m| self.error(key, m))?;
        if phases
            .iter()
            .any(|p| !self.phases[root].contains(p) || !parent_phases.contains(p))
        {
            return Err(self.error(key, "inverter phases are absent at its parent"));
        }
        Ok(phases
            .into_iter()
            .map(|p| (phase_name(p).into(), vec![(p + 1).to_string(), "n".into()]))
            .collect())
    }
}

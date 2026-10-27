//! Grounded center-tapped services, expressed with ordinary typed coils.
use super::{Reader, allocate_name, class};
use crate::glm::{GlmDialect, GlmQuantityKind, ObjectKey};
use crate::{
    Configuration, DistBus, DistLoad, DistLoadVoltageModel, DistLocationTarget, DistTransformer,
    DistWinding, DistWindingConn,
};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

pub(super) fn bus_class(name: &str) -> bool {
    matches!(name, "triplex_node" | "triplex_meter" | "triplex_load")
}

pub(super) fn anchor(flags: &str) -> Result<usize, &'static str> {
    if flags.bytes().filter(|&b| b == b'S').count() != 1 {
        return Err("triplex phases require one S flag and one primary phase");
    }
    let phases = super::parse_phases(&flags.replace('S', ""))?;
    if let [phase] = phases.as_slice() {
        Ok(*phase)
    } else {
        Err("triplex phases require exactly one primary phase")
    }
}

pub(super) fn allowed(class: &str, name: &str) -> bool {
    bus_class(class)
        && (matches!(name, "voltage_1" | "voltage_2" | "voltage_N" | "voltage_12")
            || property(class, name).is_some())
}

pub(super) fn property<'a>(
    class: &str,
    name: &'a str,
) -> Option<(&'a str, Option<bool>, GlmQuantityKind, &'static str)> {
    use GlmQuantityKind::{Complex, Real};
    if !bus_class(class) {
        return None;
    }
    // Legacy PNNL triplex_node power accumulators are not writable in Arras.
    if class != "triplex_load" && matches!(name, "power_1" | "power_2" | "power_12") {
        return Some((name, None, Complex, "VA"));
    }
    if class != "triplex_load" {
        return None;
    }
    let (canonical, part) = if let Some(base) = name.strip_suffix("_real") {
        (base, Some(false))
    } else if let Some(base) = name.strip_suffix("_reac") {
        (base, Some(true))
    } else {
        (name, None)
    };
    for (prefix, unit) in [
        ("constant_power_", "VA"),
        ("constant_current_", "A"),
        ("constant_impedance_", "Ohm"),
    ] {
        if canonical
            .strip_prefix(prefix)
            .is_some_and(|s| matches!(s, "1" | "2" | "12"))
        {
            let unit = match (unit, part) {
                ("VA", Some(false)) => "W",
                ("VA", Some(true)) => "VAr",
                _ => unit,
            };
            return Some((
                canonical,
                part,
                if part.is_some() { Real } else { Complex },
                unit,
            ));
        }
    }
    None
}

pub(super) fn quantity_spec(class: &str, name: &str) -> Option<(GlmQuantityKind, &'static str)> {
    if bus_class(class) && matches!(name, "voltage_1" | "voltage_2" | "voltage_N" | "voltage_12") {
        Some((GlmQuantityKind::Complex, "V"))
    } else {
        property(class, name).map(|(_, _, kind, unit)| (kind, unit))
    }
}

impl Reader<'_> {
    pub(super) fn triplex_loads(&mut self, key: ObjectKey, root: ObjectKey) -> Result<(), Error> {
        let kind = class(&self.index.objects()[key.ordinal()]);
        if !bus_class(kind) {
            return Err(self.error(key, "ordinary child cannot attach to a triplex bus"));
        }
        let flags = self
            .text(key, "phases")?
            .ok_or_else(|| self.error(key, "triplex child requires phases"))?;
        if anchor(&flags).map_err(|m| self.error(key, m))? != self.triplex[&root] {
            return Err(self.error(key, "triplex child primary phase conflicts with its parent"));
        }
        if self.text(key, "bustype")?.is_some_and(|b| b != "PQ") {
            return Err(self.error(key, "triplex voltage sources are not qualified yet"));
        }
        let nominal = self.nominal[&root];
        if self
            .real(key, "nominal_voltage")
            .is_some_and(|v| v.total_cmp(&nominal).is_ne())
        {
            return Err(self.error(
                key,
                "triplex child nominal voltage conflicts with its parent",
            ));
        }
        let zip = self.triplex_zip(key)?;
        for law in ["power", "current", "impedance"] {
            for (suffix, terminals, scale) in [
                ("1", ["1", "n"], 1.0),
                ("2", ["n", "2"], 1.0),
                ("12", ["1", "2"], 2.0),
            ] {
                let field = if kind == "triplex_load" {
                    format!("constant_{law}_{suffix}")
                } else if law == "power" {
                    format!("power_{suffix}")
                } else {
                    continue;
                };
                let zip_power = zip.get(&field).copied();
                let Some(value) = zip_power.or_else(|| self.quantity(key, &field)) else {
                    continue;
                };
                if kind != "triplex_load" && self.project.dialect() == GlmDialect::Arras4_3 {
                    return Err(self.error(key, "Arras triplex_node power accumulators are hidden; use an explicit triplex_load"));
                }
                let voltage = nominal * scale;
                let power = if zip_power.is_some() {
                    value
                } else {
                    match law {
                        "power" => value,
                        // Triplex load current uses local voltage angle, with a zero-angle
                        // reference on every primary phase, unlike ordinary load current.
                        "current" => voltage * value.conj(),
                        _ if value == Complex64::default() => Complex64::default(),
                        _ => voltage.powi(2) / value.conj(),
                    }
                };
                if !voltage.is_finite()
                    || !power.re.is_finite()
                    || !power.im.is_finite()
                    || (value != Complex64::default() && power == Complex64::default())
                {
                    return Err(
                        self.error(key, "triplex load normalization overflows or underflows")
                    );
                }
                let name = allocate_name(
                    &mut self.identities,
                    &format!("{}__glm_{law}_{suffix}", self.names[&key]),
                );
                let mut load = DistLoad::new(
                    &name,
                    &self.names[&root],
                    terminals.map(str::to_owned).to_vec(),
                    Configuration::SinglePhase,
                    vec![power.re],
                    vec![power.im],
                );
                let v_nom = vec![voltage];
                load.voltage_model = match law {
                    "power" => DistLoadVoltageModel::ConstantPower { v_nom },
                    "current" => DistLoadVoltageModel::ConstantCurrent { v_nom },
                    _ => DistLoadVoltageModel::ConstantImpedance { v_nom },
                };
                self.net.loads_mut().push(load);
                if key != root {
                    self.attach(key, DistLocationTarget::Load, name);
                }
            }
        }
        Ok(())
    }

    pub(super) fn center_tapped_transformer(
        &mut self,
        key: ObjectKey,
        cfg: ObjectKey,
    ) -> Result<(), Error> {
        let flags = self
            .text(key, "phases")?
            .ok_or_else(|| self.error(key, "center-tapped transformer requires phases"))?;
        let phase = anchor(&flags).map_err(|m| self.error(key, m))?;
        let (from, to) = self.center_tapped_endpoints(key, phase)?;
        let primary = self.positive_geometry(cfg, "primary_voltage")?;
        let secondary = self.positive_geometry(cfg, "secondary_voltage")?;
        let rating = self.center_tapped_rating(cfg, phase)?;
        let impedances = self.center_tapped_impedances(cfg)?;
        let shunt = self
            .quantity(cfg, "shunt_impedance")
            .unwrap_or(Complex64::new(999_999_999.0, 999_999_999.0));
        let core_power = Complex64::new(rating / shunt.re, rating / shunt.im);
        if shunt.re <= 0.0
            || shunt.im <= 0.0
            || !core_power.re.is_finite()
            || !core_power.im.is_finite()
            || core_power.re == 0.0
            || core_power.im == 0.0
        {
            return Err(self.error(
                cfg,
                "center-tapped core requires positive finite shunt resistance and reactance",
            ));
        }
        let name = self.names[&key].clone();
        let core = allocate_name(&mut self.identities, &format!("{name}__glm_core"));
        let mut core_bus = DistBus::new(&core, vec!["1".into(), "n".into()]);
        core_bus.grounded.push("n".into());
        self.net.buses_mut().push(core_bus);
        // A zero-leakage fourth winding exposes the internal ideal core. Putting
        // its shunt on an external winding would bypass primary copper loss.
        let mut windings = Vec::new();
        for (bus, terminals, voltage, r) in [
            (
                self.names[&from].as_str(),
                vec![(phase + 1).to_string(), "n".into()],
                primary,
                impedances[0].re,
            ),
            (
                self.names[&to].as_str(),
                vec!["1".into(), "n".into()],
                secondary,
                impedances[1].re,
            ),
            (
                self.names[&to].as_str(),
                vec!["n".into(), "2".into()],
                secondary,
                impedances[2].re,
            ),
            (core.as_str(), vec!["1".into(), "n".into()], primary, 0.0),
        ] {
            let mut winding =
                DistWinding::new(bus, terminals, DistWindingConn::Wye, voltage, rating);
            winding.r_pct = 100.0 * r;
            windings.push(winding);
        }
        let [x0, x1, x2] = impedances.map(|z| 100.0 * z.im);
        let mut transformer = DistTransformer::new(
            &name,
            windings,
            vec![x0 + x1, x0 + x2, x0, x1 + x2, x1, x2],
            1,
        );
        transformer
            .extras
            .insert("ppm_antifloat".into(), serde_json::json!(0.0));
        let core_load = allocate_name(&mut self.identities, &format!("{name}__glm_core_loss"));
        let mut load = DistLoad::new(
            &core_load,
            &core,
            vec!["1".into(), "n".into()],
            Configuration::SinglePhase,
            vec![core_power.re],
            vec![core_power.im],
        );
        load.voltage_model = DistLoadVoltageModel::ConstantImpedance {
            v_nom: vec![primary],
        };
        self.net.loads_mut().push(load);
        self.net.transformers_mut().push(transformer);
        self.attach(key, DistLocationTarget::Transformer, name.clone());
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_TRANSFORMER_EQUIVALENT,
            format!("{name}: center-tapped winding polarity and leakage use four typed windings; auxiliary bus {core} and impedance load {core_load} preserve the internal core branch; include the core load when aggregating transformer losses")));
        Ok(())
    }
}

impl Reader<'_> {
    fn center_tapped_endpoints(
        &self,
        key: ObjectKey,
        phase: usize,
    ) -> Result<(ObjectKey, ObjectKey), Error> {
        let mut endpoints = Vec::new();
        for field in ["from", "to"] {
            let object = self
                .index
                .resolve_property(key, field)?
                .ok_or_else(|| self.error(key, format!("missing {field} reference")))?;
            let root = *self
                .roots
                .get(&object)
                .ok_or_else(|| self.error(key, "transformer endpoint is not an electrical bus"))?;
            if object != root {
                return Err(self.error(
                    key,
                    "center-tapped transformer requires unparented endpoints",
                ));
            }
            endpoints.push(root);
        }
        let (from, to) = (endpoints[0], endpoints[1]);
        if self.triplex.contains_key(&from)
            || !self.phases[&from].contains(&phase)
            || self.triplex.get(&to) != Some(&phase)
        {
            return Err(self.error(key, "center-tapped transformer requires an ordinary primary and matching triplex secondary"));
        }
        Ok((from, to))
    }

    fn center_tapped_rating(&mut self, cfg: ObjectKey, phase: usize) -> Result<f64, Error> {
        let total = self.transformer_rating(cfg, false)?;
        let phase_sum: f64 = ["powerA_rating", "powerB_rating", "powerC_rating"]
            .iter()
            .map(|f| self.real(cfg, f).unwrap_or(0.0))
            .sum();
        let rating = if phase_sum == 0.0 {
            total / 3.0
        } else {
            1000.0
                * self
                    .real(cfg, &format!("power{}_rating", super::phase_name(phase)))
                    .unwrap_or(0.0)
        };
        if !rating.is_finite() || rating <= 0.0 {
            return Err(self.error(
                cfg,
                "center-tapped transformer requires a positive active-phase rating",
            ));
        }
        Ok(rating)
    }

    fn center_tapped_impedances(&self, cfg: ObjectKey) -> Result<[Complex64; 3], Error> {
        let z = self.quantity(cfg, "impedance").unwrap_or_default();
        let mut z1 = self.quantity(cfg, "impedance1").unwrap_or_default();
        let mut z2 = self.quantity(cfg, "impedance2").unwrap_or_default();
        if z.re <= 0.0 || z.im <= 0.0 || z1.re < 0.0 || z1.im < 0.0 || z2.re < 0.0 || z2.im < 0.0 {
            return Err(self.error(cfg, "invalid center-tapped transformer leakage impedance"));
        }
        if z1 == Complex64::default() && z2.re != 0.0 && z2.im != 0.0 {
            z1 = z2;
        } else if z2 == Complex64::default() && z1.re != 0.0 && z1.im != 0.0 {
            z2 = z1;
        }
        let impedances = if z1 == Complex64::default() {
            [
                Complex64::new(0.5 * z.re, 0.8 * z.im),
                Complex64::new(z.re, 0.4 * z.im),
                Complex64::new(z.re, 0.4 * z.im),
            ]
        } else {
            [z, z1, z2]
        };
        if impedances.iter().any(|z| z.re <= 0.0 || z.im <= 0.0) {
            return Err(self.error(
                cfg,
                "zero secondary leakage components need separate native qualification",
            ));
        }
        Ok(impedances)
    }
}

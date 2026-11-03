//! PNNL static constant-PQ generators; dynamic machine behavior is separate.
use super::{Reader, allocate_name, class, phase_name};
use crate::glm::{GlmDialect, GlmQuantityKind, ObjectKey};
use crate::{Configuration, DistGenerator, DistLocationTarget};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

pub(super) fn quantity_spec(class: &str, name: &str) -> Option<(GlmQuantityKind, &'static str)> {
    use GlmQuantityKind::{Complex, Real};
    if class != "diesel_dg" {
        return None;
    }
    match name {
        "Rated_V" => Some((Real, "V")),
        "Rated_VA" => Some((Real, "VA")),
        "power_out_A" | "power_out_B" | "power_out_C" => Some((Complex, "VA")),
        "real_power_out_A" | "real_power_out_B" | "real_power_out_C" => Some((Real, "W")),
        "reactive_power_out_A" | "reactive_power_out_B" | "reactive_power_out_C" => {
            Some((Real, "VAr"))
        }
        _ => None,
    }
}

pub(super) fn allowed(class: &str, name: &str) -> bool {
    class == "diesel_dg" && (name == "Gen_type" || quantity_spec(class, name).is_some())
}

impl Reader<'_> {
    pub(super) fn generators(&mut self) -> Result<(), Error> {
        let keys = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "diesel_dg")
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        for key in keys {
            self.constant_pq_generator(key)?;
        }
        Ok(())
    }

    #[allow(clippy::float_cmp)] // Native initialization uses an exact -1 scalar sentinel.
    fn constant_pq_generator(&mut self, key: ObjectKey) -> Result<(), Error> {
        if self.project.dialect() != GlmDialect::Pnnl5_3 {
            return Err(self.error(key, "pinned Arras diesel_dg uses a different machine model; PNNL CONSTANT_PQ cannot be inferred from its declarations"));
        }
        if self
            .text(key, "Gen_type")?
            .is_some_and(|kind| kind != "CONSTANT_PQ")
        {
            return Err(self.error(key, "only the static CONSTANT_PQ diesel_dg mode is qualified; dynamic machine initialization is not a fixed setpoint"));
        }
        let root = self.generator_bus(key)?;
        // The native warning says 5000 kVA, but the pinned code assigns 0.5e6 VA.
        let declared_rating = self.real(key, "Rated_VA").unwrap_or(0.0);
        let rating = if declared_rating > 0.0 {
            declared_rating
        } else {
            500_000.0
        };
        let limit = rating / 3.0;
        if limit <= 0.0 || !limit.is_finite() {
            return Err(self.error(key, "generator per-phase rating underflows or is nonfinite"));
        }
        let original = self.names[&key].clone();
        for phase in 0..3 {
            let letter = phase_name(phase);
            let mut power = self
                .quantity(key, &format!("power_out_{letter}"))
                .unwrap_or_default();
            // These are separate native fields applied at init, not source-order
            // aliases of power_out. Explicit -1 retains the complex component.
            if let Some(value) = self.real(key, &format!("real_power_out_{letter}"))
                && value != -1.0
            {
                power.re = value;
            }
            if let Some(value) = self.real(key, &format!("reactive_power_out_{letter}"))
                && value != -1.0
            {
                power.im = value;
            }
            let magnitude = power.re.hypot(power.im);
            if !magnitude.is_finite() {
                return Err(self.error(key, "generator phase power magnitude overflows"));
            }
            if magnitude > limit {
                let pf = power.re / magnitude;
                power = Complex64::new(
                    limit * pf,
                    (limit * (1.0 - pf * pf).sqrt()).copysign(power.im),
                );
            }
            let name = allocate_name(&mut self.identities, &format!("{original}__{letter}"));
            let mut generator = DistGenerator::new(
                &name,
                &self.names[&root],
                vec![(phase + 1).to_string(), "n".into()],
                Configuration::SinglePhase,
                vec![power.re],
                vec![power.im],
            );
            generator.s_max = Some(vec![limit]);
            self.net.generators_mut().push(generator);
            self.attach(key, DistLocationTarget::Generator, name);
        }
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_GENERATOR_EQUIVALENT,
            format!("generator {original}: three phase records retain native initialized constant-PQ injections, including scalar overrides and clipping to {limit} VA per phase; total initialized rating is {rating} VA; Rated_V only validates the parent voltage")));
        Ok(())
    }

    fn generator_bus(&self, key: ObjectKey) -> Result<ObjectKey, Error> {
        let parent = self
            .index
            .parent(key)
            .ok_or_else(|| self.error(key, "diesel_dg requires an explicit electrical parent"))?;
        let root = *self.roots.get(&parent).ok_or_else(|| {
            self.error(key, "diesel_dg parent must be an ordinary electrical bus")
        })?;
        if !matches!(
            class(&self.index.objects()[parent.ordinal()]),
            "node" | "load" | "meter"
        ) || self.triplex.contains_key(&root)
            || self.phases[&root] != [0, 1, 2]
        {
            return Err(self.error(
                key,
                "diesel_dg parent requires three ordinary phases in this static profile",
            ));
        }
        let flags = self
            .text(parent, "phases")?
            .ok_or_else(|| self.error(key, "generator parent requires explicit phases"))?;
        if flags.contains('D') || super::parse_phases(&flags).ok().as_deref() != Some(&[0, 1, 2]) {
            return Err(self.error(
                key,
                "generator parent requires ordinary ABC phases without delta flags",
            ));
        }
        let nominal = self
            .real(parent, "nominal_voltage")
            .filter(|v| *v > 0.0)
            .ok_or_else(|| self.error(key, "generator parent requires positive nominal_voltage"))?;
        let rated_v = self.real(key, "Rated_V").unwrap_or(0.0);
        if rated_v > 0.0 && !(0.99..=1.01).contains(&(rated_v / 3.0_f64.sqrt() / nominal)) {
            return Err(self.error(key, "native diesel_dg Rated_V must match its parent's line-line nominal voltage within 1%"));
        }
        Ok(root)
    }
}

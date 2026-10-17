//! Native delta–grounded-wye port equations as three phase-pair coils.
use super::{Reader, allocate_name};
use crate::glm::ObjectKey;
use crate::{DistLocationTarget, DistTransformer, DistWinding, DistWindingConn};
use powerio_core::{Diagnostic, Error};

impl Reader<'_> {
    pub(super) fn delta_grounded_transformer(
        &mut self,
        key: ObjectKey,
        cfg: ObjectKey,
        phases: &[usize],
    ) -> Result<(), Error> {
        if phases != [0, 1, 2] {
            return Err(self.error(key, "DELTA_GWYE requires all three phases"));
        }
        let primary = self.positive_geometry(cfg, "primary_voltage")?;
        let secondary_ll = self.positive_geometry(cfg, "secondary_voltage")?;
        let secondary = secondary_ll / 3.0_f64.sqrt();
        let rating = self.transformer_rating(cfg, true)? / 3.0;
        let z = self.quantity(cfg, "impedance").ok_or_else(|| {
            self.error(
                cfg,
                "DELTA_GWYE requires positive series resistance and reactance",
            )
        })?;
        if z.re <= 0.0 || z.im <= 0.0 {
            return Err(self.error(
                cfg,
                "DELTA_GWYE requires positive series resistance and reactance",
            ));
        }
        let mut endpoints = Vec::new();
        for field in ["from", "to"] {
            let object = self
                .index
                .resolve_property(key, field)?
                .ok_or_else(|| self.error(key, format!("transformer requires {field}")))?;
            let root = self
                .roots
                .get(&object)
                .ok_or_else(|| self.error(key, "transformer endpoint is not an electrical bus"))?;
            if self.triplex.contains_key(root)
                || phases.iter().any(|p| !self.phases[root].contains(p))
            {
                return Err(self.error(key, "transformer phase is absent at an endpoint"));
            }
            endpoints.push(self.names[root].clone());
        }
        // Native step-down and step-up (including equal line-line ratings)
        // use opposite phase-pair orientations. Encoding the actual two
        // terminals avoids reliance on a target's default delta clock angle.
        let negative = if primary > secondary_ll {
            [2, 0, 1]
        } else {
            [1, 2, 0]
        };
        let original = self.names[&key].clone();
        for (phase, other) in negative.into_iter().enumerate() {
            let hot = (phase + 1).to_string();
            let name = allocate_name(
                &mut self.identities,
                &format!("{original}__glm_{}", ['A', 'B', 'C'][phase]),
            );
            let from = DistWinding::new(
                &endpoints[0],
                vec![hot.clone(), (other + 1).to_string()],
                DistWindingConn::Wye,
                primary,
                rating,
            );
            let mut to = DistWinding::new(
                &endpoints[1],
                vec![hot, "n".into()],
                DistWindingConn::Wye,
                secondary,
                rating,
            );
            to.r_pct = 100.0 * z.re;
            // A one-phase winding's v_ref is its coil voltage and s_rating
            // is one third of total. Thus Zbase = Vsecondary_LL² / Stotal.
            let mut transformer =
                DistTransformer::new(&name, vec![from, to], vec![100.0 * z.im], 1);
            transformer
                .extras
                .insert("ppm_antifloat".into(), serde_json::json!(0.0));
            self.net.transformers_mut().push(transformer);
            self.attach(key, DistLocationTarget::Transformer, name);
        }
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_TRANSFORMER_EQUIVALENT,
            format!("{original}: DELTA_GWYE becomes three phase-pair coils with native step-up/down polarity and no core branch; all external port currents are preserved")));
        Ok(())
    }
}

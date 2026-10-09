//! Triplex geometry with the native zero-neutral-voltage boundary condition.
use super::{METERS_PER_MILE, Reader, allocate_name, class, split_phase};
use crate::glm::{GlmQuantityKind, ObjectKey};
use crate::{DistLine, DistLineCode, DistLocationTarget};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

pub(super) fn allowed(class: &str, name: &str) -> bool {
    match class {
        "triplex_line" => matches!(name, "phases" | "from" | "to" | "configuration" | "length"),
        "triplex_line_configuration" => {
            matches!(name, "conductor_1" | "conductor_2" | "conductor_N")
                || quantity_spec(class, name).is_some()
        }
        "triplex_line_conductor" => quantity_spec(class, name).is_some(),
        _ => false,
    }
}

pub(super) fn quantity_spec(class: &str, name: &str) -> Option<(GlmQuantityKind, &'static str)> {
    use GlmQuantityKind::{Complex, Real};
    match (class, name) {
        ("triplex_line", "length") | ("triplex_line_conductor", "geometric_mean_radius") => {
            Some((Real, "ft"))
        }
        ("triplex_line_configuration", "insulation_thickness" | "diameter") => Some((Real, "in")),
        ("triplex_line_configuration", "z11" | "z12" | "z21" | "z22") => {
            Some((Complex, "Ohm/mile"))
        }
        ("triplex_line_conductor", "resistance") => Some((Real, "Ohm/mile")),
        _ => None,
    }
}

impl Reader<'_> {
    pub(super) fn triplex_lines(&mut self) -> Result<(), Error> {
        let keys = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "triplex_line")
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        for key in keys {
            let flags = self
                .text(key, "phases")?
                .ok_or_else(|| self.error(key, "triplex line requires phases"))?;
            let phase = split_phase::anchor(&flags).map_err(|m| self.error(key, m))?;
            let from = self.triplex_endpoint(key, "from", phase)?;
            let to = self.triplex_endpoint(key, "to", phase)?;
            let config =
                self.geometry_reference(key, "configuration", "triplex_line_configuration")?;
            let length = self.positive_geometry(key, "length")? * 0.3048;
            if !length.is_finite() || length == 0.0 {
                return Err(self.error(key, "triplex line length overflows or underflows"));
            }
            let name = self.names[&key].clone();
            let code_name = allocate_name(&mut self.identities, &format!("{name}__glm_linecode"));
            let code = self.triplex_line_code(config, &code_name)?;
            self.net.line_codes_mut().push(code);
            let terminals = vec!["1".into(), "2".into(), "n".into()];
            self.net.lines_mut().push(DistLine::new(
                &name,
                &self.names[&from],
                &self.names[&to],
                terminals.clone(),
                terminals,
                &code_name,
                length,
            ));
            self.attach(key, DistLocationTarget::Line, name);
            self.attach(config, DistLocationTarget::LineCode, code_name);
        }
        Ok(())
    }

    fn triplex_endpoint(
        &self,
        key: ObjectKey,
        field: &str,
        phase: usize,
    ) -> Result<ObjectKey, Error> {
        let object = self
            .index
            .resolve_property(key, field)?
            .ok_or_else(|| self.error(key, format!("triplex line requires {field}")))?;
        if self.roots.get(&object) != Some(&object) || self.triplex.get(&object) != Some(&phase) {
            return Err(self.error(
                key,
                "triplex line requires unparented triplex endpoints with the same primary phase",
            ));
        }
        Ok(object)
    }

    fn triplex_line_code(&mut self, config: ObjectKey, name: &str) -> Result<DistLineCode, Error> {
        if ["z11", "z12", "z21", "z22"].iter().any(|field| {
            self.quantity(config, field)
                .is_some_and(|z| z != Complex64::default())
        }) {
            return Err(self.error(config, "direct triplex matrices do not determine neutral currents; their reduced profile is not qualified yet"));
        }
        let diameter = self.positive_geometry(config, "diameter")?;
        let insulation = self.positive_geometry(config, "insulation_thickness")?;
        let d12 = (diameter + 2.0 * insulation) / 12.0;
        let dn = (diameter + insulation) / 12.0;
        let distances = [[0.0, d12, dn], [d12, 0.0, dn], [dn, dn, 0.0]];
        let mut wires = Vec::new();
        for field in ["conductor_1", "conductor_2", "conductor_N"] {
            wires.push(self.geometry_reference(config, field, "triplex_line_conductor")?);
        }
        let f = self.net.base_frequency();
        let earth = (100.0 / f).ln() / 2.0 + 7.6786;
        let mut r = vec![vec![0.0; 3]; 3];
        let mut x = r.clone();
        for (i, &wire) in wires.iter().enumerate() {
            let resistance = self.positive_geometry(wire, "resistance")?;
            let gmr = self.positive_geometry(wire, "geometric_mean_radius")?;
            for j in i..3 {
                let distance = if i == j { gmr } else { distances[i][j] };
                if !distance.is_finite()
                    || distance <= 0.0
                    || !(1.0 / distance).is_finite()
                    || 1.0 / distance == 0.0
                {
                    return Err(
                        self.error(config, "triplex conductor spacing overflows or underflows")
                    );
                }
                let z = Complex64::new(
                    0.001_588_36 * f + if i == j { resistance } else { 0.0 },
                    0.002_022_37 * f * (earth - distance.ln()),
                ) / METERS_PER_MILE;
                if !z.re.is_finite() || !z.im.is_finite() {
                    return Err(self.error(config, "triplex impedance is nonfinite"));
                }
                r[i][j] = z.re;
                r[j][i] = z.re;
                x[i][j] = z.im;
                x[j][i] = z.im;
            }
        }
        let mut code = DistLineCode::new(name, r, x);
        code.source = Some("GridLAB-D triplex modified Carson; full conductor primitive with neutral grounded at both endpoints".into());
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_GEOMETRY_MATERIALIZED,
            format!("{}: triplex geometry becomes a three-conductor primitive at {} Hz with neutral grounded at both endpoints; this preserves native grounded-neutral reduction and reconstructed neutral current, not a floating service neutral", self.names[&config], f)));
        Ok(code)
    }
}

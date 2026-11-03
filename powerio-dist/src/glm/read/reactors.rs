//! Static uncoupled series reactors, including the native NR zero fallback.
use super::{Reader, allocate_name, class, parse_phases, phase_name};
use crate::glm::{GlmQuantityKind, ObjectKey};
use crate::{DistLine, DistLineCode, DistLocationTarget, DistSwitch};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

pub(super) fn alias(class: &str, name: &str) -> Option<(String, bool)> {
    if class != "series_reactor" {
        return None;
    }
    for phase in ["A", "B", "C"] {
        for (part, imaginary) in [("resistance", false), ("reactance", true)] {
            if name == format!("phase_{phase}_{part}") {
                return Some((format!("phase_{phase}_impedance"), imaginary));
            }
        }
    }
    None
}
pub(super) fn quantity_spec(class: &str, name: &str) -> Option<(GlmQuantityKind, &'static str)> {
    if class != "series_reactor" {
        return None;
    }
    if alias(class, name).is_some() {
        return Some((GlmQuantityKind::Real, "Ohm"));
    }
    if matches!(
        name,
        "phase_A_impedance" | "phase_B_impedance" | "phase_C_impedance"
    ) {
        return Some((GlmQuantityKind::Complex, "Ohm"));
    }
    if name == "rated_current_limit"
        || ["A", "B", "C"].iter().any(|p| {
            name == format!("continuous_rating_{p}") || name == format!("emergency_rating_{p}")
        })
    {
        return Some((GlmQuantityKind::Real, "A"));
    }
    None
}
pub(super) fn allowed(class: &str, name: &str) -> bool {
    class == "series_reactor"
        && (matches!(name, "phases" | "from" | "to") || quantity_spec(class, name).is_some())
}
impl Reader<'_> {
    pub(super) fn reactors(&mut self) -> Result<(), Error> {
        let keys = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "series_reactor")
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        for key in keys {
            self.reactor(key)?;
        }
        Ok(())
    }
    fn reactor(&mut self, key: ObjectKey) -> Result<(), Error> {
        let phases = parse_phases(
            self.text(key, "phases")?
                .as_deref()
                .ok_or_else(|| self.error(key, "reactor requires explicit phases"))?,
        )
        .map_err(|m| self.error(key, m))?;
        let from = self.ordinary_link_endpoint(key, "from", &phases)?;
        let to = self.ordinary_link_endpoint(key, "to", &phases)?;
        let mut finite = Vec::new();
        let mut ideal = Vec::new();
        for &phase in &phases {
            let mut z = self
                .quantity(key, &format!("phase_{}_impedance", phase_name(phase)))
                .unwrap_or_default();
            if z == Complex64::default() && self.newton_raphson {
                z = Complex64::new(0.00005, -0.00005);
            }
            if z == Complex64::default() {
                ideal.push((phase + 1).to_string());
            } else {
                finite.push((phase, z));
            }
        }
        let original = self.names[&key].clone();
        if !ideal.is_empty() {
            let name = if finite.is_empty() {
                original.clone()
            } else {
                allocate_name(&mut self.identities, &format!("{original}__ideal"))
            };
            self.net.switches_mut().push(DistSwitch::new(
                &name,
                &from,
                &to,
                ideal.clone(),
                ideal,
                false,
            ));
            self.attach(key, DistLocationTarget::Switch, name);
        }
        if !finite.is_empty() {
            self.reactor_line(key, &from, &to, &finite, finite.len() == phases.len());
        }
        if self.index.objects()[key.ordinal()]
            .properties()
            .iter()
            .any(|p| p.name.contains("rating_") || p.name == "rated_current_limit")
        {
            self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_SOURCE_ONLY,
                format!("reactor {original}: current ratings remain in source; typed limit projection is unfinished")));
        }
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_REACTOR_EQUIVALENT,
            format!("reactor {original}: phase impedances use uncoupled unit-length lines; FBS zero impedances use ideal contacts; NR zero impedances use the native 1/(10000+j10000) ohm fallback")));
        Ok(())
    }
    fn reactor_line(
        &mut self,
        key: ObjectKey,
        from: &str,
        to: &str,
        values: &[(usize, Complex64)],
        complete: bool,
    ) {
        let original = self.names[&key].clone();
        let name = if complete {
            original
        } else {
            allocate_name(&mut self.identities, &format!("{original}__impedance"))
        };
        let code_name = allocate_name(&mut self.identities, &format!("{name}__glm_reactor"));
        let mut r = vec![vec![0.0; values.len()]; values.len()];
        let mut x = r.clone();
        let mut terminals = Vec::new();
        for (i, &(phase, z)) in values.iter().enumerate() {
            r[i][i] = z.re;
            x[i][i] = z.im;
            terminals.push((phase + 1).to_string());
        }
        self.net
            .line_codes_mut()
            .push(DistLineCode::new(&code_name, r, x));
        self.net.lines_mut().push(DistLine::new(
            &name,
            from,
            to,
            terminals.clone(),
            terminals,
            code_name,
            1.0,
        ));
        self.attach(key, DistLocationTarget::Line, name);
    }
}

//! Static native switch initialization and exact contact circuit mappings.

use super::{Reader, allocate_name, class, literal, parse_phases};
use crate::diagnostics::codes;
use crate::glm::ObjectKey;
use crate::{DistLine, DistLineCode, DistLocationTarget, DistSwitch};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

impl Reader<'_> {
    pub(super) fn switches(&mut self) -> Result<(), Error> {
        let keys = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "switch")
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        for key in keys {
            let phases = parse_phases(
                self.text(key, "phases")?
                    .as_deref()
                    .ok_or_else(|| self.error(key, "switch requires explicit phases"))?,
            )
            .map_err(|m| self.error(key, m))?;
            let from = self.ordinary_link_endpoint(key, "from", &phases)?;
            let to = self.ordinary_link_endpoint(key, "to", &phases)?;
            let (closed, impedance) = self.switch_state(key, &phases)?;
            let name = self.names[&key].clone();
            for state in [true, false] {
                let terminals = phases
                    .iter()
                    .filter(|&&p| closed[p] == state)
                    .map(|p| (p + 1).to_string())
                    .collect::<Vec<_>>();
                if terminals.is_empty() {
                    continue;
                }
                let component_name = if terminals.len() == phases.len() {
                    name.clone()
                } else {
                    allocate_name(
                        &mut self.identities,
                        &format!("{name}__glm_{}", if state { "closed" } else { "open" }),
                    )
                };
                if state && impedance != Complex64::new(0.0, 0.0) {
                    self.switch_line(key, &component_name, &from, &to, terminals, impedance);
                } else {
                    self.net.switches_mut().push(DistSwitch::new(
                        &component_name,
                        &from,
                        &to,
                        terminals.clone(),
                        terminals,
                        !state,
                    ));
                    self.attach(key, DistLocationTarget::Switch, component_name);
                }
            }
        }
        Ok(())
    }

    pub(super) fn ordinary_link_endpoint(
        &self,
        key: ObjectKey,
        field: &str,
        phases: &[usize],
    ) -> Result<String, Error> {
        let object = self
            .index
            .resolve_property(key, field)?
            .ok_or_else(|| self.error(key, format!("missing link {field} reference")))?;
        let root = self
            .roots
            .get(&object)
            .ok_or_else(|| self.error(key, format!("link {field} is not an electrical bus")))?;
        if self.triplex.contains_key(root) || phases.iter().any(|p| !self.phases[root].contains(p))
        {
            return Err(self.error(key, "link phases are absent at an endpoint"));
        }
        Ok(self.names[root].clone())
    }

    fn switch_state(
        &mut self,
        key: ObjectKey,
        phases: &[usize],
    ) -> Result<([bool; 3], Complex64), Error> {
        let mut closed = [true; 3];
        let mut status = true;
        let mut banked = true;
        let mut impedance = Complex64::new(-1.0, -1.0);
        for property in self.index.objects()[key.ordinal()].properties() {
            let state = || -> Result<bool, Error> {
                match literal(property).map_err(|m| self.error(key, m))?.as_str() {
                    "CLOSED" => Ok(true),
                    "OPEN" => Ok(false),
                    _ => Err(self.error(key, "switch state must be OPEN or CLOSED")),
                }
            };
            match property.name {
                "status" => status = state()?,
                "phase_A_state" => closed[0] = state()?,
                "phase_B_state" => closed[1] = state()?,
                "phase_C_state" => closed[2] = state()?,
                "operating_mode" => {
                    banked = match literal(property).map_err(|m| self.error(key, m))?.as_str() {
                        "BANKED" => true,
                        "INDIVIDUAL" => false,
                        _ => {
                            return Err(self
                                .error(key, "switch operating_mode must be BANKED or INDIVIDUAL"));
                        }
                    }
                }
                "switch_impedance" => {
                    impedance = self
                        .quantity(key, property.name)
                        .expect("quantity validated");
                }
                "switch_resistance" => {
                    impedance.re = self.real(key, property.name).expect("quantity validated");
                }
                "switch_reactance" => {
                    impedance.im = self.real(key, property.name).expect("quantity validated");
                }
                _ => {}
            }
        }
        let declared = closed;
        if !status {
            closed = [false; 3];
        } else if banked {
            let count = phases.iter().filter(|&&p| closed[p]).count();
            closed = [2 * count >= phases.len(); 3];
        }
        if phases.iter().any(|&p| declared[p] != closed[p]) {
            self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_SWITCH_STATE_NORMALIZED,
                format!("switch {}: fixed status and banked initialization determine the contact states; no control execution", self.names[&key])));
        }
        let fallback = if self.newton_raphson {
            self.switch_default_resistance
        } else {
            0.0
        };
        if impedance.re < 0.0 || (self.newton_raphson && impedance.re == 0.0) {
            impedance.re = fallback;
        }
        if impedance.im < 0.0 {
            impedance.im = -fallback;
        } else if self.newton_raphson && impedance.im == 0.0 {
            impedance.im = fallback;
        }
        Ok((closed, impedance))
    }

    fn switch_line(
        &mut self,
        key: ObjectKey,
        name: &str,
        from: &str,
        to: &str,
        terminals: Vec<String>,
        impedance: Complex64,
    ) {
        let code_name = allocate_name(
            &mut self.identities,
            &format!("{name}__glm_contact_impedance"),
        );
        let n = terminals.len();
        let mut r = vec![vec![0.0; n]; n];
        let mut x = r.clone();
        for i in 0..n {
            r[i][i] = impedance.re;
            x[i][i] = impedance.im;
        }
        self.net
            .line_codes_mut()
            .push(DistLineCode::new(&code_name, r, x));
        // Unit length carries the lumped contact impedance without inventing a
        // geographic length; there are no end shunts or conductor couplings.
        self.net.lines_mut().push(DistLine::new(
            name,
            from,
            to,
            terminals.clone(),
            terminals,
            code_name,
            1.0,
        ));
        self.attach(key, DistLocationTarget::Line, name.to_owned());
        self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_SWITCH_EQUIVALENT,
            format!("switch {}: finite closed contacts mapped to exact uncoupled line {name} with unit normalization length", self.names[&key])));
    }
}

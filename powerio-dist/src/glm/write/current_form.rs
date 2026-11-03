//! Exact scalar current-form line projection for the native NR solver.
use super::{Writer, complex_literal, phase_name};
use crate::{DistLine, DistLineCode, DistLocationTarget, current_form::ScalarCurrentForm};
use powerio_core::Error;
use std::collections::BTreeMap;
use std::fmt::Write as _;

impl Writer<'_> {
    pub(super) fn current_form_line(
        &mut self,
        line: &DistLine,
        codes: &BTreeMap<&str, &DistLineCode>,
    ) -> Result<bool, Error> {
        if self.ideal_plan.is_some() {
            return Ok(false);
        }
        let Some(code) = codes.get(line.linecode.as_str()) else {
            return Ok(false);
        };
        let Some(plan) = ScalarCurrentForm::for_line(line, code) else {
            return Ok(false);
        };
        let phases = self.link_phases(
            &line.name,
            &line.bus_from,
            &line.bus_to,
            &line.terminal_map_from,
            &line.terminal_map_to,
        )?;
        let p = phase_name(phases[0]);
        let aux = self.generated_name(&format!("{}__glm_current", line.name));
        let voltage_error = self.voltage_error();
        let _ = writeln!(
            self.out,
            "object load {{ name \"{aux}\"; phases {p}N; nominal_voltage {}; maximum_voltage_error {voltage_error}; constant_impedance_{p} {}; }}",
            self.nominal[line.bus_from.as_str()],
            complex_literal(plan.auxiliary_impedance())?
        );
        let to_line = self.generated_name(&format!("{aux}__to"));
        for (name, bus, z) in [
            (&line.name, &line.bus_from, -plan.scale),
            (&to_line, &line.bus_to, plan.scale),
        ] {
            let cfg = self.generated_name(&format!("{name}__configuration"));
            let _ = writeln!(
                self.out,
                "object line_configuration {{ name \"{cfg}\"; z{}{} {z}+0j;",
                phases[0] + 1,
                phases[0] + 1
            );
            if name == &line.name {
                self.coordinates(DistLocationTarget::LineCode, &code.name);
            }
            self.out.push_str("}\n");
            let _ = writeln!(
                self.out,
                "object overhead_line {{ name \"{name}\"; phases {p}N; from \"{bus}\"; to \"{aux}\"; length 1609.344 m; configuration \"{cfg}\"; "
            );
            if name == &line.name {
                self.coordinates(DistLocationTarget::Line, &line.name);
            }
            self.out.push_str("}\n");
        }
        for (side, bus, z) in [
            ("from", &line.bus_from, plan.scale),
            ("to", &line.bus_to, -plan.scale),
        ] {
            let name = self.generated_name(&format!("{aux}__{side}_shunt"));
            let _ = writeln!(
                self.out,
                "object load {{ name \"{name}\"; parent \"{bus}\"; phases {p}N; nominal_voltage {}; constant_impedance_{p} {z}+0j; }}",
                self.nominal[bus.as_str()]
            );
        }
        if line.i_max.is_some()
            || line.s_max.is_some()
            || line.route.is_some()
            || code.i_max.is_some()
            || code.s_max.is_some()
        {
            self.dropped(format!(
                "line {}: thermal limits/routes have no current-form output fields",
                line.name
            ));
        }
        self.diagnostics.push(&crate::diagnostics::codes::EMIT_GLM_VALUE_REBASED,format!("line {}: current scalar impedance uses auxiliary voltage {aux}=1ohm*I; physical end currents/powers include generated end shunts, and internal losses must not be reported as line losses",line.name));
        Ok(true)
    }
}

//! Exact scalar near-zero impedance lines without a large terminal admittance.
use super::{C, DssWriter, num};
use crate::{DistLine, MulticonductorNetwork, current_form::ScalarCurrentForm};
use num_complex::Complex64;

impl DssWriter {
    pub(super) fn current_form_line(
        &mut self,
        net: &MulticonductorNetwork,
        line: &DistLine,
    ) -> bool {
        let Some(code) = net
            .line_codes()
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(&line.linecode))
        else {
            return false;
        };
        let Some(plan) = ScalarCurrentForm::for_line(line, code) else {
            return false;
        };
        self.check_name("line", &line.name);
        let mut used = super::ideal_transformers::used_names(net);
        used.extend(net.loads().iter().map(|l| l.name.to_ascii_lowercase()));
        let mut prefix = format!("_pio_current_{}", line.name);
        while used
            .iter()
            .any(|n| n.contains(&prefix.to_ascii_lowercase()))
        {
            prefix.push('_');
        }
        let aux = format!("{prefix}.1");
        let from = self.bus_ref(&line.bus_from, &line.terminal_map_from);
        let to = self.bus_ref(&line.bus_to, &line.terminal_map_to);
        for (name, port, z) in [
            (&line.name, &from, -plan.scale),
            (&format!("{prefix}_to"), &to, plan.scale),
        ] {
            self.line_out(&format!("New Line.{name} phases=1 bus1={port} bus2={aux} length=1 units=m r1={z} x1=0 r0={z} x0=0 c1=0 c0=0 basefreq={}",num(net.base_frequency())));
        }
        for (name, bus, y) in [
            (
                format!("{prefix}_from_shunt"),
                from,
                Complex64::new(1.0 / plan.scale, 0.0),
            ),
            (
                format!("{prefix}_to_shunt"),
                to,
                Complex64::new(-1.0 / plan.scale, 0.0),
            ),
            (
                format!("{prefix}_aux_shunt"),
                aux,
                -plan.impedance / plan.scale / plan.scale,
            ),
        ] {
            self.line_out(&format!("New Load.{name} phases=1 bus1={bus} conn=wye model=2 kv=1 kw={} kvar={} vlowpu=0 vminpu=0 vmaxpu=1000000",num(1000.0*y.re),num(-1000.0*y.im)));
        }
        self.warn(&C::EMIT_DSS_IMPEDANCE_EQUIVALENT,format!("line {}: finite scalar impedance {} uses auxiliary voltage {prefix}=1ohm*I; physical from current includes {prefix}_from_shunt and physical to current includes {prefix}_to plus {prefix}_to_shunt; internal element losses are not physical line losses",line.name,plan.impedance));
        if line.i_max.is_some()
            || line.s_max.is_some()
            || line.route.is_some()
            || code.i_max.is_some()
            || code.s_max.is_some()
        {
            self.warn(&C::EMIT_DSS_FIELD_DROPPED,format!("line {}: thermal limits and route metadata are not represented on the current-form elements",line.name));
        }
        true
    }
}

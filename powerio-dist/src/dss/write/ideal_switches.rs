//! Closed contacts without OpenDSS's default finite switch impedance.
use super::{C, DssWriter, num};
use crate::DistSwitch;
use std::collections::BTreeSet;
use std::fmt::Write as _;

impl DssWriter {
    pub(super) fn ideal_closed_switch(
        &mut self,
        sw: &DistSwitch,
        used: &mut BTreeSet<String>,
    ) -> bool {
        // Retained impedance declarations (including PMD matrices) follow
        // the existing source-aware path. Open contacts need no cancellation.
        if sw.open
            || !sw.extras.is_empty()
            || sw.terminal_map_from.is_empty()
            || sw.terminal_map_from.len() != sw.terminal_map_to.len()
        {
            return false;
        }
        let mut name = format!("_pio_switch_{}", sw.name);
        while !used.insert(name.to_ascii_lowercase()) {
            name.push('_');
        }
        let count = sw.terminal_map_from.len();
        let from = self.bus_ref(&sw.bus_from, &sw.terminal_map_from);
        let to = self.bus_ref(&sw.bus_to, &sw.terminal_map_to);
        let from_ground = self.grounded.get(&sw.bus_from.to_ascii_lowercase());
        let to_ground = self.grounded.get(&sw.bus_to.to_ascii_lowercase());
        let nodes = sw
            .terminal_map_from
            .iter()
            .zip(&sw.terminal_map_to)
            .enumerate()
            .map(|(i, (a, b))| {
                if from_ground.is_some_and(|g| g.contains(a))
                    && to_ground.is_some_and(|g| g.contains(b))
                {
                    "0".into()
                } else {
                    (i + 1).to_string()
                }
            })
            .collect::<Vec<String>>();
        let internal = format!("{name}.{}", nodes.join("."));
        // R + (-R) enforces equality of every paired conductor voltage.
        // The original Line identity exposes incoming currents; the added
        // line's far end exposes outgoing powers at the physical second bus.
        let mut line = format!(
            "New Line.{} bus1={from} bus2={internal} phases={count} switch=y length=1 units=m r1=1 r0=1 x1=0 x0=0 c1=0 c0=0",
            sw.name
        );
        match sw.i_max.as_deref() {
            Some([amps, ..]) if amps.is_finite() => {
                let _ = write!(line, " emergamps={}", num(*amps));
            }
            Some(_) => self.warn(
                &C::EMIT_DSS_FIELD_DROPPED,
                format!(
                    "line {}: empty or nonfinite first ampacity is not emitted",
                    sw.name
                ),
            ),
            None => {}
        }
        self.line_out(&line);
        self.line_out(&format!("New Line.{name} bus1={internal} bus2={to} phases={count} length=1 units=m r1=-1 r0=-1 x1=0 x0=0 c1=0 c0=0"));
        self.warn(&C::EMIT_DSS_IDEAL_SWITCH_EQUIVALENT,format!("switch {}: closed ideal contacts use one-ohm resistances cancelled by Line.{name}; external to-port powers and total losses include that line",sw.name));
        true
    }
}

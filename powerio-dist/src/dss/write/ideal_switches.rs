//! Closed contacts without OpenDSS's default finite switch impedance.
use super::{C, DssWriter, num};
use crate::DistSwitch;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write as _;

impl DssWriter {
    pub(super) fn ideal_closed_switch(
        &mut self,
        sw: &DistSwitch,
        used: &mut BTreeSet<String>,
    ) -> bool {
        // Retained impedance declarations (including nonzero PMD matrices)
        // follow the existing source-aware path. All zero PMD matrices
        // declare an ideal contact. Its length then has no effect.
        // Open contacts need no cancellation.
        if sw.open
            || sw.extras.iter().any(|(key, value)| match key.as_str() {
                "ravens_mrid"
                | "cim_mrid"
                | "ravens_switch_class"
                | "cim_class"
                | "ravens_normal_open"
                | "pmd_length" => false,
                "pmd_rs" | "pmd_xs" | "pmd_g_fr" | "pmd_g_to" | "pmd_b_fr" | "pmd_b_to" => {
                    !all_zero(value)
                }
                _ => true,
            })
            || sw.terminal_map_from.is_empty()
            || sw.terminal_map_from.len() != sw.terminal_map_to.len()
        {
            return false;
        }
        // Identity and normal-open metadata do not add electrical impedance.
        // Report their omission just as the ordinary line path does.
        let _ = self.extras_tail("line", &sw.name, &sw.extras);
        let mut name = format!("_pio_switch_{}", sw.name);
        while !used.insert(name.to_ascii_lowercase()) {
            name.push('_');
        }
        let count = sw.terminal_map_from.len();
        let from = self.bus_ref(&sw.bus_from, &sw.terminal_map_from);
        let to = self.bus_ref(&sw.bus_to, &sw.terminal_map_to);
        let from_ground = self.grounded.get(&sw.bus_from.to_ascii_lowercase());
        let to_ground = self.grounded.get(&sw.bus_to.to_ascii_lowercase());
        // The internal bus repeats the node numbers of the ends it joins, so
        // both added lines keep the same node list at each end, as PMD's Kron
        // reduction requires (#765). A conductor grounded at only one end
        // takes the other end's node, or the next free one.
        let from_nodes = node_numbers(&from, &sw.bus_from);
        let to_nodes = node_numbers(&to, &sw.bus_to);
        let mut taken = BTreeSet::new();
        let nodes = sw
            .terminal_map_from
            .iter()
            .zip(&sw.terminal_map_to)
            .enumerate()
            .map(|(i, (a, b))| {
                if from_ground.is_some_and(|g| g.contains(a))
                    && to_ground.is_some_and(|g| g.contains(b))
                {
                    return "0".to_string();
                }
                let mut node = [from_nodes.get(i), to_nodes.get(i)]
                    .into_iter()
                    .flatten()
                    .find(|n| *n != "0" && !taken.contains(*n))
                    .cloned()
                    .unwrap_or_default();
                if node.is_empty() {
                    // At most `count - 1` nodes are taken, so one is free.
                    node = (1..=count)
                        .map(|n| n.to_string())
                        .find(|n| !taken.contains(n))
                        .unwrap_or_default();
                }
                taken.insert(node.clone());
                node
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

/// The node numbers of a `bus.n1.n2` reference the writer built for `bus`.
pub(super) fn node_numbers(reference: &str, bus: &str) -> Vec<String> {
    reference
        .strip_prefix(bus)
        .and_then(|rest| rest.strip_prefix('.'))
        .map(|nodes| nodes.split('.').map(str::to_string).collect())
        .unwrap_or_default()
}

/// Whether a retained PMD matrix holds only zeros.
fn all_zero(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().all(all_zero),
        other => other.as_f64() == Some(0.0),
    }
}

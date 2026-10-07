//! Canonical native nodes for a declared phase-labelled conductor network.
//! Closed ideal switches may join bundles only when they do not accidentally
//! join another phase. Reference conductors never participate in this union.

use super::write::{ExperimentalMulticonductorOptions, Result, error};
use crate::MulticonductorNetwork;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Topology {
    pub nodes: BTreeMap<i64, (f64, BTreeSet<String>)>,
    pub bus_ids: BTreeMap<String, i64>,
}

fn root(parent: &mut BTreeMap<String, String>, bus: &str) -> String {
    let mut key = bus.to_owned();
    let mut path = Vec::new();
    while parent[&key] != key {
        path.push(key.clone());
        key = parent[&key].clone();
    }
    for item in path {
        parent.insert(item, key.clone());
    }
    key
}

impl Topology {
    pub fn new(
        net: &MulticonductorNetwork,
        options: &ExperimentalMulticonductorOptions,
    ) -> Result<Self> {
        if options.nominal_ll_volts.len() != net.buses().len() {
            return Err(error("nominal_ll_volts must name every bus exactly once"));
        }
        let mut parent = BTreeMap::new();
        let mut phases = BTreeMap::<String, BTreeSet<String>>::new();
        for bus in net.buses() {
            let voltage = options.nominal_ll_volts.get(&bus.id).ok_or_else(|| {
                error(format!("bus {}: missing nominal line-line voltage", bus.id))
            })?;
            if !voltage.is_finite() || *voltage <= 0.0 {
                return Err(error(
                    "nominal line-line voltage must be finite and positive",
                ));
            }
            parent.insert(bus.id.clone(), bus.id.clone());
            let selected = bus_phases(net, bus)?;
            phases.insert(bus.id.clone(), selected);
        }
        for switch in net.switches() {
            if switch.open {
                return Err(error(format!(
                    "switch {}: open-switch authoring not yet implemented",
                    switch.name
                )));
            }
            if switch.terminal_map_from != switch.terminal_map_to {
                return Err(error(format!(
                    "switch {}: conductor permutation requires explicit mapping",
                    switch.name
                )));
            }
            let terminals = switch
                .terminal_map_from
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            if terminals
                .iter()
                .any(|t| !matches!(t.as_str(), "1" | "2" | "3"))
            {
                return Err(error(
                    "switching a reference/neutral conductor requires explicit native circuit mapping",
                ));
            }
            let a = root(&mut parent, &switch.bus_from);
            let b = root(&mut parent, &switch.bus_to);
            if a == b {
                continue;
            }
            if options.nominal_ll_volts[&a].to_bits() != options.nominal_ll_volts[&b].to_bits() {
                return Err(error(
                    "closed switch joins different declared voltage levels",
                ));
            }
            if phases[&a]
                .intersection(&phases[&b])
                .any(|p| !terminals.contains(p))
            {
                return Err(error(format!(
                    "switch {}: collapsing a partial switch would short another conductor",
                    switch.name
                )));
            }
            let (keep, remove) = if a < b { (a, b) } else { (b, a) };
            let additional = phases.remove(&remove).unwrap();
            phases.get_mut(&keep).unwrap().extend(additional);
            parent.insert(remove, keep);
        }
        let roots = phases
            .keys()
            .enumerate()
            .map(|(i, k)| Ok((k.clone(), i64::try_from(i + 1).map_err(error)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let nodes = phases
            .into_iter()
            .map(|(name, p)| (roots[&name], (options.nominal_ll_volts[&name], p)))
            .collect();
        let bus_ids = net
            .buses()
            .iter()
            .map(|b| (b.id.clone(), roots[&root(&mut parent, &b.id)]))
            .collect();
        Ok(Self { nodes, bus_ids })
    }
}

pub(super) fn connection(terminals: &[String]) -> Result<i64> {
    let names = terminals.iter().map(String::as_str).collect::<Vec<_>>();
    match names.as_slice() {
        ["1"] => Ok(1),
        ["2"] => Ok(2),
        ["3"] => Ok(3),
        ["1", "2"] => Ok(4),
        ["2", "3"] => Ok(5),
        ["3", "1"] => Ok(6),
        ["1", "2", "3"] => Ok(7),
        _ => Err(error(format!(
            "unsupported native phase order {terminals:?}"
        ))),
    }
}

fn bus_phases(net: &MulticonductorNetwork, bus: &crate::DistBus) -> Result<BTreeSet<String>> {
    let mut selected = BTreeSet::new();
    for terminal in &bus.terminals {
        if matches!(terminal.as_str(), "1" | "2" | "3") {
            if bus.grounded.contains(terminal) {
                return Err(error(
                    "grounded active phase requires a separate native circuit mapping",
                ));
            }
            selected.insert(terminal.clone());
        } else if !bus.grounded.contains(terminal) {
            // A floating source star is local to exactly one source.
            // Loads, lines and switches are checked separately and may
            // not address it, even if they happen to call it "neutral".
            let references = net
                .sources()
                .iter()
                .filter(|s| s.bus == bus.id && s.reference_terminal.as_ref() == Some(terminal))
                .count();
            if references != 1 {
                return Err(error(format!(
                    "bus {} terminal {terminal}: external/floating neutral circuit is not yet writable",
                    bus.id
                )));
            }
        }
    }
    Ok(selected)
}

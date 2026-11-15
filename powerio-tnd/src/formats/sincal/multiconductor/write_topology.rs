//! Canonical native nodes for a declared phase-labelled conductor network.
//! Closed ideal switches may join bundles only when they do not accidentally
//! join another phase. Reference conductors never participate in this union.

use super::write::{ExperimentalMulticonductorOptions, Result, error};
use powerio_dist::MulticonductorNetwork;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Topology {
    pub nodes: BTreeMap<i64, (f64, BTreeSet<String>)>,
    pub bus_ids: BTreeMap<String, i64>,
}

impl Topology {
    pub fn new(
        net: &MulticonductorNetwork,
        options: &ExperimentalMulticonductorOptions,
    ) -> Result<Self> {
        if options.nominal_ll_volts.len() != net.buses().len() {
            return Err(error("nominal_ll_volts must name every bus exactly once"));
        }
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
            let selected = bus_phases(net, bus)?;
            phases.insert(bus.id.clone(), selected);
        }
        // Buses by id order, so the smaller index is the smaller id and the
        // workspace's one union-find keeps the smaller id as each root.
        let ids: Vec<String> = phases.keys().cloned().collect();
        let index: BTreeMap<String, usize> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        let mut sets = powerio_core::union_find::UnionFind::new(ids.len());
        for switch in net.switches() {
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
            if switch.open {
                // An open path declares conductor endpoints but creates no
                // equality constraint and must not merge native nodes.
                continue;
            }
            let ra = sets.find(index[&switch.bus_from]);
            let rb = sets.find(index[&switch.bus_to]);
            if ra == rb {
                continue;
            }
            let (a, b) = (&ids[ra], &ids[rb]);
            if options.nominal_ll_volts[a].to_bits() != options.nominal_ll_volts[b].to_bits() {
                return Err(error(
                    "closed switch joins different declared voltage levels",
                ));
            }
            if phases[a]
                .intersection(&phases[b])
                .any(|p| !terminals.contains(p))
            {
                return Err(error(format!(
                    "switch {}: collapsing a partial switch would short another conductor",
                    switch.name
                )));
            }
            let (keep, remove) = (ra.min(rb), ra.max(rb));
            let additional = phases.remove(&ids[remove]).unwrap();
            phases.get_mut(&ids[keep]).unwrap().extend(additional);
            sets.union(keep, remove);
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
            .map(|b| (b.id.clone(), roots[&ids[sets.find(index[&b.id])]]))
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

fn bus_phases(
    net: &MulticonductorNetwork,
    bus: &powerio_dist::DistBus,
) -> Result<BTreeSet<String>> {
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

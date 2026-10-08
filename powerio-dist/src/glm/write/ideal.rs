//! Exact FBS contacts with a checked, source-oriented forest.

use super::{Writer, flags, phase_name, unsupported, validate_line_matrices};
use crate::diagnostics::codes;
use crate::{DistComponentLocation, DistLocationTarget, DistSwitch};
use powerio_core::Error;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;

pub(super) struct IdealPlan<'a> {
    pub(super) groups: Vec<ContactGroup<'a>>,
    pub(super) reversed: BTreeSet<String>,
}

pub(super) struct ContactGroup<'a> {
    name: String,
    pub(super) from: &'a str,
    pub(super) to: &'a str,
    states: BTreeMap<usize, bool>,
    members: Vec<&'a DistSwitch>,
    location: Option<&'a DistComponentLocation>,
}

impl<'a> Writer<'a> {
    pub(super) fn prepare_ideal_contacts(&mut self) -> Result<(), Error> {
        if self.net.switches().iter().all(|s| s.open) {
            return Ok(());
        }
        let groups = self.contact_groups()?;
        let reversed = self.orient_ideal_forest(&groups)?;
        self.diagnostics.push(&codes::EMIT_GLM_IDEAL_CONTACTS,
            "ideal contacts select native zero-impedance FBS output; the complete link graph is a checked forest oriented away from its source buses");
        for name in &reversed {
            self.diagnostics.push(&codes::EMIT_GLM_IDEAL_CONTACTS,
                format!("link {name}: native endpoints reversed to point away from the source; conductor identities and electrical parameters are unchanged"));
        }
        self.ideal_plan = Some(IdealPlan { groups, reversed });
        Ok(())
    }

    fn contact_groups(&mut self) -> Result<Vec<ContactGroup<'a>>, Error> {
        let mut groups: BTreeMap<(&str, &str), ContactGroup<'a>> = BTreeMap::new();
        for switch in self.net.switches() {
            if !switch.extras.is_empty() {
                return Err(unsupported(format!(
                    "switch {} has uninterpreted electrical extras",
                    switch.name
                )));
            }
            let phases = self.link_phases(
                &switch.name,
                &switch.bus_from,
                &switch.bus_to,
                &switch.terminal_map_from,
                &switch.terminal_map_to,
            )?;
            let (from, to) = if switch.bus_from <= switch.bus_to {
                (switch.bus_from.as_str(), switch.bus_to.as_str())
            } else {
                (switch.bus_to.as_str(), switch.bus_from.as_str())
            };
            let group = groups.entry((from, to)).or_insert_with(|| ContactGroup {
                name: String::new(),
                from,
                to,
                states: BTreeMap::new(),
                members: Vec::new(),
                location: None,
            });
            for phase in phases {
                if group.states.insert(phase, switch.open).is_some() {
                    return Err(unsupported(
                        "parallel ideal contacts on the same phase require an additional exact output equivalent",
                    ));
                }
            }
            if let Some(location) = self
                .locations
                .get(&(DistLocationTarget::Switch, switch.name.as_str()))
                .copied()
            {
                if group
                    .location
                    .is_some_and(|p| p.point != location.point || p.kind != location.kind)
                {
                    return Err(unsupported(
                        "grouped FBS contacts have distinct geographic points; one native object cannot preserve both attachments",
                    ));
                }
                group.location = Some(location);
            }
            group.members.push(switch);
        }
        let mut groups: Vec<_> = groups.into_values().collect();
        for group in &mut groups {
            group.name = if group.members.len() == 1 {
                group.members[0].name.clone()
            } else {
                self.generated_name(&format!("{}__{}__glm_contact", group.from, group.to))
            };
        }

        Ok(groups)
    }

    fn orient_ideal_forest(&self, groups: &[ContactGroup<'a>]) -> Result<BTreeSet<String>, Error> {
        // Include open groups: even open ties participate in native FBS rank
        // assignment. Counting only energized edges can accept invalid output.
        let mut neighbors: BTreeMap<&str, Vec<(&str, String)>> = BTreeMap::new();
        let mut original_from = BTreeMap::new();
        let mut edge_phases = BTreeMap::new();
        let codes: BTreeMap<_, _> = self
            .net
            .line_codes()
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();
        for line in self.net.lines() {
            let phases = self.link_phases(
                &line.name,
                &line.bus_from,
                &line.bus_to,
                &line.terminal_map_from,
                &line.terminal_map_to,
            )?;
            let code = codes.get(line.linecode.as_str()).ok_or_else(|| {
                unsupported(format!("line {} has no exact linecode identity", line.name))
            })?;
            validate_line_matrices(line, code, phases.len())?;
            edge_phases.insert(line.name.as_str(), phases);
        }
        for group in groups {
            edge_phases.insert(group.name.as_str(), group.states.keys().copied().collect());
        }
        for (name, from, to) in self
            .net
            .lines()
            .iter()
            .map(|l| (l.name.as_str(), l.bus_from.as_str(), l.bus_to.as_str()))
            .chain(groups.iter().map(|g| (g.name.as_str(), g.from, g.to)))
        {
            neighbors
                .entry(from)
                .or_default()
                .push((to, name.to_owned()));
            neighbors
                .entry(to)
                .or_default()
                .push((from, name.to_owned()));
            original_from.insert(name, from);
        }
        let mut seen: BTreeSet<&str> = self.source_voltages.keys().copied().collect();
        let mut queue: VecDeque<_> = seen.iter().copied().collect();
        let mut edges = BTreeSet::new();
        let mut reversed = BTreeSet::new();
        while let Some(bus) = queue.pop_front() {
            for (neighbor, name) in neighbors.get(bus).into_iter().flatten() {
                if !edges.insert(name.as_str()) {
                    continue;
                }
                if !seen.insert(neighbor) {
                    return Err(unsupported(
                        "ideal closed switch output requires a forest with one source bus per component; cycles, parallel links and connected multiple sources need another exact equivalent",
                    ));
                }
                if self.phases[neighbor]
                    .iter()
                    .any(|p| !edge_phases[name.as_str()].contains(p))
                {
                    return Err(unsupported(format!(
                        "FBS bus {neighbor} has phases absent from its incoming link; disconnected terminal output needs another exact equivalent"
                    )));
                }
                if original_from[name.as_str()] != bus {
                    reversed.insert(name.clone());
                }
                queue.push_back(neighbor);
            }
        }
        if seen.len() != self.net.buses().len() {
            return Err(unsupported(
                "ideal closed switch output has a component without a fixed source",
            ));
        }
        Ok(reversed)
    }

    pub(super) fn ideal_contacts(&mut self, plan: IdealPlan<'a>) {
        for group in plan.groups {
            let (from, to) = if plan.reversed.contains(&group.name) {
                (group.to, group.from)
            } else {
                (group.from, group.to)
            };
            let phases = flags(&group.states.keys().copied().collect::<Vec<_>>());
            let status = if group.states.values().all(|&open| open) {
                "OPEN"
            } else {
                "CLOSED"
            };
            let _ = writeln!(
                self.out,
                "object switch {{\n    name {};\n    phases {phases}N;\n    from {from};\n    to {to};\n    operating_mode INDIVIDUAL;\n    status {status};\n    switch_impedance 0+0j;",
                group.name
            );
            for (phase, open) in group.states {
                let state = if open { "OPEN" } else { "CLOSED" };
                let _ = writeln!(self.out, "    phase_{}_state {state};", phase_name(phase));
            }
            if let Some(location) = group.location {
                self.coordinates(DistLocationTarget::Switch, &location.name);
            }
            self.out.push_str("}\n\n");
            for member in &group.members {
                self.written_locations
                    .insert((DistLocationTarget::Switch, member.name.as_str()));
                if member.i_max.is_some() {
                    self.dropped(format!(
                        "switch {}: contact current ratings are not emitted",
                        member.name
                    ));
                }
                if group.members.len() > 1 {
                    self.diagnostics.push(&codes::EMIT_GLM_IDEAL_CONTACTS,
                        format!("switch {}: disjoint phase contacts materialized in native switch {}; component identity is grouped", member.name, group.name));
                }
            }
        }
    }
}

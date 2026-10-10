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
    pub(super) open_aliases: BTreeSet<String>,
}

pub(super) struct ContactGroup<'a> {
    pub(super) name: String,
    pub(super) from: &'a str,
    pub(super) to: &'a str,
    states: BTreeMap<usize, bool>,
    members: Vec<&'a DistSwitch>,
    location: Option<&'a DistComponentLocation>,
}

impl<'a> Writer<'a> {
    pub(super) fn prepare_ideal_contacts(&mut self) -> Result<(), Error> {
        if self.net.switches().iter().all(|s| s.open) && self.regulators.groups.is_empty() {
            return Ok(());
        }
        let groups = self.contact_groups()?;
        let (reversed, open_aliases) = self.orient_ideal_forest(&groups)?;
        self.diagnostics.push(&codes::EMIT_GLM_IDEAL_CONTACTS,
            "ideal contacts and winding constraints select native zero-impedance FBS output; conducting links and required open-island attachments form a checked source-oriented forest; redundant fully open ties use unloaded terminal aliases");
        for name in &reversed {
            self.diagnostics.push(&codes::EMIT_GLM_IDEAL_CONTACTS,
                format!("link {name}: native endpoints reversed to point away from the source; conductor identities and electrical parameters are unchanged"));
        }
        self.ideal_plan = Some(IdealPlan {
            groups,
            reversed,
            open_aliases,
        });
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
                group.from = &group.members[0].bus_from;
                group.to = &group.members[0].bus_to;
                group.members[0].name.clone()
            } else {
                self.generated_name(&format!("{}__{}__glm_contact", group.from, group.to))
            };
        }

        Ok(groups)
    }

    #[allow(clippy::too_many_lines)] // One forest check over contracted electrical devices.
    fn orient_ideal_forest(
        &self,
        groups: &[ContactGroup<'a>],
    ) -> Result<(BTreeSet<String>, BTreeSet<String>), Error> {
        // Conducting paths set the forest first. A redundant fully open link
        // uses unloaded aliases, avoiding native rank cycles and bus-current
        // contamination. Other open links retain their island attachment.
        // Contract only private core circuits certified by Services::discover.
        type Link<'b> = (&'b str, &'b str, Vec<usize>, Vec<usize>, bool);
        let mut links: BTreeMap<&str, Link<'_>> = BTreeMap::new();
        let codes: BTreeMap<_, _> = self
            .net
            .line_codes()
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();
        for line in self.net.lines() {
            let service = self.services.buses.contains_key(line.bus_from.as_str());
            let phases = if service {
                self.phases[line.bus_from.as_str()].clone()
            } else {
                self.link_phases(
                    &line.name,
                    &line.bus_from,
                    &line.bus_to,
                    &line.terminal_map_from,
                    &line.terminal_map_to,
                )?
            };
            let code = codes.get(line.linecode.as_str()).ok_or_else(|| {
                unsupported(format!("line {} has no exact linecode identity", line.name))
            })?;
            validate_line_matrices(
                line,
                code,
                if service {
                    line.terminal_map_from.len()
                } else {
                    phases.len()
                },
            )?;
            links.insert(
                &line.name,
                (
                    &line.bus_from,
                    &line.bus_to,
                    phases.clone(),
                    phases,
                    !service,
                ),
            );
        }
        for group in groups {
            let phases: Vec<_> = group.states.keys().copied().collect();
            links.insert(
                &group.name,
                (group.from, group.to, phases.clone(), phases, true),
            );
        }
        for (&name, group) in &self.regulators.groups {
            let phases = group
                .members
                .iter()
                .map(|t| super::regulators::phase(t))
                .collect::<Result<Vec<_>, _>>()?;
            links.insert(name, (group.from, group.to, phases.clone(), phases, true));
        }
        for t in self.net.transformers() {
            if self.regulators.leaders.contains_key(t.name.as_str()) {
                continue;
            }
            if let Some(&leader) = self.coil_banks.leaders.get(t.name.as_str()) {
                if leader != t.name {
                    continue;
                }
                links.insert(
                    &t.name,
                    (
                        &t.windings[0].bus,
                        &t.windings[1].bus,
                        vec![0, 1, 2],
                        vec![0, 1, 2],
                        false,
                    ),
                );
            } else if let Some(center) = self.services.centers.get(t.name.as_str()) {
                links.insert(
                    &t.name,
                    (
                        &t.windings[0].bus,
                        &t.windings[1].bus,
                        vec![center.phase],
                        vec![0, 1],
                        false,
                    ),
                );
            } else {
                return Err(unsupported(format!(
                    "transformer {} needs a qualified native FBS winding/core mapping",
                    t.name
                )));
            }
        }
        let mut neighbors: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
        for (&name, (from, to, from_phases, to_phases, _)) in &links {
            for (bus, link_phases) in [(*from, from_phases), (*to, to_phases)] {
                if self
                    .phases
                    .get(bus)
                    .is_none_or(|p| link_phases.iter().any(|q| !p.contains(q)))
                {
                    return Err(unsupported(format!(
                        "FBS link {name} uses an absent endpoint or phase"
                    )));
                }
            }
            neighbors.entry(from).or_default().push((to, name));
            neighbors.entry(to).or_default().push((from, name));
        }
        let mut seen: BTreeSet<&str> = self.source_voltages.keys().copied().collect();
        let mut queue: VecDeque<_> = seen.iter().copied().collect();
        let mut edges = BTreeSet::new();
        let mut reversed = BTreeSet::new();
        let open_names: BTreeSet<_> = groups
            .iter()
            .filter(|g| g.states.values().all(|&open| open))
            .map(|g| g.name.as_str())
            .collect();
        let mut admitted_open = BTreeSet::new();
        let mut open_aliases = BTreeSet::new();
        loop {
            while let Some(bus) = queue.pop_front() {
                for &(neighbor, name) in neighbors.get(bus).into_iter().flatten() {
                    if open_names.contains(name) && !admitted_open.contains(name) {
                        continue;
                    }
                    if !edges.insert(name) {
                        continue;
                    }
                    if !seen.insert(neighbor) {
                        return Err(unsupported(
                            "ideal FBS output requires a forest with one source bus per component; cycles, parallel links and connected multiple sources need another exact equivalent",
                        ));
                    }
                    let (from, _, from_phases, to_phases, can_reverse) = &links[name];
                    let reverse = *from != bus;
                    let incoming = if reverse { from_phases } else { to_phases };
                    if self.phases[neighbor].iter().any(|p| !incoming.contains(p)) {
                        return Err(unsupported(format!(
                            "FBS bus {neighbor} has phases absent from its incoming link; disconnected terminal output needs another exact equivalent"
                        )));
                    }
                    if reverse {
                        if !can_reverse {
                            return Err(unsupported(format!(
                                "FBS link {name} needs a qualified reversed service or transformer mapping"
                            )));
                        }
                        reversed.insert(name.to_owned());
                    }
                    queue.push_back(neighbor);
                }
            }
            let next = groups.iter().find(|g| {
                open_names.contains(g.name.as_str())
                    && !edges.contains(g.name.as_str())
                    && (seen.contains(g.from) || seen.contains(g.to))
            });
            let Some(group) = next else {
                break;
            };
            if seen.contains(group.from) && seen.contains(group.to) {
                edges.insert(group.name.as_str());
                open_aliases.insert(group.name.clone());
            } else {
                admitted_open.insert(group.name.as_str());
                queue.push_back(if seen.contains(group.from) {
                    group.from
                } else {
                    group.to
                });
            }
        }
        if seen.len() + self.services.core_buses.len() != self.net.buses().len() {
            return Err(unsupported(
                "ideal FBS output has a component without a fixed source",
            ));
        }
        Ok((reversed, open_aliases))
    }

    fn open_contact_alias(&mut self, name: &str, side: &str, bus: &str, phases: &str) -> String {
        let alias = self.generated_name(&format!("{name}__glm_open_{side}"));
        let voltage_error = self.voltage_error();
        let _ = writeln!(
            self.out,
            "object node {{ name \"{alias}\"; parent \"{bus}\"; phases {phases}N; nominal_voltage {}; maximum_voltage_error {voltage_error}; }}",
            self.nominal[bus]
        );
        alias
    }

    pub(super) fn ideal_contacts(&mut self, plan: &IdealPlan<'a>) {
        for group in &plan.groups {
            let (from_bus, to_bus) = if plan.reversed.contains(&group.name) {
                (group.to, group.from)
            } else {
                (group.from, group.to)
            };
            let phases = flags(&group.states.keys().copied().collect::<Vec<_>>());
            let (from, to) = if plan.open_aliases.contains(&group.name) {
                let a = self.open_contact_alias(&group.name, "from", from_bus, &phases);
                let b = self.open_contact_alias(&group.name, "to", to_bus, &phases);
                self.diagnostics.push(&codes::EMIT_GLM_IDEAL_CONTACTS,format!("switch {}: fully open tie uses unloaded terminal aliases parented to {from_bus} and {to_bus}; both endpoint voltages are preserved and native current/loss contamination and rank cycles are avoided",group.name));
                (a, b)
            } else {
                (from_bus.to_owned(), to_bus.to_owned())
            };
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
            for (&phase, &open) in &group.states {
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

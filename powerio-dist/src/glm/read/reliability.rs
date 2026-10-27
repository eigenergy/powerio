//! Future manual outages and a fault checker that cannot remove declared phases.
use super::{Reader, class};
use crate::glm::{GlmDialect, ObjectKey, Statement};
use powerio_core::{Diagnostic, Error, SourceSpan};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(super) fn allowed(class: &str, name: &str) -> bool {
    match class {
        "eventgen" => matches!(name, "fault_type" | "manual_outages"),
        "fault_check" => matches!(
            name,
            "check_mode" | "strictly_radial" | "grid_association" | "eventgen_object"
        ),
        _ => false,
    }
}
impl Reader<'_> {
    pub(super) fn connected_swing_pq(
        &mut self,
        key: ObjectKey,
        root: ObjectKey,
    ) -> Result<(), Error> {
        let has_association = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "fault_check")
            .any(|o| {
                self.text(o.key(), "grid_association")
                    .ok()
                    .flatten()
                    .is_some_and(|v| matches!(v.as_str(), "true" | "TRUE" | "1"))
            });
        if self.project.dialect() != GlmDialect::Pnnl5_3
            || !self.newton_raphson
            || key != root
            || !has_association
        {
            return Err(self.error(key,"SWING_PQ requires the qualified NR grid-association profile and a connection to a fixed SWING source"));
        }
        // inactive_reliability subsequently proves every phase is connected to
        // a fixed SWING source. Native associate_grids disables SWING_PQ voltage
        // enforcement in that case; an island requiring promotion is refused.
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_CONTROL_FROZEN,
            format!("{}: grid-connected SWING_PQ remains a PQ bus; fixed-source phase coverage is required before returning the network",self.names[&key])));
        Ok(())
    }
    pub(super) fn reliability_module(
        &self,
        body: &[Statement],
        span: &SourceSpan,
    ) -> Result<(), Error> {
        for statement in body {
            if let Statement::Simple { tokens, .. } = statement
                && tokens.len() == 2
                && tokens[0].text() == "report_event_log"
                && matches!(
                    tokens[1].text(),
                    "true" | "false" | "TRUE" | "FALSE" | "0" | "1"
                )
            {
                continue;
            }
            return Err(self
                .project
                .electrical_error(span, "unsupported reliability module configuration"));
        }
        Ok(())
    }
    pub(super) fn inactive_reliability(&mut self) -> Result<(), Error> {
        let keys = self
            .index
            .objects()
            .iter()
            .filter(|o| matches!(class(o), "fault_check" | "eventgen"))
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return Ok(());
        }
        if self.project.dialect() != GlmDialect::Pnnl5_3 || !self.newton_raphson {
            return Err(self.error(
                keys[0],
                "inactive reliability declarations are qualified only for the PNNL NR profile",
            ));
        }
        for &key in &keys {
            if class(&self.index.objects()[key.ordinal()]) == "eventgen" {
                self.future_outages(key)?;
            } else {
                if self
                    .text(key, "check_mode")?
                    .is_some_and(|v| !matches!(v.as_str(), "SINGLE" | "ONCHANGE" | "ALL"))
                {
                    return Err(self.error(
                        key,
                        "fault_check mode requires additional static initialization mapping",
                    ));
                }
                for field in ["strictly_radial", "grid_association"] {
                    if self.text(key, field)?.is_some_and(|v| {
                        !matches!(v.as_str(), "true" | "false" | "TRUE" | "FALSE" | "0" | "1")
                    }) {
                        return Err(self.error(key, "fault_check flag must be boolean"));
                    }
                }
                if let Some(event) = self.index.resolve_property(key, "eventgen_object")?
                    && class(&self.index.objects()[event.ordinal()]) != "eventgen"
                {
                    return Err(
                        self.error(key, "fault_check eventgen_object must identify an eventgen")
                    );
                }
            }
        }
        self.require_supported_phases(keys[0])?;
        self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_CONTROL_FROZEN,
            "reliability: all declared bus phases reach a fixed source and manual outages occur on dates after the clock window; no protection, outage or restoration sequence is executed; declarations remain in source"));
        Ok(())
    }
    fn future_outages(&self, key: ObjectKey) -> Result<(), Error> {
        let end = self.clock_end(key)?;
        let schedule=self.text(key,"manual_outages")?.ok_or_else(||self.error(key,"eventgen requires explicit future manual_outages; stochastic events are not a static state"))?;
        let parts = schedule.split(',').map(str::trim).collect::<Vec<_>>();
        if parts.is_empty() || !parts.len().is_multiple_of(3) {
            return Err(self.error(key, "manual_outages requires object,start,restore triples"));
        }
        for event in parts.as_chunks::<3>().0 {
            self.index
                .resolve_reference(event[0])?
                .ok_or_else(|| self.error(key, "outage target cannot be root"))?;
            let start = calendar(event[1]).ok_or_else(|| {
                self.error(key, "outage start requires a local calendar timestamp")
            })?;
            let restore = calendar(event[2]).ok_or_else(|| {
                self.error(
                    key,
                    "outage restoration requires a local calendar timestamp",
                )
            })?;
            // Different local calendar dates avoid ambiguous repeated hours at
            // daylight-saving transitions. Same-date scheduling stays unqualified.
            if start[..3] <= end[..3] || restore <= start {
                return Err(self.error(key,"outage must start on a date after the entire clock window and restore later; active, prior and same-date events require explicit state mapping"));
            }
        }
        Ok(())
    }
    fn clock_end(&self, key: ObjectKey) -> Result<[u32; 6], Error> {
        let mut start = None;
        let mut end = None;
        let mut clocks = 0;
        for statement in self.project.statements() {
            if let Statement::Block { header, body, .. } = statement
                && header.first().is_some_and(|t| t.text() == "clock")
            {
                clocks += 1;
                for entry in body {
                    if let Statement::Simple { tokens, .. } = entry
                        && tokens.len() == 2
                    {
                        let field = tokens[0].text();
                        if matches!(field, "timestamp" | "starttime" | "stoptime") {
                            let value=calendar(tokens[1].text().trim_matches(['\'', '"'])).ok_or_else(||self.error(key,"reliability clock requires explicit local calendar timestamps"))?;
                            if field == "stoptime" {
                                end = Some(value);
                            } else {
                                start = Some(value);
                            }
                        }
                    }
                }
            }
        }
        let start = start
            .ok_or_else(|| self.error(key, "future outages require an explicit clock start"))?;
        let end = end.unwrap_or(start);
        if clocks != 1 || end < start {
            return Err(self.error(key, "future outages require one ordered clock window"));
        }
        Ok(end)
    }
    fn require_supported_phases(&self, key: ObjectKey) -> Result<(), Error> {
        let mut graph = BTreeMap::<(ObjectKey, usize), Vec<(ObjectKey, usize)>>::new();
        let closed = self
            .net
            .lines()
            .iter()
            .map(|l| (l.name.as_str(), l.terminal_map_from.as_slice()))
            .chain(
                self.net
                    .switches()
                    .iter()
                    .filter(|s| !s.open)
                    .map(|s| (s.name.as_str(), s.terminal_map_from.as_slice())),
            )
            .collect::<BTreeMap<_, _>>();
        for object in self.index.objects() {
            if !matches!(
                class(object),
                "overhead_line"
                    | "underground_line"
                    | "triplex_line"
                    | "transformer"
                    | "regulator"
                    | "switch"
                    | "series_reactor"
            ) {
                continue;
            }
            let from = self
                .index
                .resolve_property(object.key(), "from")?
                .and_then(|k| self.roots.get(&k).copied());
            let to = self
                .index
                .resolve_property(object.key(), "to")?
                .and_then(|k| self.roots.get(&k).copied());
            let (Some(from), Some(to)) = (from, to) else {
                return Err(self.error(key, "reliability link endpoints require resolved buses"));
            };
            let flags = self.text(object.key(), "phases")?.unwrap_or_default();
            for phase in flags
                .bytes()
                .filter(|c| matches!(c, b'A'..=b'C'))
                .map(|c| usize::from(c - b'A'))
            {
                if class(object) == "switch"
                    && !self
                        .attachments
                        .get(&object.key())
                        .into_iter()
                        .flatten()
                        .any(|(_, name)| {
                            closed
                                .get(name.as_str())
                                .is_some_and(|t| t.contains(&(phase + 1).to_string()))
                        })
                {
                    continue;
                }
                graph.entry((from, phase)).or_default().push((to, phase));
                graph.entry((to, phase)).or_default().push((from, phase));
            }
        }
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::new();
        for (&root, phases) in &self.phases {
            if self
                .net
                .sources()
                .iter()
                .any(|s| s.bus == self.names[&root])
            {
                for &phase in phases {
                    seen.insert((root, phase));
                    queue.push_back((root, phase));
                }
            }
        }
        while let Some(node) = queue.pop_front() {
            for &next in graph.get(&node).into_iter().flatten() {
                if seen.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        for (&root, phases) in &self.phases {
            let required = self
                .triplex
                .get(&root)
                .map_or_else(|| phases.clone(), |&p| vec![p]);
            if required.iter().any(|&p| !seen.contains(&(root, p))) {
                return Err(self.error(key,format!("fault checking could change unsupported phases at {}; explicit initialized island state is required",self.names[&root])));
            }
        }
        Ok(())
    }
}
fn calendar(text: &str) -> Option<[u32; 6]> {
    let parts = text
        .split(['-', ' ', ':'])
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let [year, month, day, hour, minute, second]: [u32; 6] = parts.try_into().ok()?;
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return None,
    };
    (year >= 1970 && day > 0 && day <= days && hour < 24 && minute < 60 && second < 60)
        .then_some([year, month, day, hour, minute, second])
}

//! Ordered ordinary Line interpretation for static snapshots.

use super::line_history::Snapshot;
use super::linecode;
use super::raw::RawDss;
use std::collections::{HashMap, HashSet};
mod state;
pub(super) use state::State;

pub(super) fn resolve(raw: &RawDss, point: Snapshot) -> Result<State, String> {
    let mut cache = HashMap::new();
    let mut codes = HashMap::new();
    let mut visiting = HashSet::new();
    let mut pending = vec![(point, false)];
    let mut budget = 1_usize << 22;
    while let Some((point, ready)) = pending.pop() {
        if cache.contains_key(&point) {
            continue;
        }
        let object = raw.objects.get(point.object).ok_or("missing Line object")?;
        if object.class != "line" {
            return Err("expected Line history".into());
        }
        let history = raw
            .line_history
            .get(&point.object)
            .ok_or("missing Line history")?;
        let copies = history
            .copies
            .get(..point.copies)
            .ok_or("invalid Line copy prefix")?;
        if ready {
            let state = replay(raw, point, &cache, &mut codes, &mut budget)?;
            charge(&mut budget, state.n * state.n * 6)?;
            cache.insert(point, state);
            visiting.remove(&point);
        } else {
            if !visiting.insert(point) {
                return Err("cyclic Line history".into());
            }
            charge(&mut budget, 1)?;
            pending.push((point, true));
            for copy in copies.iter().rev() {
                pending.push((copy.source, false));
            }
        }
    }
    cache
        .remove(&point)
        .ok_or_else(|| "missing resolved Line".into())
}

fn charge(budget: &mut usize, amount: usize) -> Result<(), String> {
    *budget = budget
        .checked_sub(amount)
        .ok_or("Line evaluation work limit exceeded")?;
    Ok(())
}

fn replay(
    raw: &RawDss,
    point: Snapshot,
    cache: &HashMap<Snapshot, State>,
    codes: &mut HashMap<Snapshot, linecode::State>,
    budget: &mut usize,
) -> Result<State, String> {
    let object = &raw.objects[point.object];
    let history = &raw.line_history[&point.object];
    let props = object
        .props
        .get(..point.properties)
        .ok_or("invalid Line property prefix")?;
    let edits = object
        .edits
        .get(..point.edits)
        .ok_or("invalid Line edit prefix")?;
    let events = history
        .events
        .get(..point.events)
        .ok_or("invalid Line calculation prefix")?;
    let copies = &history.copies[..point.copies];
    let mut state = State::new(history.constructor_frequency, &object.name);
    let (mut property, mut edit, mut copy, mut event) = (0, 0, 0, 0);
    loop {
        let next = copies.get(copy);
        if let Some(e) = events.get(event).filter(|e| {
            e.position.properties == property
                && e.position.edits == edit
                && e.position.copies == copy
        }) {
            charge(budget, state.n * state.n * 6)?;
            // Native BuildY marks SystemYChanged, not each Line YPrim.
            if let Some((terminal, conductor, closed)) = e.terminal_state {
                state.set_closed(terminal, conductor, closed)?;
            } else if !e.rebuild_only {
                state.solve(e.frequency)?;
            }
            event += 1;
        } else if edit < edits.len()
            && edits[edit] == property
            && next.is_none_or(|c| edit < c.edit_start)
        {
            // TLine.EndEdit does not recalculate electrical matrices.
            edit += 1;
        } else if let Some(c) = next.filter(|c| c.start == property && c.edit_start == edit) {
            if c.end < c.start
                || c.end > props.len()
                || c.edit_end < edit
                || c.edit_end > edits.len()
            {
                return Err("invalid Line copy span".into());
            }
            state.make_like(cache.get(&c.source).ok_or("unresolved Line copy")?);
            property = c.end;
            edit = c.edit_end;
            copy += 1;
        } else if property < props.len() {
            charge(budget, state.n * state.n * 6 + 1)?;
            if props[property].name.as_deref() == Some("linecode") {
                let point = history
                    .linecodes
                    .get(&property)
                    .ok_or("missing linecode binding")?
                    .ok_or("LineCode did not exist at assignment")?;
                if let std::collections::hash_map::Entry::Vacant(entry) = codes.entry(point) {
                    let code = linecode::resolve_with_budget(raw, point, budget)?;
                    charge(budget, code.n * code.n * 3)?;
                    entry.insert(code);
                }
                state.fetch_code(&codes[&point]);
            } else {
                state.apply(&props[property], raw)?;
            }
            property += 1;
        } else {
            break;
        }
    }
    if (edit, copy, event) != (edits.len(), copies.len(), events.len()) {
        return Err("unconsumed Line history".into());
    }
    Ok(state)
}

#[cfg(test)]
mod tests;

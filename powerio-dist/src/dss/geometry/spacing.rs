//! LineSpacing snapshots. Geometry assignment copies coordinates; units are metadata.
use crate::dss::{
    line_history::Snapshot,
    raw::{RawDss, RawProp},
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub(super) struct State {
    pub x: Vec<f64>,
    pub height: Vec<f64>,
    pub phases: usize,
    pub units: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            x: vec![0.0; 3],
            height: vec![0.0; 3],
            phases: 3,
            units: 5,
        }
    }
}

impl State {
    fn apply(&mut self, prop: &RawProp, raw: &RawDss) -> Result<(), String> {
        let key = prop.name.as_deref().ok_or("unnamed LineSpacing property")?;
        match key {
            "nconds" | "nphases" => {
                let value = prop
                    .value
                    .to_f64(Some(&raw.vars))
                    .map_err(|e| e.to_string())?;
                let minimum = usize::from(key == "nconds");
                if !value.is_finite()
                    || value.fract() != 0.0
                    || !(minimum as f64..=32.0).contains(&value)
                {
                    return Err(format!("invalid LineSpacing {key}"));
                }
                if key == "nphases" {
                    self.phases = value as usize;
                } else {
                    // Native realloc retains existing entries but does not initialize
                    // newly allocated positions. Require them to be set before use.
                    self.x.resize(value as usize, f64::NAN);
                    self.height.resize(value as usize, f64::NAN);
                    self.units = 5;
                }
            }
            "x" | "h" => {
                let values = prop
                    .value
                    .to_vector(Some(&raw.vars))
                    .map_err(|e| e.to_string())?;
                let target = if key == "x" {
                    &mut self.x
                } else {
                    &mut self.height
                };
                for (i, entry) in target.iter_mut().enumerate() {
                    *entry = values.get(i).copied().unwrap_or(0.0);
                    if !entry.is_finite() {
                        return Err(format!("nonfinite LineSpacing {key}"));
                    }
                }
            }
            "units" => self.units = crate::dss::linecode::units(&prop.value.text),
            _ => return Err(format!("unqualified LineSpacing property {key}")),
        }
        Ok(())
    }
}

pub(super) fn resolve_with_budget(
    raw: &RawDss,
    point: Snapshot,
    budget: &mut usize,
) -> Result<State, String> {
    let mut cache = HashMap::new();
    let mut visiting = HashSet::new();
    let mut pending = vec![(point, false)];
    while let Some((point, ready)) = pending.pop() {
        if cache.contains_key(&point) {
            continue;
        }
        let object = raw
            .objects
            .get(point.object)
            .ok_or("missing LineSpacing object")?;
        if object.class != "linespacing" {
            return Err("expected LineSpacing history".into());
        }
        let history = raw
            .line_history
            .get(&point.object)
            .ok_or("missing LineSpacing history")?;
        let copies = history
            .copies
            .get(..point.copies)
            .ok_or("invalid LineSpacing copy prefix")?;
        *budget = budget
            .checked_sub(32)
            .ok_or("LineSpacing evaluation work limit exceeded")?;
        if ready {
            let mut state = State::default();
            let props = object
                .props
                .get(..point.properties)
                .ok_or("invalid LineSpacing property prefix")?;
            let mut offset = 0;
            for copy in copies {
                let before = props
                    .get(offset..copy.start)
                    .ok_or("invalid LineSpacing copy span")?;
                *budget = budget
                    .checked_sub(before.len())
                    .ok_or("LineSpacing evaluation work limit exceeded")?;
                for prop in before {
                    state.apply(prop, raw)?;
                }
                state = cache
                    .get(&copy.source)
                    .cloned()
                    .ok_or("unresolved LineSpacing copy")?;
                if copy.source.object == point.object {
                    // Native self-copy resets Units before reading the aliased source.
                    state.units = 5;
                }
                offset = copy.end;
            }
            let remaining = props.get(offset..).ok_or("invalid LineSpacing copy span")?;
            *budget = budget
                .checked_sub(remaining.len())
                .ok_or("LineSpacing evaluation work limit exceeded")?;
            for prop in remaining {
                state.apply(prop, raw)?;
            }
            cache.insert(point, state);
            visiting.remove(&point);
        } else {
            if !visiting.insert(point) {
                return Err("cyclic LineSpacing history".into());
            }
            pending.push((point, true));
            for c in copies.iter().rev() {
                pending.push((c.source, false));
            }
        }
    }
    cache
        .remove(&point)
        .ok_or_else(|| "missing resolved LineSpacing".into())
}

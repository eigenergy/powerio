//! Ordered overhead LineGeometry definitions, retaining live wire identities.
use super::{EarthModel, Geometry, wire};
use crate::dss::{
    line_history::Snapshot,
    raw::{RawDss, RawProp},
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, serde::Serialize)]
pub(in crate::dss) struct Conductor {
    pub wire: Option<usize>,
    pub x: f64,
    pub height: f64,
    // Native -1 means not assigned. Selecting the conductor replaces it
    // with the last assigned unit, initially feet.
    pub units: Option<usize>,
}

#[derive(Clone, Debug)]
pub(in crate::dss) struct Definition {
    pub conductors: Vec<Conductor>,
    pub phases: usize,
    pub reduce: bool,
    pub normamps: f64,
    pub emergamps: f64,
    active: usize,
    last_unit: usize,
}

impl Default for Definition {
    fn default() -> Self {
        Self {
            conductors: Vec::new(),
            phases: 0,
            reduce: false,
            normamps: 0.0,
            emergamps: 0.0,
            active: 0,
            last_unit: 5,
        }
    }
}

impl Definition {
    fn resize(&mut self, n: usize) {
        self.conductors = vec![
            Conductor {
                wire: None,
                x: 0.0,
                height: 0.0,
                units: None
            };
            n
        ];
        self.active = 0;
        self.last_unit = 5;
    }

    fn copy(&mut self, source: &Self) {
        // NConds resets both selection fields even when the size is equal.
        self.resize(source.conductors.len());
        self.conductors.clone_from(&source.conductors);
        self.phases = source.phases;
        self.reduce = source.reduce;
        self.normamps = source.normamps;
        self.emergamps = source.emergamps;
    }

    fn bind(
        &mut self,
        index: usize,
        point: Snapshot,
        raw: &RawDss,
        budget: &mut usize,
    ) -> Result<(), String> {
        self.conductors
            .get_mut(index)
            .ok_or("invalid geometry conductor selector")?
            .wire = Some(point.object);
        // Native copies ratings from the conductor's state NOW, while the
        // eventual matrix calculation reads the live conductor object.
        if index == 0 {
            let source = wire::resolve_with_budget(raw, point, budget)?;
            if self.normamps == 0.0 && source.normamps > 0.0 {
                self.normamps = source.normamps;
            }
            if self.emergamps == 0.0 && source.emergamps > 0.0 {
                self.emergamps = source.emergamps;
            }
        }
        Ok(())
    }

    fn apply(
        &mut self,
        raw: &RawDss,
        object: usize,
        offset: usize,
        prop: &RawProp,
        budget: &mut usize,
    ) -> Result<(), String> {
        let key = prop
            .name
            .as_deref()
            .ok_or("unnamed LineGeometry property")?;
        let number = || -> Result<f64, String> {
            let value = prop
                .value
                .to_f64(Some(&raw.vars))
                .map_err(|e| e.to_string())?;
            if value.is_finite() {
                Ok(value)
            } else {
                Err(format!("nonfinite LineGeometry {key}"))
            }
        };
        let count = |minimum: usize| -> Result<usize, String> {
            let value = number()?;
            if value.fract() != 0.0 || !(minimum as f64..=32.0).contains(&value) {
                return Err(format!("invalid LineGeometry {key}"));
            }
            Ok(value as usize)
        };
        match key {
            "nconds" => self.resize(count(1)?),
            "nphases" => self.phases = count(0)?,
            "cond" => {
                self.active = count(1)? - 1;
                self.conductors
                    .get_mut(self.active)
                    .ok_or("invalid geometry conductor selector")?
                    .units
                    .get_or_insert(self.last_unit);
            }
            "x" | "h" | "units" => {
                let conductor = self
                    .conductors
                    .get_mut(self.active)
                    .ok_or("geometry has no active conductor")?;
                match key {
                    "x" => conductor.x = number()?,
                    "h" => conductor.height = number()?,
                    _ => {
                        let units = crate::dss::linecode::units(&prop.value.text);
                        conductor.units = Some(units);
                        self.last_unit = units;
                    }
                }
            }
            "wire" | "wires" => {
                let refs = raw.line_history[&object]
                    .wires
                    .get(&offset)
                    .ok_or("missing WireData binding")?;
                let count = if key == "wire" {
                    1
                } else {
                    self.conductors.len()
                };
                if refs.len() != count || count == 0 {
                    return Err("geometry wire array has wrong length".into());
                }
                for (index, point) in refs.iter().enumerate() {
                    self.bind(
                        if key == "wire" { self.active } else { index },
                        point.ok_or("WireData did not exist at assignment")?,
                        raw,
                        budget,
                    )?;
                }
                if key == "wires" {
                    self.active = count - 1;
                }
            }
            "normamps" => self.normamps = number()?,
            "emergamps" => self.emergamps = number()?,
            "reduce" => self.reduce = prop.value.text.starts_with(['y', 'Y', 't', 'T']),
            // These are retained metadata, not inputs to static impedance.
            "seasons" | "ratings" | "linetype" => {}
            _ => return Err(format!("unqualified LineGeometry property {key}")),
        }
        Ok(())
    }

    /// The caller chooses the WireData prefixes visible at this calculation.
    /// Matrix/cache replay must not substitute final wire values for a past
    /// native calculation. Definition identity and coordinates remain separate.
    pub fn materialize(
        &self,
        raw: &RawDss,
        earth_model: EarthModel,
        rho: f64,
        mut visible_wire: impl FnMut(usize) -> Snapshot,
    ) -> Result<Geometry, String> {
        let mut budget = 1_usize << 22;
        let wires = self
            .conductors
            .iter()
            .map(|conductor| {
                let object = conductor.wire.ok_or("geometry conductor has no WireData")?;
                let state = wire::resolve_with_budget(raw, visible_wire(object), &mut budget)?;
                let meters = wire::meters(conductor.units.unwrap_or(0));
                Ok(state.at(conductor.x * meters, conductor.height * meters))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Geometry {
            wires,
            phases: self.phases,
            reduce: self.reduce,
            earth_model,
            rho,
        })
    }
}

pub(super) fn resolve(raw: &RawDss, point: Snapshot) -> Result<Definition, String> {
    let mut cache = HashMap::new();
    let mut visiting = HashSet::new();
    let mut pending = vec![(point, false)];
    let mut budget = 1_usize << 22;
    while let Some((point, ready)) = pending.pop() {
        if cache.contains_key(&point) {
            continue;
        }
        let object = raw
            .objects
            .get(point.object)
            .ok_or("missing LineGeometry object")?;
        if object.class != "linegeometry" {
            return Err("expected LineGeometry history".into());
        }
        let history = raw
            .line_history
            .get(&point.object)
            .ok_or("missing LineGeometry history")?;
        let copies = history
            .copies
            .get(..point.copies)
            .ok_or("invalid LineGeometry copy prefix")?;
        budget = budget
            .checked_sub(1)
            .ok_or("LineGeometry evaluation work limit exceeded")?;
        if ready {
            let mut state = Definition::default();
            let props = object
                .props
                .get(..point.properties)
                .ok_or("invalid LineGeometry property prefix")?;
            let mut offset = 0;
            for copy in copies {
                if copy.source.object == point.object {
                    return Err(
                        "native LineGeometry self-copy resets its own conductor data".into(),
                    );
                }
                for (i, prop) in props
                    .get(offset..copy.start)
                    .ok_or("invalid geometry copy span")?
                    .iter()
                    .enumerate()
                {
                    state.apply(raw, point.object, offset + i, prop, &mut budget)?;
                }
                budget = budget
                    .checked_sub(copy.start - offset + 32)
                    .ok_or("LineGeometry evaluation work limit exceeded")?;
                state.copy(
                    cache
                        .get(&copy.source)
                        .ok_or("unresolved LineGeometry copy")?,
                );
                offset = copy.end;
            }
            let remaining = props.get(offset..).ok_or("invalid geometry copy span")?;
            budget = budget
                .checked_sub(remaining.len())
                .ok_or("LineGeometry evaluation work limit exceeded")?;
            for (i, prop) in remaining.iter().enumerate() {
                state.apply(raw, point.object, offset + i, prop, &mut budget)?;
            }
            cache.insert(point, state);
            visiting.remove(&point);
        } else {
            if !visiting.insert(point) {
                return Err("cyclic LineGeometry history".into());
            }
            pending.push((point, true));
            for copy in copies.iter().rev() {
                pending.push((copy.source, false));
            }
        }
    }
    cache
        .remove(&point)
        .ok_or_else(|| "missing resolved LineGeometry".into())
}

#[cfg(test)]
mod tests;

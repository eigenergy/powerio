//! Ordered native WireData values, before conversion to SI.
use crate::dss::{
    line_history::Snapshot,
    raw::{RawDss, RawProp},
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct State {
    pub rdc: f64,
    pub rac: f64,
    pub runits: usize,
    pub gmr: f64,
    pub gmrunits: usize,
    pub radius: f64,
    pub radunits: usize,
    pub capacitance_radius: f64,
    pub normamps: f64,
    pub emergamps: f64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            rdc: -1.0,
            rac: -1.0,
            runits: 0,
            gmr: -1.0,
            gmrunits: 0,
            radius: -1.0,
            radunits: 0,
            capacitance_radius: -1.0,
            normamps: -1.0,
            emergamps: -1.0,
        }
    }
}

impl State {
    fn apply(&mut self, prop: &RawProp, raw: &RawDss) -> Result<(), String> {
        let key = prop.name.as_deref().ok_or("unnamed WireData property")?;
        let number = || -> Result<f64, String> {
            let value = prop
                .value
                .to_f64(Some(&raw.vars))
                .map_err(|e| e.to_string())?;
            if !value.is_finite() {
                return Err(format!("nonfinite WireData {key}"));
            }
            Ok(value)
        };
        let units = || crate::dss::linecode::units(&prop.value.text);
        match key {
            "rdc" => {
                self.rdc = number()?;
                if self.rac < 0.0 {
                    self.rac = self.rdc * 1.02;
                }
            }
            "rac" => {
                self.rac = number()?;
                if self.rdc < 0.0 {
                    self.rdc = self.rac / 1.02;
                }
            }
            "runits" => self.runits = units(),
            "gmrac" => {
                self.gmr = number()?;
                if self.gmr <= 0.0 {
                    return Err("WireData GMR must be positive".into());
                }
                if self.radius < 0.0 {
                    self.radius = self.gmr / 0.7788;
                }
            }
            "gmrunits" => {
                self.gmrunits = units();
                if self.radunits == 0 {
                    self.radunits = self.gmrunits;
                }
            }
            "radius" | "diam" => {
                self.radius = number()? / if key == "diam" { 2.0 } else { 1.0 };
                if self.radius <= 0.0 {
                    return Err("WireData radius must be positive".into());
                }
                if self.gmr < 0.0 {
                    self.gmr = 0.7788 * self.radius;
                }
                if self.capacitance_radius < 0.0 {
                    self.capacitance_radius = self.radius;
                }
            }
            "radunits" => {
                self.radunits = units();
                if self.gmrunits == 0 {
                    self.gmrunits = self.radunits;
                }
            }
            "capradius" => {
                self.capacitance_radius = number()?;
                if self.capacitance_radius == 0.0 {
                    return Err("WireData capacitance radius cannot be zero".into());
                }
            }
            "normamps" => {
                self.normamps = number()?;
                if self.emergamps < 0.0 {
                    self.emergamps = 1.5 * self.normamps;
                }
            }
            "emergamps" => {
                self.emergamps = number()?;
                if self.normamps < 0.0 {
                    self.normamps = self.emergamps / 1.5;
                }
            }
            // Seasonal ratings are retained in the source; they do not alter
            // this static conductor impedance or the two scalar ratings.
            "seasons" | "ratings" => {}
            _ => return Err(format!("unqualified WireData property {key}")),
        }
        Ok(())
    }

    pub fn at(&self, x: f64, height: f64) -> super::Wire {
        super::Wire {
            x,
            height,
            radius: self.radius * meters(self.radunits),
            // LineConstants uses physical radius when the stored capradius
            // remains negative (e.g. a GMR-only conductor definition).
            capacitance_radius: if self.capacitance_radius < 0.0 {
                self.radius
            } else {
                self.capacitance_radius
            } * meters(self.radunits),
            gmr: self.gmr * meters(self.gmrunits),
            rdc: self.rdc / meters(self.runits),
            rac: self.rac / meters(self.runits),
        }
    }
}

pub(super) fn meters(units: usize) -> f64 {
    // DSS To_Meters interprets unspecified units as meters. Unit setters
    // change interpretation, not the stored numeric values.
    [
        1.0, 1609.344, 304.8, 1000.0, 1.0, 0.3048, 0.0254, 0.01, 0.001,
    ][units]
}

#[cfg(test)]
pub(super) fn resolve(raw: &RawDss, point: Snapshot) -> Result<State, String> {
    resolve_with_budget(raw, point, &mut (1_usize << 22))
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
            .ok_or("missing WireData object")?;
        if object.class != "wiredata" {
            return Err("expected WireData history".into());
        }
        let history = raw
            .line_history
            .get(&point.object)
            .ok_or("missing WireData history")?;
        let copies = history
            .copies
            .get(..point.copies)
            .ok_or("invalid WireData copy prefix")?;
        *budget = budget
            .checked_sub(1)
            .ok_or("WireData evaluation work limit exceeded")?;
        if ready {
            let mut state = State::default();
            let props = object
                .props
                .get(..point.properties)
                .ok_or("invalid WireData property prefix")?;
            let mut offset = 0;
            for copy in copies {
                let before = props
                    .get(offset..copy.start)
                    .ok_or("invalid WireData copy span")?;
                *budget = budget
                    .checked_sub(before.len())
                    .ok_or("WireData evaluation work limit exceeded")?;
                for prop in before {
                    state.apply(prop, raw)?;
                }
                state = cache
                    .get(&copy.source)
                    .cloned()
                    .ok_or("unresolved WireData copy")?;
                offset = copy.end;
            }
            let remaining = props.get(offset..).ok_or("invalid WireData copy span")?;
            *budget = budget
                .checked_sub(remaining.len())
                .ok_or("WireData evaluation work limit exceeded")?;
            for prop in remaining {
                state.apply(prop, raw)?;
            }
            cache.insert(point, state);
            visiting.remove(&point);
        } else {
            if !visiting.insert(point) {
                return Err("cyclic WireData history".into());
            }
            pending.push((point, true));
            for c in copies.iter().rev() {
                pending.push((c.source, false));
            }
        }
    }
    cache
        .remove(&point)
        .ok_or_else(|| "missing resolved WireData".into())
}

#[cfg(test)]
mod tests;

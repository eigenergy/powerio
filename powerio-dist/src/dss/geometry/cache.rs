//! Shared native geometry calculation state; line caches are separate.
use super::{EarthModel, Geometry, definition, wire};
use crate::dss::{line_history::Snapshot, linecode::LineMatrices, raw::RawDss};
use std::collections::BTreeMap;

#[derive(Debug)]
struct Entry {
    point: Snapshot,
    allocated: usize,
    cable_allocations: usize,
    dirty: bool,
    rho: f64,
    rho_changed: bool,
    input: Option<Geometry>,
    matrices: Option<(f64, LineMatrices)>,
}

pub(in crate::dss) struct Context<'a> {
    raw: &'a RawDss,
    visible: BTreeMap<usize, Snapshot>,
    entries: BTreeMap<usize, Entry>,
    pub active_earth: Result<EarthModel, String>,
}

impl<'a> Context<'a> {
    pub fn new(raw: &'a RawDss) -> Self {
        Self {
            raw,
            visible: BTreeMap::new(),
            entries: BTreeMap::new(),
            active_earth: raw.geometry_earth.clone().unwrap_or(Ok(EarthModel::Deri)),
        }
    }

    pub fn observe(&mut self, point: Snapshot, key: Option<&str>) -> Result<(), String> {
        self.visible.insert(point.object, point);
        if self.raw.objects[point.object].class != "linegeometry" {
            return Ok(());
        }
        let entry = self.entries.entry(point.object).or_insert_with(|| Entry {
            point,
            allocated: 0,
            cable_allocations: 0,
            dirty: true,
            rho: 100.0,
            rho_changed: true,
            input: None,
            matrices: None,
        });
        entry.point = point;
        if matches!(key, Some("nconds" | "like" | "cncable" | "cncables")) {
            let definition = definition::resolve(self.raw, point)?;
            if definition.cable_allocations != entry.cable_allocations {
                entry.cable_allocations = definition.cable_allocations;
                entry.input = None;
                entry.matrices = None;
            }
            if definition.conductors.len() != entry.allocated {
                entry.allocated = definition.conductors.len();
                entry.rho = 100.0;
                entry.input = None;
                entry.matrices = None;
                entry.rho_changed = true;
            }
        }
        if key == Some("nphases")
            && let Some(input) = &mut entry.input
        {
            // Native changes the cable phase count immediately, but does not
            // invalidate a matrix already calculated at this frequency.
            input.phases = definition::resolve(self.raw, point)?
                .phases
                .min(input.wires.len());
        }
        if matches!(
            key,
            Some(
                "nconds"
                    | "wire"
                    | "x"
                    | "h"
                    | "units"
                    | "spacing"
                    | "wires"
                    | "cncable"
                    | "tscable"
                    | "cncables"
                    | "tscables"
                    | "like"
            )
        ) {
            entry.dirty = true;
        }
        Ok(())
    }

    pub fn spacing(&self, object: usize) -> Result<super::spacing::State, String> {
        let point = self
            .visible
            .get(&object)
            .ok_or("LineSpacing is not visible")?;
        super::spacing::resolve_with_budget(self.raw, *point, &mut (1_usize << 22))
    }

    pub fn conductor(&self, object: usize) -> Result<wire::State, String> {
        let point = self
            .visible
            .get(&object)
            .ok_or("conductor is not visible")?;
        wire::resolve_with_budget(self.raw, *point, &mut (1_usize << 22))
    }

    pub fn definition(&self, object: usize) -> Result<definition::Definition, String> {
        let point = self
            .entries
            .get(&object)
            .ok_or("LineGeometry did not exist at assignment")?
            .point;
        definition::resolve(self.raw, point)
    }

    pub fn set_rho(&mut self, object: usize, rho: f64) -> Result<(), String> {
        let entry = self
            .entries
            .get_mut(&object)
            .ok_or("missing geometry cache")?;
        if !rho.is_finite() || rho <= 0.0 {
            return Err("geometry earth resistivity must be positive finite".into());
        }
        entry.rho_changed |= entry.rho.to_bits() != rho.to_bits();
        entry.rho = rho;
        Ok(())
    }

    pub fn calculate(&mut self, object: usize, frequency: f64) -> Result<LineMatrices, String> {
        let entry = self
            .entries
            .get_mut(&object)
            .ok_or("missing geometry cache")?;
        if entry.dirty {
            let definition = definition::resolve(self.raw, entry.point)?;
            let mut input = definition.materialize(
                self.raw,
                self.active_earth.clone()?,
                entry.rho,
                |object| self.visible[&object],
            )?;
            let n = input.wires.len();
            // Calc retains any previous Kron order. Reduce can replace that
            // order, but turning Reduce off does not discard a cached reduction.
            let old_keep = entry
                .matrices
                .as_ref()
                .map(|(_, m)| m.r.len())
                .filter(|&keep| keep < n);
            let keep = if definition.reduce && definition.phases > 0 && definition.phases < n {
                Some(definition.phases)
            } else {
                old_keep
            };
            input.reduction_order = keep;
            entry.input = Some(input);
        }
        if entry.dirty
            || entry.rho_changed
            || entry
                .matrices
                .as_ref()
                .is_none_or(|(f, _)| f.to_bits() != frequency.to_bits())
        {
            let input = entry
                .input
                .as_mut()
                .ok_or("geometry has no calculated conductor data")?;
            input.rho = entry.rho;
            input.earth_model = self.active_earth.clone()?;
            entry.matrices = Some((frequency, input.calculate(frequency)?));
            entry.dirty = false;
            entry.rho_changed = false;
        }
        Ok(entry.matrices.as_ref().unwrap().1.clone())
    }

    pub fn scale(matrix: &mut LineMatrices, length: f64, units: usize) {
        let factor = length * wire::meters(units);
        for values in [&mut matrix.r, &mut matrix.x, &mut matrix.b] {
            for value in values.iter_mut().flatten() {
                *value *= factor;
            }
        }
    }
}

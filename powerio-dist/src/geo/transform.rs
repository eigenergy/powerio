//! Point attachment edits shared by network transformations.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{DistComponentLocation, DistLocationTarget, MulticonductorNetwork};

/// Preserve point order and exact identities without scanning the point table
/// for every rewritten component. Callers validate the input before building it.
pub(crate) struct ComponentLocationEdits {
    points: Vec<Option<DistComponentLocation>>,
    index: BTreeMap<(DistLocationTarget, String), usize>,
}

impl ComponentLocationEdits {
    pub(crate) fn new(network: &MulticonductorNetwork) -> Self {
        Self {
            points: network
                .component_locations()
                .iter()
                .cloned()
                .map(Some)
                .collect(),
            index: network
                .component_locations()
                .iter()
                .enumerate()
                .map(|(i, p)| ((p.target, p.name.clone()), i))
                .collect(),
        }
    }

    /// Move a point to a replacement component, or retain an omitted component's
    /// point in the caller's existing audit action. Coordinates never change.
    pub(crate) fn rewrite(
        &mut self,
        source: DistLocationTarget,
        name: &str,
        replacement: Option<(DistLocationTarget, &str)>,
        details: &mut BTreeMap<String, Value>,
    ) {
        let Some(index) = self.index.remove(&(source, name.to_owned())) else {
            return;
        };
        let mut point = self.points[index].take().expect("indexed point exists");
        details.insert("source_location".to_owned(), json!(point));
        if let Some((target, name)) = replacement {
            point.target = target;
            name.clone_into(&mut point.name);
            self.index.insert((target, name.to_owned()), index);
            self.points[index] = Some(point);
        }
    }

    /// Copy a configuration point to a derived configuration. The original
    /// configuration and its point remain available for unreduced uses.
    pub(crate) fn copy_linecode(
        &mut self,
        source: &str,
        target: &str,
        details: &mut BTreeMap<String, Value>,
    ) -> Result<bool, String> {
        let Some(&index) = self
            .index
            .get(&(DistLocationTarget::LineCode, source.to_owned()))
        else {
            return Ok(false);
        };
        if self.index.len() >= super::MAX_COMPONENT_LOCATIONS {
            return Err("derived component locations exceed the record limit".to_owned());
        }
        let mut point = self.points[index]
            .as_ref()
            .expect("indexed point exists")
            .clone();
        details.insert("source_location".to_owned(), json!(point));
        target.clone_into(&mut point.name);
        self.index
            .insert((point.target, point.name.clone()), self.points.len());
        self.points.push(Some(point));
        Ok(true)
    }

    pub(crate) fn apply(self, network: &mut MulticonductorNetwork) {
        if !self.points.is_empty() {
            *network.component_locations_mut() = self.points.into_iter().flatten().collect();
        }
    }
}

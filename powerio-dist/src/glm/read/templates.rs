//! Passive equipment libraries do not instantiate electrical devices.
use super::{Reader, class};
use crate::glm::ObjectKey;
use powerio_core::Error;
use std::collections::BTreeSet;

pub(super) fn is_template(class: &str) -> bool {
    matches!(
        class,
        "line_configuration"
            | "line_spacing"
            | "overhead_line_conductor"
            | "underground_line_conductor"
            | "transformer_configuration"
            | "regulator_configuration"
            | "triplex_line_configuration"
            | "triplex_line_conductor"
    )
}

impl Reader<'_> {
    pub(super) fn used_templates(&self) -> Result<BTreeSet<ObjectKey>, Error> {
        let mut pending = self
            .index
            .objects()
            .iter()
            .filter(|o| !is_template(class(o)))
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        while let Some(key) = pending.pop() {
            if !visited.insert(key) {
                continue;
            }
            // These are equipment references, not arbitrary string properties.
            // Quantity expressions cannot dereference object properties in the
            // current frontend. Unsupported active classes still fail inventory.
            for field in [
                "configuration",
                "spacing",
                "conductor_A",
                "conductor_B",
                "conductor_C",
                "conductor_N",
                "conductor_1",
                "conductor_2",
            ] {
                if let Some(target) = self.index.resolve_property(key, field)? {
                    pending.push(target);
                }
            }
        }
        Ok(visited)
    }
}

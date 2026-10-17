//! The unique upstream root of a directed FBS tree keeps its fixed phasors.
use super::{Reader, class};
use crate::glm::ObjectKey;
use powerio_core::{Diagnostic, Error};
use std::collections::{BTreeMap, BTreeSet};

impl Reader<'_> {
    pub(super) fn implicit_fbs_source(&mut self) -> Result<Option<ObjectKey>, Error> {
        if self.newton_raphson {
            return Ok(None);
        }
        for &key in self.roots.keys() {
            if self.text(key, "bustype")?.as_deref() == Some("SWING") {
                return Ok(None);
            }
        }
        let mut candidates: BTreeSet<_> = self.roots.values().copied().collect();
        let mut incoming = BTreeSet::new();
        let mut children: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for object in self.index.objects() {
            let key = object.key();
            if matches!(class(object), "switch" | "regulator") {
                return Err(self.error(
                    key,
                    "implicit FBS source with switches/regulators is not qualified",
                ));
            }
            if !matches!(
                class(object),
                "overhead_line" | "underground_line" | "transformer"
            ) {
                continue;
            }
            let from = self
                .index
                .resolve_property(key, "from")?
                .ok_or_else(|| self.error(key, "FBS link requires from"))?;
            let to = self
                .index
                .resolve_property(key, "to")?
                .ok_or_else(|| self.error(key, "FBS link requires to"))?;
            if self.roots.get(&from) != Some(&from)
                || self.roots.get(&to) != Some(&to)
                || !incoming.insert(to)
            {
                return Err(self.error(key, "implicit FBS source requires unparented endpoints and one incoming link per bus"));
            }
            candidates.remove(&to);
            children.entry(from).or_default().push(to);
        }
        let root_count = self.roots.values().collect::<BTreeSet<_>>().len();
        let Some(&source) = candidates.first().filter(|_| candidates.len() == 1) else {
            return Err(self.project.electrical_error(
                self.index
                    .objects()
                    .first()
                    .ok_or_else(|| {
                        Error::new(
                            &crate::diagnostics::codes::READ_GLM_ELECTRICAL_UNSUPPORTED,
                            "FBS input has no electrical bus",
                        )
                    })?
                    .span(),
                "implicit FBS source requires one connected directed tree",
            ));
        };
        let mut pending = vec![source];
        let mut visited = BTreeSet::new();
        while let Some(key) = pending.pop() {
            if !visited.insert(key) {
                return Err(self.error(
                    key,
                    "implicit FBS source cannot be inferred through a cycle",
                ));
            }
            pending.extend(children.get(&key).into_iter().flatten().copied());
        }
        if visited.len() != root_count {
            return Err(self.error(
                source,
                "implicit FBS source requires one connected directed tree",
            ));
        }
        self.diagnostics.push(Diagnostic::of(
            &crate::diagnostics::codes::READ_GLM_IMPLICIT_SOURCE,
            format!(
                "{}: the unique upstream root of this FBS tree supplies its fixed voltage phasors",
                self.names[&source]
            ),
        ));
        Ok(Some(source))
    }
}

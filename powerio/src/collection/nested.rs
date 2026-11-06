use super::{
    Arc, ArchiveLimits, ArtifactPath, Cancellation, Collection, CollectionEntry, CollectionLimits,
    CollectionSelection, Error, Result, SourceTreeLimits, codes, selection_error,
};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub(super) struct Session(Arc<Mutex<Usage>>);
#[derive(Debug)]
struct Usage {
    entries: usize,
    expanded: u64,
    work: u64,
}
impl Session {
    pub(super) fn new(entries: &[CollectionEntry]) -> Self {
        Self(Arc::new(Mutex::new(Usage {
            entries: entries.len(),
            expanded: entries.iter().map(|e| e.bytes).sum(),
            work: 0,
        })))
    }
    pub(super) fn charge(&self, bytes: u64, max_work: u64) -> Result<()> {
        let mut usage = self
            .0
            .lock()
            .map_err(|_| selection_error("collection budget lock poisoned"))?;
        charge_work(&mut usage, bytes, max_work)
    }
    pub(super) fn initialize_archive(
        &self,
        entries: usize,
        input: u64,
        max_work: u64,
    ) -> Result<()> {
        let mut usage = self
            .0
            .lock()
            .map_err(|_| selection_error("collection budget lock poisoned"))?;
        usage.entries = entries;
        usage.work = input.saturating_add(usage.expanded);
        if usage.work > max_work {
            return Err(Error::new(&codes::LIMIT, "collection work limit exceeded"));
        }
        Ok(())
    }
}
impl Collection {
    /// Open exactly one named child ZIP. Its namespace is independent: sources
    /// cannot resolve against a parent or sibling. All descendants and clones
    /// share monotonic count, expansion, and work budgets. Failed attempts may
    /// consume work; repeated opens/parses count again rather than reset limits.
    pub fn open_nested(&self, member: &str) -> Result<Self> {
        self.cancel.check()?;
        if self.zip_layers >= self.limits.max_zip_layers {
            return Err(Error::new(
                &codes::LIMIT,
                "ZIP decoding layer limit exceeded",
            ));
        }
        let path = ArtifactPath::new(member)?;
        let bytes = self
            .tree
            .source(None, None, self.limits.tree)?
            .buffer(&path)?
            .shared_bytes();
        let mut usage = self
            .session
            .0
            .lock()
            .map_err(|_| selection_error("collection budget lock poisoned"))?;
        let limits = self.remaining_limits(&usage)?;
        charge_work(&mut usage, bytes.len() as u64, self.limits.work_bytes)?;
        // Hold the budget lock during admission: sibling threads cannot each
        // allocate the same remaining allowance concurrently.
        // Reserve declared expansion before any payload read, including attempts
        // that later fail CRC validation. Repeated corrupt children cannot reset work.
        let archive = powerio_archive::Archive::open(Arc::clone(&bytes), limits.archive)?;
        let expanded = archive
            .entries()
            .map(powerio_archive::ArchiveEntry::size)
            .sum();
        charge_work(&mut usage, expanded, self.limits.work_bytes)?;
        charge_work(&mut usage, bytes.len() as u64, self.limits.work_bytes)?;
        usage.entries += archive.entries().len();
        usage.expanded += expanded;
        drop(archive);
        let mut child = Self::from_zip_cancellable(member, bytes, limits, self.cancel.clone())?;
        child.session = self.session.clone();
        child.limits = self.limits;
        child.zip_layers = self.zip_layers + 1;
        child.containers.clone_from(&self.containers);
        child.containers.push(member.to_owned());
        Ok(child)
    }
    /// Cancel subsequent discovery, child acquisition, and selected parses.
    /// ZIP reads check the signal between chunks. Electrical parsers retain
    /// their own bounded work rules; synchronous WASM may be terminated by its worker.
    pub fn cancellation(&self) -> Cancellation {
        self.cancel.clone()
    }

    fn remaining_limits(&self, usage: &Usage) -> Result<CollectionLimits> {
        let entries = self
            .limits
            .archive
            .max_entries()
            .min(self.limits.tree.max_files())
            .saturating_sub(usage.entries);
        let bytes = self
            .limits
            .archive
            .max_expanded_bytes()
            .min(self.limits.tree.max_bytes())
            .saturating_sub(usage.expanded);
        if entries == 0 || bytes == 0 {
            return Err(Error::new(
                &codes::LIMIT,
                "cumulative collection budget exhausted",
            ));
        }
        Ok(CollectionLimits {
            tree: SourceTreeLimits::new(entries, bytes, self.limits.tree.max_depth())?,
            archive: ArchiveLimits::new(
                self.limits.archive.max_input_bytes(),
                entries,
                bytes,
                self.limits.archive.max_ratio(),
            )?,
            work_bytes: self.limits.work_bytes.saturating_sub(usage.work),
            ..self.limits
        })
    }
    pub(super) fn charge_selection(&self, selected: &CollectionSelection) -> Result<()> {
        let mut usage = self
            .session
            .0
            .lock()
            .map_err(|_| selection_error("collection budget lock poisoned"))?;
        // Charge the scoped source, conservatively including retained companions.
        // This does not estimate model/Arrow allocation or replace parser guards.
        // A separate view keeps accounting reads out of parser observations.
        let source = self.source(selected)?;
        let primary = source.primary_buffer().ok();
        let mut buffers = Vec::new();
        for name in source.entry_names()? {
            if primary.as_ref().is_none_or(|p| p.name() != name.as_str()) {
                buffers.push(source.root_buffer(name.as_str())?);
            }
        }
        if let Some(primary) = primary {
            buffers.push(primary);
        }
        for buffer in buffers {
            let bytes = buffer.content_bytes();
            self.cancel.check()?;
            charge_work(&mut usage, bytes.len() as u64, self.limits.work_bytes)?;
            // Native ZIP layers are charged even for nonstandard file suffixes.
            // A signature indicates transport, never an electrical format choice.
            // Match adapter admission, including prefixed archives selected by
            // suffix and BOM-stripped ZIP signatures. Companion assets of a
            // DSS/GLM source are not themselves decoded by a native adapter.
            let extension = buffer.name().rsplit_once('.').map(|(_, e)| e);
            let sincal = matches!(
                selected.format_token(),
                Some("sincal" | "sincal-balanced" | "sincal-multiconductor")
            ) || selected.format.is_none()
                && extension.is_some_and(|e| e.eq_ignore_ascii_case("sinx"));
            let native = !selected.has_companions()
                && (sincal && !bytes.starts_with(b"SQLite format 3\0")
                    || extension.is_some_and(|e| e.eq_ignore_ascii_case("zip"))
                    || bytes.starts_with(b"PK\x03\x04")
                    || bytes.starts_with(b"PK\x05\x06"));
            if native {
                if self.zip_layers >= self.limits.max_zip_layers {
                    return Err(Error::new(
                        &codes::LIMIT,
                        "native ZIP exceeds decoding layer limit",
                    ));
                }
                let mut limits = self.remaining_limits(&usage)?;
                let remaining_work = self.limits.work_bytes.saturating_sub(usage.work) / 2;
                limits.archive = ArchiveLimits::new(
                    limits.archive.max_input_bytes(),
                    limits.archive.max_entries(),
                    limits.archive.max_expanded_bytes().min(remaining_work),
                    limits.archive.max_ratio(),
                )?;
                powerio_archive::session::verify_native(
                    bytes,
                    limits.archive,
                    &self.cancel,
                    |entries, expanded| {
                        charge_work(
                            &mut usage,
                            expanded.saturating_mul(2),
                            self.limits.work_bytes,
                        )?;
                        usage.entries += entries;
                        usage.expanded += expanded;
                        Ok(())
                    },
                )?;
            }
        }
        Ok(())
    }
}
fn charge_work(usage: &mut Usage, bytes: u64, limit: u64) -> Result<()> {
    usage.work = usage.work.saturating_add(bytes);
    if usage.work > limit {
        Err(Error::new(
            &codes::LIMIT,
            "cumulative collection work limit exceeded",
        ))
    } else {
        Ok(())
    }
}

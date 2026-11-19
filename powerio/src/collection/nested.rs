use super::{
    Arc, ArchiveLimits, ArtifactPath, Cancellation, Collection, CollectionEntry, CollectionLimits,
    CollectionSelection, Error, Result, SourceTreeLimits, codes, selection_error,
};
use std::sync::Mutex;

/// Counters shared by a collection, its clones, and every nested child: the
/// archive content admitted into the session's namespaces. They grow only
/// while a namespace is retained; a child that fails verification releases
/// what it reserved.
#[derive(Clone, Debug)]
pub(super) struct Session(Arc<Mutex<Usage>>);
#[derive(Debug)]
struct Usage {
    entries: usize,
    expanded: u64,
}
impl Session {
    pub(super) fn new(entries: &[CollectionEntry]) -> Self {
        Self(Arc::new(Mutex::new(Usage {
            entries: entries.len(),
            expanded: entries.iter().map(|e| e.bytes).sum(),
        })))
    }
    /// Count the admitted ZIP's own entries, directories included.
    pub(super) fn initialize_archive(&self, entries: usize) -> Result<()> {
        let mut usage = self
            .0
            .lock()
            .map_err(|_| selection_error("collection budget lock poisoned"))?;
        usage.entries = entries;
        Ok(())
    }
}

/// The input and decompression bytes one operation touches, bounded by
/// `CollectionLimits::work_bytes`. Every operation starts from zero: a
/// selection parsed twice costs the same each time and never fails because
/// of earlier operations on the collection.
pub(super) struct Work {
    used: u64,
    limit: u64,
}
impl Work {
    pub(super) fn new(limit: u64) -> Self {
        Self { used: 0, limit }
    }
    pub(super) fn charge(&mut self, bytes: u64) -> Result<()> {
        self.used = self.used.saturating_add(bytes);
        if self.used > self.limit {
            Err(Error::new(&codes::LIMIT, "collection work limit exceeded"))
        } else {
            Ok(())
        }
    }
    fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used)
    }
}
impl Collection {
    /// Open exactly one named child ZIP. Its namespace is independent: sources
    /// cannot resolve against a parent or sibling. All descendants and clones
    /// share the monotonic entry and expansion counters of admitted content,
    /// so reopening a child counts again, while each open's own input and
    /// decompression work is bounded on its own.
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
        let mut work = Work::new(self.limits.work_bytes);
        work.charge(bytes.len() as u64)?;
        let mut usage = self
            .session
            .0
            .lock()
            .map_err(|_| selection_error("collection budget lock poisoned"))?;
        let limits = self.remaining_limits(&usage)?;
        // Hold the budget lock during admission: sibling threads cannot each
        // allocate the same remaining allowance concurrently. The declared
        // expansion is reserved before any payload read; a child that fails
        // verification retains nothing, so it releases that reservation.
        let archive = powerio_archive::Archive::open(Arc::clone(&bytes), limits.archive)?;
        let entries = archive.entries().len();
        let expanded = archive
            .entries()
            .map(powerio_archive::ArchiveEntry::size)
            .sum();
        work.charge(expanded)?;
        work.charge(bytes.len() as u64)?;
        usage.entries += entries;
        usage.expanded += expanded;
        drop(archive);
        let mut child = match Self::from_zip_cancellable(member, bytes, limits, self.cancel.clone())
        {
            Ok(child) => child,
            Err(error) => {
                usage.entries -= entries;
                usage.expanded -= expanded;
                return Err(error);
            }
        };
        child.session = self.session.clone();
        child.limits = self.limits;
        child.zip_layers = self.zip_layers + 1;
        child.origin_name.clone_from(&self.origin_name);
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
            ..self.limits
        })
    }
    pub(super) fn charge_selection(&self, selected: &CollectionSelection) -> Result<()> {
        // Charge the scoped source, conservatively including retained companions.
        // This does not estimate model/Arrow allocation or replace parser guards.
        // A separate view keeps accounting reads out of parser observations.
        let mut work = Work::new(self.limits.work_bytes);
        let source = self.source(selected)?;
        let primary = source.primary_buffer().ok();
        let acquired_tables = primary
            .as_ref()
            .is_some_and(|p| selected.has_sincal_tables(p.content_bytes()));
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
            work.charge(bytes.len() as u64)?;
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
            let native = !acquired_tables
                && !selected.has_companions()
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
                // Verification streams the member and retains nothing, so its
                // expansion is this operation's work, not admitted content.
                let allowance = work.remaining() / 2;
                if allowance == 0 {
                    return Err(Error::new(&codes::LIMIT, "collection work limit exceeded"));
                }
                let archive = ArchiveLimits::new(
                    self.limits.archive.max_input_bytes(),
                    self.limits.archive.max_entries(),
                    self.limits.archive.max_expanded_bytes().min(allowance),
                    self.limits.archive.max_ratio(),
                )?;
                powerio_archive::session::verify_native(
                    bytes,
                    archive,
                    &self.cancel,
                    |_, expanded| {
                        work.charge(expanded.saturating_mul(2))?;
                        Ok(())
                    },
                )?;
            }
        }
        Ok(())
    }
}

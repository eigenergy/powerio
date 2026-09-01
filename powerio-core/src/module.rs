use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, TryReserveError};
use std::fmt;

use serde_json::Value;

use crate::validation::valid_nonempty_text;
use crate::{
    Diagnostic, DiagnosticId, Error, HistoryEntry, HistoryId, Producer, Source, SourceDescriptor,
    SourceId, SourceMapEntry, SourceRelation, SourceSpan,
};

#[derive(Clone, Debug)]
struct ModuleRecords {
    producer: Producer,
    sources: Vec<SourceDescriptor>,
    source_map: Vec<SourceMapEntry>,
    history: Vec<HistoryEntry>,
    extensions: BTreeMap<String, Value>,
    retained_source: Option<Source>,
    /// Identity indexes maintained by the `add_*` methods, the only mutation
    /// paths. Duplicate detection and span source resolution consult these
    /// instead of scanning previously inserted records, so populating a module
    /// with N records costs O(N) expected rather than O(N^2).
    source_positions: HashMap<SourceId, usize>,
    diagnostic_ids: HashSet<DiagnosticId>,
    history_ids: HashSet<HistoryId>,
}

impl Default for ModuleRecords {
    fn default() -> Self {
        Self {
            producer: Producer::powerio(),
            sources: Vec::new(),
            source_map: Vec::new(),
            history: Vec::new(),
            extensions: BTreeMap::new(),
            retained_source: None,
            source_positions: HashMap::new(),
            diagnostic_ids: HashSet::new(),
            history_ids: HashSet::new(),
        }
    }
}

fn allocation_refused(cause: TryReserveError) -> Error {
    Error::new(
        &crate::codes::REQUEST_RECORD_ALLOCATION_REFUSED,
        "cannot reserve the record identity index",
    )
    .with_cause(cause)
}

/// One typed PowerIO compiler unit.
///
/// `T` has no PowerIO marker bound. Dynamic parsing and stored JSON register a
/// finite set elsewhere, while Rust applications can use any value here.
pub struct PioModule<T> {
    /// The typed value carried by this module.
    pub value: T,
    /// Diagnostics produced while acquiring, validating, or deriving the
    /// value.
    pub diagnostics: Vec<Diagnostic>,
    records: ModuleRecords,
}

impl<T: Clone> Clone for PioModule<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            diagnostics: self.diagnostics.clone(),
            records: self.records.clone(),
        }
    }
}

impl<T> PioModule<T> {
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            value,
            diagnostics: Vec::new(),
            records: ModuleRecords::default(),
        }
    }

    #[must_use]
    pub fn into_value(self) -> T {
        self.value
    }

    #[must_use]
    pub const fn producer(&self) -> &Producer {
        &self.records.producer
    }

    #[must_use]
    pub fn sources(&self) -> &[SourceDescriptor] {
        &self.records.sources
    }

    #[must_use]
    pub fn source_map(&self) -> &[SourceMapEntry] {
        &self.records.source_map
    }

    #[must_use]
    pub fn history(&self) -> &[HistoryEntry] {
        &self.records.history
    }

    #[must_use]
    pub const fn extensions(&self) -> &BTreeMap<String, Value> {
        &self.records.extensions
    }

    #[must_use]
    pub const fn source(&self) -> Option<&Source> {
        self.records.retained_source.as_ref()
    }

    #[must_use]
    pub fn with_producer(mut self, producer: Producer) -> Self {
        self.records.producer = producer;
        self
    }

    #[must_use]
    pub fn with_source(mut self, source: Source) -> Self {
        self.records.retained_source = Some(source);
        self
    }

    /// Drop the retained source owner: the operation that calls this changed
    /// the value, so a same format write must serialize the value rather than
    /// echo bytes the value no longer matches. Descriptors, diagnostics, and
    /// history stay.
    #[must_use]
    pub fn sever_source(mut self) -> Self {
        self.records.retained_source = None;
        self
    }

    /// Append a finding, applying the same duplicate identity and span
    /// reference checks as [`PioModule::add_diagnostic`]. There is no unchecked
    /// path onto a module's records.
    pub fn with_diagnostic(mut self, diagnostic: Diagnostic) -> Result<Self, Error> {
        self.add_diagnostic(diagnostic)?;
        Ok(self)
    }

    pub fn add_source_descriptor(&mut self, source: SourceDescriptor) -> Result<(), Error> {
        if self.records.sources.len() >= crate::validation::MAX_MODULE_SOURCES {
            return Err(record_cap("sources", crate::validation::MAX_MODULE_SOURCES));
        }
        if self.records.source_positions.contains_key(source.id()) {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_DUPLICATE_ID,
                format!("duplicate source ID `{}`", source.id()),
            ));
        }
        self.records
            .source_positions
            .try_reserve(1)
            .map_err(allocation_refused)?;
        self.records
            .source_positions
            .insert(source.id().clone(), self.records.sources.len());
        self.records.sources.push(source);
        Ok(())
    }

    pub fn add_source_map_entry(&mut self, entry: SourceMapEntry) -> Result<(), Error> {
        if self.records.source_map.len() >= crate::validation::MAX_MODULE_SOURCE_MAP_ENTRIES {
            return Err(record_cap(
                "source map entries",
                crate::validation::MAX_MODULE_SOURCE_MAP_ENTRIES,
            ));
        }
        for span in entry.spans() {
            validate_span(span, &self.records.sources, &self.records.source_positions)?;
        }
        self.records.source_map.push(entry);
        Ok(())
    }

    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) -> Result<(), Error> {
        if self.diagnostics.len() >= crate::validation::MAX_MODULE_DIAGNOSTICS {
            return Err(record_cap(
                "diagnostics",
                crate::validation::MAX_MODULE_DIAGNOSTICS,
            ));
        }
        if let Some(id) = diagnostic.id()
            && self.records.diagnostic_ids.contains(id)
        {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_DUPLICATE_ID,
                format!("duplicate diagnostic ID `{id}`"),
            ));
        }
        for span in diagnostic.spans() {
            validate_span(span, &self.records.sources, &self.records.source_positions)?;
        }
        if let Some(id) = diagnostic.id() {
            self.records
                .diagnostic_ids
                .try_reserve(1)
                .map_err(allocation_refused)?;
            self.records.diagnostic_ids.insert(id.clone());
        }
        self.diagnostics.push(diagnostic);
        Ok(())
    }

    pub fn add_history_entry(&mut self, entry: HistoryEntry) -> Result<(), Error> {
        if self.records.history.len() >= crate::validation::MAX_MODULE_HISTORY_ENTRIES {
            return Err(record_cap(
                "history entries",
                crate::validation::MAX_MODULE_HISTORY_ENTRIES,
            ));
        }
        if self.records.history_ids.contains(entry.id()) {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_DUPLICATE_ID,
                format!("duplicate history ID `{}`", entry.id()),
            ));
        }
        self.records
            .history_ids
            .try_reserve(1)
            .map_err(allocation_refused)?;
        self.records.history_ids.insert(entry.id().clone());
        self.records.history.push(entry);
        Ok(())
    }

    pub fn insert_extension(
        &mut self,
        namespace: impl Into<String>,
        value: Value,
    ) -> Result<Option<Value>, Error> {
        let namespace = namespace.into();
        if !valid_extension_namespace(&namespace) {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_EXTENSION,
                "extension keys must be bounded namespaced strings",
            ));
        }
        if self.records.extensions.len() >= crate::validation::MAX_MODULE_EXTENSION_KEYS
            && !self.records.extensions.contains_key(&namespace)
        {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_TOO_LARGE,
                format!(
                    "a module carries at most {} extension keys",
                    crate::validation::MAX_MODULE_EXTENSION_KEYS
                ),
            ));
        }
        Ok(self.records.extensions.insert(namespace, value))
    }

    /// Verify cross-record references that cannot be checked by constructors.
    pub fn verify_records(&self) -> Result<(), Error> {
        let source_ids: BTreeSet<_> = self
            .records
            .sources
            .iter()
            .map(SourceDescriptor::id)
            .collect();
        if source_ids.len() != self.records.sources.len() {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_DUPLICATE_ID,
                "module contains duplicate source IDs",
            ));
        }
        for entry in &self.records.source_map {
            for span in entry.spans() {
                validate_span(span, &self.records.sources, &self.records.source_positions)?;
            }
        }

        let diagnostic_ids: BTreeSet<&DiagnosticId> =
            self.diagnostics.iter().filter_map(Diagnostic::id).collect();
        if diagnostic_ids.len()
            != self
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.id().is_some())
                .count()
        {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_DUPLICATE_ID,
                "module contains duplicate diagnostic IDs",
            ));
        }
        for diagnostic in &self.diagnostics {
            for span in diagnostic.spans() {
                validate_span(span, &self.records.sources, &self.records.source_positions)?;
            }
            for related in diagnostic.related() {
                if !diagnostic_ids.contains(related) {
                    return Err(Error::new(
                        &crate::codes::REQUEST_RECORD_INVALID_IDENTIFIER,
                        format!("diagnostic refers to unknown diagnostic `{related}`"),
                    ));
                }
            }
        }

        let history_ids: BTreeSet<_> = self.records.history.iter().map(HistoryEntry::id).collect();
        if history_ids.len() != self.records.history.len() {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_DUPLICATE_ID,
                "module contains duplicate history IDs",
            ));
        }
        if self
            .records
            .extensions
            .keys()
            .any(|namespace| !valid_extension_namespace(namespace))
        {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_INVALID_EXTENSION,
                "module contains an extension key that is not namespaced",
            ));
        }
        Ok(())
    }

    /// Drop records that address the old value: the operation that calls this
    /// replaced the value with one of a different kind, so RFC 6901 targets
    /// into the old value no longer identify anything. Every diagnostic
    /// keeps its code, message, severity, and spans but loses its target,
    /// and the source map (whose entries are keyed by such targets) is
    /// cleared. Pair this with [`PioModule::map_value`] in a kind changing
    /// transform so the module still serializes.
    pub fn sever_value_targets(&mut self) {
        for diagnostic in &mut self.diagnostics {
            diagnostic.clear_target();
        }
        self.records.source_map.clear();
    }

    /// Derive a semantically new value and record the operation that produced
    /// it. Source descriptors, diagnostics, prior history, and extensions stay
    /// with the module. Locators into the replaced value and retained bytes
    /// that no longer encode the result are invalidated.
    pub fn derive_value<U>(
        self,
        producer: Producer,
        history: HistoryEntry,
        derive: impl FnOnce(T) -> U,
    ) -> Result<PioModule<U>, Error> {
        self.try_derive_value(producer, history, |value| Ok(derive(value)))
    }

    /// Fallible form of [`PioModule::derive_value`].
    pub fn try_derive_value<U>(
        self,
        producer: Producer,
        history: HistoryEntry,
        derive: impl FnOnce(T) -> Result<U, Error>,
    ) -> Result<PioModule<U>, Error> {
        let Self {
            value,
            mut diagnostics,
            mut records,
        } = self;
        let value = derive(value)?;
        records.producer = producer;
        records.retained_source = None;
        records.source_map.clear();
        for diagnostic in &mut diagnostics {
            diagnostic.clear_target();
        }

        let mut derived = PioModule {
            value,
            diagnostics,
            records,
        };
        derived.add_history_entry(history)?;
        derived.verify_records()?;
        Ok(derived)
    }

    /// Parse a value that depends on this module and one additional source.
    ///
    /// Records that describe the input value are retargeted below
    /// `input_target`. The additional source receives distinct source IDs,
    /// becomes the retained source, and is mapped to the new value's root.
    /// This is the shared module operation for readers whose document is not
    /// self contained, such as a solution file that must be interpreted
    /// against its calculation instance.
    ///
    /// # Errors
    /// The derived value cannot be built, a target is not an RFC 6901
    /// pointer, or the combined records exceed a module bound.
    pub fn try_derive_from_source<U>(
        self,
        input_target: &str,
        source: Source,
        producer: Producer,
        history: HistoryEntry,
        derive: impl FnOnce(T, &Source) -> Result<(U, Vec<Diagnostic>), Error>,
    ) -> Result<PioModule<U>, Error> {
        // Validate the prefix before consuming the input value.
        SourceMapEntry::new(input_target, SourceRelation::Transformed, Vec::new())?;

        let Self {
            value,
            mut diagnostics,
            mut records,
        } = self;
        let (value, mut source_diagnostics) = derive(value, &source)?;

        for diagnostic in &mut diagnostics {
            diagnostic.prefix_target(input_target)?;
        }
        let old_source_map = std::mem::take(&mut records.source_map);
        for entry in old_source_map {
            records.source_map.push(SourceMapEntry::new(
                format!("{input_target}{}", entry.target()),
                entry.relation(),
                entry.spans().to_vec(),
            )?);
        }

        let mut source_ids = HashMap::new();
        let mut root_spans = Vec::new();
        for (index, buffer) in source.acquired_buffers().into_iter().enumerate() {
            let mut suffix = index + 1;
            let id = loop {
                let candidate = SourceId::new(format!("solution-source-{suffix}"))?;
                if !records.source_positions.contains_key(&candidate)
                    && !source_ids.values().any(|existing| existing == &candidate)
                {
                    break candidate;
                }
                suffix += 1;
            };
            source_ids.insert(buffer.id().clone(), id.clone());

            let name = std::path::Path::new(buffer.name())
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| buffer.name());
            let mut descriptor =
                SourceDescriptor::new(id.clone(), name, buffer.bytes().len() as u64)?;
            if let Some(format) = source.format() {
                descriptor = descriptor.with_format(format.clone());
            }
            if records.sources.len() >= crate::validation::MAX_MODULE_SOURCES {
                return Err(record_cap("sources", crate::validation::MAX_MODULE_SOURCES));
            }
            records
                .source_positions
                .try_reserve(1)
                .map_err(allocation_refused)?;
            records
                .source_positions
                .insert(id.clone(), records.sources.len());
            records.sources.push(descriptor);
            root_spans.push(SourceSpan::new(id, 0, buffer.bytes().len() as u64)?);
        }

        for diagnostic in &mut source_diagnostics {
            diagnostic.remap_span_sources(|id| {
                source_ids.get(id).cloned().unwrap_or_else(|| id.clone())
            })?;
        }
        records.producer = producer;
        records.retained_source = Some(source);

        let mut derived = PioModule {
            value,
            diagnostics,
            records,
        };
        for spans in root_spans.chunks(crate::validation::MAX_SOURCE_MAP_SPANS) {
            derived.add_source_map_entry(SourceMapEntry::new(
                "",
                SourceRelation::Aggregated,
                spans.to_vec(),
            )?)?;
        }
        for diagnostic in source_diagnostics {
            derived.add_diagnostic(diagnostic)?;
        }
        derived.add_history_entry(history)?;
        derived.verify_records()?;
        Ok(derived)
    }

    /// Move the value and every module record into another typed module.
    #[must_use]
    pub fn map_value<U>(self, convert: impl FnOnce(T) -> U) -> PioModule<U> {
        PioModule {
            value: convert(self.value),
            diagnostics: self.diagnostics,
            records: self.records,
        }
    }

    /// Move the value through a fallible conversion, keeping every module
    /// record on success. On failure the conversion's error is returned and
    /// the records are dropped with the consumed value; a caller that must
    /// keep the source or findings on the failure route takes them off the
    /// module first with [`PioModule::take_source`] and the public
    /// `diagnostics` field.
    pub fn try_map_value<U, E>(
        self,
        convert: impl FnOnce(T) -> Result<U, E>,
    ) -> Result<PioModule<U>, E> {
        let Self {
            value,
            diagnostics,
            records,
        } = self;
        Ok(PioModule {
            value: convert(value)?,
            diagnostics,
            records,
        })
    }

    /// Take the retained source owner off the module, leaving descriptors,
    /// diagnostics, and history in place. The module then reads as
    /// constructed in memory until a source is reattached.
    pub fn take_source(&mut self) -> Option<Source> {
        self.records.retained_source.take()
    }

    /// Assemble the module a parser returns: the typed value, one descriptor
    /// per acquired buffer of the retained source, a coarse root source map,
    /// and the reader's findings.
    ///
    /// # Errors
    /// A duplicate acquired buffer identity, an invalid buffer name or span,
    /// or a finding that fails the record checks of
    /// [`PioModule::add_diagnostic`].
    pub fn parsed(value: T, source: Source, diagnostics: Vec<Diagnostic>) -> Result<Self, Error> {
        let mut module = Self::new(value);
        let mut root_spans = Vec::new();
        for buffer in source.acquired_buffers() {
            // The stored descriptor names the file, never the local path,
            // and carries the resolved format so a same format write can
            // default to it.
            let name = std::path::Path::new(buffer.name())
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_else(|| buffer.name());
            let mut descriptor =
                SourceDescriptor::new(buffer.id().clone(), name, buffer.bytes().len() as u64)?;
            if let Some(format) = source.format() {
                descriptor = descriptor.with_format(format.clone());
            }
            module.add_source_descriptor(descriptor)?;
            root_spans.push(SourceSpan::new(
                buffer.id().clone(),
                0,
                buffer.bytes().len() as u64,
            )?);
        }
        // Parsers can add field precise mappings later. The root mapping is
        // the minimum durable connection between the typed value and every
        // buffer that produced it, so a module handed to another process does
        // not retain source descriptors with no corresponding value target.
        // Chunk it because one source map entry has a bounded span list while
        // a directory source can acquire more files than that bound.
        for spans in root_spans.chunks(crate::validation::MAX_SOURCE_MAP_SPANS) {
            module.add_source_map_entry(SourceMapEntry::new(
                "",
                SourceRelation::Aggregated,
                spans.to_vec(),
            )?)?;
        }
        let mut module = module.with_source(source);
        for record in diagnostics {
            module.add_diagnostic(record)?;
        }
        Ok(module)
    }

    /// Internal cross-crate support for recoverable consuming narrowing.
    #[doc(hidden)]
    // Boxing the failure would allocate and violate the recoverable no-copy
    // narrowing rule. The caller gets the original module by value.
    #[allow(clippy::result_large_err)]
    pub fn __try_map_value<U>(
        self,
        convert: impl FnOnce(T) -> Result<U, T>,
    ) -> Result<PioModule<U>, PioModule<T>> {
        let Self {
            value,
            diagnostics,
            records,
        } = self;
        match convert(value) {
            Ok(value) => Ok(PioModule {
                value,
                diagnostics,
                records,
            }),
            Err(value) => Err(PioModule {
                value,
                diagnostics,
                records,
            }),
        }
    }
}

impl<T: Clone> PioModule<T> {
    /// Begin an atomic edit of this module's value and findings.
    ///
    /// The candidate uses `T::clone`; values backed by shared tables retain
    /// those tables until [`StagedEdit::value_mut`] performs their own copy on
    /// write detachment. Dropping the staged edit, or any failed operation on
    /// it, leaves this module untouched.
    pub fn stage_edit(&mut self) -> StagedEdit<'_, T> {
        let candidate = PioModule {
            value: self.value.clone(),
            diagnostics: self.diagnostics.clone(),
            records: self.records.clone(),
        };
        StagedEdit {
            module: self,
            candidate,
        }
    }
}

/// An isolated candidate for one atomic module edit.
pub struct StagedEdit<'a, T: Clone> {
    module: &'a mut PioModule<T>,
    candidate: PioModule<T>,
}

impl<T: Clone> StagedEdit<'_, T> {
    #[must_use]
    pub const fn value(&self) -> &T {
        &self.candidate.value
    }

    pub const fn value_mut(&mut self) -> &mut T {
        &mut self.candidate.value
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.candidate.diagnostics
    }

    /// Add a finding to the candidate under the normal module record checks.
    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) -> Result<(), Error> {
        self.candidate.add_diagnostic(diagnostic)
    }

    /// Validate and atomically replace the original module with this
    /// candidate. A committed edit keeps source descriptors and prior records,
    /// but invalidates retained bytes and source mappings that describe the
    /// pre-edit value.
    pub fn commit(self, producer: Producer, history: HistoryEntry) -> Result<(), Error> {
        let Self {
            module,
            mut candidate,
        } = self;
        candidate.records.producer = producer;
        candidate.records.retained_source = None;
        candidate.records.source_map.clear();
        candidate.add_history_entry(history)?;
        candidate.verify_records()?;
        *module = candidate;
        Ok(())
    }
}

impl<T: fmt::Debug> fmt::Debug for PioModule<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PioModule")
            .field("value", &self.value)
            .field("diagnostics", &self.diagnostics)
            .field("records", &self.records)
            .finish()
    }
}

/// The uniform record count refusal every adder applies at its maximum.
fn record_cap(what: &str, max: usize) -> Error {
    Error::new(
        &crate::codes::REQUEST_RECORD_TOO_LARGE,
        format!("the module already holds the maximum {max} {what}"),
    )
}

fn validate_span(
    span: &SourceSpan,
    sources: &[SourceDescriptor],
    positions: &HashMap<SourceId, usize>,
) -> Result<(), Error> {
    let Some(source) = positions
        .get(span.source())
        .and_then(|position| sources.get(*position))
    else {
        return Err(Error::new(
            &crate::codes::REQUEST_RECORD_INVALID_SPAN,
            format!("source span refers to unknown source `{}`", span.source()),
        ));
    };
    if span.byte_end() > source.byte_length() {
        return Err(Error::new(
            &crate::codes::REQUEST_RECORD_INVALID_SPAN,
            format!(
                "source span end {} exceeds source `{}` length {}",
                span.byte_end(),
                span.source(),
                source.byte_length()
            ),
        ));
    }
    Ok(())
}

fn valid_extension_namespace(namespace: &str) -> bool {
    valid_nonempty_text(namespace)
        && !namespace.starts_with('.')
        && !namespace.ends_with('.')
        && namespace.contains('.')
        && namespace.split('.').all(|segment| !segment.is_empty())
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use super::*;
    use crate::{DiagnosticSeverity, HistoryKind, SourceRelation};

    #[test]
    fn modules_accept_unregistered_application_values() {
        struct ApplicationValue(Rc<()>);
        let module = PioModule::new(ApplicationValue(Rc::new(())));
        assert_eq!(Rc::strong_count(&module.value.0), 1);
    }

    #[test]
    fn map_value_moves_records_and_retained_source_without_allocation() {
        let bytes: Arc<[u8]> = b"source".as_slice().into();
        let source = Source::from_memory("case.m", Arc::clone(&bytes)).unwrap();
        let diagnostic = Diagnostic::of(&crate::codes::VALIDATE_TIME_SERIES_SHAPE, "kept");
        let module = PioModule::new(String::from("value"))
            .with_source(source)
            .with_diagnostic(diagnostic)
            .unwrap();
        let diagnostics_pointer = module.diagnostics.as_ptr();
        let source_pointer = module
            .source()
            .unwrap()
            .primary_buffer()
            .unwrap()
            .bytes()
            .as_ptr();
        let mapped = module.map_value(String::into_bytes);
        assert_eq!(mapped.value, b"value");
        assert_eq!(mapped.diagnostics.as_ptr(), diagnostics_pointer);
        assert_eq!(
            mapped
                .source()
                .unwrap()
                .primary_buffer()
                .unwrap()
                .bytes()
                .as_ptr(),
            source_pointer
        );
    }

    #[test]
    fn failed_try_map_returns_the_original_module_and_records() {
        let module = PioModule::new(String::from("value"))
            .with_diagnostic(Diagnostic::of(
                &crate::codes::VALIDATE_TIME_SERIES_SHAPE,
                "kept",
            ))
            .unwrap();
        let diagnostics_pointer = module.diagnostics.as_ptr();
        let recovered = module
            .__try_map_value::<usize>(Err)
            .expect_err("conversion fails");
        assert_eq!(recovered.value, "value");
        assert_eq!(recovered.diagnostics.as_ptr(), diagnostics_pointer);
    }

    #[test]
    fn parsed_module_maps_the_value_to_its_source() {
        let source = Source::from_memory("case.m", b"source".as_slice()).unwrap();
        let module = PioModule::parsed(1_u8, source, Vec::new()).unwrap();
        assert_eq!(module.source_map().len(), 1);
        let entry = &module.source_map()[0];
        assert_eq!(entry.target(), "");
        assert_eq!(entry.relation(), SourceRelation::Aggregated);
        assert_eq!(entry.spans().len(), 1);
        assert_eq!(entry.spans()[0].byte_start(), 0);
        assert_eq!(entry.spans()[0].byte_end(), 6);
    }

    #[test]
    fn record_references_and_namespaces_are_checked() {
        let source_id = SourceId::new("input").unwrap();
        let mut module = PioModule::new(1_u8);
        module
            .add_source_descriptor(SourceDescriptor::new(source_id.clone(), "case.m", 4).unwrap())
            .unwrap();
        let span = SourceSpan::new(source_id, 0, 4).unwrap();
        module
            .add_source_map_entry(
                SourceMapEntry::new("/value", SourceRelation::Exact, vec![span.clone()]).unwrap(),
            )
            .unwrap();
        module
            .add_diagnostic(
                Diagnostic::new(
                    crate::DiagnosticCode::new("PARTNER.TEST.FINDING").unwrap(),
                    DiagnosticSeverity::Note,
                    "note",
                )
                .with_id(DiagnosticId::new("d1").unwrap())
                .with_span(span)
                .unwrap(),
            )
            .unwrap();
        module
            .add_history_entry(
                HistoryEntry::new(HistoryId::new("h1").unwrap(), HistoryKind::Parse, "parse")
                    .unwrap(),
            )
            .unwrap();
        module
            .insert_extension("org.example", Value::Bool(true))
            .unwrap();
        assert!(module.verify_records().is_ok());
        assert!(
            module
                .insert_extension("not-namespaced", Value::Null)
                .is_err()
        );

        let invalid = SourceSpan::new(SourceId::new("input").unwrap(), 0, 5).unwrap();
        assert!(
            module
                .add_source_map_entry(
                    SourceMapEntry::new("", SourceRelation::Exact, vec![invalid]).unwrap()
                )
                .is_err()
        );
    }

    #[test]
    fn semantic_derivation_records_provenance_and_invalidates_old_value_locators() {
        let source = Source::from_memory("case.m", b"source".as_slice()).unwrap();
        let span = SourceSpan::new(
            source.primary_buffer().unwrap().id().clone(),
            0,
            b"source".len() as u64,
        )
        .unwrap();
        let diagnostic = Diagnostic::new(
            crate::DiagnosticCode::new("PARTNER.TEST.DERIVE").unwrap(),
            DiagnosticSeverity::Note,
            "kept",
        )
        .with_id(DiagnosticId::new("d1").unwrap())
        .with_target("/old/value")
        .unwrap()
        .with_span(span)
        .unwrap();
        let mut module = PioModule::parsed(3_u8, source, vec![diagnostic]).unwrap();
        module
            .add_history_entry(
                HistoryEntry::new(HistoryId::new("h1").unwrap(), HistoryKind::Parse, "parse")
                    .unwrap(),
            )
            .unwrap();
        module
            .insert_extension("org.example", Value::Bool(true))
            .unwrap();

        let derived = module
            .derive_value(
                Producer::new("tellegen", "1.0.0").unwrap(),
                HistoryEntry::new(
                    HistoryId::new("h2").unwrap(),
                    HistoryKind::Transform,
                    "build instance",
                )
                .unwrap(),
                |value| usize::from(value) * 2,
            )
            .unwrap();

        assert_eq!(derived.value, 6);
        assert_eq!(derived.producer().name(), "tellegen");
        assert_eq!(derived.sources().len(), 1);
        assert!(derived.source().is_none());
        assert!(derived.source_map().is_empty());
        assert_eq!(derived.diagnostics.len(), 1);
        assert!(derived.diagnostics[0].target().is_none());
        assert_eq!(derived.diagnostics[0].spans().len(), 1);
        assert_eq!(derived.history().len(), 2);
        assert_eq!(
            derived.extensions().get("org.example"),
            Some(&Value::Bool(true))
        );
    }

    #[test]
    fn staged_edits_commit_atomically_or_leave_the_module_untouched() {
        let source = Source::from_memory("case.m", b"source".as_slice()).unwrap();
        let diagnostic = Diagnostic::new(
            crate::DiagnosticCode::new("PARTNER.TEST.EDIT").unwrap(),
            DiagnosticSeverity::Note,
            "existing",
        )
        .with_id(DiagnosticId::new("d1").unwrap())
        .with_target("/0")
        .unwrap();
        let mut module = PioModule::parsed([1_u8, 2], source, vec![diagnostic]).unwrap();
        module
            .add_history_entry(
                HistoryEntry::new(HistoryId::new("h1").unwrap(), HistoryKind::Parse, "parse")
                    .unwrap(),
            )
            .unwrap();

        {
            let mut staged = module.stage_edit();
            staged.value_mut()[0] = 8;
        }
        assert_eq!(module.value, [1, 2]);
        assert!(module.source().is_some());

        let mut staged = module.stage_edit();
        staged.value_mut()[0] = 9;
        let error = staged
            .commit(
                Producer::new("editor", "1.0.0").unwrap(),
                HistoryEntry::new(HistoryId::new("h1").unwrap(), HistoryKind::Edit, "edit")
                    .unwrap(),
            )
            .expect_err("duplicate history makes the staged commit fail");
        assert_eq!(error.category(), crate::ErrorCategory::Request);
        assert_eq!(module.value, [1, 2]);
        assert!(module.source().is_some());
        assert_eq!(module.history().len(), 1);

        let mut staged = module.stage_edit();
        staged.value_mut()[0] = 7;
        staged
            .add_diagnostic(
                Diagnostic::new(
                    crate::DiagnosticCode::new("PARTNER.TEST.EDITED").unwrap(),
                    DiagnosticSeverity::Note,
                    "changed",
                )
                .with_id(DiagnosticId::new("d2").unwrap())
                .with_target("/0")
                .unwrap(),
            )
            .unwrap();
        staged
            .commit(
                Producer::new("editor", "1.0.0").unwrap(),
                HistoryEntry::new(HistoryId::new("h2").unwrap(), HistoryKind::Edit, "edit")
                    .unwrap(),
            )
            .unwrap();

        assert_eq!(module.value, [7, 2]);
        assert_eq!(module.producer().name(), "editor");
        assert!(module.source().is_none());
        assert!(module.source_map().is_empty());
        assert_eq!(module.sources().len(), 1);
        assert_eq!(module.diagnostics.len(), 2);
        assert_eq!(module.diagnostics[0].target(), Some("/0"));
        assert_eq!(module.history().len(), 2);
    }
}

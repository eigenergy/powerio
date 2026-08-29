use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, TryReserveError};
use std::fmt;

use serde_json::Value;

use crate::validation::valid_nonempty_text;
use crate::{
    Diagnostic, DiagnosticId, Error, HistoryEntry, HistoryId, Producer, Source, SourceDescriptor,
    SourceId, SourceMapEntry, SourceSpan,
};

#[derive(Debug)]
struct ModuleRecords {
    producer: Producer,
    sources: Vec<SourceDescriptor>,
    source_map: Vec<SourceMapEntry>,
    diagnostics: Vec<Diagnostic>,
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
            diagnostics: Vec::new(),
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
    value: T,
    records: ModuleRecords,
}

impl<T> PioModule<T> {
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            value,
            records: ModuleRecords::default(),
        }
    }

    #[must_use]
    pub const fn value(&self) -> &T {
        &self.value
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
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.records.diagnostics
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
        if self.records.diagnostics.len() >= crate::validation::MAX_MODULE_DIAGNOSTICS {
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
        self.records.diagnostics.push(diagnostic);
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

        let diagnostic_ids: BTreeSet<&DiagnosticId> = self
            .records
            .diagnostics
            .iter()
            .filter_map(Diagnostic::id)
            .collect();
        if diagnostic_ids.len()
            != self
                .records
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
        for diagnostic in &self.records.diagnostics {
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
        for diagnostic in &mut self.records.diagnostics {
            diagnostic.clear_target();
        }
        self.records.source_map.clear();
    }

    /// Move the value and every module record into another typed module.
    #[must_use]
    pub fn map_value<U>(self, convert: impl FnOnce(T) -> U) -> PioModule<U> {
        PioModule {
            value: convert(self.value),
            records: self.records,
        }
    }

    /// Move the value through a fallible conversion, keeping every module
    /// record on success. On failure the conversion's error is returned and
    /// the records are dropped with the consumed value; a caller that must
    /// keep the source or findings on the failure route takes them off the
    /// module first ([`PioModule::take_source`], [`PioModule::diagnostics`]).
    pub fn try_map_value<U, E>(
        self,
        convert: impl FnOnce(T) -> Result<U, E>,
    ) -> Result<PioModule<U>, E> {
        let Self { value, records } = self;
        Ok(PioModule {
            value: convert(value)?,
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
    /// per acquired buffer of the retained source, and the reader's findings.
    ///
    /// # Errors
    /// A duplicate acquired buffer identity, an invalid buffer name, or a
    /// finding that fails the record checks of [`PioModule::add_diagnostic`].
    pub fn parsed(value: T, source: Source, diagnostics: Vec<Diagnostic>) -> Result<Self, Error> {
        let mut module = Self::new(value);
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
        let Self { value, records } = self;
        match convert(value) {
            Ok(value) => Ok(PioModule { value, records }),
            Err(value) => Err(PioModule { value, records }),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for PioModule<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PioModule")
            .field("value", &self.value)
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
        assert_eq!(Rc::strong_count(&module.value().0), 1);
    }

    #[test]
    fn map_value_moves_records_and_retained_source_without_allocation() {
        let bytes: Arc<[u8]> = b"source".as_slice().into();
        let source = Source::from_bytes("case.m", Arc::clone(&bytes)).unwrap();
        let diagnostic = Diagnostic::of(&crate::codes::VALIDATE_TIME_SERIES_SHAPE, "kept");
        let module = PioModule::new(String::from("value"))
            .with_source(source)
            .with_diagnostic(diagnostic)
            .unwrap();
        let diagnostics_pointer = module.diagnostics().as_ptr();
        let source_pointer = module
            .source()
            .unwrap()
            .primary_buffer()
            .unwrap()
            .bytes()
            .as_ptr();
        let mapped = module.map_value(String::into_bytes);
        assert_eq!(mapped.value(), b"value");
        assert_eq!(mapped.diagnostics().as_ptr(), diagnostics_pointer);
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
        let diagnostics_pointer = module.diagnostics().as_ptr();
        let recovered = module
            .__try_map_value::<usize>(Err)
            .expect_err("conversion fails");
        assert_eq!(recovered.value(), "value");
        assert_eq!(recovered.diagnostics().as_ptr(), diagnostics_pointer);
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
}

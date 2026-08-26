use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value;

use crate::validation::valid_nonempty_text;
use crate::{
    Diagnostic, DiagnosticId, Error, HistoryEntry, Producer, Source, SourceDescriptor,
    SourceMapEntry, SourceSpan,
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
        }
    }
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

    #[must_use]
    pub fn with_diagnostic(mut self, diagnostic: Diagnostic) -> Self {
        self.records.diagnostics.push(diagnostic);
        self
    }

    pub fn add_source_descriptor(&mut self, source: SourceDescriptor) -> Result<(), Error> {
        if self
            .records
            .sources
            .iter()
            .any(|existing| existing.id() == source.id())
        {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_DUPLICATE_ID,
                format!("duplicate source ID `{}`", source.id()),
            ));
        }
        self.records.sources.push(source);
        Ok(())
    }

    pub fn add_source_map_entry(&mut self, entry: SourceMapEntry) -> Result<(), Error> {
        for span in entry.spans() {
            validate_span(span, &self.records.sources)?;
        }
        self.records.source_map.push(entry);
        Ok(())
    }

    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) -> Result<(), Error> {
        if let Some(id) = diagnostic.id()
            && self
                .records
                .diagnostics
                .iter()
                .filter_map(Diagnostic::id)
                .any(|existing| existing == id)
        {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_DUPLICATE_ID,
                format!("duplicate diagnostic ID `{id}`"),
            ));
        }
        for span in diagnostic.spans() {
            validate_span(span, &self.records.sources)?;
        }
        self.records.diagnostics.push(diagnostic);
        Ok(())
    }

    pub fn add_history_entry(&mut self, entry: HistoryEntry) -> Result<(), Error> {
        if self
            .records
            .history
            .iter()
            .any(|existing| existing.id() == entry.id())
        {
            return Err(Error::new(
                &crate::codes::REQUEST_RECORD_DUPLICATE_ID,
                format!("duplicate history ID `{}`", entry.id()),
            ));
        }
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
                validate_span(span, &self.records.sources)?;
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
                validate_span(span, &self.records.sources)?;
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

    /// Move the value and every module record into another typed module.
    #[must_use]
    pub fn map_value<U>(self, convert: impl FnOnce(T) -> U) -> PioModule<U> {
        PioModule {
            value: convert(self.value),
            records: self.records,
        }
    }

    /// Internal cross-crate support for recoverable consuming narrowing.
    #[doc(hidden)]
    // Boxing the failure would allocate and violate the recoverable no-copy
    // narrowing contract. The caller gets the original module by value.
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

fn validate_span(span: &SourceSpan, sources: &[SourceDescriptor]) -> Result<(), Error> {
    let Some(source) = sources.iter().find(|source| source.id() == span.source()) else {
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
    use crate::{
        DiagnosticId, DiagnosticSeverity, HistoryId, HistoryKind, SourceId, SourceRelation,
    };

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
            .with_diagnostic(diagnostic);
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
        let module = PioModule::new(String::from("value")).with_diagnostic(Diagnostic::of(
            &crate::codes::VALIDATE_TIME_SERIES_SHAPE,
            "kept",
        ));
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
                .with_span(span),
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

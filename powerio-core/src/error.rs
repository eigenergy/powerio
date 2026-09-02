use std::fmt;

use crate::{
    Diagnostic, DiagnosticInfo, DiagnosticSeverity, ErrorCategory, Source, SourceSpan,
    render_diagnostic,
};

type BoxedCause = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Failure to produce an operation's requested output.
///
/// Every value contains a registered error diagnostic. An underlying I/O or
/// library error remains available through [`std::error::Error::source`].
pub struct Error {
    diagnostics: Vec<Diagnostic>,
    cause: Option<BoxedCause>,
    retained_source: Option<Source>,
}

impl Error {
    /// Construct a failure from a registered code carrying a category.
    ///
    /// A code that ends an operation must declare a category, because the
    /// category is what a binding and an exit status project the failure onto.
    /// A code that does not is a registry defect: the finding keeps its own
    /// code so its identity is not lost, and a `REQUEST.DIAGNOSTIC.MISSING_CATEGORY`
    /// note records the defect. A debug build asserts instead, so the defect
    /// surfaces in tests rather than in a released binding.
    #[must_use]
    pub fn new(info: &'static DiagnosticInfo, message: impl Into<String>) -> Self {
        debug_assert!(
            info.category.is_some(),
            "{} ends an operation but declares no error category",
            info.code
        );
        let mut diagnostics =
            vec![Diagnostic::of(info, message).with_severity(DiagnosticSeverity::Error)];
        if info.category.is_none() {
            diagnostics.push(
                Diagnostic::of(
                    &crate::codes::REQUEST_DIAGNOSTIC_MISSING_CATEGORY,
                    format!("{} declares no error category", info.code),
                )
                .with_severity(DiagnosticSeverity::Note),
            );
        }
        Self {
            diagnostics,
            cause: None,
            retained_source: None,
        }
    }

    /// Add a diagnostic emitted before or while the operation failed.
    #[must_use]
    pub fn with_diagnostic(mut self, diagnostic: Diagnostic) -> Self {
        self.diagnostics.push(diagnostic);
        self
    }

    /// Add diagnostics without changing their order.
    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) -> Self {
        self.diagnostics.extend(diagnostics);
        self
    }

    /// Retain the implementation error that caused this operation failure.
    #[must_use]
    pub fn with_cause(mut self, cause: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }

    /// Retain the shared input owner needed to interpret diagnostic spans.
    #[must_use]
    pub fn with_source(mut self, source: Source) -> Self {
        self.retained_source = Some(source);
        self
    }

    /// Attach the byte range of the record that ended the operation to the
    /// failure's error diagnostic.
    ///
    /// The diagnostic keeps at most `limits::MAX_DIAGNOSTIC_SPANS` spans. A
    /// span past that limit is not attached; the refusal is recorded as a
    /// note so the omission stays visible.
    #[must_use]
    pub fn with_span(mut self, span: SourceSpan) -> Self {
        let Some(position) = self
            .diagnostics
            .iter()
            .position(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Error)
        else {
            return self;
        };
        if let Err(refused) = self.diagnostics[position].add_span(span) {
            self.diagnostics.extend(
                refused
                    .into_diagnostics()
                    .into_iter()
                    .map(|diagnostic| diagnostic.with_severity(DiagnosticSeverity::Note)),
            );
        }
        self
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// The registered entry of the diagnostic that ended the operation, when
    /// the failure was built from one.
    #[must_use]
    pub fn info(&self) -> Option<&'static crate::DiagnosticInfo> {
        self.diagnostics
            .first()
            .and_then(Diagnostic::registered_info)
    }

    /// Coarse projection from the first registered error diagnostic.
    #[must_use]
    pub fn category(&self) -> ErrorCategory {
        self.diagnostics
            .iter()
            .find(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Error)
            .and_then(Diagnostic::registered_info)
            .and_then(|info| info.category)
            .unwrap_or(ErrorCategory::Data)
    }

    #[must_use]
    pub const fn retained_source(&self) -> Option<&Source> {
        self.retained_source.as_ref()
    }

    #[must_use]
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let diagnostic = self
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Error);
        match diagnostic {
            Some(diagnostic) => formatter.write_str(&render_diagnostic(diagnostic)),
            None => formatter.write_str("PowerIO operation failed without a diagnostic"),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Error")
            .field("category", &self.category())
            .field("diagnostics", &self.diagnostics)
            .field("cause", &self.cause.as_ref().map(ToString::to_string))
            .field("retained_source", &self.retained_source)
            .finish()
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn an_error_has_one_error_diagnostic_and_a_registered_category() {
        let error = Error::new(
            &crate::codes::VALIDATE_TIME_SERIES_SHAPE,
            "two values for one point",
        );
        assert_eq!(error.category(), ErrorCategory::Data);
        assert_eq!(error.diagnostics().len(), 1);
        assert_eq!(error.diagnostics()[0].severity(), DiagnosticSeverity::Error);
        assert!(
            error
                .to_string()
                .starts_with("VALIDATE.TIME_SERIES.SHAPE: ")
        );
    }

    #[test]
    fn a_span_attaches_to_the_diagnostic_that_ended_the_operation() {
        let source = crate::SourceId::new("/input").unwrap();
        let error = Error::new(&crate::codes::READ_IO_READ, "read failed")
            .with_diagnostic(
                Diagnostic::of(&crate::codes::READ_IO_READ, "context")
                    .with_severity(DiagnosticSeverity::Note),
            )
            .with_span(SourceSpan::new(source.clone(), 4, 9).unwrap());
        let spans = error.diagnostics()[0].spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].source(), &source);
        assert_eq!((spans[0].byte_start(), spans[0].byte_end()), (4, 9));
        assert!(error.diagnostics()[1].spans().is_empty());

        // Past the span limit the range is not attached and a note records
        // the refusal, so nothing is dropped silently.
        let mut crowded = Error::new(&crate::codes::READ_IO_READ, "read failed");
        for _ in 0..crate::validation::MAX_DIAGNOSTIC_SPANS {
            crowded = crowded.with_span(SourceSpan::new(source.clone(), 0, 1).unwrap());
        }
        let overflow = crowded.with_span(SourceSpan::new(source, 0, 1).unwrap());
        assert_eq!(
            overflow.diagnostics()[0].spans().len(),
            crate::validation::MAX_DIAGNOSTIC_SPANS
        );
        assert_eq!(
            overflow.diagnostics().last().unwrap().severity(),
            DiagnosticSeverity::Note
        );
    }

    #[test]
    fn cause_and_shared_source_are_retained() {
        let source = Source::from_memory("input.bin", vec![0, 255]).unwrap();
        let byte_pointer = source.primary_buffer().unwrap().bytes().as_ptr();
        let error = Error::new(&crate::codes::READ_IO_READ, "read failed")
            .with_cause(std::io::Error::other("cause"))
            .with_source(source);
        assert_eq!(error.source().unwrap().to_string(), "cause");
        assert_eq!(
            error
                .retained_source()
                .unwrap()
                .primary_buffer()
                .unwrap()
                .bytes()
                .as_ptr(),
            byte_pointer
        );
    }
}

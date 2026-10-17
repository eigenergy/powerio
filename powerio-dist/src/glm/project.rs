use powerio_core::{Diagnostic, DiagnosticInfo, Error, Source, SourceId, SourceSpan};

use super::{
    GlmDialect, PreparedGlm, PreprocessOptions, RawGlm, Statement, parse_raw_source,
    preprocess_source,
};
use crate::diagnostics::codes;

/// Source project and expanded syntax, without electrical interpretation.
/// Syntax spans address the expansion; [`Self::original_spans`] locates the
/// corresponding original bytes, including substitutions and included files.
#[derive(Clone, Debug)]
pub struct GlmProject {
    prepared: PreparedGlm,
    syntax: RawGlm,
    dialect: GlmDialect,
}

impl GlmProject {
    #[must_use]
    pub const fn dialect(&self) -> GlmDialect {
        self.dialect
    }

    #[must_use]
    pub fn source(&self) -> &Source {
        &self.prepared.source
    }

    #[must_use]
    pub fn statements(&self) -> &[Statement] {
        &self.syntax.statements
    }

    #[must_use]
    pub fn expanded_text(&self) -> &str {
        self.prepared.text()
    }

    #[must_use]
    pub fn dependencies(&self) -> &[SourceId] {
        self.prepared.dependencies()
    }

    pub(super) fn has_absolute_include(&self) -> bool {
        self.prepared.has_absolute_include()
    }

    /// Translate a span from this project's syntax into original source spans.
    #[must_use]
    pub fn original_spans(&self, span: &SourceSpan) -> Vec<SourceSpan> {
        self.prepared
            .original_spans(span.byte_start() as usize..span.byte_end() as usize)
    }

    pub(crate) fn fail(&self, span: &SourceSpan, message: impl Into<String>) -> Error {
        mapped_error(&self.prepared, &codes::PARSE_GLM_OBJECT, span, message)
    }

    pub(super) fn electrical_error(&self, span: &SourceSpan, message: impl Into<String>) -> Error {
        mapped_error(
            &self.prepared,
            &codes::READ_GLM_ELECTRICAL_UNSUPPORTED,
            span,
            message,
        )
    }

    pub(super) fn quantity_error(&self, span: &SourceSpan, message: &str) -> Error {
        mapped_error(&self.prepared, &codes::PARSE_GLM_QUANTITY, span, message)
    }

    pub(super) fn ignored_quantity_unit(
        &self,
        span: &SourceSpan,
        suffix: &str,
    ) -> Result<Diagnostic, Error> {
        let mut diagnostic = Diagnostic::of(
            &codes::READ_GLM_UNIT_IGNORED,
            format!(
                "native quantity conversion ignores suffix {suffix:?}; retained the native numeric value"
            ),
        );
        for original in self
            .prepared
            .original_span_iter(span.byte_start() as usize..span.byte_end() as usize)
            .take(powerio_core::limits::MAX_DIAGNOSTIC_SPANS + 1)
        {
            diagnostic = diagnostic.with_span(original)?;
        }
        Ok(diagnostic)
    }
}

fn mapped_error(
    prepared: &PreparedGlm,
    info: &'static DiagnosticInfo,
    span: &SourceSpan,
    message: impl Into<String>,
) -> Error {
    let mut result = Error::new(info, message).with_source(prepared.source.clone());
    for original in prepared
        .original_span_iter(span.byte_start() as usize..span.byte_end() as usize)
        .take(powerio_core::limits::MAX_DIAGNOSTIC_SPANS + 1)
    {
        result = result.with_span(original);
    }
    result
}

/// Preprocess the supported explicit profile and parse its complete syntax.
/// Failures retain the original project and point into original buffers rather
/// than presenting expanded byte offsets as if they belonged to an input file.
pub fn parse_project(source: Source, options: PreprocessOptions) -> Result<GlmProject, Error> {
    let dialect = options.dialect;
    let prepared = preprocess_source(source, options)?;
    let expanded = Source::from_memory("expanded.glm", prepared.text().as_bytes())?;
    match parse_raw_source(expanded) {
        Ok(syntax) => Ok(GlmProject {
            prepared,
            syntax,
            dialect,
        }),
        Err(error) => {
            let Some(first) = error.diagnostics().first() else {
                return Err(error.with_source(prepared.source));
            };
            let info = first.registered_info().unwrap_or(&codes::PARSE_GLM_SYNTAX);
            if let Some(span) = first.spans().first() {
                Err(mapped_error(&prepared, info, span, first.message()))
            } else {
                Err(Error::new(info, first.message()).with_source(prepared.source))
            }
        }
    }
}

//! Structured diagnostics.
//!
//! A free-form `Vec<String>` warning is useful for a human but opaque to CI, an
//! agent, or a downstream solver. Every finding a frontend, lowering pass, or
//! backend records carries a stable [`DiagnosticCode`], a [`DiagnosticSeverity`],
//! the [`DiagnosticStage`] it came from, a human message, and (where known) the
//! element path and [`SourceRef`] it refers to. Human-readable warnings should
//! be rendered from these, not the other way around.

use serde::{Deserialize, Serialize};

use crate::provenance::SourceRef;

/// A stable, dotted diagnostic code, e.g. `EMIT.PSSE.DROP_ANGLE_LIMITS`.
///
/// The leading segment is the namespace and names the stage family:
/// `PARSE`, `READ`, `IR`, `VALIDATE`, `FIDELITY`, `LOWER`, `EMIT`, `BINDING`,
/// `PARTNER`, `PERF`. The conventional shape is `NAMESPACE.SOURCE_OR_TARGET.SPECIFIC`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct DiagnosticCode(pub String);

impl DiagnosticCode {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    /// The leading dotted segment (the namespace), e.g. `EMIT` for
    /// `EMIT.PSSE.DROP_ANGLE_LIMITS`.
    pub fn namespace(&self) -> &str {
        self.0.split('.').next().unwrap_or("")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DiagnosticCode {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for DiagnosticCode {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Severity, ordered worst-last so [`Ord`] gives the dominant severity of a set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    /// Useful in development; normally hidden.
    Debug,
    /// A provenance or normalization event worth recording.
    Info,
    /// Usable, but semantics were defaulted, approximated, lost, or the target
    /// is incomplete.
    Warning,
    /// The package exists but the model is not valid for the intended use
    /// without repair.
    Error,
    /// The package could not be produced.
    Fatal,
}

/// The compiler stage that emitted a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DiagnosticStage {
    Parse,
    Read,
    Canonicalize,
    Validate,
    Lower,
    Emit,
    Bind,
    Partner,
}

/// One structured finding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StructuredDiagnostic {
    pub code: DiagnosticCode,
    pub severity: DiagnosticSeverity,
    pub stage: DiagnosticStage,
    pub message: String,
    /// JSON pointer (or best-effort locator) of the element the finding is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<SourceRef>,
    /// Code-specific structured payload, e.g. `{"dropped_fields": ["angmin"]}`.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub details: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_action: Option<String>,
    /// Workflows for which this finding is safe to ignore, e.g.
    /// `["power_flow", "opf"]`. Empty means "no such assurance".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub safe_to_ignore: Vec<String>,
}

impl StructuredDiagnostic {
    /// A minimal finding; fill the optional locators with the builder methods.
    pub fn new(
        code: impl Into<DiagnosticCode>,
        severity: DiagnosticSeverity,
        stage: DiagnosticStage,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            stage,
            message: message.into(),
            element_path: None,
            source_ref: None,
            details: serde_json::Map::new(),
            suggested_action: None,
            safe_to_ignore: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_element_path(mut self, path: impl Into<String>) -> Self {
        self.element_path = Some(path.into());
        self
    }

    #[must_use]
    pub fn with_source_ref(mut self, source_ref: SourceRef) -> Self {
        self.source_ref = Some(source_ref);
        self
    }

    #[must_use]
    pub fn with_suggested_action(mut self, action: impl Into<String>) -> Self {
        self.suggested_action = Some(action.into());
        self
    }
}

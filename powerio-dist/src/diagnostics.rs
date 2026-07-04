//! Structured diagnostics for distribution conversions.
//!
//! This mirrors the `.pio.json` diagnostic shape without depending on
//! `powerio-pkg`, which already depends on this crate.

use serde::{Deserialize, Serialize};

/// A stable dotted diagnostic code, e.g. `EMIT.BMOPF.TRANSFORMER_UNSUPPORTED`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct DiagnosticCode(pub String);

impl DiagnosticCode {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

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

/// Severity, ordered worst last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Debug,
    Info,
    Warning,
    Error,
    Fatal,
}

/// The conversion stage that emitted a diagnostic.
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

/// One structured conversion finding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StructuredDiagnostic {
    pub code: DiagnosticCode,
    pub severity: DiagnosticSeverity,
    pub stage: DiagnosticStage,
    pub message: String,
    /// JSON pointer or best effort element locator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_path: Option<String>,
    /// Code specific structured payload.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub details: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_action: Option<String>,
    /// Workflows for which this finding is safe to ignore.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub safe_to_ignore: Vec<String>,
}

impl StructuredDiagnostic {
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
    pub fn with_details(mut self, details: serde_json::Map<String, serde_json::Value>) -> Self {
        self.details = details;
        self
    }

    #[must_use]
    pub fn with_suggested_action(mut self, action: impl Into<String>) -> Self {
        self.suggested_action = Some(action.into());
        self
    }
}

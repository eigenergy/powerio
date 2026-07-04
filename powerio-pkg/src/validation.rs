//! The package-level validation summary.

use serde::{Deserialize, Serialize};

use crate::diagnostics::{DiagnosticSeverity, StructuredDiagnostic};

/// Overall validation status, ordered worst-last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Ok,
    Info,
    Warning,
    Error,
    Fatal,
}

impl ValidationStatus {
    fn from_severity(s: DiagnosticSeverity) -> Self {
        match s {
            DiagnosticSeverity::Debug => ValidationStatus::Ok,
            DiagnosticSeverity::Info => ValidationStatus::Info,
            DiagnosticSeverity::Warning => ValidationStatus::Warning,
            DiagnosticSeverity::Error => ValidationStatus::Error,
            DiagnosticSeverity::Fatal => ValidationStatus::Fatal,
        }
    }
}

/// Counts per severity. All five are always present; zero where unused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ValidationCounts {
    #[serde(default)]
    pub fatal: u32,
    #[serde(default)]
    pub error: u32,
    #[serde(default)]
    pub warning: u32,
    #[serde(default)]
    pub info: u32,
    #[serde(default)]
    pub debug: u32,
}

impl ValidationCounts {
    fn add(&mut self, s: DiagnosticSeverity) {
        match s {
            DiagnosticSeverity::Fatal => self.fatal += 1,
            DiagnosticSeverity::Error => self.error += 1,
            DiagnosticSeverity::Warning => self.warning += 1,
            DiagnosticSeverity::Info => self.info += 1,
            DiagnosticSeverity::Debug => self.debug += 1,
        }
    }
}

/// The status of one named validation pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ValidationPass {
    pub name: String,
    pub status: ValidationStatus,
}

impl ValidationPass {
    pub fn new(name: impl Into<String>, status: ValidationStatus) -> Self {
        Self {
            name: name.into(),
            status,
        }
    }
}

/// A cheap-to-inspect summary of validation: an overall status, per-severity
/// counts, and the named passes that ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ValidationSummary {
    pub status: ValidationStatus,
    pub counts: ValidationCounts,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passes: Vec<ValidationPass>,
}

impl ValidationSummary {
    /// A clean pass with no findings.
    pub fn ok() -> Self {
        Self {
            status: ValidationStatus::Ok,
            counts: ValidationCounts::default(),
            passes: Vec::new(),
        }
    }

    /// Derive counts and the dominant status from a set of diagnostics.
    pub fn from_diagnostics(diagnostics: &[StructuredDiagnostic]) -> Self {
        let mut counts = ValidationCounts::default();
        let mut status = ValidationStatus::Ok;
        for d in diagnostics {
            counts.add(d.severity);
            status = status.max(ValidationStatus::from_severity(d.severity));
        }
        Self {
            status,
            counts,
            passes: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_passes(mut self, passes: Vec<ValidationPass>) -> Self {
        self.passes = passes;
        self
    }
}

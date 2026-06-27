//! A record of one lowering pass.
//!
//! Lowering is where PowerIO is a compiler rather than a parser: every pass that
//! transforms one model into another (normalization, multiconductor to balanced,
//! emission to a target format) appends a [`LoweringRecord`] to the package's
//! `lowering_history`, so the transformation is auditable. The most consequential
//! case, multiconductor to balanced, must be an explicit pass with diagnostics,
//! never a silent positive-sequence projection.

use serde::{Deserialize, Serialize};

use crate::diagnostics::StructuredDiagnostic;
use crate::model::ModelKind;
use crate::validation::ValidationStatus;

/// One lowering/normalization/emission pass and what it changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoweringRecord {
    /// A stable pass name, e.g. `normalize-balanced` or `multiconductor-to-balanced`.
    pub pass: String,
    pub input_kind: ModelKind,
    pub output_kind: ModelKind,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub options: serde_json::Map<String, serde_json::Value>,
    /// Modeling assumptions the pass relied on (e.g. "balanced four-wire feeder").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
    /// Approximations the pass introduced (e.g. "Kron reduction of neutral").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approximations: Vec<String>,
    /// Fields/constraints dropped because the output family cannot carry them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<StructuredDiagnostic>,
    pub validation_status: ValidationStatus,
}

impl LoweringRecord {
    pub fn new(pass: impl Into<String>, input_kind: ModelKind, output_kind: ModelKind) -> Self {
        Self {
            pass: pass.into(),
            input_kind,
            output_kind,
            options: serde_json::Map::new(),
            assumptions: Vec::new(),
            approximations: Vec::new(),
            dropped_fields: Vec::new(),
            diagnostics: Vec::new(),
            validation_status: ValidationStatus::Ok,
        }
    }
}

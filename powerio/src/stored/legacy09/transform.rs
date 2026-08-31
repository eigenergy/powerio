//! Frozen 0.9 transformation history records.
//!
//! These records exist only to decode and round trip released 0.9
//! `NetworkPackage` documents before the one way upgrade. Live transformations
//! use `powerio_core::HistoryEntry` and `powerio_core::Diagnostic`.

use serde::{Deserialize, Serialize};

use crate::stored::legacy09::diagnostics::StructuredDiagnostic;
use crate::stored::legacy09::model::ModelKind;
use crate::stored::legacy09::validation::ValidationStatus;

/// One 0.9 lowering record, with its released serialized shape unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LoweringRecord {
    pub pass: String,
    pub input_kind: ModelKind,
    pub output_kind: ModelKind,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub options: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approximations: Vec<String>,
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

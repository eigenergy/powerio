//! The diagnostic record and the severity ladder.

use serde::{Deserialize, Serialize};

use crate::{DiagnosticCode, DiagnosticInfo, DiagnosticStage};

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
    /// The model exists but is not valid for the intended use without repair.
    Error,
    /// The operation could not complete. An error is a diagnostic that ended
    /// the operation, and it is the one severity that also carries a
    /// [`crate::ErrorCategory`].
    Fatal,
}

/// A pointer into one source artifact: where a canonical field came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SourceRef {
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// Byte offset, for binary sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
    /// Record / section / object type, e.g. `bus`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// Canonical field / property name, e.g. `vm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Raw token / value, when safe to embed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_token: Option<String>,
}

impl SourceRef {
    /// Create a reference to a declared source artifact.
    pub fn new(source_id: impl Into<String>) -> Self {
        Self {
            source_id: source_id.into(),
            line: None,
            column: None,
            byte_offset: None,
            record: None,
            field: None,
            raw_token: None,
        }
    }

    /// Set the field or property name.
    #[must_use]
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    /// Set the source record, section, or object type.
    #[must_use]
    pub fn with_record(mut self, record: impl Into<String>) -> Self {
        self.record = Some(record.into());
        self
    }

    /// Set the source line number.
    #[must_use]
    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }
}

/// One structured finding.
///
/// The stage is not a field: it is the first segment of the code, read back
/// through [`StructuredDiagnostic::stage`], so a record cannot state a stage
/// its code contradicts. The serialized form still carries `stage` for a
/// consumer that prefers an enum to a string split, and it is optional there:
/// a producer whose namespace is outside powerio's ten omits it, and a reader
/// that wants the truth decodes the code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "DiagnosticWire", into = "DiagnosticWire")]
pub struct StructuredDiagnostic {
    pub code: DiagnosticCode,
    pub severity: DiagnosticSeverity,
    /// One line. Never contains a newline, never repeats the code, and is
    /// covered by no stability promise: a consumer branches on the code.
    pub message: String,
    /// JSON pointer (or best-effort locator) of the element the finding is about.
    pub element_path: Option<String>,
    pub source_ref: Option<SourceRef>,
    /// Code-specific structured payload, e.g. `{"dropped_fields": ["angmin"]}`.
    pub details: serde_json::Map<String, serde_json::Value>,
    pub suggested_action: Option<String>,
    /// Workflows for which this finding is safe to ignore, e.g.
    /// `["power_flow", "opf"]`. Empty means "no such assurance".
    pub safe_to_ignore: Vec<String>,
}

impl StructuredDiagnostic {
    /// A minimal finding; fill the optional locators with the builder methods.
    pub fn new(
        code: impl Into<DiagnosticCode>,
        severity: DiagnosticSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            message: message.into(),
            element_path: None,
            source_ref: None,
            details: serde_json::Map::new(),
            suggested_action: None,
            safe_to_ignore: Vec::new(),
        }
    }

    /// A finding from its registry entry, taking the entry's default severity.
    /// This is how an emission site names a code: the registry is the only
    /// place a code literal is written, so every emitted code is registered.
    pub fn of(info: &DiagnosticInfo, message: impl Into<String>) -> Self {
        Self::new(info.code, info.severity, message)
    }

    /// The stage this finding came from, decoded from the code. `None` when the
    /// namespace is outside powerio's ten.
    #[must_use]
    pub fn stage(&self) -> Option<DiagnosticStage> {
        self.code.stage()
    }

    #[must_use]
    pub fn with_severity(mut self, severity: DiagnosticSeverity) -> Self {
        self.severity = severity;
        self
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

/// One structured finding.
// The serialized shape of `StructuredDiagnostic`: `stage` is written from the
// code and dropped on read, so a document cannot state a stage its code
// contradicts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
struct DiagnosticWire {
    code: DiagnosticCode,
    severity: DiagnosticSeverity,
    /// The stage family, decoded from the first segment of `code`. Advisory:
    /// omitted when the namespace is outside powerio's ten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage: Option<DiagnosticStage>,
    message: String,
    /// JSON pointer (or best-effort locator) of the element the finding is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    element_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_ref: Option<SourceRef>,
    /// Code-specific structured payload, e.g. `{"dropped_fields": ["angmin"]}`.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    details: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suggested_action: Option<String>,
    /// Workflows for which this finding is safe to ignore, e.g.
    /// `["power_flow", "opf"]`. Empty means "no such assurance".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    safe_to_ignore: Vec<String>,
}

impl From<StructuredDiagnostic> for DiagnosticWire {
    fn from(d: StructuredDiagnostic) -> Self {
        let stage = d.stage();
        Self {
            code: d.code,
            severity: d.severity,
            stage,
            message: d.message,
            element_path: d.element_path,
            source_ref: d.source_ref,
            details: d.details,
            suggested_action: d.suggested_action,
            safe_to_ignore: d.safe_to_ignore,
        }
    }
}

impl From<DiagnosticWire> for StructuredDiagnostic {
    fn from(w: DiagnosticWire) -> Self {
        // The incoming `stage` is dropped: the code is the one source.
        Self {
            code: w.code,
            severity: w.severity,
            message: w.message,
            element_path: w.element_path,
            source_ref: w.source_ref,
            details: w.details,
            suggested_action: w.suggested_action,
            safe_to_ignore: w.safe_to_ignore,
        }
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for StructuredDiagnostic {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "StructuredDiagnostic".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        "powerio_diag::StructuredDiagnostic".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <DiagnosticWire as schemars::JsonSchema>::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StructuredDiagnostic {
        StructuredDiagnostic::new(
            "EMIT.PSSE.FIELD_DROPPED",
            DiagnosticSeverity::Warning,
            "generator cost curves have no PSS/E record and are dropped",
        )
    }

    #[test]
    fn the_wire_stage_comes_from_the_code() {
        let value = serde_json::to_value(sample()).unwrap();
        assert_eq!(value["stage"], serde_json::json!("emit"));
        assert_eq!(sample().stage(), Some(DiagnosticStage::Emit));
    }

    #[test]
    fn a_namespace_outside_the_ten_omits_the_wire_stage() {
        let d = StructuredDiagnostic::new(
            "W.FEEDER.VOLTAGE_LOW",
            DiagnosticSeverity::Warning,
            "a downstream verifier's own code",
        );
        let value = serde_json::to_value(&d).unwrap();
        assert!(value.get("stage").is_none());
        assert_eq!(d.stage(), None);
    }

    #[test]
    fn a_document_stage_that_contradicts_its_code_reads_back_from_the_code() {
        // Every pre-0.9 document carries a required `stage`; a refused include
        // was written with `parse` under a `READ` code.
        let json = r#"{
            "code": "READ.DSS.INCLUDE_REFUSED",
            "severity": "error",
            "stage": "parse",
            "message": "redirect ../shared.dss: refused"
        }"#;
        let d: StructuredDiagnostic = serde_json::from_str(json).unwrap();
        assert_eq!(d.stage(), Some(DiagnosticStage::Read));
        let value = serde_json::to_value(&d).unwrap();
        assert_eq!(value["stage"], serde_json::json!("read"));
    }

    #[test]
    fn a_document_without_a_stage_loads() {
        let json = r#"{
            "code": "EMIT.PSSE.FIELD_DROPPED",
            "severity": "warning",
            "message": "dropped"
        }"#;
        let d: StructuredDiagnostic = serde_json::from_str(json).unwrap();
        assert_eq!(d.stage(), Some(DiagnosticStage::Emit));
    }

    #[test]
    fn the_optional_fields_stay_off_the_wire_when_empty() {
        let value = serde_json::to_value(sample()).unwrap();
        let object = value.as_object().unwrap();
        for absent in [
            "element_path",
            "source_ref",
            "details",
            "suggested_action",
            "safe_to_ignore",
        ] {
            assert!(!object.contains_key(absent), "{absent}");
        }
        assert_eq!(object.len(), 4);
        assert!(serde_json::to_string(&sample()).unwrap().starts_with(
            r#"{"code":"EMIT.PSSE.FIELD_DROPPED","severity":"warning","stage":"emit","message":"#
        ));
    }

    #[test]
    fn severity_orders_worst_last() {
        let mut severities = [
            DiagnosticSeverity::Error,
            DiagnosticSeverity::Debug,
            DiagnosticSeverity::Fatal,
            DiagnosticSeverity::Info,
            DiagnosticSeverity::Warning,
        ];
        severities.sort_unstable();
        assert_eq!(severities.last(), Some(&DiagnosticSeverity::Fatal));
    }

    #[test]
    fn a_round_trip_keeps_every_field() {
        let d = sample()
            .with_element_path("/gen/1")
            .with_source_ref(SourceRef::new("case").with_line(12).with_field("gencost"))
            .with_details(
                serde_json::json!({"element": "gencost"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .with_suggested_action("write the costs to a sibling document")
            .with_severity(DiagnosticSeverity::Info);
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(
            serde_json::from_str::<StructuredDiagnostic>(&json).unwrap(),
            d
        );
    }
}

//! The diagnostic record and the severity ladder.

use serde::{Deserialize, Serialize};

use powerio_core::DiagnosticInfo;

use crate::legacy_diag::{DiagnosticCode, DiagnosticStage};

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

impl DiagnosticSeverity {
    /// Project a 1.0 registry entry onto the 0.9 ladder. `Fatal` was the 0.9
    /// spelling for the finding that ends an operation, which is exactly the
    /// entry that declares an error category.
    #[must_use]
    pub fn from_runtime(info: &DiagnosticInfo) -> Self {
        use powerio_core::DiagnosticSeverity as Runtime;

        match info.severity {
            Runtime::Error if info.category.is_some() => Self::Fatal,
            Runtime::Error => Self::Error,
            Runtime::Warning => Self::Warning,
            Runtime::Remark => Self::Info,
            Runtime::Note => Self::Debug,
        }
    }
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
#[serde(from = "SerializedDiagnostic", into = "SerializedDiagnostic")]
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
    pub fn of(info: &'static DiagnosticInfo, message: impl Into<String>) -> Self {
        Self::new(info.code, DiagnosticSeverity::from_runtime(info), message)
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
struct SerializedDiagnostic {
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

impl From<StructuredDiagnostic> for SerializedDiagnostic {
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

impl From<SerializedDiagnostic> for StructuredDiagnostic {
    fn from(s: SerializedDiagnostic) -> Self {
        // The incoming `stage` is dropped: the code is the one source.
        Self {
            code: s.code,
            severity: s.severity,
            message: s.message,
            element_path: s.element_path,
            source_ref: s.source_ref,
            details: s.details,
            suggested_action: s.suggested_action,
            safe_to_ignore: s.safe_to_ignore,
        }
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for StructuredDiagnostic {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "StructuredDiagnostic".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        "crate::legacy_diag::StructuredDiagnostic".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <SerializedDiagnostic as schemars::JsonSchema>::json_schema(generator)
    }
}

// The 1.0 runtime record has four severities, a target, and byte spans; the
// 0.9 document has five severities and a seven field source pointer. A crate
// below this one now emits the runtime record, so writing a 0.9 document
// projects it back. The projection is one way: it is only used to keep writing
// documents the released reader accepts.
impl From<powerio_core::Diagnostic> for StructuredDiagnostic {
    fn from(diagnostic: powerio_core::Diagnostic) -> Self {
        use powerio_core::DiagnosticSeverity as Runtime;

        let severity = match diagnostic.severity() {
            // A runtime error that ends an operation declares a category; the
            // 0.9 document spelled that Fatal.
            Runtime::Error
                if diagnostic
                    .registered_info()
                    .is_some_and(|i| i.category.is_some()) =>
            {
                DiagnosticSeverity::Fatal
            }
            Runtime::Error => DiagnosticSeverity::Error,
            Runtime::Warning => DiagnosticSeverity::Warning,
            Runtime::Remark => DiagnosticSeverity::Info,
            Runtime::Note => DiagnosticSeverity::Debug,
        };

        let mut record = Self::new(diagnostic.code(), severity, diagnostic.message());
        if let Some(target) = diagnostic.target() {
            record = record.with_element_path(target);
        }
        if let Some(span) = diagnostic.spans().first() {
            record = record.with_source_ref(
                SourceRef::new(span.source().as_str()).with_byte_offset(span.byte_start()),
            );
        }
        if let Some(action) = diagnostic.suggested_action() {
            record = record.with_suggested_action(action);
        }
        if !diagnostic.details().is_empty() {
            record = record.with_details(diagnostic.details().clone());
        }
        record
    }
}

impl SourceRef {
    /// Byte offset of the finding inside its source.
    #[must_use]
    pub fn with_byte_offset(mut self, offset: u64) -> Self {
        self.byte_offset = Some(offset);
        self
    }
}

// Reading a 0.9 document back into a 1.0 runtime record. The code becomes an
// external one: a registry entry is a compile time item, and a document can
// carry a code from a producer this build does not know.
impl From<StructuredDiagnostic> for powerio_core::Diagnostic {
    fn from(record: StructuredDiagnostic) -> Self {
        use powerio_core::DiagnosticSeverity as Runtime;

        let severity = match record.severity {
            DiagnosticSeverity::Fatal | DiagnosticSeverity::Error => Runtime::Error,
            DiagnosticSeverity::Warning => Runtime::Warning,
            DiagnosticSeverity::Info => Runtime::Remark,
            DiagnosticSeverity::Debug => Runtime::Note,
        };
        let code = powerio_core::DiagnosticCode::new(record.code.as_str()).unwrap_or_else(|_| {
            powerio_core::DiagnosticCode::new("PARTNER.LEGACY.UNCODED").expect("static code")
        });
        let mut diagnostic = powerio_core::Diagnostic::new(code, severity, record.message);
        if let Some(path) = record.element_path {
            // A stored locator past the runtime bounds is dropped visibly:
            // the finding survives and says a locator existed and how long
            // it was, rather than silently changing identity.
            let byte_length = path.len();
            if diagnostic.set_target(path).is_err() {
                let _ = diagnostic
                    .insert_detail("dropped_target_bytes", serde_json::Value::from(byte_length));
            }
        }
        if let Some(action) = record.suggested_action {
            diagnostic = diagnostic.with_suggested_action(action);
        }
        if !record.details.is_empty() {
            // Same rule for a stored detail map past the runtime bounds:
            // keep the finding, mark the loss.
            let key_count = record.details.len();
            if diagnostic.set_details(record.details).is_err() {
                let _ = diagnostic
                    .insert_detail("dropped_detail_keys", serde_json::Value::from(key_count));
            }
        }
        diagnostic
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
    fn the_serialized_stage_comes_from_the_code() {
        let value = serde_json::to_value(sample()).unwrap();
        assert_eq!(value["stage"], serde_json::json!("emit"));
        assert_eq!(sample().stage(), Some(DiagnosticStage::Emit));
    }

    #[test]
    fn a_namespace_outside_the_ten_omits_the_serialized_stage() {
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
    fn the_empty_optional_fields_are_not_serialized() {
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

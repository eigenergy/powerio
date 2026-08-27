//! The frozen 0.9 structured diagnostic records, retained only for the one
//! way `.pio.json` upgrade. Live 1.0 findings are `powerio_core::Diagnostic`;
//! the facade's registry lives in [`crate::codes`].

pub use crate::codes;
pub use crate::stored::legacy09::legacy_diag::{
    DiagnosticCode, DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic,
};

/// One structured package diagnostic as a module record, carrying `target`
/// when the caller could translate the legacy element path into the module
/// value's own pointer grammar. Unregistered legacy codes fall back to a
/// permanent marker rather than refusing the record.
pub(crate) fn to_module_diagnostic(
    legacy: &StructuredDiagnostic,
    target: Option<String>,
) -> powerio_core::Diagnostic {
    use powerio_core::{Diagnostic, DiagnosticCode};
    let code = DiagnosticCode::new(legacy.code.as_str())
        .unwrap_or_else(|_| DiagnosticCode::new("LEGACY.UNKNOWN").expect("static code is valid"));
    let severity = match legacy.severity {
        DiagnosticSeverity::Error => powerio_core::DiagnosticSeverity::Error,
        DiagnosticSeverity::Warning => powerio_core::DiagnosticSeverity::Warning,
        _ => powerio_core::DiagnosticSeverity::Note,
    };
    let mut diagnostic = Diagnostic::new(code, severity, legacy.message.clone());
    if let Some(target) = target
        && let Ok(with) = diagnostic.clone().with_target(target)
    {
        diagnostic = with;
    }
    diagnostic
}

/// A legacy `/model/{kind}/...` element path in the module value's own
/// grammar, or `None` when the path points outside the current value.
pub(crate) fn translate_legacy_target(element_path: Option<&str>, kind: &str) -> Option<String> {
    let path = element_path?;
    let prefix = format!("/model/{kind}");
    let rest = path.strip_prefix(&prefix)?;
    if rest.is_empty() {
        return None;
    }
    rest.starts_with('/').then(|| rest.to_owned())
}

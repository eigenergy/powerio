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
    let code = DiagnosticCode::new(legacy.code.as_str()).unwrap_or_else(|_| {
        DiagnosticCode::new("PARTNER.LEGACY.UNCODED").expect("static code is valid")
    });
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

#[cfg(test)]
mod tests {
    /// Every `DiagnosticCode::new(LITERAL).expect(...)` / `.unwrap()` site in
    /// this crate that hardcodes a fallback or sentinel code, so one of them
    /// can never again pass review malformed and panic instead of falling
    /// back. This file's own fallback did exactly that until now:
    /// `LEGACY.UNKNOWN` has two segments, one short of the three the grammar
    /// requires, so a legacy diagnostic whose own code failed validation
    /// panicked reaching for the fallback rather than using it.
    #[test]
    fn every_hardcoded_diagnostic_code_literal_is_well_formed() {
        for code in [
            "PARTNER.LEGACY.UNCODED", // this file, and legacy_diag/record.rs
            "READ.TEST.NOTE",         // powerio/tests/stored_module.rs
            "READ.CASE.FILLER",       // powerio/tests/module_lowering.rs
        ] {
            assert!(
                powerio_core::code_is_well_formed(code),
                "{code} would panic its DiagnosticCode::new(...).expect(...) site"
            );
        }
    }
}

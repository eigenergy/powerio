//! Structured diagnostics.
//!
//! The record, the code grammar, the severity ladder, and the stage family live
//! in `powerio-diag`; this module re-exports them so a package finding and a
//! distribution finding are the same type. A finding carries a stable
//! [`DiagnosticCode`], a [`DiagnosticSeverity`], a human message, and where
//! known an element path, a [`crate::provenance::SourceRef`], details, and a
//! suggested action. Human-readable warnings are rendered from these, never the
//! other way around.
//!
//! The stage a finding came from is the first segment of its code, read back
//! through [`StructuredDiagnostic::stage`]; the ten namespaces are `PARSE`,
//! `READ`, `CANONICALIZE`, `VALIDATE`, `LOWER`, `BUILD`, `EMIT`, `BIND`,
//! `PARTNER`, `REQUEST`.

pub use powerio_diag::{
    CodeStatus, DiagnosticCode, DiagnosticInfo, DiagnosticSeverity, DiagnosticStage,
    StructuredDiagnostic, check_registry, code_is_well_formed, render_line, render_lines,
};

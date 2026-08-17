//! Structured diagnostics for distribution conversions.
//!
//! The record itself lives in `powerio-diag`, below this crate and below the
//! `.pio.json` document model, so a distribution finding reaches a package
//! without a translation step. What stays here are the codes this crate emits.

pub use powerio_diag::{
    DiagnosticCode, DiagnosticInfo, DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic,
    render_line, render_lines,
};

/// A `Redirect`/`Compile`/`Buscoords` include the reader refused because it
/// escapes the case directory. Severity `Error`: the parse continued, but
/// the network is incomplete.
pub const READ_DSS_INCLUDE_REFUSED: &str = "READ.DSS.INCLUDE_REFUSED";

/// The reader stopped following `Redirect`/`Compile`/`Buscoords` includes
/// because the case exceeded the include budget. Severity `Error`: the parse
/// continued, but the network is incomplete.
pub const READ_DSS_INCLUDE_BUDGET: &str = "READ.DSS.INCLUDE_BUDGET";

/// A BMOPF field the schema types as a number holds something else. Severity
/// `Error`: the field reads as `NaN`, which serializes on as an unbounded
/// limit, so the parse states a fact the source never gave.
pub const READ_BMOPF_FIELD_NOT_A_NUMBER: &str = "READ.BMOPF.FIELD_NOT_A_NUMBER";

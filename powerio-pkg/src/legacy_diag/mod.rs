//! The frozen 0.9 diagnostic vocabulary that the 0.9 stored document uses.
//!
//! The 1.0 runtime record lives in `powerio-core`: four MLIR severities, a
//! target and byte spans instead of a `SourceRef`, and private fields. A 0.9
//! `.pio.json` document was written against the five-severity record with its
//! seven-field source pointer, so reading and writing that document needs the
//! shape it was written in. This module is that shape and nothing else builds
//! on it. It leaves with the crate when the facade takes over the stored
//! document.

pub mod category;
pub mod code;
pub mod collect;
pub mod nonfinite;
pub mod record;
pub mod registry;
pub mod render;

pub use category::ErrorCategory;
pub use code::{DiagnosticCode, DiagnosticStage, code_is_well_formed};
pub use collect::Diagnostics;
pub use record::{DiagnosticSeverity, SourceRef, StructuredDiagnostic};
pub use registry::{CodeStatus, DiagnosticInfo, check_registry, check_scope_ownership};
pub use render::{render_line, render_lines};

/// Declare one crate's diagnostic registry against the 0.9 vocabulary.
#[macro_export]
macro_rules! legacy_diagnostic_codes {
    ($(
        $(#[$attr:meta])*
        $name:ident = $code:literal, $severity:ident, $summary:literal
        $(, category = $category:ident)?
        $(, retired = $since:literal)? ;
    )*) => {
        $(
            $(#[$attr])*
            pub const $name: $crate::legacy_diag::DiagnosticInfo =
                $crate::legacy_diag::DiagnosticInfo::new(
                    $code,
                    $crate::legacy_diag::DiagnosticSeverity::$severity,
                    $summary,
                )
                $(.with_category($crate::legacy_diag::ErrorCategory::$category))?
                $(.retired($since))?;
        )*

        /// Every code declared by this registry.
        pub const ALL: &[&$crate::legacy_diag::DiagnosticInfo] = &[$(&$name),*];
    };
}

//! Dependency neutral compiler infrastructure shared by PowerIO crates.
//!
//! This crate owns source buffers, diagnostics, operation errors, the generic
//! [`PioModule`], repeated value containers, and output destinations. It does
//! not own electrical network, calculation, matrix, or dynamic value types.

mod codes;
mod diagnostic;
mod error;
mod module;
mod output;
mod records;
mod scenario;
mod source;
mod time_series;
mod validation;

pub use codes::CORE_DIAGNOSTIC_CODES;
pub use diagnostic::{
    CodeStatus, Diagnostic, DiagnosticCode, DiagnosticInfo, DiagnosticSeverity, DiagnosticStage,
    ErrorCategory, check_registry, check_scope_ownership, code_is_well_formed, render_diagnostic,
    render_diagnostics,
};
pub use error::Error;
pub use module::PioModule;
pub use output::{ArtifactPath, Destination, MemoryArtifact, WriteResult, WrittenOutput};
pub use records::{
    DiagnosticId, Digest, DigestAlgorithm, HistoryEntry, HistoryId, HistoryKind, Producer,
    SourceDescriptor, SourceId, SourceMapEntry, SourceRelation, SourceSpan,
};
pub use scenario::{SCENARIO_PROBABILITY_TOLERANCE, Scenario, ScenarioId, ScenarioSet};
pub use source::{FormatId, Source, SourceBuffer};
pub use time_series::{TimePoint, TimeSeries, checked_dimension_product};

/// Declare one crate's diagnostic registry.
///
/// Each code literal appears once in the declaration and the generated `ALL`
/// slice drives registry checks and reference generation.
#[macro_export]
macro_rules! diagnostic_codes {
    ($(
        $(#[$attr:meta])*
        $name:ident = $code:literal, $severity:ident, $summary:literal
        $(, category = $category:ident)?
        $(, retired = $since:literal)? ;
    )*) => {
        $(
            $(#[$attr])*
            pub const $name: $crate::DiagnosticInfo = $crate::DiagnosticInfo::new(
                $code,
                $crate::DiagnosticSeverity::$severity,
                $summary,
            )
            $(.with_category($crate::ErrorCategory::$category))?
            $(.retired($since))?;
        )*

        /// Every code declared by this registry.
        pub const ALL: &[&$crate::DiagnosticInfo] = &[$(&$name),*];
    };
}

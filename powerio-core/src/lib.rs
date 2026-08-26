//! Dependency neutral compiler infrastructure shared by PowerIO crates.
//!
//! This crate owns source buffers, diagnostics, operation errors, the generic
//! [`PioModule`], repeated value containers, and output destinations. It does
//! not own electrical network, calculation, matrix, or dynamic value types.

mod bounded;
mod codes;
mod diagnostic;
mod error;
mod module;
pub(crate) mod nonfinite;
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
pub use output::{
    __commit_staged_file, ArtifactPath, Destination, MemoryArtifact, WriteResult, WrittenOutput,
};
pub use records::{
    DiagnosticId, Digest, DigestAlgorithm, HistoryEntry, HistoryId, HistoryKind, Producer,
    SourceDescriptor, SourceId, SourceMapEntry, SourceRelation, SourceSpan,
};
pub use scenario::{SCENARIO_PROBABILITY_TOLERANCE, Scenario, ScenarioId, ScenarioSet};
pub use source::{FormatId, Source, SourceBuffer};
pub use time_series::{TimePoint, TimeSeries};

/// Cross-crate implementation support.
///
/// Audit outcome for every hidden item this crate exposes: the mutable
/// diagnostic collector and the checked dimension helper are crate private;
/// each emitting sibling crate carries its own byte identical crate-private
/// collector copy instead of importing one through a hidden path. What remains
/// here is the nonfinite serde adapter pair, which wraps a whole serializer or
/// deserializer inside the network types' serde trait impls. Duplicating that
/// machinery per crate would let the one shared float spelling diverge, so it
/// stays a single hidden seam: unstable, never re-exported by the facade, and
/// not accepted or returned by any public PowerIO operation.
#[doc(hidden)]
pub mod __implementation {
    /// The serde adapters that spell nonfinite floats for JSON.
    pub mod nonfinite {
        pub use crate::nonfinite::*;
    }
}

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

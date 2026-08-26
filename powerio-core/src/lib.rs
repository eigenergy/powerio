//! Dependency neutral compiler infrastructure shared by PowerIO crates.
//!
//! This crate owns source buffers, diagnostics, operation errors, the generic
//! [`PioModule`], repeated value containers, and output destinations. It does
//! not own electrical network, calculation, matrix, or dynamic value types.

mod codes;
mod collect;
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
pub use output::{ArtifactPath, Destination, MemoryArtifact, WriteResult, WrittenOutput};
pub use records::{
    DiagnosticId, Digest, DigestAlgorithm, HistoryEntry, HistoryId, HistoryKind, Producer,
    SourceDescriptor, SourceId, SourceMapEntry, SourceRelation, SourceSpan,
};
pub use scenario::{SCENARIO_PROBABILITY_TOLERANCE, Scenario, ScenarioId, ScenarioSet};
pub use source::{FormatId, Source, SourceBuffer};
pub use time_series::{TimePoint, TimeSeries};

/// Cross-crate implementation support.
///
/// These items exist because sibling crates in this workspace need them, not
/// because they are part of the PowerIO API. Nothing here is stable, and no
/// public PowerIO operation accepts or returns any of it. The facade does not
/// re-export this module.
#[doc(hidden)]
pub mod __implementation {
    /// The mutable collector an emitting pass threads through its call tree.
    /// An operation returns `Vec<Diagnostic>` or an `Error`, never this.
    pub use crate::collect::Diagnostics;
    /// The serde adapters that spell nonfinite floats for JSON.
    pub mod nonfinite {
        pub use crate::nonfinite::*;
    }
    /// Overflow-checked table sizing.
    pub use crate::time_series::checked_dimension_product;
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

//! Dependency neutral compiler infrastructure shared by PowerIO crates.
//!
//! This crate owns source buffers, diagnostics, operation errors, the generic
//! [`PioModule`], repeated value containers, and output destinations. It does
//! not own electrical network, calculation, matrix, or dynamic value types.

mod bounded;
pub mod codes;
mod component_id;
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
pub use component_id::ComponentId;
pub use diagnostic::{
    CodeStatus, Diagnostic, DiagnosticCode, DiagnosticInfo, DiagnosticSeverity, DiagnosticStage,
    ErrorCategory, check_registry, check_scope_ownership, code_is_well_formed, render_diagnostic,
    render_diagnostics,
};
pub use error::Error;
pub use module::{PioModule, StagedEdit};
pub use output::{
    ArtifactPath, Destination, EmitResult, EmittedOutput, Fidelity, IntoDestination,
    MemoryArtifact, OutputLayout,
};
pub use records::{
    DiagnosticId, Digest, DigestAlgorithm, HistoryEntry, HistoryId, HistoryKind, Producer,
    SourceDescriptor, SourceId, SourceMapEntry, SourceRelation, SourceSpan,
};
pub use scenario::{SCENARIO_PROBABILITY_TOLERANCE, Scenario, ScenarioId, ScenarioSet};
pub use source::{FormatId, IntoSource, MEMORY_SOURCE_NAME, Source, SourceBuffer};
pub use time_series::{TimePoint, TimeSeries};

/// Decode time record limits, shared by every PowerIO serialization.
///
/// Stored `.pio.json` records and core records must refuse the same hostile
/// inputs: every sequence, map, and string is bounded while it is decoded,
/// before the full collection has been retained. The helpers here run inside
/// serde visitors (`#[serde(deserialize_with = ...)]`), so the only transient
/// allocation is the JSON scanner's own token buffer.
pub mod limits {
    pub use crate::bounded::{BoundedStr, TruncatedStr, bounded_json_map, bounded_vec};
    pub use crate::validation::{
        MAX_DIAGNOSTIC_CODE_BYTES, MAX_DIAGNOSTIC_DETAIL_KEYS, MAX_DIAGNOSTIC_MESSAGE_BYTES,
        MAX_DIAGNOSTIC_MESSAGE_DECODE_BYTES, MAX_DIAGNOSTIC_RELATED, MAX_DIAGNOSTIC_SPANS,
        MAX_DIAGNOSTIC_TARGET_BYTES, MAX_HISTORY_NOTES, MAX_HISTORY_PARAMETERS,
        MAX_IDENTIFIER_BYTES, MAX_MODULE_DIAGNOSTICS, MAX_MODULE_EXTENSION_KEYS,
        MAX_MODULE_HISTORY_ENTRIES, MAX_MODULE_SOURCE_MAP_ENTRIES, MAX_MODULE_SOURCES,
        MAX_SOURCE_MAP_SPANS,
    };
}

/// Cross-crate implementation support.
///
/// Audit outcome for every `#[doc(hidden)]` `pub` item this crate exposes:
/// the mutable diagnostic collector and the checked dimension helper are
/// crate private; each emitting sibling crate carries its own byte identical
/// crate-private collector copy instead of importing one through a hidden
/// path. Two items remain, both re-exported here. The nonfinite serde
/// adapter pair wraps a whole serializer or deserializer inside the network
/// types' serde trait impls; duplicating that machinery per crate would let
/// the one shared float spelling diverge. `__commit_staged_file` commits a
/// file a streaming writer outside this crate already staged itself, for a
/// writer whose artifact must never be materialized in memory; every other
/// commit goes through [`Destination`]. Both stay a single hidden seam:
/// unstable, never re-exported by the facade, and not accepted or returned
/// by any public PowerIO operation.
#[doc(hidden)]
pub mod __implementation {
    /// The serde adapters that spell nonfinite floats for JSON.
    pub mod nonfinite {
        pub use crate::nonfinite::*;
    }

    /// Commit an already staged file onto its destination without
    /// materializing the artifact in memory first.
    pub use crate::output::__commit_staged_file;
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

#[cfg(test)]
mod tests {
    /// The doc comment on [`__implementation`] claims to enumerate every
    /// `#[doc(hidden)]` `pub` item this crate re-exports from its root. A
    /// future edit that adds another one, or that adds an item inside
    /// `__implementation` the comment does not name, would go unnoticed
    /// without this: `__implementation` must be the crate's only top level
    /// `#[doc(hidden)]` item, and its own direct items must be exactly the
    /// two the comment names.
    #[test]
    fn hidden_root_items_match_the_implementation_module_note() {
        let source = include_str!("lib.rs");

        let top_level_hidden = source
            .lines()
            .filter(|line| line.trim() == "#[doc(hidden)]")
            .count();
        assert_eq!(
            top_level_hidden, 1,
            "exactly one #[doc(hidden)] item is expected at the crate root: __implementation"
        );

        let start = source
            .find("pub mod __implementation {")
            .expect("the __implementation module must exist");
        let mut depth = 0i64;
        let mut direct_items = Vec::new();
        for (index, line) in source[start..].lines().enumerate() {
            if index == 0 {
                depth += i64::try_from(line.matches('{').count()).unwrap();
                depth -= i64::try_from(line.matches('}').count()).unwrap();
                continue;
            }
            if depth == 1 {
                let trimmed = line.trim();
                if trimmed.starts_with("pub mod ") || trimmed.starts_with("pub use ") {
                    direct_items.push(trimmed.to_owned());
                }
            }
            depth += i64::try_from(line.matches('{').count()).unwrap();
            depth -= i64::try_from(line.matches('}').count()).unwrap();
            if depth <= 0 {
                break;
            }
        }

        assert_eq!(
            direct_items,
            vec![
                "pub mod nonfinite {".to_owned(),
                "pub use crate::output::__commit_staged_file;".to_owned(),
            ],
            "__implementation's direct items no longer match the audit note above it"
        );
    }
}

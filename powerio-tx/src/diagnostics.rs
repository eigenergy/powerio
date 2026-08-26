//! The codes this crate emits, and the registry gates over them.
//!
//! The record, the code grammar, the severity ladder, and the stage family live
//! in `powerio-core`. What lives here is the transmission side registry: one
//! [`DiagnosticInfo`] per code, declared once, so an emission site names an
//! entry rather than a loose string and every emitted code is registered by
//! construction.
//!
//! Codes are families, not one per site: what differs between two sites of a
//! family is which field or record it was, which belongs in `details` where a
//! consumer can read it, rather than in a code nobody can enumerate.

// The collector is crate-private implementation support, not API: each
// emitting crate carries its own copy (src/collect.rs) and never exports it.
pub(crate) use crate::collect::Diagnostics;

pub use powerio_core::{
    Diagnostic, DiagnosticCode, DiagnosticInfo, DiagnosticSeverity, DiagnosticStage, ErrorCategory,
    check_registry, code_is_well_formed, render_diagnostic, render_diagnostics,
};

use crate::format::TargetFormat;

/// The write side family every target shares.
///
/// A writer's fidelity losses are the same eleven questions for every target —
/// what was dropped, what was defaulted, what was collapsed — so the family is
/// declared once per target and the shared writer passes take the target's
/// family rather than a label they cannot turn into a code.
#[derive(Clone, Copy, Debug)]
pub struct EmitFamily {
    /// A field the target format has no column or key for.
    pub field_dropped: DiagnosticInfo,
    /// A whole element or record the target does not model.
    pub record_dropped: DiagnosticInfo,
    /// A value the target requires and the source never stated.
    pub value_defaulted: DiagnosticInfo,
    /// Richer structure reduced to what the target's one field can hold.
    pub value_collapsed: DiagnosticInfo,
    /// A stated value replaced by another the target can represent.
    pub value_substituted: DiagnosticInfo,
    /// A value shortened to the target's width, e.g. a cost curve order.
    pub value_truncated: DiagnosticInfo,
    /// A branch rating set beyond the target's rate_a/rate_b/rate_c.
    pub rating_set_dropped: DiagnosticInfo,
    /// Source format passthrough fields the writer does not replay.
    pub extras_dropped: DiagnosticInfo,
    /// The area table, which is typed rather than passthrough.
    pub areas_dropped: DiagnosticInfo,
    /// A non-finite value written as a sentinel or a JSON null.
    pub not_a_number: DiagnosticInfo,
    /// The network has no reference bus for the target's solver to key on.
    pub reference_missing: DiagnosticInfo,
    /// A normalized line lands in the target's transformer section.
    pub element_relabeled: DiagnosticInfo,
}

macro_rules! emit_family {
    ($name:ident, $scope:literal, $label:literal) => {
        /// The write side family for this target.
        pub const $name: EmitFamily = EmitFamily {
            field_dropped: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".FIELD_DROPPED"),
                DiagnosticSeverity::Warning,
                concat!("a field ", $label, " has no place for was dropped"),
            ),
            record_dropped: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".RECORD_DROPPED"),
                DiagnosticSeverity::Warning,
                concat!("an element ", $label, " does not model was dropped"),
            ),
            value_defaulted: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".VALUE_DEFAULTED"),
                DiagnosticSeverity::Warning,
                concat!("a value ", $label, " requires was synthesized"),
            ),
            value_collapsed: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".VALUE_COLLAPSED"),
                DiagnosticSeverity::Warning,
                concat!("structure reduced to what ", $label, " can carry"),
            ),
            value_substituted: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".VALUE_SUBSTITUTED"),
                DiagnosticSeverity::Warning,
                concat!("a stated value was replaced by one ", $label, " can hold"),
            ),
            value_truncated: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".VALUE_TRUNCATED"),
                DiagnosticSeverity::Warning,
                concat!("a value was shortened to the width ", $label, " carries"),
            ),
            rating_set_dropped: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".RATING_SET_DROPPED"),
                DiagnosticSeverity::Warning,
                concat!(
                    "a branch rating set beyond rate_a/rate_b/rate_c was dropped: ",
                    $label,
                    " has no field for it"
                ),
            ),
            extras_dropped: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".EXTRAS_DROPPED"),
                DiagnosticSeverity::Warning,
                concat!(
                    "source format passthrough fields the ",
                    $label,
                    " writer does not replay were dropped"
                ),
            ),
            areas_dropped: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".AREAS_DROPPED"),
                DiagnosticSeverity::Warning,
                concat!("the area table was dropped: ", $label, " emits none"),
            ),
            not_a_number: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".NOT_A_NUMBER"),
                DiagnosticSeverity::Warning,
                concat!(
                    "a non-finite value was written as the sentinel ",
                    $label,
                    " uses, because it has no Inf or NaN"
                ),
            ),
            reference_missing: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".REFERENCE_MISSING"),
                DiagnosticSeverity::Warning,
                concat!(
                    "the network has no reference bus, which ",
                    $label,
                    " consumers reject"
                ),
            ),
            element_relabeled: DiagnosticInfo::new(
                concat!("EMIT.", $scope, ".ELEMENT_RELABELED"),
                DiagnosticSeverity::Warning,
                concat!(
                    "a normalized line reads as a transformer in the ",
                    $label,
                    " layout"
                ),
            ),
        };
    };
}

impl EmitFamily {
    /// Every entry, for the registry gates and the generated reference.
    #[must_use]
    pub fn entries(&'static self) -> [&'static DiagnosticInfo; 12] {
        [
            &self.field_dropped,
            &self.record_dropped,
            &self.value_defaulted,
            &self.value_collapsed,
            &self.value_substituted,
            &self.value_truncated,
            &self.rating_set_dropped,
            &self.extras_dropped,
            &self.areas_dropped,
            &self.not_a_number,
            &self.reference_missing,
            &self.element_relabeled,
        ]
    }
}

/// One [`EmitFamily`] per write target, plus the codes a single reader or a
/// single writer owns.
pub mod codes {
    use super::{DiagnosticInfo, DiagnosticSeverity, EmitFamily};

    emit_family!(EMIT_MATPOWER, "MATPOWER", "MATPOWER .m");
    emit_family!(EMIT_PSSE, "PSSE", "PSS/E .raw");
    emit_family!(EMIT_PSLF, "PSLF", "PSLF .epc");
    emit_family!(EMIT_PANDAPOWER, "PANDAPOWER", "pandapower JSON");
    emit_family!(EMIT_PYPSA, "PYPSA", "the PyPSA CSV folder");
    emit_family!(EMIT_POWERWORLD, "POWERWORLD", "PowerWorld .aux");
    emit_family!(EMIT_POWERMODELS, "POWERMODELS", "PowerModels JSON");
    emit_family!(EMIT_EGRET, "EGRET", "egret JSON");
    emit_family!(EMIT_SURGE, "SURGE", "Surge JSON");
    emit_family!(EMIT_PIO_JSON, "PIO_JSON", "the .pio.json snapshot");

    powerio_core::diagnostic_codes! {
        // PARSE: the source text could not be decoded as given.
        /// A leading UTF-8 byte order mark was removed before the reader saw
        /// the text, so a same-format echo differs by exactly those bytes.
        PARSE_SOURCE_BOM_STRIPPED = "PARSE.SOURCE.BOM_STRIPPED", Remark,
            "a leading UTF-8 byte order mark was removed before the reader ran", retired = "1.0.0";
        PARSE_MATPOWER_MALFORMED = "PARSE.MATPOWER.MALFORMED", Error,
            "a MATPOWER matrix is missing, short, unparseable, or unbalanced", category = Parse;
        PARSE_SOURCE_MALFORMED = "PARSE.SOURCE.MALFORMED", Error,
            "a format reader refused the source it was given", category = Parse;

        // READ: decoded, but not representable in the canonical model.
        READ_PSSE_FIELD_DROPPED = "READ.PSSE.FIELD_DROPPED", Warning,
            "a PSS/E field with no canonical home was dropped";
        READ_PSSE_VALUE_SUBSTITUTED = "READ.PSSE.VALUE_SUBSTITUTED", Warning,
            "a PSS/E value the record states could not be used as given";
        READ_PSSE_VALUE_UNSUPPORTED = "READ.PSSE.VALUE_UNSUPPORTED", Warning,
            "a PSS/E code word (CZ, CW, CM) outside the modeled set was read as the default";
        READ_PSSE_REFERENCE_DROPPED = "READ.PSSE.REFERENCE_DROPPED", Warning,
            "a PSS/E control pointer names a bus the case does not declare";
        READ_PSSE_SECTION_UNSUPPORTED = "READ.PSSE.SECTION_UNSUPPORTED", Warning,
            "a PSS/E section is preserved in a same-format echo only";
        READ_PSSE_RETAINED_SOURCE_ONLY = "READ.PSSE.RETAINED_SOURCE_ONLY", Remark,
            "a PSS/E field survives in extras rather than in a typed field";

        READ_PSLF_VALUE_DEFAULTED = "READ.PSLF.VALUE_DEFAULTED", Warning,
            "a PSLF value the model needs was not in the source and was defaulted";
        READ_PSLF_VALUE_APPROXIMATED = "READ.PSLF.VALUE_APPROXIMATED", Warning,
            "a PSLF value was read through an approximation the .epc model forces";
        READ_PSLF_RECORD_DROPPED = "READ.PSLF.RECORD_DROPPED", Warning,
            "a PSLF record could not be mapped and was dropped";
        READ_PSLF_SOURCE_MALFORMED = "READ.PSLF.SOURCE_MALFORMED", Warning,
            "a PSLF section header, count, or end marker disagrees with the records";
        READ_PSLF_RETAINED_SOURCE_ONLY = "READ.PSLF.RETAINED_SOURCE_ONLY", Remark,
            "a PSLF section survives in the retained source or in extras only";

        READ_PANDAPOWER_FIELD_DROPPED = "READ.PANDAPOWER.FIELD_DROPPED", Warning,
            "a pandapower field with no canonical home was dropped";
        READ_PANDAPOWER_TABLE_UNSUPPORTED = "READ.PANDAPOWER.TABLE_UNSUPPORTED", Warning,
            "a pandapower table is not mapped into the canonical model";

        READ_PYPSA_TABLE_UNSUPPORTED = "READ.PYPSA.TABLE_UNSUPPORTED", Warning,
            "a PyPSA table is not mapped into the canonical model";
        READ_PYPSA_VALUE_APPROXIMATED = "READ.PYPSA.VALUE_APPROXIMATED", Warning,
            "a PyPSA element was read through the nearest canonical element";
        READ_PYPSA_NAME_REMAPPED = "READ.PYPSA.NAME_REMAPPED", Warning,
            "a PyPSA bus name collides with another and was keyed by its numeric id";

        READ_POWERWORLD_VALUE_DEFAULTED = "READ.POWERWORLD.VALUE_DEFAULTED", Warning,
            "a PowerWorld field this binary vintage does not locate was defaulted";
        READ_POWERWORLD_RETAINED_SOURCE_ONLY = "READ.POWERWORLD.RETAINED_SOURCE_ONLY", Warning,
            "a PowerWorld aux data block survives in the retained source only";

        READ_POWERMODELS_RECORD_DROPPED = "READ.POWERMODELS.RECORD_DROPPED", Warning,
            "a PowerModels document states more than the canonical snapshot holds";
        READ_POWERMODELS_FIELD_DROPPED = "READ.POWERMODELS.FIELD_DROPPED", Warning,
            "a PowerModels field the canonical model cannot state was dropped";

        READ_GOC3_VALUE_INFERRED = "READ.GOC3.VALUE_INFERRED", Warning,
            "a GO Challenge 3 value the document never states was inferred";
        READ_GOC3_RETAINED_SOURCE_ONLY = "READ.GOC3.RETAINED_SOURCE_ONLY", Warning,
            "a GO Challenge 3 section survives in the retained source only";

        READ_OPFDATA_FIELD_DROPPED = "READ.OPFDATA.FIELD_DROPPED", Warning,
            "an OPFData field outside the published schema is not in the snapshot";
        READ_OPFDATA_VALUE_INFERRED = "READ.OPFDATA.VALUE_INFERRED", Warning,
            "OPFData carries no identity or frequency, so the reader synthesized them";
        READ_OPFDATA_RETAINED_SOURCE_ONLY = "READ.OPFDATA.RETAINED_SOURCE_ONLY", Warning,
            "an OPFData grid feature survives in the retained source only";

        READ_SURGE_RETAINED_SOURCE_ONLY = "READ.SURGE.RETAINED_SOURCE_ONLY", Warning,
            "a Surge section survives in the retained source only";

        READ_GEO_SOURCE_MALFORMED = "READ.GEO.SOURCE_MALFORMED", Warning,
            "a geo layer row could not be read and was skipped";
        READ_GEO_NOTES_TRUNCATED = "READ.GEO.NOTES_TRUNCATED", Warning,
            "the geo reader stopped recording notes at its budget";

        READ_IO_FAILED = "READ.IO.FAILED", Error,
            "the case file could not be read", category = Io;

        // CANONICALIZE: normalization of an already-read network.
        CANONICALIZE_NORMALIZE_BOUNDS_CLAMPED = "CANONICALIZE.NORMALIZE.BOUNDS_CLAMPED", Remark,
            "a branch angle difference bound was clamped into the modeled range";
        CANONICALIZE_NORMALIZE_NO_REFERENCE_BUS = "CANONICALIZE.NORMALIZE.NO_REFERENCE_BUS", Error,
            "no reference bus can be established: no bus hosts an in-service generator",
            category = Data;
        CANONICALIZE_NORMALIZE_REFERENCE_DESIGNATED =
            "CANONICALIZE.NORMALIZE.REFERENCE_DESIGNATED", Warning,
            "the case states no surviving reference bus, so normalization designated a slack";
        CANONICALIZE_NORMALIZE_GEN_COST_ABSENT =
            "CANONICALIZE.NORMALIZE.GEN_COST_ABSENT", Warning,
            "the solver-ready copy has in-service generators and no cost data, so any cost objective built from it is zero";
        CANONICALIZE_NORMALIZE_INVALID_OPTION = "CANONICALIZE.NORMALIZE.INVALID_OPTION", Error,
            "a normalize option is outside the range it is defined on", category = Data;
        CANONICALIZE_NORMALIZE_INVALID_BASE_MVA = "CANONICALIZE.NORMALIZE.INVALID_BASE_MVA", Error,
            "the case base MVA is not a positive finite number", category = Data;

        // BUILD: assembling a derived object from a network that already parsed.
        BUILD_INDEX_UNKNOWN_BUS = "BUILD.INDEX.UNKNOWN_BUS", Error,
            "an element references a bus id the case does not declare", category = Data;
        BUILD_INDEX_REFERENCE_BUS_COUNT = "BUILD.INDEX.REFERENCE_BUS_COUNT", Error,
            "the index needs exactly one reference bus", category = Data;
        BUILD_INDEX_UNGROUNDED_COMPONENT = "BUILD.INDEX.UNGROUNDED_COMPONENT", Error,
            "a connected component has no reference bus to ground", category = Data;
        BUILD_BRANCH_ZERO_IMPEDANCE = "BUILD.BRANCH.ZERO_IMPEDANCE", Error,
            "a branch has a zero matrix denominator under the selected build options",
            category = Data;
        BUILD_BRANCH_NOT_A_NUMBER = "BUILD.BRANCH.NOT_A_NUMBER", Error,
            "a branch susceptance is not finite", category = Data;
        BUILD_BRANCH_DEGENERATE_TAP = "BUILD.BRANCH.DEGENERATE_TAP", Error,
            "a branch tap ratio is too small to divide by", category = Data;
        BUILD_GEO_UNLOCATED_ELEMENTS = "BUILD.GEO.UNLOCATED_ELEMENTS", Error,
            "a geo apply left elements with no location or route", category = Data;
        BUILD_GEO_APPLY_SUMMARY = "BUILD.GEO.APPLY_SUMMARY", Remark,
            "how many elements a geo apply located";
        BUILD_GEO_UNMATCHED_FEATURE = "BUILD.GEO.UNMATCHED_FEATURE", Warning,
            "a geo feature matched no element in the network";

        // VALIDATE: the case's own internal consistency.
        /// Emitted by the stored document's payload validation in the facade;
        /// declared here because this crate owns the balanced model.
        VALIDATE_BALANCED_STRUCTURE = "VALIDATE.BALANCED.STRUCTURE", Error,
            "a balanced payload's referential integrity does not hold";
        VALIDATE_BALANCED_VALUE_DOMAIN = "VALIDATE.BALANCED.VALUE_DOMAIN", Warning,
            "a balanced payload value is outside the domain the model states";
        VALIDATE_BALANCED_PAYLOAD_IDENTITY = "VALIDATE.BALANCED.PAYLOAD_IDENTITY", Error,
            "a balanced payload's uid identity does not hold";
        /// Retired in 0.9.0: every transmission read finding now carries its
        /// own code, so the stored document no longer wraps them under one
        /// catch-all.
        READ_TRANSMISSION_PARSE_WARNING = "READ.TRANSMISSION.PARSE_WARNING", Warning,
            "a transmission parse finding with no identity of its own", retired = "0.9.0";
        VALIDATE_GEN_COST_MISSING = "VALIDATE.GEN_COST.MISSING", Error,
            "a generator carries no cost data under a policy that requires one", category = Data;
        VALIDATE_GEN_COST_NOT_A_NUMBER = "VALIDATE.GEN_COST.NOT_A_NUMBER", Error,
            "a default generator cost field is not finite", category = Data;
        VALIDATE_GEN_COST_PATCH_INVALID = "VALIDATE.GEN_COST.PATCH_INVALID", Error,
            "a generator cost patch row is not usable", category = Data;
        VALIDATE_GEN_COST_COUNT_MISMATCH = "VALIDATE.GEN_COST.COUNT_MISMATCH", Error,
            "the cost table has neither one row per generator nor two", category = Data;
        VALIDATE_DC_LINE_COST_COUNT_MISMATCH = "VALIDATE.DC_LINE_COST.COUNT_MISMATCH", Error,
            "the dcline cost table has other than one row per dcline", category = Data;

        // LOWER: a policy applied on the way into a target.
        TRANSFORM_GEN_COST_POLICY_APPLIED = "TRANSFORM.GEN_COST.POLICY_APPLIED", Remark,
            "a write time generator cost policy patched or synthesized costs";

        // REQUEST: the call named something powerio does not provide.
        REQUEST_FORMAT_UNKNOWN = "REQUEST.FORMAT.UNKNOWN", Error,
            "the named case format is not one powerio reads", category = Request;
        REQUEST_FORMAT_WRITE_UNSUPPORTED = "REQUEST.FORMAT.WRITE_UNSUPPORTED", Error,
            "the named case format is read only and has no writer", category = Request;

        // Write side codes a single target owns.
        /// The default `.raw` target is revision 33, so writing a newer source
        /// through it re-emits the older layout.
        EMIT_PSSE_DOWNGRADED = "EMIT.PSSE.DOWNGRADED", Warning,
            "a newer PSS/E revision was written into an older layout";
        EMIT_PSSE_RATING_SET_REMAPPED = "EMIT.PSSE.RATING_SET_REMAPPED", Remark,
            "a named branch rating set was written into a PSS/E numbered rating slot";
    }

    /// Every write target's family, in the order [`super::registry`] reports
    /// them.
    pub const EMIT_FAMILIES: [&EmitFamily; 10] = [
        &EMIT_MATPOWER,
        &EMIT_PSSE,
        &EMIT_PSLF,
        &EMIT_PANDAPOWER,
        &EMIT_PYPSA,
        &EMIT_POWERWORLD,
        &EMIT_POWERMODELS,
        &EMIT_EGRET,
        &EMIT_SURGE,
        &EMIT_PIO_JSON,
    ];
}

/// Every code this crate declares.
#[must_use]
pub fn registry() -> Vec<&'static DiagnosticInfo> {
    let mut all: Vec<&'static DiagnosticInfo> = codes::ALL.to_vec();
    for family in codes::EMIT_FAMILIES {
        all.extend(family.entries());
    }
    all
}

impl TargetFormat {
    /// The write side family for this target.
    #[must_use]
    pub fn emit_family(self) -> &'static EmitFamily {
        match self {
            TargetFormat::Matpower => &codes::EMIT_MATPOWER,
            TargetFormat::Psse { .. } => &codes::EMIT_PSSE,
            TargetFormat::Pslf => &codes::EMIT_PSLF,
            TargetFormat::PandapowerJson => &codes::EMIT_PANDAPOWER,
            TargetFormat::PowerWorld => &codes::EMIT_POWERWORLD,
            TargetFormat::PowerModelsJson => &codes::EMIT_POWERMODELS,
            TargetFormat::EgretJson => &codes::EMIT_EGRET,
            TargetFormat::SurgeJson => &codes::EMIT_SURGE,
            // Neither GOC3 nor OPFData has a writer: the request is refused
            // before any family is consulted, and
            // `REQUEST.FORMAT.WRITE_UNSUPPORTED` carries it.
            TargetFormat::Goc3Json | TargetFormat::DeepMindOpfDataJson => &codes::EMIT_PIO_JSON,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_sound() {
        let problems = check_registry(registry());
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn every_write_target_has_a_family_of_its_own() {
        let mut scopes: Vec<&str> = codes::EMIT_FAMILIES
            .iter()
            .map(|f| f.field_dropped.code.split('.').nth(1).unwrap())
            .collect();
        scopes.sort_unstable();
        scopes.dedup();
        assert_eq!(scopes.len(), codes::EMIT_FAMILIES.len());
        assert_eq!(
            TargetFormat::Psse { rev: 33 }
                .emit_family()
                .field_dropped
                .code,
            "EMIT.PSSE.FIELD_DROPPED"
        );
    }
}

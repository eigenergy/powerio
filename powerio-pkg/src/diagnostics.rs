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
//!
//! [`codes`] is this crate's registry: one entry per code it emits, so an
//! emission site names an entry rather than a loose string.

pub use powerio_diag::{
    CodeStatus, DiagnosticCode, DiagnosticInfo, DiagnosticSeverity, DiagnosticStage, Diagnostics,
    StructuredDiagnostic, check_registry, code_is_well_formed, render_line, render_lines,
};

pub mod codes {
    powerio_diag::diagnostic_codes! {
        // READ: what a reader's own findings arrive as once the package lifts
        // them. A reader finding keeps the code its crate gave it; these are
        // the package's own.
        READ_PACKAGE_OPERATING_POINTS_DROPPED = "READ.PACKAGE.OPERATING_POINTS_DROPPED", Warning,
            "a time series could not be lifted into operating points";
        /// Retired in 0.9.0 for a code that names its three segments.
        READ_OPERATING_POINTS_DROPPED = "READ.OPERATING_POINTS_DROPPED", Warning,
            "a time series could not be lifted into operating points", retired = "0.9.0";
        /// Retired in 0.9.0 for the per format spelling above.
        READ_GOC3_OPERATING_POINTS_DROPPED = "READ.GOC3.OPERATING_POINTS_DROPPED", Warning,
            "a GO Challenge 3 time series could not be lifted", retired = "0.9.0";
        /// Retired in 0.9.0: every transmission read finding now carries its
        /// own code, so the package no longer wraps them under one catch-all.
        READ_TRANSMISSION_PARSE_WARNING = "READ.TRANSMISSION.PARSE_WARNING", Warning,
            "a transmission parse finding with no identity of its own", retired = "0.9.0";

        // VALIDATE: the document's own internal consistency, nothing else.
        VALIDATE_BALANCED_STRUCTURE = "VALIDATE.BALANCED.STRUCTURE", Error,
            "a balanced payload's referential integrity does not hold";
        VALIDATE_BALANCED_VALUE_DOMAIN = "VALIDATE.BALANCED.VALUE_DOMAIN", Warning,
            "a balanced payload value is outside the domain the model states";
        VALIDATE_BALANCED_PAYLOAD_IDENTITY = "VALIDATE.BALANCED.PAYLOAD_IDENTITY", Error,
            "a balanced payload's uid identity does not hold";
        VALIDATE_MULTI_STRUCTURE = "VALIDATE.MULTI.STRUCTURE", Error,
            "a multiconductor payload's referential integrity does not hold";
        VALIDATE_MULTI_TERMINAL_MAP = "VALIDATE.MULTI.TERMINAL_MAP", Error,
            "a multiconductor terminal map does not match the element it belongs to";
        VALIDATE_MULTI_UNTYPED_OBJECT = "VALIDATE.MULTI.UNTYPED_OBJECT", Warning,
            "a multiconductor object is carried untyped";
        VALIDATE_MULTI_NO_VOLTAGE_SOURCE = "VALIDATE.MULTI.NO_VOLTAGE_SOURCE", Warning,
            "a multiconductor payload declares no voltage source";
        VALIDATE_PACKAGE_STUDY_MODEL_KIND = "VALIDATE.PACKAGE.STUDY_MODEL_KIND", Error,
            "the study block's model kind disagrees with the payload";
        VALIDATE_PACKAGE_STUDY_IDENTITY = "VALIDATE.PACKAGE.STUDY_IDENTITY", Error,
            "a study edit names a uid the payload does not declare";
        VALIDATE_PACKAGE_OPERATING_IDENTITY = "VALIDATE.PACKAGE.OPERATING_IDENTITY", Error,
            "an operating point names a uid the payload does not declare";

        // LOWER: the multiconductor to balanced pass.
        LOWER_MULTI_TO_BALANCED_AMBIGUOUS_TERMINAL_MAP =
            "LOWER.MULTI_TO_BALANCED.AMBIGUOUS_TERMINAL_MAP", Error,
            "a terminal map does not determine one balanced phase assignment";
        LOWER_MULTI_TO_BALANCED_BALANCED_VALUE_DOMAIN =
            "LOWER.MULTI_TO_BALANCED.BALANCED_VALUE_DOMAIN", Warning,
            "a lowered value is outside the domain the balanced model states";
        LOWER_MULTI_TO_BALANCED_DROPPED_LOAD_VOLTAGE_MODEL =
            "LOWER.MULTI_TO_BALANCED.DROPPED_LOAD_VOLTAGE_MODEL", Warning,
            "a load voltage model has no balanced spelling and was dropped";
        LOWER_MULTI_TO_BALANCED_DROPPED_OPEN_SWITCH =
            "LOWER.MULTI_TO_BALANCED.DROPPED_OPEN_SWITCH", Info,
            "an open switch was dropped from the balanced network";
        LOWER_MULTI_TO_BALANCED_INVALID_BALANCED_OUTPUT =
            "LOWER.MULTI_TO_BALANCED.INVALID_BALANCED_OUTPUT", Error,
            "the lowered network does not validate";
        LOWER_MULTI_TO_BALANCED_INVALID_BASE_MVA =
            "LOWER.MULTI_TO_BALANCED.INVALID_BASE_MVA", Error,
            "the requested base MVA is not a positive finite number";
        LOWER_MULTI_TO_BALANCED_INVALID_LINECODE_MATRIX =
            "LOWER.MULTI_TO_BALANCED.INVALID_LINECODE_MATRIX", Error,
            "a linecode matrix cannot be reduced to a balanced impedance";
        LOWER_MULTI_TO_BALANCED_INVALID_PHASE_REFERENCE =
            "LOWER.MULTI_TO_BALANCED.INVALID_PHASE_REFERENCE", Error,
            "a phase reference names a conductor the element does not have";
        LOWER_MULTI_TO_BALANCED_INVALID_SHUNT_MATRIX =
            "LOWER.MULTI_TO_BALANCED.INVALID_SHUNT_MATRIX", Error,
            "a shunt matrix cannot be reduced to a balanced admittance";
        LOWER_MULTI_TO_BALANCED_KRON_REDUCTION_REQUIRED =
            "LOWER.MULTI_TO_BALANCED.KRON_REDUCTION_REQUIRED", Info,
            "a conductor set needs a Kron reduction the pass does not perform";
        LOWER_MULTI_TO_BALANCED_LINECODE_TERMINAL_MISMATCH =
            "LOWER.MULTI_TO_BALANCED.LINECODE_TERMINAL_MISMATCH", Error,
            "a linecode's dimension disagrees with the line's terminal map";
        LOWER_MULTI_TO_BALANCED_MISSING_PHASE_REFERENCE =
            "LOWER.MULTI_TO_BALANCED.MISSING_PHASE_REFERENCE", Error,
            "an element states no phase reference to lower against";
        LOWER_MULTI_TO_BALANCED_NONFINITE_LINE_LENGTH =
            "LOWER.MULTI_TO_BALANCED.NONFINITE_LINE_LENGTH", Error,
            "a line length is not finite, so its impedance is undefined";
        LOWER_MULTI_TO_BALANCED_PHASE_MAP_MISMATCH =
            "LOWER.MULTI_TO_BALANCED.PHASE_MAP_MISMATCH", Error,
            "the two ends of an element do not agree on their phase map";
        LOWER_MULTI_TO_BALANCED_SEQUENCE_COUPLING_DROPPED =
            "LOWER.MULTI_TO_BALANCED.SEQUENCE_COUPLING_DROPPED", Info,
            "sequence coupling has no balanced spelling and was dropped";
        LOWER_MULTI_TO_BALANCED_UNKNOWN_BUS = "LOWER.MULTI_TO_BALANCED.UNKNOWN_BUS", Error,
            "an element references a bus the multiconductor payload does not declare";
        LOWER_MULTI_TO_BALANCED_UNKNOWN_LINECODE =
            "LOWER.MULTI_TO_BALANCED.UNKNOWN_LINECODE", Error,
            "a line references a linecode the payload does not declare";
        LOWER_MULTI_TO_BALANCED_UNKNOWN_SOURCE_BUS =
            "LOWER.MULTI_TO_BALANCED.UNKNOWN_SOURCE_BUS", Error,
            "a voltage source references a bus the payload does not declare";
        LOWER_MULTI_TO_BALANCED_UNSUPPORTED_CLOSED_SWITCH =
            "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CLOSED_SWITCH", Error,
            "a closed switch shape has no balanced spelling";
        LOWER_MULTI_TO_BALANCED_UNSUPPORTED_CONDUCTOR_SET =
            "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_CONDUCTOR_SET", Error,
            "a conductor set has no balanced spelling";
        LOWER_MULTI_TO_BALANCED_UNSUPPORTED_OBJECT =
            "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_OBJECT", Error,
            "an object has no balanced spelling and was dropped";
        LOWER_MULTI_TO_BALANCED_UNSUPPORTED_TRANSFORMER =
            "LOWER.MULTI_TO_BALANCED.UNSUPPORTED_TRANSFORMER", Error,
            "a transformer shape has no balanced spelling";
        LOWER_MULTI_TO_BALANCED_WRONG_MODEL_KIND =
            "LOWER.MULTI_TO_BALANCED.WRONG_MODEL_KIND", Error,
            "the package does not carry a multiconductor payload to lower";

        // Failures.
        PARSE_PACKAGE_MALFORMED = "PARSE.PACKAGE.MALFORMED", Fatal,
            "the document is not well formed .pio.json", category = Parse;
        PARSE_PACKAGE_UNSUPPORTED_VERSION = "PARSE.PACKAGE.UNSUPPORTED_VERSION", Fatal,
            "the document comes from a lineage this build does not read", category = Parse;
        VALIDATE_PACKAGE_MODEL_KIND_MISMATCH = "VALIDATE.PACKAGE.MODEL_KIND_MISMATCH", Fatal,
            "the document's model_kind disagrees with the payload it carries", category = Data;
        REQUEST_PACKAGE_NO_SUCH_INDEX = "REQUEST.PACKAGE.NO_SUCH_INDEX", Fatal,
            "the call names an operating point or study index the document does not carry",
            category = Data;
        REQUEST_PACKAGE_WRONG_MODEL_KIND = "REQUEST.PACKAGE.WRONG_MODEL_KIND", Fatal,
            "the call asks for a model family the document does not carry", category = Data;
        BUILD_PACKAGE_PAYLOAD_FAILED = "BUILD.PACKAGE.PAYLOAD_FAILED", Fatal,
            "the payload could not be built, applied, or serialized", category = Data;
        EMIT_PACKAGE_SERIALIZE_FAILED = "EMIT.PACKAGE.SERIALIZE_FAILED", Fatal,
            "serializing the package to JSON failed", category = Output;
    }
}

/// Every code this crate declares.
#[must_use]
pub fn registry() -> Vec<&'static DiagnosticInfo> {
    codes::ALL.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_sound() {
        let problems = check_registry(registry());
        assert!(problems.is_empty(), "{problems:#?}");
    }
}

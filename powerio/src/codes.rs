//! The facade's diagnostic code registry: the stored module, upgrade,
//! legacy 0.9 package, and multiconductor to balanced transformation
//! entries of the workspace registry. An emission site names an entry
//! rather than a loose string.

powerio_core::diagnostic_codes! {
    READ_MODULE_UNSUPPORTED = "READ.MODULE.UNSUPPORTED", Error,
        "the stored module names a schema or version this build does not read", category = Request;
    READ_MODULE_INVALID = "READ.MODULE.INVALID", Error,
        "the stored module document is structurally invalid", category = Data;
    READ_MODULE_LEGACY_STUDY = "READ.MODULE.LEGACY_STUDY", Error,
        "a 0.9 package with a nonempty study block needs a revision materialized before upgrade", category = Request;
    READ_MODULE_LEGACY_FIELD = "READ.MODULE.LEGACY_FIELD", Error,
        "a 0.9 operating point update field has no state quantity", category = Data;
    READ_MODULE_UPGRADED = "READ.MODULE.UPGRADED", Note,
        "a released 0.9 package was upgraded one way to the stored module";
    READ_MODULE_OBJECTIVE_TERM_RETIRED = "READ.MODULE.OBJECTIVE_TERM_RETIRED", Warning,
        "a retired solver setting was removed from a stored OPF objective";
    READ_MODULE_BRANCH_DUAL_SPLIT = "READ.MODULE.BRANCH_DUAL_SPLIT", Warning,
        "a legacy signed branch dual was split into directional multipliers";
    REQUEST_STATE_NOT_A_COLLECTION = "REQUEST.STATE.NOT_A_COLLECTION", Error,
        "the value carries no time or scenario collection to select from",
        category = Request;
    REQUEST_STATE_WRONG_SELECTOR = "REQUEST.STATE.WRONG_SELECTOR", Error,
        "the selector kind does not match what the collection keys by",
        category = Request;
    REQUEST_STATE_OUT_OF_RANGE = "REQUEST.STATE.OUT_OF_RANGE", Error,
        "the requested time position is outside the collection's axis",
        category = Request;
    REQUEST_STATE_UNBOUND_EXPORT = "REQUEST.STATE.UNBOUND_EXPORT", Error,
        "the selected item's static materialization is not bound yet",
        category = Request;
    REQUEST_STATE_UNKNOWN_SCENARIO = "REQUEST.STATE.UNKNOWN_SCENARIO", Error,
        "the requested scenario ID is not declared by the set",
        category = Request;
    // READ: what a reader's own findings arrive as once the package lifts
    // them. A reader finding keeps the code its crate gave it; these are
    // the package's own.
    READ_PACKAGE_OPERATING_POINTS_DROPPED = "READ.PACKAGE.OPERATING_POINTS_DROPPED", Warning,
        "a time series could not be lifted into operating points";
    /// Retired in 0.9.0 for a code that names its three segments.
    READ_OPERATING_POINTS_DROPPED = "READ.OPERATING_POINTS_DROPPED", Warning,
        "a time series could not be lifted into operating points", retired = "0.9.0";

    // VALIDATE: the document's own internal consistency, nothing else.
    // The payload level VALIDATE.BALANCED.* and VALIDATE.MULTI.* codes are
    // declared by the network crates that own those models; this module
    // still emits them from the payload validation profile.
    VALIDATE_PACKAGE_STUDY_MODEL_KIND = "VALIDATE.PACKAGE.STUDY_MODEL_KIND", Error,
        "the study block's model kind disagrees with the payload";
    VALIDATE_PACKAGE_STUDY_IDENTITY = "VALIDATE.PACKAGE.STUDY_IDENTITY", Error,
        "a study edit names a uid the payload does not declare";
    VALIDATE_PACKAGE_OPERATING_IDENTITY = "VALIDATE.PACKAGE.OPERATING_IDENTITY", Error,
        "an operating point names a uid the payload does not declare";

    // LOWER: the multiconductor to balanced pass.
    TRANSFORM_MULTI_TO_BALANCED_AMBIGUOUS_TERMINAL_MAP =
        "TRANSFORM.MULTI_TO_BALANCED.AMBIGUOUS_TERMINAL_MAP", Error,
        "a terminal map does not determine one balanced phase assignment";
    TRANSFORM_MULTI_TO_BALANCED_BALANCED_VALUE_DOMAIN =
        "TRANSFORM.MULTI_TO_BALANCED.BALANCED_VALUE_DOMAIN", Warning,
        "a lowered value is outside the domain the balanced model states";
    TRANSFORM_MULTI_TO_BALANCED_DROPPED_LOAD_VOLTAGE_MODEL =
        "TRANSFORM.MULTI_TO_BALANCED.DROPPED_LOAD_VOLTAGE_MODEL", Warning,
        "a load voltage model has no balanced spelling and was dropped";
    TRANSFORM_MULTI_TO_BALANCED_DROPPED_OPEN_SWITCH =
        "TRANSFORM.MULTI_TO_BALANCED.DROPPED_OPEN_SWITCH", Remark,
        "an open switch was dropped from the balanced network";
    TRANSFORM_MULTI_TO_BALANCED_INVALID_BALANCED_OUTPUT =
        "TRANSFORM.MULTI_TO_BALANCED.INVALID_BALANCED_OUTPUT", Error,
        "the lowered network does not validate";
    TRANSFORM_MULTI_TO_BALANCED_INVALID_BASE_MVA =
        "TRANSFORM.MULTI_TO_BALANCED.INVALID_BASE_MVA", Error,
        "the requested base MVA is not a positive finite number";
    TRANSFORM_MULTI_TO_BALANCED_INVALID_LINECODE_MATRIX =
        "TRANSFORM.MULTI_TO_BALANCED.INVALID_LINECODE_MATRIX", Error,
        "a linecode matrix cannot be reduced to a balanced impedance";
    TRANSFORM_MULTI_TO_BALANCED_INVALID_PHASE_REFERENCE =
        "TRANSFORM.MULTI_TO_BALANCED.INVALID_PHASE_REFERENCE", Error,
        "a phase reference names a conductor the element does not have";
    TRANSFORM_MULTI_TO_BALANCED_INVALID_SHUNT_MATRIX =
        "TRANSFORM.MULTI_TO_BALANCED.INVALID_SHUNT_MATRIX", Error,
        "a shunt matrix cannot be reduced to a balanced admittance";
    TRANSFORM_MULTI_TO_BALANCED_KRON_REDUCTION_REQUIRED =
        "TRANSFORM.MULTI_TO_BALANCED.KRON_REDUCTION_REQUIRED", Remark,
        "a conductor set needs a Kron reduction the pass does not perform";
    TRANSFORM_MULTI_TO_BALANCED_LINECODE_TERMINAL_MISMATCH =
        "TRANSFORM.MULTI_TO_BALANCED.LINECODE_TERMINAL_MISMATCH", Error,
        "a linecode's dimension disagrees with the line's terminal map";
    TRANSFORM_MULTI_TO_BALANCED_MISSING_PHASE_REFERENCE =
        "TRANSFORM.MULTI_TO_BALANCED.MISSING_PHASE_REFERENCE", Error,
        "an element states no phase reference to lower against";
    TRANSFORM_MULTI_TO_BALANCED_NONFINITE_LINE_LENGTH =
        "TRANSFORM.MULTI_TO_BALANCED.NONFINITE_LINE_LENGTH", Error,
        "a line length is not finite, so its impedance is undefined";
    TRANSFORM_MULTI_TO_BALANCED_PHASE_MAP_MISMATCH =
        "TRANSFORM.MULTI_TO_BALANCED.PHASE_MAP_MISMATCH", Error,
        "the two ends of an element do not agree on their phase map";
    TRANSFORM_MULTI_TO_BALANCED_SEQUENCE_COUPLING_DROPPED =
        "TRANSFORM.MULTI_TO_BALANCED.SEQUENCE_COUPLING_DROPPED", Remark,
        "sequence coupling has no balanced spelling and was dropped";
    TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_BUS = "TRANSFORM.MULTI_TO_BALANCED.UNKNOWN_BUS", Error,
        "an element references a bus the multiconductor payload does not declare";
    TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_LINECODE =
        "TRANSFORM.MULTI_TO_BALANCED.UNKNOWN_LINECODE", Error,
        "a line references a linecode the payload does not declare";
    TRANSFORM_MULTI_TO_BALANCED_UNKNOWN_SOURCE_BUS =
        "TRANSFORM.MULTI_TO_BALANCED.UNKNOWN_SOURCE_BUS", Error,
        "a voltage source references a bus the payload does not declare";
    /// Retired in 1.0.0: an unrated identity switch now merges its buses,
    /// and the remaining closed switch shapes carry their own codes.
    TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_CLOSED_SWITCH =
        "TRANSFORM.MULTI_TO_BALANCED.UNSUPPORTED_CLOSED_SWITCH", Error,
        "a closed switch shape has no balanced spelling", retired = "0.10.0";
    TRANSFORM_MULTI_TO_BALANCED_RATED_CLOSED_SWITCH =
        "TRANSFORM.MULTI_TO_BALANCED.RATED_CLOSED_SWITCH", Error,
        "merging a rated closed switch would erase its flow limit";
    TRANSFORM_MULTI_TO_BALANCED_SWITCH_TERMINAL_MISMATCH =
        "TRANSFORM.MULTI_TO_BALANCED.SWITCH_TERMINAL_MISMATCH", Error,
        "the two ends of a closed switch do not map identical conductors";
    TRANSFORM_MULTI_TO_BALANCED_SWITCH_MERGE_CONFLICT =
        "TRANSFORM.MULTI_TO_BALANCED.SWITCH_MERGE_CONFLICT", Error,
        "the buses a closed switch would merge state conflicting data";
    TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_CONDUCTOR_SET =
        "TRANSFORM.MULTI_TO_BALANCED.UNSUPPORTED_CONDUCTOR_SET", Error,
        "a conductor set has no balanced spelling";
    TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_OBJECT =
        "TRANSFORM.MULTI_TO_BALANCED.UNSUPPORTED_OBJECT", Error,
        "an object has no balanced spelling and was dropped";
    TRANSFORM_MULTI_TO_BALANCED_UNSUPPORTED_TRANSFORMER =
        "TRANSFORM.MULTI_TO_BALANCED.UNSUPPORTED_TRANSFORMER", Error,
        "a transformer shape has no balanced spelling";
    TRANSFORM_MULTI_TO_BALANCED_RECORD_CAP =
        "TRANSFORM.MULTI_TO_BALANCED.RECORD_CAP", Error,
        "the lowered module's records would exceed a module maximum",
        category = Request;
    TRANSFORM_MULTI_TO_BALANCED_WRONG_MODEL_KIND =
        "TRANSFORM.MULTI_TO_BALANCED.WRONG_MODEL_KIND", Error,
        "the module does not carry a multiconductor payload to lower",
        category = Request;

    // Failures.
    PARSE_MODULE_MALFORMED = "PARSE.MODULE.MALFORMED", Error,
        "the document is not well formed .pio.json", category = Parse;
    /// Retired in 0.10.0 for the module vocabulary.
    PARSE_PACKAGE_MALFORMED = "PARSE.PACKAGE.MALFORMED", Error,
        "the document is not well formed .pio.json", category = Parse, retired = "0.10.0";
    PARSE_MODULE_UNSUPPORTED_VERSION = "PARSE.MODULE.UNSUPPORTED_VERSION", Error,
        "the document comes from a lineage this build does not read", category = Parse;
    /// Retired in 0.10.0 for the module vocabulary.
    PARSE_PACKAGE_UNSUPPORTED_VERSION = "PARSE.PACKAGE.UNSUPPORTED_VERSION", Error,
        "the document comes from a lineage this build does not read", category = Parse, retired = "0.10.0";
    VALIDATE_MODULE_MODEL_KIND_MISMATCH = "VALIDATE.MODULE.MODEL_KIND_MISMATCH", Error,
        "the document's model_kind disagrees with the payload it carries", category = Data;
    /// Retired in 0.10.0 for the module vocabulary.
    VALIDATE_PACKAGE_MODEL_KIND_MISMATCH = "VALIDATE.PACKAGE.MODEL_KIND_MISMATCH", Error,
        "the document's model_kind disagrees with the payload it carries", category = Data, retired = "0.10.0";
    REQUEST_MODULE_NO_SUCH_INDEX = "REQUEST.MODULE.NO_SUCH_INDEX", Error,
        "the call names an operating point or study index the document does not carry",
        category = Request;
    /// Retired in 0.10.0 for the module vocabulary.
    REQUEST_PACKAGE_NO_SUCH_INDEX = "REQUEST.PACKAGE.NO_SUCH_INDEX", Error,
        "the call names an operating point or study index the document does not carry",
        category = Request, retired = "0.10.0";
    REQUEST_MODULE_WRONG_MODEL_KIND = "REQUEST.MODULE.WRONG_MODEL_KIND", Error,
        "the call asks for a model family the document does not carry", category = Request;
    /// Retired in 0.10.0 for the module vocabulary.
    REQUEST_PACKAGE_WRONG_MODEL_KIND = "REQUEST.PACKAGE.WRONG_MODEL_KIND", Error,
        "the call asks for a model family the document does not carry", category = Request, retired = "0.10.0";
    BUILD_MODULE_PAYLOAD_FAILED = "BUILD.MODULE.PAYLOAD_FAILED", Error,
        "the payload could not be built, applied, or serialized", category = Data;
    /// Retired in 0.10.0 for the module vocabulary.
    BUILD_PACKAGE_PAYLOAD_FAILED = "BUILD.PACKAGE.PAYLOAD_FAILED", Error,
        "the payload could not be built, applied, or serialized", category = Data, retired = "0.10.0";
    EMIT_MODULE_SERIALIZE_FAILED = "EMIT.MODULE.SERIALIZE_FAILED", Error,
        "serializing the stored document to JSON failed", category = Output;
    /// Retired in 0.10.0 for the module vocabulary.
    EMIT_PACKAGE_SERIALIZE_FAILED = "EMIT.PACKAGE.SERIALIZE_FAILED", Error,
        "serializing the stored document to JSON failed", category = Output, retired = "0.10.0";
}

/// Every code this crate declares.
#[must_use]
pub fn registry() -> Vec<&'static powerio_core::DiagnosticInfo> {
    ALL.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_sound() {
        let problems = powerio_core::check_registry(registry());
        assert!(problems.is_empty(), "{problems:#?}");
    }
}

//! The facade's diagnostic code registry. An emission site names an entry
//! rather than a loose string.

powerio_core::diagnostic_codes! {
    REQUEST_PARSE_POWERIO_IR = "REQUEST.PARSE.POWERIO_IR", Error,
        "PowerIO IR is decoded with deserialize, not parse", category = Request;
    READ_MODULE_UNSUPPORTED = "READ.MODULE.UNSUPPORTED", Error,
        "the stored module names a schema or version this build does not read", category = Request;
    READ_MODULE_INVALID = "READ.MODULE.INVALID", Error,
        "the stored module document is structurally invalid", category = Data;
    VALIDATE_COLLECTION_EMPTY = "VALIDATE.COLLECTION.EMPTY", Error,
        "a dynamically constructed collection needs at least one value to infer its type", category = Data;
    VALIDATE_COLLECTION_ELEMENT_TYPE = "VALIDATE.COLLECTION.ELEMENT_TYPE", Error,
        "all values in a collection must have the same supported structural type", category = Data;
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
    REQUEST_MODULE_WRONG_MODEL_KIND = "REQUEST.MODULE.WRONG_MODEL_KIND", Error,
        "the call asks for a model family the document does not carry", category = Request;
    EMIT_MODULE_SERIALIZE_FAILED = "EMIT.MODULE.SERIALIZE_FAILED", Error,
        "serializing the stored document to JSON failed", category = Output;
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

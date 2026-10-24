//! The facade's diagnostic code registry. An emission site names an entry
//! rather than a loose string.

use powerio_core::DiagnosticInfo;

mod facade {
    powerio_core::diagnostic_codes! {
        REQUEST_SINCAL_OPTIONS_PROFILE = "REQUEST.PARSE.SINCAL_OPTIONS_PROFILE", Error,
            "multiconductor SINCAL options require the matching explicit profile", category = Request;
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
        REQUEST_EMIT_INVALID_OPTIONS = "REQUEST.EMIT.INVALID_OPTIONS", Error,
            "emission options do not apply to the requested format or value", category = Request;
        EMIT_SINCAL_PACKAGING_FAILED = "EMIT.MODULE.SINCAL_PACKAGING_FAILED", Error,
            "experimental native SINCAL container construction failed", category = Output;

        // Failures.
        REQUEST_MODULE_WRONG_MODEL_KIND = "REQUEST.MODULE.WRONG_MODEL_KIND", Error,
            "the call asks for a model family the document does not carry", category = Request;
        TRANSFORM_LINDIST3FLOW_OPTIONS_SERIALIZE_FAILED =
            "TRANSFORM.LINDIST3FLOW.OPTIONS_SERIALIZE_FAILED", Error,
            "serializing LinDist3Flow options for transform history failed", category = Output;
        EMIT_MODULE_SERIALIZE_FAILED = "EMIT.MODULE.SERIALIZE_FAILED", Error,
            "serializing the stored document to JSON failed", category = Output;
        EMIT_MODULE_RECORD_CAP = "EMIT.MODULE.RECORD_CAP", Error,
            "a stored record would exceed a bound the PowerIO IR reader enforces",
            category = Request;
    }
}

#[doc(inline)]
pub use facade::*;
#[doc(inline)]
pub use powerio_tnd::diagnostics::*;

/// Every code this crate declares or re-exports, in registry order. As in
/// 0.11.5 it includes the transformation codes, which now live in tnd.
pub const ALL: &[&DiagnosticInfo] = &concat::<
    { facade::ALL.len() + powerio_tnd::diagnostics::ALL.len() },
>(facade::ALL, powerio_tnd::diagnostics::ALL);

const fn concat<const N: usize>(
    first: &[&'static DiagnosticInfo],
    second: &[&'static DiagnosticInfo],
) -> [&'static DiagnosticInfo; N] {
    assert!(first.len() + second.len() == N);
    let mut all = [first[0]; N];
    let mut i = 1;
    while i < N {
        all[i] = if i < first.len() {
            first[i]
        } else {
            second[i - first.len()]
        };
        i += 1;
    }
    all
}

/// Every code this crate declares.
#[must_use]
pub fn registry() -> Vec<&'static DiagnosticInfo> {
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

    #[test]
    fn all_keeps_every_code_the_facade_listed_in_0_11_5() {
        assert_eq!(
            ALL.len(),
            facade::ALL.len() + powerio_tnd::diagnostics::ALL.len()
        );
        for code in powerio_tnd::diagnostics::ALL {
            assert!(
                ALL.iter().any(|listed| listed.code == code.code),
                "{}",
                code.code
            );
        }
        assert!(
            ALL.iter()
                .any(|code| code.code == "TRANSFORM.MULTI_TO_BALANCED.AMBIGUOUS_TERMINAL_MAP")
        );
    }
}

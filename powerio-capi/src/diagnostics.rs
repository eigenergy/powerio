//! The codes the language boundary itself emits, and the workspace gates.
//!
//! `BIND` is the one namespace with no Rust error type behind it: these are the
//! failures a C entry point detects from an argument's representation alone —
//! a null pointer, bytes that are not UTF-8, a caught panic. A failure that
//! needs powerio's own vocabulary to detect (an unknown format name, an index
//! the handle does not have) belongs to `REQUEST` and comes from the crate that
//! owns the vocabulary.

pub use powerio_core::{
    Diagnostic, DiagnosticInfo, DiagnosticSeverity, check_registry, check_scope_ownership,
};

pub mod codes {
    powerio_core::diagnostic_codes! {
        BIND_CAPI_NULL_HANDLE = "BIND.CAPI.NULL_HANDLE", Error,
            "a handle argument was NULL", category = Data;
        BIND_CAPI_NULL_ARGUMENT = "BIND.CAPI.NULL_ARGUMENT", Error,
            "a required non-handle argument was NULL", category = Data;
        BIND_CAPI_INVALID_UTF8 = "BIND.CAPI.INVALID_UTF8", Error,
            "a string argument is not valid UTF-8", category = Data;
        BIND_CAPI_INDEX_OUT_OF_RANGE = "BIND.CAPI.INDEX_OUT_OF_RANGE", Error,
            "an index argument cannot be converted or is out of range", category = Data;
        BIND_CAPI_LENGTH_MISMATCH = "BIND.CAPI.LENGTH_MISMATCH", Error,
            "a caller buffer length does not match the documented dimension", category = Data;
        BIND_CAPI_INTERIOR_NUL = "BIND.CAPI.INTERIOR_NUL", Error,
            "a string powerio produced holds an interior NUL and cannot cross as a C string",
            category = Data;
        BIND_CAPI_PANIC = "BIND.CAPI.PANIC", Error,
            "a panic was caught at the boundary and did not cross it", category = Data;
        BIND_CAPI_INVALID_OPTIONS = "BIND.CAPI.INVALID_OPTIONS", Error,
            "an options struct declared a size or a field value this build cannot honor",
            category = Data;
        PARSE_CAPI_JSON_MALFORMED = "PARSE.CAPI.JSON_MALFORMED", Error,
            "a JSON document handed to an entry point could not be decoded", category = Parse;
        READ_CAPI_IO_FAILED = "READ.CAPI.IO_FAILED", Error,
            "an entry point could not read the file it was given", category = Io;
        EMIT_CAPI_SERIALIZE_FAILED = "EMIT.CAPI.SERIALIZE_FAILED", Error,
            "a document or table an entry point returns could not be built or serialized",
            category = Output;
        BIND_CAPI_UNCODED_FAILURE = "BIND.CAPI.UNCODED_FAILURE", Error,
            "a library failure reached the boundary carrying no finding of its own",
            category = Data;
        REQUEST_CAPI_ARROW_TABLE_UNKNOWN = "REQUEST.CAPI.ARROW_TABLE_UNKNOWN", Error,
            "the caller asked for an Arrow table id this surface does not have",
            category = Request;
        REQUEST_CAPI_FEATURE_DISABLED = "REQUEST.CAPI.FEATURE_DISABLED", Error,
            "the build lacks the cargo feature the request needs", category = Request;
        REQUEST_CAPI_UNKNOWN_FORMULA = "REQUEST.CAPI.UNKNOWN_FORMULA", Error,
            "the caller named a branch susceptance formula this surface does not have",
            category = Request;
        REQUEST_CAPI_TYPE_MISMATCH = "REQUEST.CAPI.TYPE_MISMATCH", Error,
            "the value does not have the structural type required by the operation",
            category = Request;
        REQUEST_CAPI_QUANTITY_UNKNOWN = "REQUEST.CAPI.QUANTITY_UNKNOWN", Error,
            "the requested operating point quantity is not defined",
            category = Request;
        REQUEST_CAPI_ALLOCATION_UNKNOWN = "REQUEST.CAPI.ALLOCATION_UNKNOWN", Error,
            "the requested load allocation rule is not defined",
            category = Request;
    }
}

/// Every code this crate declares.
#[must_use]
pub fn registry() -> Vec<&'static DiagnosticInfo> {
    codes::ALL.to_vec()
}

// The workspace gate. This crate is the only one that depends on all five
// library crates at once, and the release features CI job builds it with every
// feature on, so it is the one place a code shared by two crates shows up.
#[cfg(all(test, feature = "dist", feature = "prob", feature = "matrix"))]
mod workspace {
    use super::*;

    fn registries() -> Vec<(&'static str, Vec<&'static DiagnosticInfo>)> {
        vec![
            ("powerio-tx", powerio_tx::diagnostics::registry()),
            ("powerio (stored + transform)", powerio::codes::registry()),
            #[cfg(feature = "gridfm")]
            (
                "powerio (gridfm reader)",
                powerio::gridfm_codes::ALL.to_vec(),
            ),
            ("powerio-dist", powerio_dist::diagnostics::registry()),
            ("powerio-matrix", powerio_matrix::diagnostics::registry()),
            ("powerio-prob", powerio_prob::diagnostics::registry()),
            ("powerio-capi", registry()),
        ]
    }

    #[test]
    fn every_code_in_the_workspace_is_registered_once_and_well_formed() {
        let all: Vec<&DiagnosticInfo> = registries()
            .into_iter()
            .flat_map(|(_, entries)| entries)
            .collect();
        let problems = check_registry(all.iter().copied());
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn no_two_crates_claim_one_scope() {
        let owned = registries();
        let borrowed: Vec<(&str, &[&DiagnosticInfo])> = owned
            .iter()
            .map(|(name, entries)| (*name, entries.as_slice()))
            .collect();
        let problems = check_scope_ownership(&borrowed);
        assert!(problems.is_empty(), "{problems:#?}");
    }

    /// Any stable code string the ABI implementation spells inline resolves to
    /// a registered entry in some workspace registry, so a bare unregistered
    /// literal cannot reach a `PioError`. The module's own test block may
    /// fabricate codes and is excluded. No inline strings is also valid: ABI
    /// code should normally refer to registry entries directly.
    #[test]
    fn every_code_string_the_abi_emits_is_registered() {
        let source = include_str!("lib.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("split yields the leading source");
        let registered: std::collections::BTreeSet<&str> = registries()
            .into_iter()
            .flat_map(|(_, entries)| entries)
            .map(|entry| entry.code)
            .collect();
        for piece in source.split('"').skip(1).step_by(2) {
            let dotted = piece.split('.').count() >= 3
                && piece.split('.').all(|segment| {
                    !segment.is_empty()
                        && segment.bytes().all(|byte| {
                            byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'
                        })
                });
            if dotted {
                assert!(registered.contains(piece), "`{piece}` is not registered");
            }
        }
    }
}

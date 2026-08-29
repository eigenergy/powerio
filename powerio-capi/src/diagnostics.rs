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
            category = Data;
        REQUEST_CAPI_FEATURE_DISABLED = "REQUEST.CAPI.FEATURE_DISABLED", Error,
            "the build lacks the cargo feature the request needs", category = Data;
    }
}

/// One errbuf message: `CODE: message`, the code from a registered entry.
///
/// Every failure this boundary reports is built here, so the token a consumer
/// branches on is always present and appears exactly once.
pub(crate) fn coded(info: &'static DiagnosticInfo, message: impl std::fmt::Display) -> String {
    format!("{}: {message}", info.code)
}

/// A library error that knows its own registry entry. The five error types are
/// the only ones with a code of their own; a failure raised at the boundary
/// itself takes a `BIND.CAPI.*` entry through [`coded`].
pub(crate) trait CodedError: std::fmt::Display {
    /// The stable code string. Errors from different crates carry entries from
    /// different registries, so the code is what they agree on.
    fn code_str(&self) -> &'static str;
}

/// A library error as its errbuf line.
pub(crate) fn err_line<E: CodedError>(e: E) -> String {
    format!("{}: {e}", e.code_str())
}

impl CodedError for powerio::Error {
    fn code_str(&self) -> &'static str {
        // Unlike powerio_tx::Error, powerio_core::Error can carry no
        // registered finding (a cause wrapped with `with_cause` alone); the
        // boundary's own uncoded entry covers that case.
        self.info()
            .map_or(codes::BIND_CAPI_UNCODED_FAILURE.code, |info| info.code)
    }
}

/// `powerio::Error` above is `powerio_core::Error`, the type `powerio::parse`
/// and the source layer return; the balanced network readers and writers
/// still raise their own `powerio_tx::Error`, reached through the facade at
/// its module path (`powerio::error::Error`, since this crate has no direct
/// `powerio-tx` dependency) as e.g. `powerio::GeoLayer::parse_bytes`, so it
/// needs its own impl.
impl CodedError for powerio::error::Error {
    fn code_str(&self) -> &'static str {
        self.code().code
    }
}

#[cfg(feature = "matrix")]
impl CodedError for powerio_matrix::Error {
    fn code_str(&self) -> &'static str {
        self.code().code
    }
}

#[cfg(feature = "dist")]
impl CodedError for powerio_dist::Error {
    fn code_str(&self) -> &'static str {
        self.code().code
    }
}

#[cfg(feature = "prob")]
impl CodedError for powerio_prob::Error {
    fn code_str(&self) -> &'static str {
        self.code().code
    }
}

#[cfg(feature = "prob")]
impl CodedError for powerio_prob::ScopfError {
    fn code_str(&self) -> &'static str {
        self.code().code
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
            ("powerio", powerio::diagnostics::registry()),
            ("powerio (stored + transform)", powerio::codes::registry()),
            #[cfg(feature = "gridfm")]
            (
                "powerio (gridfm reader)",
                powerio::gridfm::codes::ALL.to_vec(),
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

    // The three catch-alls this release retires exist only because the strings
    // they wrapped had no identity of their own. They stay registered so a
    // document carrying one still reads, and stay unemitted.
    #[test]
    fn the_three_catch_alls_are_registered_and_retired() {
        use powerio_core::CodeStatus;
        let all: Vec<&DiagnosticInfo> = registries()
            .into_iter()
            .flat_map(|(_, entries)| entries)
            .collect();
        let mut codes = vec!["READ.TRANSMISSION.PARSE_WARNING", "READ.DIST.PARSE_WARNING"];
        // registries() only contributes the gridfm registry under this
        // feature, so the expectation follows the same gate.
        if cfg!(feature = "gridfm") {
            codes.push("READ.GRIDFM.FIDELITY_WARNING");
        }
        for code in codes {
            let entry = all
                .iter()
                .find(|entry| entry.code == code)
                .unwrap_or_else(|| panic!("{code} is not registered"));
            assert!(
                matches!(entry.status, CodeStatus::Retired { .. }),
                "{code} is still active"
            );
        }
    }
}

//! The codes the language boundary itself emits, and the workspace gates.
//!
//! `BIND` is the one namespace with no Rust error type behind it: these are the
//! failures a C entry point detects from an argument's representation alone —
//! a null pointer, bytes that are not UTF-8, a caught panic. A failure that
//! needs powerio's own vocabulary to detect (an unknown format name, an index
//! the handle does not have) belongs to `REQUEST` and comes from the crate that
//! owns the vocabulary.

pub use powerio_diag::{
    DiagnosticInfo, DiagnosticSeverity, StructuredDiagnostic, check_registry, check_scope_ownership,
};

pub mod codes {
    powerio_diag::diagnostic_codes! {
        BIND_CAPI_NULL_HANDLE = "BIND.CAPI.NULL_HANDLE", Fatal,
            "a handle argument was NULL", category = Data;
        BIND_CAPI_NULL_ARGUMENT = "BIND.CAPI.NULL_ARGUMENT", Fatal,
            "a required non-handle argument was NULL", category = Data;
        BIND_CAPI_INVALID_UTF8 = "BIND.CAPI.INVALID_UTF8", Fatal,
            "a string argument is not valid UTF-8", category = Data;
        BIND_CAPI_INDEX_OUT_OF_RANGE = "BIND.CAPI.INDEX_OUT_OF_RANGE", Fatal,
            "an index argument cannot be converted or is out of range", category = Data;
        BIND_CAPI_INTERIOR_NUL = "BIND.CAPI.INTERIOR_NUL", Fatal,
            "a string powerio produced holds an interior NUL and cannot cross as a C string",
            category = Data;
        BIND_CAPI_PANIC = "BIND.CAPI.PANIC", Fatal,
            "a panic was caught at the boundary and did not cross it", category = Data;
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
#[cfg(all(
    test,
    feature = "dist",
    feature = "pkg",
    feature = "prob",
    feature = "matrix"
))]
mod workspace {
    use super::*;

    fn registries() -> Vec<(&'static str, Vec<&'static DiagnosticInfo>)> {
        vec![
            ("powerio", powerio::diagnostics::registry()),
            ("powerio-dist", powerio_dist::diagnostics::registry()),
            ("powerio-pkg", powerio_pkg::diagnostics::registry()),
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
        use powerio_diag::CodeStatus;
        let all: Vec<&DiagnosticInfo> = registries()
            .into_iter()
            .flat_map(|(_, entries)| entries)
            .collect();
        for code in [
            "READ.TRANSMISSION.PARSE_WARNING",
            "READ.GRIDFM.FIDELITY_WARNING",
            "READ.DIST.PARSE_WARNING",
        ] {
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

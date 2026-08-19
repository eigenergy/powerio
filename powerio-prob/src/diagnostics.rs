//! The codes this crate emits.
//!
//! The record, the code grammar, and the severity ladder live in
//! `powerio-diag`. This crate raises failures only: a wrapped hub or matrix
//! failure keeps its own code, so the entries here are the three an instance
//! build raises itself.

pub use powerio_diag::{DiagnosticInfo, DiagnosticSeverity, check_registry};

pub mod codes {
    powerio_diag::diagnostic_codes! {
        BUILD_INSTANCE_NO_GENERATORS = "BUILD.INSTANCE.NO_GENERATORS", Fatal,
            "the case has no generator for the problem to dispatch", category = Data;
        BUILD_INSTANCE_UNSUPPORTED_COST_MODEL = "BUILD.INSTANCE.UNSUPPORTED_COST_MODEL", Fatal,
            "a generator cost model the instance builder cannot state", category = Data;
        BUILD_INSTANCE_CONCAVE_COST = "BUILD.INSTANCE.CONCAVE_COST", Fatal,
            "a generator cost row states a concave curve", category = Data;
        READ_INSTANCE_IO_FAILED = "READ.INSTANCE.IO_FAILED", Fatal,
            "an instance file could not be read", category = Io;
        PARSE_SCOPF_MALFORMED = "PARSE.SCOPF.MALFORMED", Fatal,
            "the SCOPF document is not well formed JSON", category = Parse;
        READ_SCOPF_INVALID_DOCUMENT = "READ.SCOPF.INVALID_DOCUMENT", Fatal,
            "the SCOPF document decodes but does not describe an instance", category = Parse;
        REQUEST_SCOPF_FORMAT_UNKNOWN = "REQUEST.SCOPF.FORMAT_UNKNOWN", Fatal,
            "the named SCOPF source format is not one this build reads",
            category = UnknownFormat;
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

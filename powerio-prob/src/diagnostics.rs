//! The codes this crate emits.
//!
//! The record, the code grammar, and the severity ladder live in
//! `powerio-core`. This crate raises failures only: a wrapped hub or matrix
//! failure keeps its own code, so the entries here are the three an instance
//! build raises itself.

pub use powerio_core::{DiagnosticInfo, DiagnosticSeverity, check_registry};

pub mod codes {
    powerio_core::diagnostic_codes! {
        /// Retired in 0.9.0 for the per format spelling
        /// `READ.PACKAGE.OPERATING_POINTS_DROPPED`. Declared here because the
        /// GO Challenge 3 lift belongs to the calculation crate at 1.0.
        READ_GOC3_OPERATING_POINTS_DROPPED = "READ.GOC3.OPERATING_POINTS_DROPPED", Warning,
            "a GO Challenge 3 time series could not be lifted", retired = "0.9.0";
        BUILD_STATE_SHAPE_MISMATCH = "BUILD.STATE.SHAPE_MISMATCH", Error,
            "an operating point column disagrees with the resolved identity layout",
            category = Data;
        BUILD_STATE_IDENTITY_UNKNOWN = "BUILD.STATE.IDENTITY_UNKNOWN", Error,
            "an operating point names an element identity the network does not declare",
            category = Data;
        BUILD_INSTANCE_NO_GENERATORS = "BUILD.INSTANCE.NO_GENERATORS", Error,
            "the case has no generator for the problem to dispatch", category = Data;
        BUILD_INSTANCE_UNSUPPORTED_COST_MODEL = "BUILD.INSTANCE.UNSUPPORTED_COST_MODEL", Error,
            "a generator cost model the instance builder cannot state", category = Data;
        BUILD_INSTANCE_CONCAVE_COST = "BUILD.INSTANCE.CONCAVE_COST", Error,
            "a generator cost row states a concave curve", category = Data;
        READ_INSTANCE_IO_FAILED = "READ.INSTANCE.IO_FAILED", Error,
            "an instance file could not be read", category = Io;
        PARSE_SCOPF_MALFORMED = "PARSE.SCOPF.MALFORMED", Error,
            "the SCOPF document is not well formed JSON", category = Parse;
        READ_SCOPF_INVALID_DOCUMENT = "READ.SCOPF.INVALID_DOCUMENT", Error,
            "the SCOPF document decodes but does not describe an instance", category = Parse;
        REQUEST_SCOPF_FORMAT_UNKNOWN = "REQUEST.SCOPF.FORMAT_UNKNOWN", Error,
            "the named SCOPF source format is not one this build reads",
            category = Request;
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

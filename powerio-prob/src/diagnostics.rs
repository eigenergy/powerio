//! The codes this crate emits.
//!
//! The record, the code grammar, and the severity ladder live in
//! `powerio-diag`. This crate raises failures only: a wrapped hub or matrix
//! failure keeps its own code, so the entries here are the two an instance
//! build raises itself.

pub use powerio_diag::{DiagnosticInfo, DiagnosticSeverity, check_registry};

pub mod codes {
    powerio_diag::diagnostic_codes! {
        BUILD_INSTANCE_NO_GENERATORS = "BUILD.INSTANCE.NO_GENERATORS", Fatal,
            "the case has no generator for the problem to dispatch", category = Data;
        BUILD_INSTANCE_UNSUPPORTED_COST_MODEL = "BUILD.INSTANCE.UNSUPPORTED_COST_MODEL", Fatal,
            "a generator cost model the instance builder cannot state", category = Data;
        READ_INSTANCE_IO_FAILED = "READ.INSTANCE.IO_FAILED", Fatal,
            "an instance file could not be read", category = Io;
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

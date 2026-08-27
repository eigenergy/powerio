//! The codes this crate emits.
//!
//! The record, the code grammar, and the severity ladder live in
//! `powerio-core`; the entries here are the matrix, sensitivity, and dataset
//! side of the workspace registry. A hub failure that arrives through
//! [`crate::Error::Core`] keeps the hub's own code.

pub use powerio_core::{
    Diagnostic, DiagnosticInfo, DiagnosticSeverity, check_registry, render_diagnostic,
    render_diagnostics,
};

pub mod codes {
    powerio_core::diagnostic_codes! {
        // BUILD: assembling a derived object from a network that already parsed.
        BUILD_MATRIX_SHAPE_MISMATCH = "BUILD.MATRIX.SHAPE_MISMATCH", Error,
            "an operand's length does not match the matrix it is used with", category = Data;
        BUILD_MULTI_UNSUPPORTED_STAMP = "BUILD.MULTI.UNSUPPORTED_STAMP", Warning,
            "an element has no exact multiconductor admittance or ideal stamp and was omitted loudly";
        BUILD_SENSITIVITY_SINGULAR = "BUILD.SENSITIVITY.SINGULAR", Error,
            "the reference grounded Laplacian is singular", category = Data;
        BUILD_SENSITIVITY_INVALID_OPTION = "BUILD.SENSITIVITY.INVALID_OPTION", Error,
            "a DC sensitivity option is outside the range it is defined on", category = Data;
        /// Retired in 1.0.0: the conjugate gradient solver this reported on
        /// was replaced by a sparse direct factorization, which either
        /// succeeds or reports singularity.
        BUILD_SENSITIVITY_NO_CONVERGENCE = "BUILD.SENSITIVITY.NO_CONVERGENCE", Error,
            "the iterative DC sensitivity solve ran out of iterations", retired = "1.0.0";
        BUILD_GRIDFM_EMPTY_BATCH = "BUILD.GRIDFM.EMPTY_BATCH", Error,
            "a gridfm scenario batch holds no snapshot", category = Data;
        BUILD_GRIDFM_SCENARIO_ID_OVERFLOW = "BUILD.GRIDFM.SCENARIO_ID_OVERFLOW", Error,
            "numbering a gridfm snapshot overflows the scenario id", category = Data;
        BUILD_GRIDFM_NORMALIZED_SNAPSHOT = "BUILD.GRIDFM.NORMALIZED_SNAPSHOT", Error,
            "a gridfm snapshot is normalized and the export expects raw units", category = Data;
        BUILD_GRIDFM_NOT_A_NUMBER = "BUILD.GRIDFM.NOT_A_NUMBER", Error,
            "a gridfm snapshot field is not finite", category = Data;
        BUILD_GRIDFM_SCENARIO_SHAPE_MISMATCH = "BUILD.GRIDFM.SCENARIO_SHAPE_MISMATCH", Error,
            "a gridfm snapshot does not share the batch's base element set", category = Data;

        // READ and EMIT: this crate's own file side.
        READ_MATRIX_IO_FAILED = "READ.MATRIX.IO_FAILED", Error,
            "a matrix or dataset file could not be read", category = Io;
        EMIT_MTX_FAILED = "EMIT.MTX.FAILED", Error,
            "a matrix-market write failed", category = Output;
        EMIT_PARQUET_FAILED = "EMIT.PARQUET.FAILED", Error,
            "a gridfm Parquet write failed", category = Output;

        // READ.GRIDFM: the dataset reader's fidelity notes. The scope lives
        // here rather than in the package crate, which only forwarded them.
        READ_GRIDFM_FIELD_DROPPED = "READ.GRIDFM.FIELD_DROPPED", Warning,
            "a field the gridfm schema does not carry is absent from the network";
        READ_GRIDFM_VALUE_DEFAULTED = "READ.GRIDFM.VALUE_DEFAULTED", Warning,
            "a manifest value the reader needs was absent and was defaulted";
        READ_GRIDFM_VALUE_INFERRED = "READ.GRIDFM.VALUE_INFERRED", Warning,
            "an identity the gridfm schema does not store was synthesized";
        READ_GRIDFM_VALUE_COLLAPSED = "READ.GRIDFM.VALUE_COLLAPSED", Warning,
            "nodal totals were folded into synthetic per bus elements";
        READ_GRIDFM_ELEMENT_RELABELED = "READ.GRIDFM.ELEMENT_RELABELED", Warning,
            "a unity ratio transformer is indistinguishable from a line and reads as one";
        /// Retired in 0.9.0: every gridfm read finding now carries its own
        /// code, so the package no longer wraps them under one catch-all.
        READ_GRIDFM_FIDELITY_WARNING = "READ.GRIDFM.FIDELITY_WARNING", Warning,
            "a gridfm read finding with no identity of its own", retired = "0.9.0";
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

//! The codes this crate emits.
//!
//! The record, the code grammar, and the severity ladder live in
//! `powerio-diag`; the entries here are the matrix, sensitivity, and dataset
//! side of the workspace registry. A hub failure that arrives through
//! [`crate::Error::Core`] keeps the hub's own code.

pub use powerio_diag::{
    DiagnosticInfo, DiagnosticSeverity, Diagnostics, StructuredDiagnostic, check_registry,
    render_line, render_lines,
};

pub mod codes {
    powerio_diag::diagnostic_codes! {
        // BUILD: assembling a derived object from a network that already parsed.
        BUILD_MATRIX_SHAPE_MISMATCH = "BUILD.MATRIX.SHAPE_MISMATCH", Fatal,
            "an operand's length does not match the matrix it is used with", category = Data;
        BUILD_SENSITIVITY_SINGULAR = "BUILD.SENSITIVITY.SINGULAR", Fatal,
            "the reference grounded Laplacian is singular", category = Data;
        BUILD_SENSITIVITY_INVALID_OPTION = "BUILD.SENSITIVITY.INVALID_OPTION", Fatal,
            "a DC sensitivity option is outside the range it is defined on", category = Data;
        BUILD_SENSITIVITY_FACTORIZATION_FAILED = "BUILD.SENSITIVITY.FACTORIZATION_FAILED", Fatal,
            "the sparse DC sensitivity factorization could not be allocated or indexed", category = Data;
        /// Retired in 0.9.0: the sparse direct solver has no convergence loop.
        BUILD_SENSITIVITY_NO_CONVERGENCE = "BUILD.SENSITIVITY.NO_CONVERGENCE", Fatal,
            "the iterative DC sensitivity solve ran out of iterations", category = Data,
            retired = "0.9.0";
        BUILD_GRIDFM_EMPTY_BATCH = "BUILD.GRIDFM.EMPTY_BATCH", Fatal,
            "a gridfm scenario batch holds no snapshot", category = Data;
        BUILD_GRIDFM_SCENARIO_ID_OVERFLOW = "BUILD.GRIDFM.SCENARIO_ID_OVERFLOW", Fatal,
            "numbering a gridfm snapshot overflows the scenario id", category = Data;
        BUILD_GRIDFM_NORMALIZED_SNAPSHOT = "BUILD.GRIDFM.NORMALIZED_SNAPSHOT", Fatal,
            "a gridfm snapshot is normalized and the export expects raw units", category = Data;
        BUILD_GRIDFM_NOT_A_NUMBER = "BUILD.GRIDFM.NOT_A_NUMBER", Fatal,
            "a gridfm snapshot field is not finite", category = Data;
        BUILD_GRIDFM_SCENARIO_SHAPE_MISMATCH = "BUILD.GRIDFM.SCENARIO_SHAPE_MISMATCH", Fatal,
            "a gridfm snapshot does not share the batch's base element set", category = Data;

        // READ and EMIT: this crate's own file side.
        READ_MATRIX_IO_FAILED = "READ.MATRIX.IO_FAILED", Fatal,
            "a matrix or dataset file could not be read", category = Io;
        EMIT_MTX_FAILED = "EMIT.MTX.FAILED", Fatal,
            "a matrix-market write failed", category = Output;
        EMIT_PARQUET_FAILED = "EMIT.PARQUET.FAILED", Fatal,
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

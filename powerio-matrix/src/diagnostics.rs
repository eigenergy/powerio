//! The codes this crate emits.
//!
//! The record, the code grammar, and the severity ladder live in
//! `powerio-core`; the entries here are the matrix, sensitivity, and dataset
//! side of the workspace registry. A hub failure that arrives through
//! [`crate::Error::Transmission`] keeps the hub's own code.

pub use powerio_core::{
    Diagnostic, DiagnosticInfo, DiagnosticSeverity, check_registry, render_diagnostic,
    render_diagnostics,
};

pub mod codes {
    powerio_core::diagnostic_codes! {
        // BUILD: assembling a derived object from a network that already parsed.
        BUILD_MATRIX_SHAPE_MISMATCH = "BUILD.MATRIX.SHAPE_MISMATCH", Error,
            "an operand's length does not match the matrix it is used with", category = Data;
        BUILD_OPF_OBJECTIVE_UNSUPPORTED = "BUILD.OPF.OBJECTIVE_UNSUPPORTED", Error,
            "the OPF preparation cannot compile the instance objective", category = Data;
        BUILD_OPF_CONSTRAINT_IDENTITY_UNKNOWN = "BUILD.OPF.CONSTRAINT_IDENTITY_UNKNOWN", Error,
            "an active constraint selection names no element in its family", category = Data;
        BUILD_OPF_ELEMENT_IDENTITY_DUPLICATE = "BUILD.OPF.ELEMENT_IDENTITY_DUPLICATE", Error,
            "an OPF element family does not have unique stable identities", category = Data;
        BUILD_OPF_NODAL_COST_UNSUPPORTED = "BUILD.OPF.NODAL_COST_UNSUPPORTED", Error,
            "a nodal quadratic projection cannot carry the prepared generator cost", category = Request;
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

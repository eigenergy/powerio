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
        BUILD_INSTANCE_NO_REFERENCE_BUS = "BUILD.INSTANCE.NO_REFERENCE_BUS", Error,
            "the network states no reference (slack) bus", category = Data;
        BUILD_INSTANCE_VOLTAGE_CONTROL_CONFLICT = "BUILD.INSTANCE.VOLTAGE_CONTROL_CONFLICT", Error,
            "in service generators at one bus state conflicting voltage setpoints",
            category = Data;
        BUILD_INSTANCE_SHAPE_MISMATCH = "BUILD.INSTANCE.SHAPE_MISMATCH", Error,
            "a calculation input disagrees with the network's element tables",
            category = Data;
        BUILD_OPERATOR_ZERO_IMPEDANCE = "BUILD.OPERATOR.ZERO_IMPEDANCE", Error,
            "a zero impedance branch has no finite DC operator row", category = Data;
        BUILD_OPERATOR_NOT_A_NUMBER = "BUILD.OPERATOR.NOT_A_NUMBER", Error,
            "a branch value produced a non-finite operator entry", category = Data;
        BUILD_SOLUTION_SHAPE_MISMATCH = "BUILD.SOLUTION.SHAPE_MISMATCH", Error,
            "a solution column disagrees with the instance's element tables",
            category = Data;
        TRANSFORM_INSTANCE_DATA_DISCARDED = "TRANSFORM.INSTANCE.DATA_DISCARDED", Warning,
            "the derived calculation does not carry part of the source instance";
        TRANSFORM_INSTANCE_ASSUMPTION = "TRANSFORM.INSTANCE.ASSUMPTION", Warning,
            "the derived calculation rests on a stated modeling assumption";
        TRANSFORM_STATE_UNREPRESENTED = "TRANSFORM.STATE.UNREPRESENTED", Error,
            "an operating point states a quantity the static network cannot carry",
            category = Data;
        CANONICALIZE_MERGE_ZERO_IMPEDANCE = "CANONICALIZE.MERGE.ZERO_IMPEDANCE", Warning,
            "a zero impedance branch was merged and its flow is no longer recoverable";
        CANONICALIZE_MERGE_ATTRIBUTE_CONFLICT = "CANONICALIZE.MERGE.ATTRIBUTE_CONFLICT", Warning,
            "merged buses stated different attributes; the surviving bus's values were kept";
        BUILD_INSTANCE_UNSUPPORTED_COST_MODEL = "BUILD.INSTANCE.UNSUPPORTED_COST_MODEL", Error,
            "a generator cost model the instance builder cannot state", category = Data;
        BUILD_INSTANCE_PIECEWISE_COST_INVALID = "BUILD.INSTANCE.PIECEWISE_COST_INVALID", Error,
            "a piecewise linear generator cost row is malformed", category = Data;
        BUILD_INSTANCE_PIECEWISE_COST_NONCONVEX = "BUILD.INSTANCE.PIECEWISE_COST_NONCONVEX", Error,
            "a piecewise linear generator cost row is nonconvex", category = Data;
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

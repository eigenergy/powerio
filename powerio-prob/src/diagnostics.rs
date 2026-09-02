//! The codes this crate emits.
//!
//! The record, the code grammar, and the severity ladder live in
//! `powerio-core`. This crate raises failures only: a wrapped hub or matrix
//! failure keeps its own code, so the entries here are the three an instance
//! build raises itself.

pub use powerio_core::{DiagnosticInfo, DiagnosticSeverity, check_registry};

pub mod codes {
    powerio_core::diagnostic_codes! {
        BUILD_OPERATING_POINT_SHAPE_MISMATCH = "BUILD.OPERATING_POINT.SHAPE_MISMATCH", Error,
            "an operating point column disagrees with the resolved identity layout",
            category = Data;
        BUILD_OPERATING_POINT_IDENTITY_UNKNOWN = "BUILD.OPERATING_POINT.IDENTITY_UNKNOWN", Error,
            "an operating point names an element identity the network does not declare",
            category = Data;
        VALIDATE_UPDATE_COMPONENT_TYPE = "VALIDATE.UPDATE.COMPONENT_TYPE", Error,
            "an update names the wrong component type for its field", category = Data;
        VALIDATE_UPDATE_COMPONENT_UNKNOWN = "VALIDATE.UPDATE.COMPONENT_UNKNOWN", Error,
            "an update names no component in the target value", category = Data;
        VALIDATE_UPDATE_STABLE_ID_REQUIRED = "VALIDATE.UPDATE.STABLE_ID_REQUIRED", Error,
            "an update cannot target a table row position or an unidentified component",
            category = Data;
        VALIDATE_UPDATE_COMPONENT_AMBIGUOUS = "VALIDATE.UPDATE.COMPONENT_AMBIGUOUS", Error,
            "an update identity resolves to more than one component", category = Data;
        VALIDATE_UPDATE_FIELD_UNSUPPORTED = "VALIDATE.UPDATE.FIELD_UNSUPPORTED", Error,
            "the target electrical model does not define the requested field", category = Data;
        VALIDATE_UPDATE_VALUE_INVALID = "VALIDATE.UPDATE.VALUE_INVALID", Error,
            "an update replacement value is outside the field's domain", category = Data;
        VALIDATE_UPDATE_TERMINAL_UNKNOWN = "VALIDATE.UPDATE.TERMINAL_UNKNOWN", Error,
            "an update names no terminal on its component", category = Data;
        VALIDATE_UPDATE_DUPLICATE_FIELD = "VALIDATE.UPDATE.DUPLICATE_FIELD", Error,
            "an atomic update batch assigns one component field more than once", category = Data;
        VALIDATE_UPDATE_ALLOCATION_FAILED = "VALIDATE.UPDATE.ALLOCATION_FAILED", Error,
            "an aggregate load update has no valid allocation basis", category = Data;
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
        BUILD_SOLUTION_MULTIPLIER_INVALID = "BUILD.SOLUTION.MULTIPLIER_INVALID", Error,
            "a constraint multiplier is negative or non-finite", category = Data;
        TRANSFORM_INSTANCE_DATA_DISCARDED = "TRANSFORM.INSTANCE.DATA_DISCARDED", Warning,
            "the derived calculation does not carry part of the source instance";
        TRANSFORM_INSTANCE_ASSUMPTION = "TRANSFORM.INSTANCE.ASSUMPTION", Warning,
            "the derived calculation rests on a stated modeling assumption";
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
        REQUEST_GOC3_FORMAT_UNKNOWN = "REQUEST.GOC3.FORMAT_UNKNOWN", Error,
            "the named GOC3 source format is not one this build reads",
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

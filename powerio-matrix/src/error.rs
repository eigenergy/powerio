//! Failures the matrix and dataset builders raise.
//!
//! [`Error`] carries what this crate constructs and wraps [`powerio_tx::Error`]
//! for everything the transmission model raises underneath, so `?` moves that failure across
//! the boundary without restating it. A caller that only wants the coarse
//! split reads [`Error::category`], which is the same taxonomy the hub uses.

use powerio_core::DiagnosticInfo;
use thiserror::Error as ThisError;

use crate::diagnostics::codes;

/// A matrix, sensitivity, or dataset failure.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum Error {
    /// A failure from the balanced model, its readers, or its writers.
    #[error(transparent)]
    Transmission(#[from] powerio_tx::Error),

    /// An underlying I/O failure this crate raised itself.
    ///
    /// One the transmission model raised arrives as [`Error::Transmission`] wrapping
    /// `powerio_tx::Error::Io`, so a caller telling I/O apart from the rest reads
    /// [`Error::category`] rather than matching this variant.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A refused or failed destination commit (an output collision, an
    /// invalid inventory, a staging failure), carrying the registered core
    /// failure. Code and category delegate to it.
    #[error(transparent)]
    Commit(#[from] powerio_core::Error),

    #[error("output dimension mismatch: matrix is {n}x{n} but RHS has length {b_len}")]
    DimensionMismatch { n: usize, b_len: usize },

    #[error("dimension mismatch: `{what}` expected length {expected}, got {got}")]
    ShapeMismatch {
        what: &'static str,
        expected: usize,
        got: usize,
    },

    #[error("unsupported OPF objective: {reason}")]
    UnsupportedOpfObjective { reason: String },

    #[error("unsupported AC power flow bus specification")]
    UnsupportedAcPfSpecification,

    #[error("{family} constraint selection names unknown identity `{identity}`")]
    UnknownConstraintIdentity {
        family: &'static str,
        identity: String,
    },

    #[error("{family} has duplicate stable identity `{identity}`")]
    DuplicateElementIdentity {
        family: &'static str,
        identity: String,
    },

    #[error(
        "DC sensitivity solve failed: the reference-grounded Laplacian is singular even though every component is grounded"
    )]
    SingularNetwork,

    #[error("invalid DC sensitivity option: {reason}")]
    InvalidSensitivityOptions { reason: String },

    #[error("case has no generators; DC-OPF requires an `mpc.gen` block")]
    NoGenerators,

    #[error(
        "generator {gen_index} has an unsupported cost model (model {model}, ncost {ncost}); need polynomial model 2 with degree ≤ 2"
    )]
    UnsupportedCostModel {
        gen_index: usize,
        model: u8,
        ncost: usize,
    },

    #[error("generator {gen_index} has an invalid piecewise linear cost: {reason}")]
    InvalidPiecewiseCost {
        gen_index: usize,
        reason: PiecewiseCostInvalidity,
    },

    #[error(
        "generator {gen_index} has a nonconvex piecewise linear cost: segment {segment} has a lower slope than the preceding segment"
    )]
    NonconvexPiecewiseCost { gen_index: usize, segment: usize },

    #[error(
        "generator {gen_index} has a piecewise linear cost that cannot be projected to one nodal quadratic cost"
    )]
    PiecewiseNodalCost { gen_index: usize },

    #[error(
        "generator {gen_index} has a concave cost row (c2 = {c2}); need a nonnegative quadratic coefficient"
    )]
    ConcaveCost { gen_index: usize, c2: f64 },

    #[error("matrix-market I/O: {0}")]
    Mtx(String),

    #[error("gridfm Parquet export: {0}")]
    Parquet(String),

    #[error("gridfm scenario batch is empty; provide at least one snapshot")]
    EmptyScenarioBatch,

    #[error("gridfm scenario id overflows i64 when numbering snapshot {index} from base {base}")]
    ScenarioIdOverflow {
        base: i64,
        /// 0-based position of the snapshot whose `base + index` overflowed.
        index: usize,
    },

    #[error(
        "gridfm snapshot scenario {scenario} is normalized; gridfm export expects raw MW and degree fields"
    )]
    NormalizedGridfmSnapshot { scenario: i64 },

    #[error(
        "gridfm snapshot scenario {scenario} has non-finite {element} row {row} field `{field}`: {value}"
    )]
    NonFiniteGridfmValue {
        scenario: i64,
        element: &'static str,
        row: usize,
        field: &'static str,
        value: f64,
    },

    #[error(
        "gridfm snapshot {index} doesn't align with the scenario batch: {reason}; \
         a batch requires unique scenario ids, one system base, and fixed bus/branch/gen row identities"
    )]
    ScenarioShapeMismatch {
        /// 0-based position of the offending snapshot in the batch (independent
        /// of the snapshot's scenario id).
        index: usize,
        reason: ScenarioMismatch,
    },
}

impl Error {
    /// The registry entry for this error. The match is exhaustive over the
    /// variant set, so a new variant must be coded here before it compiles.
    ///
    /// A hub failure keeps the hub's own code: restating it here would give one
    /// failure two identities.
    #[must_use]
    pub fn code(&self) -> &'static DiagnosticInfo {
        match self {
            Error::Transmission(inner) => inner.code(),
            Error::Commit(inner) => inner.info().unwrap_or(&codes::EMIT_MTX_FAILED),
            Error::Io(_) => &codes::READ_MATRIX_IO_FAILED,
            Error::DimensionMismatch { .. } | Error::ShapeMismatch { .. } => {
                &codes::BUILD_MATRIX_SHAPE_MISMATCH
            }
            Error::UnsupportedOpfObjective { .. } => &codes::BUILD_OPF_OBJECTIVE_UNSUPPORTED,
            Error::UnsupportedAcPfSpecification => &codes::BUILD_AC_PF_SPECIFICATION_UNSUPPORTED,
            Error::UnknownConstraintIdentity { .. } => {
                &codes::BUILD_OPF_CONSTRAINT_IDENTITY_UNKNOWN
            }
            Error::DuplicateElementIdentity { .. } => &codes::BUILD_OPF_ELEMENT_IDENTITY_DUPLICATE,
            Error::SingularNetwork => &codes::BUILD_SENSITIVITY_SINGULAR,
            Error::InvalidSensitivityOptions { .. } => &codes::BUILD_SENSITIVITY_INVALID_OPTION,
            Error::EmptyScenarioBatch => &codes::BUILD_GRIDFM_EMPTY_BATCH,
            Error::ScenarioIdOverflow { .. } => &codes::BUILD_GRIDFM_SCENARIO_ID_OVERFLOW,
            Error::NormalizedGridfmSnapshot { .. } => &codes::BUILD_GRIDFM_NORMALIZED_SNAPSHOT,
            Error::NonFiniteGridfmValue { .. } => &codes::BUILD_GRIDFM_NOT_A_NUMBER,
            Error::ScenarioShapeMismatch { .. } => &codes::BUILD_GRIDFM_SCENARIO_SHAPE_MISMATCH,
            Error::NoGenerators => &powerio_prob::diagnostics::codes::BUILD_INSTANCE_NO_GENERATORS,
            Error::UnsupportedCostModel { .. } => {
                &powerio_prob::diagnostics::codes::BUILD_INSTANCE_UNSUPPORTED_COST_MODEL
            }
            Error::InvalidPiecewiseCost { .. } => {
                &powerio_prob::diagnostics::codes::BUILD_INSTANCE_PIECEWISE_COST_INVALID
            }
            Error::NonconvexPiecewiseCost { .. } => {
                &powerio_prob::diagnostics::codes::BUILD_INSTANCE_PIECEWISE_COST_NONCONVEX
            }
            Error::PiecewiseNodalCost { .. } => &codes::BUILD_OPF_NODAL_COST_UNSUPPORTED,
            Error::ConcaveCost { .. } => {
                &powerio_prob::diagnostics::codes::BUILD_INSTANCE_CONCAVE_COST
            }
            Error::Mtx(_) => &codes::EMIT_MTX_FAILED,
            Error::Parquet(_) => &codes::EMIT_PARQUET_FAILED,
        }
    }

    /// Classify this error, using the hub's taxonomy.
    ///
    /// The match is exhaustive over the variant set, so a new variant must be
    /// classified here before it compiles.
    #[must_use]
    pub fn category(&self) -> powerio_tx::ErrorCategory {
        use powerio_tx::ErrorCategory as C;
        match self {
            Error::Transmission(inner) => inner.category(),
            Error::Commit(inner) => inner.category(),
            Error::Io(_) => C::Io,
            // A well-formed case that cannot satisfy a requested operation.
            Error::DimensionMismatch { .. }
            | Error::ShapeMismatch { .. }
            | Error::UnsupportedOpfObjective { .. }
            | Error::UnsupportedAcPfSpecification
            | Error::UnknownConstraintIdentity { .. }
            | Error::DuplicateElementIdentity { .. }
            | Error::SingularNetwork
            | Error::InvalidSensitivityOptions { .. }
            | Error::EmptyScenarioBatch
            | Error::ScenarioIdOverflow { .. }
            | Error::NormalizedGridfmSnapshot { .. }
            | Error::NonFiniteGridfmValue { .. }
            | Error::ScenarioShapeMismatch { .. }
            | Error::NoGenerators
            | Error::UnsupportedCostModel { .. }
            | Error::InvalidPiecewiseCost { .. }
            | Error::NonconvexPiecewiseCost { .. }
            | Error::ConcaveCost { .. } => C::Data,
            Error::PiecewiseNodalCost { .. } => C::Request,
            // Output-side serialization write failures.
            Error::Mtx(_) | Error::Parquet(_) => C::Output,
        }
    }
}

/// Why a MATPOWER model 1 generator cost row is unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PiecewiseCostInvalidity {
    FewerThanTwoBreakpoints { declared: usize },
    Truncated { expected_values: usize, got: usize },
    NonFinitePoint { point: usize },
    NonIncreasingPower { point: usize },
}

impl std::fmt::Display for PiecewiseCostInvalidity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FewerThanTwoBreakpoints { declared } => {
                write!(f, "need at least two breakpoints, got {declared}")
            }
            Self::Truncated {
                expected_values,
                got,
            } => write!(
                f,
                "declared breakpoints require {expected_values} values, got {got}"
            ),
            Self::NonFinitePoint { point } => {
                write!(f, "breakpoint {point} has a non-finite coordinate")
            }
            Self::NonIncreasingPower { point } => write!(
                f,
                "breakpoint {point} does not have greater power than its predecessor"
            ),
        }
    }
}

/// The element counts that define a scenario batch's shared base shape. Named
/// (rather than a bare `(usize, usize, usize)`) so the three same-typed fields
/// can't be transposed silently in an error message or a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementCounts {
    pub buses: usize,
    pub branches: usize,
    pub gens: usize,
}

impl std::fmt::Display for ElementCounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} buses, {} branches, {} gens",
            self.buses, self.branches, self.gens
        )
    }
}

/// Why a gridfm scenario snapshot doesn't line up with the first snapshot's
/// batch (the row stack uses each table's row number as element identity).
///
/// This enum is `#[non_exhaustive]`; downstream matches must include a wildcard
/// arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScenarioMismatch {
    /// Element counts differ.
    Counts {
        expected: ElementCounts,
        got: ElementCounts,
    },
    /// Counts match, but the buses are listed in a different order (so the dense
    /// bus index wouldn't mean the same bus across snapshots).
    BusOrder,
    /// A branch row has a different endpoint pair or component identity.
    BranchOrder,
    /// A generator row has a different bus or component identity.
    GeneratorOrder,
    /// The snapshot uses a different system power base.
    BaseMva,
    /// Another snapshot already uses this scenario id.
    DuplicateScenarioId { scenario: i64, first_index: usize },
}

impl std::fmt::Display for ScenarioMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Counts { expected, got } => {
                write!(f, "got ({got}) vs the first snapshot's ({expected})")
            }
            Self::BusOrder => {
                write!(f, "counts match but the bus ids are in a different order")
            }
            Self::BranchOrder => write!(
                f,
                "counts match but branch endpoints or identities differ by row"
            ),
            Self::GeneratorOrder => write!(
                f,
                "counts match but generator buses or identities differ by row"
            ),
            Self::BaseMva => write!(f, "base_mva differs from the first snapshot"),
            Self::DuplicateScenarioId {
                scenario,
                first_index,
            } => write!(
                f,
                "scenario id {scenario} is already used by snapshot {first_index}"
            ),
        }
    }
}

/// The result type every fallible entry point in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use powerio_tx::ErrorCategory::{Data, Output, Parse, Request};

    #[test]
    fn category_pins_the_intended_buckets() {
        assert_eq!(Error::SingularNetwork.category(), Data);
        assert_eq!(Error::EmptyScenarioBatch.category(), Data);
        assert_eq!(Error::Mtx("write failed".into()).category(), Output);
        assert_eq!(Error::Parquet("write failed".into()).category(), Output);
    }

    // Every error is a diagnostic that ended the operation, so the code's
    // published category and `category()` are one fact.
    #[test]
    fn every_error_code_publishes_the_category_the_variant_reports() {
        let every: Vec<Error> = vec![
            powerio_tx::Error::MissingField("bus").into(),
            std::io::Error::from(std::io::ErrorKind::NotFound).into(),
            Error::DimensionMismatch { n: 2, b_len: 3 },
            Error::ShapeMismatch {
                what: "p",
                expected: 2,
                got: 3,
            },
            Error::UnsupportedOpfObjective {
                reason: "term".into(),
            },
            Error::UnknownConstraintIdentity {
                family: "branches",
                identity: "missing".into(),
            },
            Error::DuplicateElementIdentity {
                family: "branches",
                identity: "duplicate".into(),
            },
            Error::SingularNetwork,
            Error::InvalidSensitivityOptions {
                reason: "tol".into(),
            },
            Error::EmptyScenarioBatch,
            Error::ScenarioIdOverflow { base: 1, index: 0 },
            Error::NormalizedGridfmSnapshot { scenario: 1 },
            Error::NonFiniteGridfmValue {
                scenario: 1,
                element: "bus",
                row: 0,
                field: "vm",
                value: f64::NAN,
            },
            Error::ScenarioShapeMismatch {
                index: 1,
                reason: ScenarioMismatch::BusOrder,
            },
            Error::NoGenerators,
            Error::UnsupportedCostModel {
                gen_index: 0,
                model: 1,
                ncost: 4,
            },
            Error::InvalidPiecewiseCost {
                gen_index: 0,
                reason: PiecewiseCostInvalidity::FewerThanTwoBreakpoints { declared: 1 },
            },
            Error::NonconvexPiecewiseCost {
                gen_index: 0,
                segment: 1,
            },
            Error::PiecewiseNodalCost { gen_index: 0 },
            Error::ConcaveCost {
                gen_index: 0,
                c2: -0.5,
            },
            Error::Mtx("write failed".into()),
            Error::Parquet("write failed".into()),
        ];
        for error in &every {
            assert_eq!(
                error.code().category,
                Some(error.category()),
                "{}",
                error.code().code
            );
        }
    }

    #[test]
    fn piecewise_cost_failures_keep_distinct_diagnostic_meanings() {
        let malformed = Error::InvalidPiecewiseCost {
            gen_index: 2,
            reason: PiecewiseCostInvalidity::FewerThanTwoBreakpoints { declared: 1 },
        };
        let nonconvex = Error::NonconvexPiecewiseCost {
            gen_index: 2,
            segment: 1,
        };
        let nodal_projection = Error::PiecewiseNodalCost { gen_index: 2 };

        assert_eq!(
            malformed.code().code,
            "BUILD.INSTANCE.PIECEWISE_COST_INVALID"
        );
        assert_eq!(
            nonconvex.code().code,
            "BUILD.INSTANCE.PIECEWISE_COST_NONCONVEX"
        );
        assert_eq!(
            nodal_projection.code().code,
            "BUILD.OPF.NODAL_COST_UNSUPPORTED"
        );
        assert_eq!(malformed.category(), Data);
        assert_eq!(nonconvex.category(), Data);
        assert_eq!(nodal_projection.category(), Request);
    }

    #[test]
    fn a_wrapped_hub_error_keeps_its_own_category() {
        let wrapped: Error = powerio_tx::Error::MissingField("bus").into();
        assert_eq!(wrapped.category(), Parse);
        // And its message, byte for byte: the C ABI reports errors as text, so
        // a wrapper that restated the message would change what a binding sees.
        assert_eq!(
            wrapped.to_string(),
            powerio_tx::Error::MissingField("bus").to_string()
        );
    }
}

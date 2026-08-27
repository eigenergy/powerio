//! Failures the matrix and dataset builders raise.
//!
//! [`Error`] carries what this crate constructs and wraps [`powerio_tx::Error`]
//! for everything the hub raises underneath, so `?` moves a hub failure across
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
    Core(#[from] powerio_tx::Error),

    /// An underlying I/O failure this crate raised itself.
    ///
    /// One the hub raised arrives as [`Error::Core`] wrapping
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

    #[error(
        "DC sensitivity solve failed: the reference-grounded Laplacian is singular even though every component is grounded"
    )]
    SingularNetwork,

    #[error("invalid DC sensitivity option: {reason}")]
    InvalidSensitivityOptions { reason: String },

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
        "gridfm snapshot {index} doesn't match the first snapshot's element set: {reason}; \
         a scenario batch shares one base element set (same bus/branch/gen counts and bus-id order)"
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
            Error::Core(inner) => inner.code(),
            Error::Commit(inner) => inner.info().unwrap_or(&codes::EMIT_MTX_FAILED),
            Error::Io(_) => &codes::READ_MATRIX_IO_FAILED,
            Error::DimensionMismatch { .. } | Error::ShapeMismatch { .. } => {
                &codes::BUILD_MATRIX_SHAPE_MISMATCH
            }
            Error::SingularNetwork => &codes::BUILD_SENSITIVITY_SINGULAR,
            Error::InvalidSensitivityOptions { .. } => &codes::BUILD_SENSITIVITY_INVALID_OPTION,
            Error::EmptyScenarioBatch => &codes::BUILD_GRIDFM_EMPTY_BATCH,
            Error::ScenarioIdOverflow { .. } => &codes::BUILD_GRIDFM_SCENARIO_ID_OVERFLOW,
            Error::NormalizedGridfmSnapshot { .. } => &codes::BUILD_GRIDFM_NORMALIZED_SNAPSHOT,
            Error::NonFiniteGridfmValue { .. } => &codes::BUILD_GRIDFM_NOT_A_NUMBER,
            Error::ScenarioShapeMismatch { .. } => &codes::BUILD_GRIDFM_SCENARIO_SHAPE_MISMATCH,
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
            Error::Core(inner) => inner.category(),
            Error::Commit(inner) => inner.category(),
            Error::Io(_) => C::Io,
            // A well-formed case that cannot satisfy a requested operation.
            Error::DimensionMismatch { .. }
            | Error::ShapeMismatch { .. }
            | Error::SingularNetwork
            | Error::InvalidSensitivityOptions { .. }
            | Error::EmptyScenarioBatch
            | Error::ScenarioIdOverflow { .. }
            | Error::NormalizedGridfmSnapshot { .. }
            | Error::NonFiniteGridfmValue { .. }
            | Error::ScenarioShapeMismatch { .. } => C::Data,
            // Output-side serialization write failures.
            Error::Mtx(_) | Error::Parquet(_) => C::Output,
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
/// base element set (the row-stack keeps every table schema-consistent by
/// requiring the same element counts and bus-id ordering across snapshots).
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
        }
    }
}

/// The result type every fallible entry point in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use powerio_tx::ErrorCategory::{Data, Output, Parse};

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

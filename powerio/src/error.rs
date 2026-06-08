use thiserror::Error;

use crate::network::BusId;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("missing required MATPOWER field `{0}`")]
    MissingField(&'static str),

    #[error(
        "malformed MATPOWER `{field}` row {row}: expected at least {expected} columns, got {got}"
    )]
    ShortRow {
        field: &'static str,
        row: usize,
        expected: usize,
        got: usize,
    },

    #[error("could not parse `{field}` row {row} value `{value}` as f64")]
    BadFloat {
        field: &'static str,
        row: usize,
        value: String,
    },

    #[error("unbalanced brackets in MATPOWER `{0}` matrix")]
    UnbalancedBrackets(&'static str),

    #[error("element references unknown bus id {bus_id} (in-service index {element_index})")]
    UnknownBus { bus_id: BusId, element_index: usize },

    #[error("branch row {row} has zero impedance (r=0, x=0); not representable in B'")]
    ZeroImpedance { row: usize },

    #[error("branch row {row} has non-finite DC susceptance b = 1/x (x is NaN, Inf, or denormal)")]
    NonFiniteSusceptance { row: usize },

    #[error("output dimension mismatch: matrix is {n}x{n} but RHS has length {b_len}")]
    DimensionMismatch { n: usize, b_len: usize },

    #[error("case has no generators; DC-OPF requires an `mpc.gen` block")]
    NoGenerators,

    #[error("generator {gen_index} has no cost data")]
    MissingGenCost { gen_index: usize },

    #[error(
        "generator {gen_index} has an unsupported cost model (model {model}, ncost {ncost}); need polynomial model 2 with degree ≤ 2"
    )]
    UnsupportedCostModel {
        gen_index: usize,
        model: u8,
        ncost: usize,
    },

    #[error("`gen` has {gens} rows but `gencost` has {gencost}; expected {gens} (active only) or {} (active + reactive)", gens * 2)]
    GenCostCountMismatch { gens: usize, gencost: usize },

    #[error("expected exactly one reference (slack) bus, found {found}")]
    ReferenceBusCount { found: usize },

    #[error("dimension mismatch: `{what}` expected length {expected}, got {got}")]
    ShapeMismatch {
        what: &'static str,
        expected: usize,
        got: usize,
    },

    #[error(
        "network has {components} connected components; DC sensitivities require a single island"
    )]
    DisconnectedNetwork { components: usize },

    #[error(
        "DC sensitivity solve failed: the slack-grounded Laplacian is singular for a connected network"
    )]
    SingularNetwork,

    #[error(transparent)]
    Io(#[from] std::io::Error),

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
        "gridfm snapshot {index} doesn't match the first snapshot's element set: {reason}; \
         a scenario batch shares one base element set (same bus/branch/gen counts and bus-id order)"
    )]
    ScenarioShapeMismatch {
        /// 0-based position of the offending snapshot in the batch (independent
        /// of the snapshot's scenario id).
        index: usize,
        reason: ScenarioMismatch,
    },

    #[error("{format} read error: {message}")]
    FormatRead {
        format: &'static str,
        message: String,
    },

    #[error("unknown or unsupported case format: {0}")]
    UnknownFormat(String),
}

/// Coarse classification of an [`Error`], for callers that map onto their own
/// taxonomy (the Python layer's exception subclasses, C ABI status codes, a
/// CLI exit code). Distinguishing "the input file is bad" from "the operation
/// can't run on this otherwise-valid case" is the split callers actually branch
/// on, and it's a property of the error, not of the binding that surfaces it.
///
/// Deliberately *not* `#[non_exhaustive]` (unlike [`Error`]): a category-mapping
/// match should fail to compile when a category is added, so every binding is
/// forced to decide how to surface it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// Underlying I/O failure reading or writing a file.
    Io,
    /// The requested format is unknown or can't be inferred from the path.
    UnknownFormat,
    /// The input is malformed or unparseable.
    Parse,
    /// A well-formed case can't satisfy the requested operation.
    Data,
    /// An output serialization step (matrix-market, Parquet) failed.
    Output,
}

impl Error {
    /// Classify this error. The match is exhaustive over the variant set (no
    /// wildcard), so adding an `Error` variant is a compile error here until it
    /// is categorized — categorization can't silently drift as the enum grows.
    pub fn category(&self) -> ErrorCategory {
        use ErrorCategory as C;
        match self {
            Error::Io(_) => C::Io,
            Error::UnknownFormat(_) => C::UnknownFormat,
            // Malformed or unparseable input. Only the parser/format readers
            // raise these.
            Error::MissingField(_)
            | Error::ShortRow { .. }
            | Error::BadFloat { .. }
            | Error::UnbalancedBrackets(_)
            | Error::FormatRead { .. } => C::Parse,
            // A well-formed case that can't satisfy a requested operation. These
            // surface mid-build (matrix/OPF/gridfm), not at parse time —
            // `UnknownBus` and the scenario-batch checks included: the file
            // parsed, the operation can't proceed.
            Error::UnknownBus { .. }
            | Error::ZeroImpedance { .. }
            | Error::NonFiniteSusceptance { .. }
            | Error::DimensionMismatch { .. }
            | Error::NoGenerators
            | Error::MissingGenCost { .. }
            | Error::UnsupportedCostModel { .. }
            | Error::GenCostCountMismatch { .. }
            | Error::ReferenceBusCount { .. }
            | Error::ShapeMismatch { .. }
            | Error::DisconnectedNetwork { .. }
            | Error::SingularNetwork
            | Error::EmptyScenarioBatch
            | Error::ScenarioIdOverflow { .. }
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
/// `#[non_exhaustive]`: future checks (e.g. branch endpoints, voltage base) may
/// add variants, so downstream matches must keep a wildcard arm.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_pins_the_intended_buckets() {
        use ErrorCategory::*;
        // The parser/format readers raise these.
        assert_eq!(Error::MissingField("bus").category(), Parse);
        assert_eq!(
            Error::FormatRead {
                format: "psse",
                message: "bad record".into()
            }
            .category(),
            Parse
        );
        // An unmet operation precondition on an already-parsed case. UnknownBus
        // and the scenario-batch checks surface mid-build, not at parse time, so
        // they are Data, not Parse — regression guard for that classification.
        assert_eq!(Error::NoGenerators.category(), Data);
        assert_eq!(
            Error::UnknownBus {
                bus_id: BusId(7),
                element_index: 0
            }
            .category(),
            Data
        );
        assert_eq!(Error::EmptyScenarioBatch.category(), Data);
        assert_eq!(
            Error::ScenarioShapeMismatch {
                index: 1,
                reason: ScenarioMismatch::BusOrder
            }
            .category(),
            Data
        );
        // Format selection, output serialization, and underlying I/O.
        assert_eq!(Error::UnknownFormat("xyz".into()).category(), UnknownFormat);
        assert_eq!(Error::Mtx("write failed".into()).category(), Output);
        assert_eq!(
            Error::Io(std::io::Error::from(std::io::ErrorKind::NotFound)).category(),
            Io
        );
    }
}

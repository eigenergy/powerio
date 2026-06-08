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

    #[error("gridfm scenario id overflows i64 numbering {count} snapshot(s) from base {base}")]
    ScenarioIdOverflow { base: i64, count: usize },

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

/// Why a gridfm scenario snapshot doesn't line up with the first snapshot's
/// base element set (the row-stack keeps every table schema-consistent by
/// requiring the same element counts and bus-id ordering across snapshots).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioMismatch {
    /// Element counts differ: `(buses, branches, gens)`.
    Counts {
        expected: (usize, usize, usize),
        got: (usize, usize, usize),
    },
    /// Counts match, but the buses are listed in a different order (so the dense
    /// bus index wouldn't mean the same bus across snapshots).
    BusOrder,
}

impl std::fmt::Display for ScenarioMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Counts { expected, got } => {
                write!(f, "(buses, branches, gens) = {got:?} vs {expected:?}")
            }
            Self::BusOrder => {
                write!(f, "counts match but the bus ids are in a different order")
            }
        }
    }
}

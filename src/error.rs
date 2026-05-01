use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("missing required MATPOWER field `{0}`")]
    MissingField(&'static str),

    #[error("malformed MATPOWER `{field}` row {row}: expected at least {expected} columns, got {got}")]
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

    #[error("branch references unknown bus id {bus_id} (branch row {row})")]
    UnknownBus { bus_id: usize, row: usize },

    #[error("branch row {row} has zero impedance (r=0, x=0); not representable in B'")]
    ZeroImpedance { row: usize },

    #[error("output dimension mismatch: matrix is {n}x{n} but RHS has length {b_len}")]
    DimensionMismatch { n: usize, b_len: usize },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("matrix-market I/O: {0}")]
    Mtx(String),

    #[error("regex compilation failed: {0}")]
    Regex(#[from] regex::Error),
}

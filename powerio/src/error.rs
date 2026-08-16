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

    #[error("branch row {row} has a zero matrix denominator under the selected build options")]
    ZeroImpedance { row: usize },

    #[error("branch row {row} has non-finite DC susceptance b = 1/x (x is NaN, Inf, or denormal)")]
    NonFiniteSusceptance { row: usize },

    // Raised from incidence assembly and both OPF builders too, not only from
    // Y_bus, so the message names the division rather than one caller.
    #[error("branch row {row} has a tap ratio of {tap} too small to divide by")]
    DegenerateTap { row: usize, tap: f64 },

    #[error("generator {gen_index} has no cost data")]
    MissingGenCost { gen_index: usize },

    #[error("default generator cost field `{field}` is not finite: {value}")]
    NonFiniteGenCost { field: &'static str, value: f64 },

    #[error("invalid generator cost patch row {row}: {reason}")]
    InvalidGenCostPatch { row: usize, reason: String },

    #[error("`gen` has {gens} rows but `gencost` has {gencost}; expected {gens} (active only) or {} (active + reactive)", gens * 2)]
    GenCostCountMismatch { gens: usize, gencost: usize },

    #[error("expected exactly one reference (slack) bus, found {found}")]
    ReferenceBusCount { found: usize },

    #[error("base MVA must be a positive, finite number, got {base}")]
    InvalidBaseMva { base: f64 },

    #[error("invalid normalize option `{field}`: {value}")]
    InvalidNormalizeOption { field: &'static str, value: f64 },

    #[error(
        "{components} connected component(s) have no reference (slack) bus to ground; DC sensitivities need at least one reference per island"
    )]
    UngroundedComponent { components: usize },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(
        "geo apply left {buses} bus(es) with no location and {branches} branch(es) with no route"
    )]
    UnlocatedElements { buses: usize, branches: usize },

    #[error("{format} read error: {message}")]
    FormatRead {
        format: &'static str,
        message: String,
    },

    #[error("unknown or unsupported case format: {0}")]
    UnknownFormat(String),

    /// The target format is recognized but read only: it has no writer. A
    /// same-format write can still echo retained source; everything else is
    /// refused with this error rather than a misleading [`Error::UnknownFormat`].
    #[error("{format} is a read only format with no writer")]
    WriteUnsupported { format: &'static str },
}

/// Coarse classification of an [`enum@Error`], for callers that map onto their own
/// taxonomy (the Python layer's exception subclasses, C ABI status codes, a
/// CLI exit code). Distinguishing "the input file is bad" from "the operation
/// can't run on this otherwise-valid case" is the split callers actually branch
/// on, and it's a property of the error, not of the binding that surfaces it.
///
/// Unlike [`enum@Error`], this enum is not `#[non_exhaustive]`. Adding a
/// category makes exhaustive matches fail to compile, which requires each
/// binding to map the new category.
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
            // WriteUnsupported keeps the UnknownFormat category so bindings
            // surface it the same way (a ValueError, not a data error): the
            // request named a format the writer can't produce.
            Error::UnknownFormat(_) | Error::WriteUnsupported { .. } => C::UnknownFormat,
            // Malformed or unparseable input. Only the parser/format readers
            // raise these.
            Error::MissingField(_)
            | Error::ShortRow { .. }
            | Error::BadFloat { .. }
            | Error::UnbalancedBrackets(_)
            | Error::FormatRead { .. } => C::Parse,
            // A well-formed case that can't satisfy a requested operation. These
            // surface mid-build (matrix/OPF/gridfm), not at parse time —
            // `UnknownBus` and the scenario batch checks included: the file
            // parsed, the operation can't proceed.
            Error::UnknownBus { .. }
            | Error::ZeroImpedance { .. }
            | Error::NonFiniteSusceptance { .. }
            | Error::DegenerateTap { .. }
            | Error::MissingGenCost { .. }
            | Error::NonFiniteGenCost { .. }
            | Error::InvalidGenCostPatch { .. }
            | Error::GenCostCountMismatch { .. }
            | Error::ReferenceBusCount { .. }
            | Error::InvalidBaseMva { .. }
            | Error::InvalidNormalizeOption { .. }
            | Error::UngroundedComponent { .. }
            | Error::UnlocatedElements { .. } => C::Data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_pins_the_intended_buckets() {
        use ErrorCategory::{Data, Io, Parse, UnknownFormat};
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
        // surfaces mid-build, not at parse time, so it is Data, not Parse —
        // regression guard for that classification.
        assert_eq!(Error::InvalidBaseMva { base: 0.0 }.category(), Data);
        assert_eq!(
            Error::UngroundedComponent { components: 1 }.category(),
            Data
        );
        assert_eq!(
            Error::UnknownBus {
                bus_id: BusId(7),
                element_index: 0
            }
            .category(),
            Data
        );
        // Format selection and underlying I/O. Output-side serialization
        // failures belong to the crate that writes, so `Output` has no hub
        // variant; `powerio_matrix::Error` carries it.
        assert_eq!(Error::UnknownFormat("xyz".into()).category(), UnknownFormat);
        assert_eq!(
            Error::Io(std::io::Error::from(std::io::ErrorKind::NotFound)).category(),
            Io
        );
    }
}

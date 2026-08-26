use thiserror::Error;

use crate::diagnostics::{DiagnosticInfo, codes};
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

    #[error("malformed MATPOWER `{field}` row {row}: {message}")]
    BadId {
        field: &'static str,
        row: usize,
        message: String,
    },

    #[error("unbalanced brackets in MATPOWER `{0}` matrix")]
    UnbalancedBrackets(&'static str),

    #[error("element references unknown bus id {bus_id} (in-service index {element_index})")]
    UnknownBus { bus_id: BusId, element_index: usize },

    #[error("branch row {row} has a zero matrix denominator under the selected build options")]
    ZeroImpedance { row: usize },

    #[error(
        "branch row {row} has a non-finite susceptance (r or x is NaN or Inf, or the four terminal admittances overflow)"
    )]
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

    #[error(
        "`dcline` has {dclines} rows but `dclinecost` has {dclinecost}; expected one cost row per dcline"
    )]
    DcLineCostCountMismatch { dclines: usize, dclinecost: usize },

    /// Normalization could not establish a reference bus. The reference set is
    /// derived from generator presence: a bus keeps `REF` only while it hosts
    /// an in-service generator, so a `REF` typed bus with no generator does not
    /// count, and with no in-service generator there is nothing to promote.
    #[error(
        "cannot establish a reference bus: a reference bus must host an in-service generator, and this case has none"
    )]
    NoReferenceBus,

    /// An index or solver table build needs exactly one reference bus.
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

/// Coarse classification of an [`enum@Error`], for callers that map onto their
/// own taxonomy. Defined in `powerio-core` so every crate in the workspace
/// projects onto the same five tokens.
pub use powerio_core::ErrorCategory;

impl Error {
    /// An index or solver table build needs exactly one reference bus and the
    /// case states `found`.
    #[must_use]
    pub fn reference_bus_count(found: usize) -> Self {
        Error::ReferenceBusCount { found }
    }

    /// The registry entry for this error. The match is exhaustive over the
    /// variant set (no wildcard), so adding an `Error` variant is a compile
    /// error here until it is coded.
    pub fn code(&self) -> &'static DiagnosticInfo {
        match self {
            Error::MissingField(_)
            | Error::ShortRow { .. }
            | Error::BadFloat { .. }
            | Error::BadId { .. }
            | Error::UnbalancedBrackets(_) => &codes::PARSE_MATPOWER_MALFORMED,
            Error::FormatRead { .. } => &codes::PARSE_SOURCE_MALFORMED,
            Error::Io(_) => &codes::READ_IO_FAILED,
            Error::UnknownBus { .. } => &codes::BUILD_INDEX_UNKNOWN_BUS,
            Error::ZeroImpedance { .. } => &codes::BUILD_BRANCH_ZERO_IMPEDANCE,
            Error::NonFiniteSusceptance { .. } => &codes::BUILD_BRANCH_NOT_A_NUMBER,
            Error::DegenerateTap { .. } => &codes::BUILD_BRANCH_DEGENERATE_TAP,
            Error::MissingGenCost { .. } => &codes::VALIDATE_GEN_COST_MISSING,
            Error::NonFiniteGenCost { .. } => &codes::VALIDATE_GEN_COST_NOT_A_NUMBER,
            Error::InvalidGenCostPatch { .. } => &codes::VALIDATE_GEN_COST_PATCH_INVALID,
            Error::GenCostCountMismatch { .. } => &codes::VALIDATE_GEN_COST_COUNT_MISMATCH,
            Error::DcLineCostCountMismatch { .. } => &codes::VALIDATE_DC_LINE_COST_COUNT_MISMATCH,
            Error::NoReferenceBus => &codes::CANONICALIZE_NORMALIZE_NO_REFERENCE_BUS,
            Error::ReferenceBusCount { .. } => &codes::BUILD_INDEX_REFERENCE_BUS_COUNT,
            Error::InvalidBaseMva { .. } => &codes::CANONICALIZE_NORMALIZE_INVALID_BASE_MVA,
            Error::InvalidNormalizeOption { .. } => &codes::CANONICALIZE_NORMALIZE_INVALID_OPTION,
            Error::UngroundedComponent { .. } => &codes::BUILD_INDEX_UNGROUNDED_COMPONENT,
            Error::UnlocatedElements { .. } => &codes::BUILD_GEO_UNLOCATED_ELEMENTS,
            Error::UnknownFormat(_) => &codes::REQUEST_FORMAT_UNKNOWN,
            Error::WriteUnsupported { .. } => &codes::REQUEST_FORMAT_WRITE_UNSUPPORTED,
        }
    }

    /// Classify this error. The match is exhaustive over the variant set (no
    /// wildcard), so adding an `Error` variant is a compile error here until it
    /// is categorized — categorization can't silently drift as the enum grows.
    pub fn category(&self) -> ErrorCategory {
        use ErrorCategory as C;
        match self {
            Error::Io(_) => C::Io,
            // WriteUnsupported keeps the Request category so bindings
            // surface it the same way (a ValueError, not a data error): the
            // request named a format the writer can't produce.
            Error::UnknownFormat(_) | Error::WriteUnsupported { .. } => C::Request,
            // Malformed or unparseable input. Only the parser/format readers
            // raise these.
            Error::MissingField(_)
            | Error::ShortRow { .. }
            | Error::BadFloat { .. }
            | Error::BadId { .. }
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
            | Error::DcLineCostCountMismatch { .. }
            | Error::NoReferenceBus
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

    // Every error is a diagnostic that ended the operation, so the code's
    // published category and `category()` are one fact. Two spellings that can
    // disagree is what this refuses.
    #[test]
    fn every_error_code_publishes_the_category_the_variant_reports() {
        let every: Vec<Error> = vec![
            Error::MissingField("bus"),
            Error::ShortRow {
                field: "bus",
                row: 1,
                expected: 13,
                got: 3,
            },
            Error::BadFloat {
                field: "bus",
                row: 1,
                value: "x".into(),
            },
            Error::BadId {
                field: "bus",
                row: 1,
                message: "`BUS_I` value 1e300 is outside the id range 0..2^63".into(),
            },
            Error::UnbalancedBrackets("bus"),
            Error::FormatRead {
                format: "psse",
                message: "bad record".into(),
            },
            Error::Io(std::io::Error::from(std::io::ErrorKind::NotFound)),
            Error::UnknownBus {
                bus_id: BusId(7),
                element_index: 0,
            },
            Error::ZeroImpedance { row: 1 },
            Error::NonFiniteSusceptance { row: 1 },
            Error::DegenerateTap { row: 1, tap: 0.0 },
            Error::MissingGenCost { gen_index: 0 },
            Error::NonFiniteGenCost {
                field: "c2",
                value: f64::NAN,
            },
            Error::InvalidGenCostPatch {
                row: 1,
                reason: "empty".into(),
            },
            Error::GenCostCountMismatch {
                gens: 2,
                gencost: 3,
            },
            Error::DcLineCostCountMismatch {
                dclines: 1,
                dclinecost: 2,
            },
            Error::NoReferenceBus,
            Error::reference_bus_count(2),
            Error::InvalidBaseMva { base: 0.0 },
            Error::InvalidNormalizeOption {
                field: "angle_bound_pad",
                value: 0.0,
            },
            Error::UngroundedComponent { components: 1 },
            Error::UnlocatedElements {
                buses: 1,
                branches: 0,
            },
            Error::UnknownFormat("xyz".into()),
            Error::WriteUnsupported { format: "goc3" },
        ];
        for error in &every {
            let info = error.code();
            assert_eq!(
                info.category,
                Some(error.category()),
                "{} publishes {:?} but the variant reports {:?}",
                info.code,
                info.category,
                error.category()
            );
        }
    }

    #[test]
    fn the_two_reference_bus_stages_carry_different_codes() {
        assert_eq!(
            Error::NoReferenceBus.code().code,
            "CANONICALIZE.NORMALIZE.NO_REFERENCE_BUS"
        );
        assert_eq!(
            Error::reference_bus_count(2).code().code,
            "BUILD.INDEX.REFERENCE_BUS_COUNT"
        );
        // The canonicalize refusal states the generating rule; "found 0" would
        // contradict a case whose bus table types a generator-less bus REF.
        assert!(Error::NoReferenceBus.to_string().contains("generator"));
    }

    #[test]
    fn category_pins_the_intended_buckets() {
        use ErrorCategory::{Data, Io, Parse, Request};
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
        assert_eq!(Error::UnknownFormat("xyz".into()).category(), Request);
        assert_eq!(
            Error::Io(std::io::Error::from(std::io::ErrorKind::NotFound)).category(),
            Io
        );
    }
}

#[cfg(test)]
mod category_token_tests {
    use super::ErrorCategory;

    // TOKENS is written out so C consumers get the closed set without a
    // parser; this keeps it from drifting from token().
    #[test]
    fn tokens_lists_every_category_exactly_once() {
        let every = [
            ErrorCategory::Io,
            ErrorCategory::Request,
            ErrorCategory::Parse,
            ErrorCategory::Data,
            ErrorCategory::Output,
        ];
        let from_tokens: Vec<&str> = every.iter().map(|c| c.as_str()).collect();
        assert_eq!(from_tokens, ErrorCategory::TOKENS.to_vec());
    }
}

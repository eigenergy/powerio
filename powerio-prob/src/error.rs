//! Failures the problem instance builders raise.
//!
//! [`Error`] carries what this crate constructs and wraps the crates beneath
//! it, so `?` moves a failure across the boundary without restating it. A
//! caller that only wants the coarse split reads [`Error::category`], which is
//! the same taxonomy every powerio surface uses.

use powerio_core::DiagnosticInfo;
use thiserror::Error as ThisError;

use crate::diagnostics::codes;

/// A problem instance failure.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum Error {
    /// A failure from the balanced model, its readers, or its writers.
    #[error(transparent)]
    Core(#[from] powerio_tx::Error),

    /// A failure from the matrix and dataset builders.
    #[cfg(feature = "matrix")]
    #[error(transparent)]
    Matrix(#[from] powerio_matrix::Error),

    /// An underlying I/O failure reading or writing a file.
    #[error(transparent)]
    Io(#[from] std::io::Error),

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

    #[error(
        "generator {gen_index} has a concave cost row (c2 = {c2}); need a nonnegative quadratic coefficient"
    )]
    ConcaveCost { gen_index: usize, c2: f64 },
}

impl Error {
    /// The registry entry for this error. The match is exhaustive over the
    /// variant set, so a new variant must be coded here before it compiles.
    #[must_use]
    pub fn code(&self) -> &'static DiagnosticInfo {
        match self {
            Error::Core(inner) => inner.code(),
            #[cfg(feature = "matrix")]
            Error::Matrix(inner) => inner.code(),
            Error::Io(_) => &codes::READ_INSTANCE_IO_FAILED,
            Error::NoGenerators => &codes::BUILD_INSTANCE_NO_GENERATORS,
            Error::UnsupportedCostModel { .. } => &codes::BUILD_INSTANCE_UNSUPPORTED_COST_MODEL,
            Error::ConcaveCost { .. } => &codes::BUILD_INSTANCE_CONCAVE_COST,
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
            #[cfg(feature = "matrix")]
            Error::Matrix(inner) => inner.category(),
            Error::Io(_) => C::Io,
            // A well-formed case that cannot satisfy a requested operation.
            Error::NoGenerators
            | Error::UnsupportedCostModel { .. }
            | Error::ConcaveCost { .. } => C::Data,
        }
    }
}

/// The result type every fallible entry point in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use powerio_tx::ErrorCategory::{Data, Parse};

    #[test]
    fn category_pins_the_intended_buckets() {
        assert_eq!(Error::NoGenerators.category(), Data);
        assert_eq!(
            Error::UnsupportedCostModel {
                gen_index: 0,
                model: 1,
                ncost: 4
            }
            .category(),
            Data
        );
        assert_eq!(
            Error::ConcaveCost {
                gen_index: 0,
                c2: -0.5
            }
            .category(),
            Data
        );
    }

    // Every error is a diagnostic that ended the operation, so the code's
    // published category and `category()` are one fact.
    #[test]
    fn every_error_code_publishes_the_category_the_variant_reports() {
        let every: Vec<Error> = vec![
            powerio_tx::Error::MissingField("gen").into(),
            std::io::Error::from(std::io::ErrorKind::NotFound).into(),
            Error::NoGenerators,
            Error::UnsupportedCostModel {
                gen_index: 0,
                model: 1,
                ncost: 4,
            },
            Error::ConcaveCost {
                gen_index: 0,
                c2: -0.5,
            },
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
    fn a_wrapped_hub_error_keeps_its_own_category_and_message() {
        let wrapped: Error = powerio_tx::Error::MissingField("gen").into();
        assert_eq!(wrapped.category(), Parse);
        assert_eq!(
            wrapped.to_string(),
            powerio_tx::Error::MissingField("gen").to_string()
        );
    }
}

//! Failures the problem instance builders raise.
//!
//! [`Error`] carries what this crate constructs and wraps the crates beneath
//! it, so `?` moves a failure across the boundary without restating it. A
//! caller that only wants the coarse split reads [`Error::category`], which is
//! the same taxonomy every powerio surface uses.

use thiserror::Error as ThisError;

/// A problem instance failure.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum Error {
    /// A failure from the balanced model, its readers, or its writers.
    #[error(transparent)]
    Core(#[from] powerio::Error),

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
}

impl Error {
    /// Classify this error, using the hub's taxonomy.
    ///
    /// The match is exhaustive over the variant set, so a new variant must be
    /// classified here before it compiles.
    #[must_use]
    pub fn category(&self) -> powerio::ErrorCategory {
        use powerio::ErrorCategory as C;
        match self {
            Error::Core(inner) => inner.category(),
            #[cfg(feature = "matrix")]
            Error::Matrix(inner) => inner.category(),
            Error::Io(_) => C::Io,
            // A well-formed case that cannot satisfy a requested operation.
            Error::NoGenerators | Error::UnsupportedCostModel { .. } => C::Data,
        }
    }
}

/// The result type every fallible entry point in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use powerio::ErrorCategory::{Data, Parse};

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
    }

    #[test]
    fn a_wrapped_hub_error_keeps_its_own_category_and_message() {
        let wrapped: Error = powerio::Error::MissingField("gen").into();
        assert_eq!(wrapped.category(), Parse);
        assert_eq!(
            wrapped.to_string(),
            powerio::Error::MissingField("gen").to_string()
        );
    }
}

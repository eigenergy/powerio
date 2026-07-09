//! Errors from building an OPF instance.

use powerio::BusId;

/// A `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Why a DC-OPF instance could not be built from a case.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The case has no in-service generators.
    NoGenerators,
    /// A generator has no cost row.
    MissingGenCost {
        /// Index of the generator in the case.
        gen_index: usize,
    },
    /// A generator cost is present but not a polynomial of degree at most 2.
    UnsupportedCostModel {
        /// Index of the generator in the case.
        gen_index: usize,
        /// MATPOWER cost model code.
        model: u8,
        /// Number of cost coefficients.
        ncost: usize,
    },
    /// A generator references a bus that is not in the case.
    UnknownBus {
        /// Source bus id that did not resolve.
        bus_id: BusId,
        /// Index of the referencing generator.
        element_index: usize,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoGenerators => write!(f, "case has no in-service generators"),
            Error::MissingGenCost { gen_index } => {
                write!(f, "generator {gen_index} has no cost data")
            }
            Error::UnsupportedCostModel {
                gen_index,
                model,
                ncost,
            } => write!(
                f,
                "generator {gen_index} has an unsupported cost (model {model}, ncost {ncost}); \
                 only polynomials of degree at most 2 are supported"
            ),
            Error::UnknownBus {
                bus_id,
                element_index,
            } => write!(
                f,
                "generator {element_index} references unknown bus {bus_id}"
            ),
        }
    }
}

impl std::error::Error for Error {}

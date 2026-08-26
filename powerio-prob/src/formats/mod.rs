//! Format to problem mappings: sources that define a particular calculation
//! parse to the corresponding instance or solution rather than to a bare
//! network.
//!
//! A format does not become a problem instance merely because it contains
//! enough parameters to construct one. DOE GO Challenge 3 JSON defines a
//! security constrained unit commitment, so it maps to [`AcScucInstance`];
//! DeepMind OPFData explicitly represents a solved AC OPF, so it maps to
//! [`AcOpfSolution`]. Extracting only the network from either value is a
//! separate transformation that reports the discarded problem data.
//!
//! [`AcScucInstance`]: crate::AcScucInstance
//! [`AcOpfSolution`]: crate::AcOpfSolution

mod goc3;
mod opfdata;

pub use goc3::parse_goc3_instance;
pub use opfdata::parse_opfdata_solution;

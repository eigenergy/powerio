//! Format to problem mappings: sources that define a particular calculation
//! parse to the corresponding instance or solution rather than to a bare
//! network.
//!
//! A format does not become a problem instance merely because it contains
//! enough parameters to construct one. DOE GO Challenge 3 JSON defines a
//! security constrained unit commitment, so it maps to [`AcScucInstance`];
//! BMOPF JSON defines a multiconductor AC OPF, so it maps to
//! [`McAcOpfInstance`]; DeepMind OPFData explicitly represents a solved AC
//! OPF, so it maps to [`AcOpfSolution`]. Extracting only the network from
//! any of these values is a separate transformation that reports the
//! discarded problem data.
//!
//! [`AcScucInstance`]: crate::AcScucInstance
//! [`McAcOpfInstance`]: crate::McAcOpfInstance
//! [`AcOpfSolution`]: crate::AcOpfSolution

mod bmopf;
mod goc3;
mod opfdata;

pub use bmopf::parse_bmopf_instance;
pub use goc3::parse_goc3_instance;
pub use opfdata::parse_opfdata_solution;

//! Complete numerical input data for power system problem families.
//!
//! A problem instance is distinct from a source network, matrix projection,
//! solver formulation, and solution. The default build provides index based DC
//! OPF and SCOPF instances and has no workspace dependency beyond `powerio`.
//! Enable `matrix` to derive sparse DC OPF operators from an assembled instance.

mod ac;
mod dc;
pub mod scopf;

#[cfg(feature = "matrix")]
pub mod matrix;

pub use ac::{
    AcBranchData, AcBusData, AcGeneratorData, AcOpfInstance, AcOpfOptions, build_ac_opf_instance,
};
pub use dc::{
    DcBranchData, DcGeneratorData, DcOpfInstance, DcOpfOptions, NodalGeneratorData, Units,
    build_dc_opf_instance,
};
pub use powerio::{DcConvention, Error, Result};
pub use scopf::{ScopfError, ScopfInstance, ScopfResult, build_scopf_instance_from_str};

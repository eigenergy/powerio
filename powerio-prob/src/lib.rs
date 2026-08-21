//! Complete numerical input data for power system problem families.
//!
//! A problem instance is distinct from a source network, matrix projection,
//! solver formulation, and solution. The default build provides index based DC
//! OPF and SCOPF instances and has no workspace dependency beyond `powerio`.
//! Enable `matrix` to derive sparse DC OPF operators from an assembled instance.

mod ac;
mod dc;
mod limits;
mod nodal;
mod reference;
pub mod scopf;

#[cfg(feature = "matrix")]
pub mod matrix;

pub use ac::{
    AcBranchData, AcBusData, AcGeneratorData, AcOpfInstance, AcOpfOptions, NodalAcGeneratorData,
    build_ac_opf_instance,
};
pub use dc::{
    DcBranchData, DcGeneratorData, DcOpfInstance, DcOpfOptions, NodalGeneratorData, Units,
    build_dc_opf_instance,
};
pub use powerio::DcConvention;

pub mod diagnostics;
pub mod error;
pub use error::{Error, Result};
pub use reference::ReferenceBuses;
#[allow(deprecated)]
pub use scopf::build_scopf_instance_from_str;
pub use scopf::{
    IndexBase, ScopfDeviceClassLayout, ScopfError, ScopfInstance, ScopfResult, parse_scopf_str,
};

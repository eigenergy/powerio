//! Numerical problem instances derived from PowerIO networks.
//!
//! The default build provides index based data and depends only on `powerio`.
//! Enable `matrix` to derive sparse operators from an instance.

mod dc;
pub mod scopf;

#[cfg(feature = "matrix")]
pub mod matrix;

pub use dc::{
    DcBranchData, DcGeneratorData, DcOpfInstance, DcOpfOptions, NodalGeneratorData, Units,
    build_dc_opf_instance,
};
pub use powerio::{DcConvention, Error, Result};
pub use scopf::{
    ScopfError, ScopfInstance, ScopfResult, build_scopf_instance, build_scopf_instance_from_str,
};

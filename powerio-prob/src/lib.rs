//! Complete numerical input data for power system problem families.
//!
//! A problem instance is distinct from a source network, matrix projection,
//! solver formulation, and solution. The default build provides index based DC
//! OPF and SCOPF instances and has no workspace dependency beyond `powerio`.
//! Enable `matrix` to derive sparse DC OPF operators from an assembled instance.

mod dc;
#[cfg(test)]
mod dcopf_tests;
pub mod instance;
mod limits;
mod nodal;
mod reference;
pub mod scopf;
pub mod solution;
pub mod state;

#[cfg(feature = "matrix")]
pub mod matrix;

/// Solver preparation data: the contiguous dense arrays a matrix builder or
/// export assembles from a network. These arrays are never public instance
/// fields — the public instances share the typed network — and the builders
/// that consume them derive them privately from an instance.
pub(crate) mod prep {
    pub(crate) use crate::dc::{DcOpfOptions, DcOpfPreparation, build_dc_opf_preparation};
}
pub use dc::Units;
pub use powerio_tx::DcConvention;

pub mod diagnostics;
pub mod error;
pub use error::{Error, Result};
pub use instance::{
    AcBusSpecification, AcOpfInstance, AcPfInstance, AcScucInstance, ActiveConstraints,
    ActiveControlMode, ConstraintSelection, DcBusSpecification, DcOpfInstance, DcPfInstance,
    McAcOpfInstance, McAcPfInstance, MulticonductorActiveConstraints, Objective, ObjectiveTerm,
    PrescribedSourceVoltage, PrescribedTerminalPower, ZeroImpedanceMerge,
    merge_zero_impedance_buses,
};
pub use reference::ReferenceBuses;
#[allow(deprecated)]
pub use scopf::build_scopf_instance_from_str;
pub use scopf::{
    IndexBase, ScopfDeviceClassLayout, ScopfError, ScopfResult, ScucInputs, parse_scopf_str,
};
pub use solution::{
    AcOpfSolution, AcPfSolution, AcScucSolution, DcOpfSolution, DcPfSolution, GeneratorDispatch,
    McAcOpfSolution, McAcPfSolution, Producer, Residuals, ScucDeviceOutputs, ScucNetworkOutputs,
    Termination,
};
pub use state::{
    BalancedOperatingPoints, BalancedStateBuilder, MulticonductorOperatingPoints,
    MulticonductorStateBuilder, OperatingPoint,
};

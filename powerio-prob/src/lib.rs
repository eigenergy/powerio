//! Complete numerical input data for power system problem families.
//!
//! A problem instance is distinct from a source network, matrix projection,
//! solver formulation, and solution. The default build provides index based DC
//! OPF and SCOPF instances and has no workspace dependency beyond `powerio`.
//! Enable `matrix` to derive sparse DC OPF operators from an assembled instance.

mod ac;
mod dc;
pub mod formats;
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
/// export assembles from a network. These arrays are never 1.0 public
/// instance fields — the public instances share the typed network — and this
/// module exists for the bundle, export, and binding surfaces that serialize
/// them, until those move onto the public instances.
#[doc(hidden)]
pub mod prep {
    pub use crate::ac::{
        AcBranchData, AcBusData, AcGeneratorData, AcOpfOptions, AcOpfPreparation,
        NodalAcGeneratorData, build_ac_opf_preparation,
    };
    pub use crate::dc::{
        DcBranchData, DcGeneratorData, DcOpfOptions, DcOpfPreparation, NodalGeneratorData, Units,
        build_dc_opf_preparation,
    };
}
pub use powerio_tx::DcConvention;
pub use prep::{AcOpfOptions, DcOpfOptions, Units};

pub mod diagnostics;
pub mod error;
pub use error::{Error, Result};
pub use formats::{
    PypsaSequence, parse_bmopf_instance, parse_goc3_instance, parse_opfdata_solution,
    parse_pypsa_sequence,
};
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
    BALANCED_STATE_QUANTITIES, BalancedOperatingPoints, BalancedStateBuilder,
    MulticonductorOperatingPoints, MulticonductorStateBuilder, OperatingPoint,
};

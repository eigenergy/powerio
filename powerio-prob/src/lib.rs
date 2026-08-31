//! Operating points, problem instances, and solutions for power system
//! calculation families.
//!
//! A problem instance is distinct from a source network, matrix projection,
//! solver formulation, and solution. This crate stays matrix free: sparse
//! operators and the DC OPF bundle writer derived from these instances live
//! in `powerio-matrix`, which depends on this crate.

pub mod formats;
pub mod instance;
mod reference;
pub(crate) mod scopf;
pub mod solution;
pub mod state;

pub use powerio_tx::BranchSusceptanceFormula;

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
    PrescribedSourceVoltage, PrescribedTerminalPower, ScucInputs, ZeroImpedanceMerge,
    merge_zero_impedance_buses,
};
pub use reference::ReferenceBuses;
pub use solution::{
    AcOpfSolution, AcPfSolution, AcScucSolution, DcOpfSolution, DcPfSolution, GeneratorDispatch,
    McAcOpfSolution, McAcPfSolution, Producer, Residuals, SCUC_DEVICE_OUTPUT_SERIES,
    SCUC_NETWORK_OUTPUT_SERIES, ScucDeviceOutputs, ScucNetworkOutputs, Termination,
};
pub use state::{
    BALANCED_STATE_QUANTITIES, BalancedOperatingPoints, BalancedStateBuilder,
    MulticonductorOperatingPoints, MulticonductorStateBuilder, OperatingPoint,
};

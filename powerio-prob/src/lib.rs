//! Operating points, problem instances, and solutions for power system
//! calculation families.
//!
//! A problem instance is distinct from a source network, matrix projection,
//! solver formulation, and solution. This crate stays matrix free: sparse
//! operators and the DC OPF bundle writer derived from these instances live
//! in `powerio-matrix`, which depends on this crate.

pub mod formats;
pub(crate) mod goc3;
pub mod instance;
pub mod operating;
mod reference;
pub mod solution;
mod update;

pub use powerio_tx::BranchSusceptanceFormula;

pub mod diagnostics;
pub mod error;
pub use error::{Error, Result};
#[doc(hidden)]
pub use formats::{
    __decode_opfdata_solution, __decode_pypsa_sequence, __emit_goc3_output,
    __parse_goc3_output_buffer, __parse_goc3_problem_buffer, PypsaSequence,
};
pub use instance::{
    AcBusSpecification, AcOpfInstance, AcPfInstance, AcScucInstance, ActiveConstraints,
    ActiveControlMode, ConstraintSelection, DcBusSpecification, DcOpfInstance, DcPfInstance,
    McAcOpfInstance, McAcPfInstance, MulticonductorActiveConstraints, Objective, ObjectiveTerm,
    PrescribedSourceVoltage, PrescribedTerminalPower, ScucActiveReserveZone,
    ScucBranchSwitchingCost, ScucContingency, ScucDevice, ScucDeviceKind, ScucDevicePeriod,
    ScucEnergyCostBlock, ScucEnergyRequirement, ScucInitialCommitment, ScucInputs, ScucRampLimits,
    ScucReactiveCapability, ScucReactiveReserveZone, ScucReserveCosts, ScucReserveLimits,
    ScucShunt, ScucStartupCostAdjustment, ScucStartupLimit, ScucTransformerControl,
    ScucViolationCosts, ZeroImpedanceMerge, merge_zero_impedance_buses,
};
pub use operating::{
    BalancedOperatingPointBuilder, BalancedOperatingPointFlag, BalancedOperatingPointQuantity,
    MulticonductorOperatingPointBuilder, MulticonductorOperatingPointFlag,
    MulticonductorOperatingPointQuantity, OperatingPoint, OperatingPointFlags,
    OperatingPointValues,
};
pub use reference::ReferenceBuses;
pub use solution::{
    AcOpfSolution, AcPfSolution, AcScucSolution, DcOpfSolution, DcPfSolution, GeneratorDispatch,
    McAcOpfSolution, McAcPfSolution, Producer, Residuals, SCUC_DEVICE_OUTPUT_SERIES,
    SCUC_NETWORK_OUTPUT_SERIES, ScucDeviceOutputs, ScucNetworkOutputs, Termination,
    ThreeWindingTransformerTerminalActivePower, ThreeWindingTransformerTerminalPower,
};
pub use update::{
    ActivePower, ActivePowerUnit, ApparentPower, ApparentPowerUnit, BalancedCalculationInstance,
    CalculationUpdate, LoadAllocation, NetworkUpdate, OperatingPointUpdate, ReactivePower,
    ReactivePowerUnit, UpdateChange, UpdateReport, UpdatedField, apply_bus_load_active_power,
    apply_updates,
};

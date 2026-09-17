//! The public calculation instances.
//!
//! A problem instance is complete typed input for one problem family,
//! distinct from the source network, a matrix projection, a solver
//! formulation, and a solution. Every instance shares its reusable
//! electrical network as a cheap owning handle — cloning an instance clones
//! no network table — and exposes a borrowed `network()` accessor. Fields
//! are private, so the sharing strategy can change without a public break.
//!
//! The eight instances are [`DcPfInstance`], [`AcPfInstance`],
//! [`DcOpfInstance`], [`AcOpfInstance`], [`McAcPfInstance`],
//! [`McAcOpfInstance`], [`LinDist3FlowOpfInstance`], and [`AcScucInstance`]. Power flow instances carry
//! partial boundary specifications; OPF instances carry typed
//! [`Objective`] terms and active constraint selections by stable element
//! identity, with the numerical limits staying on the network.

pub(crate) mod balanced;
mod constraints;
mod lindist3flow;
mod merge;
mod multiconductor;
mod objective;
mod scuc;
pub mod scuc_inputs;

pub use balanced::{
    AcBusSpecification, AcOpfInstance, AcPfInstance, DcBusSpecification, DcOpfInstance,
    DcPfInstance,
};
pub use constraints::{ActiveConstraints, ConstraintSelection, MulticonductorActiveConstraints};
pub use lindist3flow::{
    LinDist3FlowApplicability, LinDist3FlowApplicabilityStatus, LinDist3FlowBuildOptions,
    LinDist3FlowNode, LinDist3FlowOpfInstance, LinDist3FlowOrientedConductor,
    LinDist3FlowReferencePolicy, LinDist3FlowReferenceProvenance, LinDist3FlowReferenceState,
    LinDist3FlowReferenceVoltage, LinDist3FlowTopology, LinDist3FlowUnsupported,
    check_lindist3flow_applicability,
};
pub use merge::{ZeroImpedanceMerge, merge_zero_impedance_buses};
pub use multiconductor::{
    ActiveControlMode, McAcOpfInstance, McAcPfInstance, PrescribedSourceVoltage,
    PrescribedTerminalPower,
};
pub use objective::{Objective, ObjectiveTerm};
pub use powerio_dist::{
    LinDist3FlowPreparationAction, LinDist3FlowPreparationActionKind, LinDist3FlowPreparationReport,
};
pub use scuc::AcScucInstance;
pub use scuc_inputs::{
    ScucActiveReserveZone, ScucBranchSwitchingCost, ScucContingency, ScucDevice, ScucDeviceKind,
    ScucDevicePeriod, ScucEnergyCostBlock, ScucEnergyRequirement, ScucInitialCommitment,
    ScucInputs, ScucRampLimits, ScucReactiveCapability, ScucReactiveReserveZone, ScucReserveCosts,
    ScucReserveLimits, ScucShunt, ScucStartupCostAdjustment, ScucStartupLimit,
    ScucTransformerControl, ScucViolationCosts,
};

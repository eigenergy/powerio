//! The public calculation instances.
//!
//! A problem instance is complete typed input for one problem family,
//! distinct from the source network, a matrix projection, a solver
//! formulation, and a solution. Every instance shares its reusable
//! electrical network as a cheap owning handle — cloning an instance clones
//! no network table — and exposes a borrowed `network()` accessor. Fields
//! are private, so the sharing strategy can change without a public break.
//!
//! The seven instances are [`DcPfInstance`], [`AcPfInstance`],
//! [`DcOpfInstance`], [`AcOpfInstance`], [`McAcPfInstance`],
//! [`McAcOpfInstance`], and [`AcScucInstance`]. Power flow instances carry
//! partial boundary specifications; OPF instances carry typed
//! [`Objective`] terms and active constraint selections by stable element
//! identity, with the numerical limits staying on the network.

pub(crate) mod balanced;
mod constraints;
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
pub use merge::{ZeroImpedanceMerge, merge_zero_impedance_buses};
pub use multiconductor::{
    ActiveControlMode, McAcOpfInstance, McAcPfInstance, PrescribedSourceVoltage,
    PrescribedTerminalPower,
};
pub use objective::{Objective, ObjectiveTerm};
pub use scuc::AcScucInstance;
pub use scuc_inputs::ScucInputs;

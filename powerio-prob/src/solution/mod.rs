//! The public calculation solutions.
//!
//! A solution contains values keyed by stable element identity, termination
//! information, an objective where the problem has one, and numerical
//! residuals — never a solver's variable ordering, factorization, cache, or
//! internal status objects. Each solution shares the immutable instance it
//! solves through shared ownership, just as each instance shares its network:
//! one instance can carry zero, one, or several solutions from different
//! equations, solvers, settings, or initial points, and
//! `solution.clone()` duplicates neither its instance nor its network.
//!
//! The seven solutions are [`DcPfSolution`], [`AcPfSolution`],
//! [`DcOpfSolution`], [`AcOpfSolution`], [`McAcPfSolution`],
//! [`McAcOpfSolution`], and [`AcScucSolution`].

mod balanced;
mod multiconductor;
mod scuc;

pub use balanced::{AcOpfSolution, AcPfSolution, DcOpfSolution, DcPfSolution, GeneratorDispatch};
pub use multiconductor::{McAcOpfSolution, McAcPfSolution};
pub use scuc::{AcScucSolution, ScucDeviceOutputs, ScucNetworkOutputs};

use serde::{Deserialize, Serialize};

/// How the producing calculation ended.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum Termination {
    /// The calculation converged to its stated tolerance.
    Converged,
    /// The iteration limit ended the calculation before convergence.
    IterationLimit,
    /// The calculation failed.
    Failed,
    /// The source records a solved calculation without termination
    /// information (DeepMind OPFData does).
    #[default]
    NotReported,
}

/// Numerical residuals of the solved equations, in the problem's power unit.
/// A field is `None` when the producer did not report it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Residuals {
    /// Largest absolute active power balance mismatch, MW.
    pub max_active_power_mismatch: Option<f64>,
    /// Largest absolute reactive power balance mismatch, MVAr.
    pub max_reactive_power_mismatch: Option<f64>,
}

/// The producer or solver identity a solution records, free text as the
/// producer states it.
pub type Producer = Option<String>;

//! Conventions shared by DC network models and matrix builders.

use serde::{Deserialize, Serialize};

/// Electrical convention used for DC branch coefficients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DcConvention {
    /// Use `b = 1/x` and ignore transformer taps and phase shifts.
    #[default]
    PaperPure,
    /// Use `b = 1/(x tau)` and include phase shift injections, matching
    /// MATPOWER `makeBdc`.
    Matpower,
}

impl DcConvention {
    /// Compute a branch susceptance from reactance and effective tap.
    #[must_use]
    pub fn branch_susceptance(self, reactance: f64, effective_tap: f64) -> f64 {
        match self {
            Self::PaperPure => 1.0 / reactance,
            Self::Matpower => 1.0 / (reactance * effective_tap),
        }
    }

    /// Whether phase shifts contribute to the nodal injection vector.
    #[must_use]
    pub fn includes_phase_shifts(self) -> bool {
        match self {
            Self::PaperPure => false,
            Self::Matpower => true,
        }
    }
}

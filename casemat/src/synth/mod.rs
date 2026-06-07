//! Synthetic MATPOWER style cases. Stress tests the matrix builders beyond
//! the vendored matpower test set.

mod lattice;
mod pegase_like;
mod tree;

pub use lattice::generate_lattice;
pub use pegase_like::generate_pegase_like;
pub use tree::generate_tree;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Topology {
    Tree,
    Lattice2D,
    PegaseLike,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthSpec {
    pub topology: Topology,
    pub n: usize,
    /// Branch series resistance to reactance ratio.
    pub r_over_x: f64,
    /// Mean reactance per branch (p.u.).
    pub mean_x: f64,
    /// Random seed; identical seed → identical case.
    pub seed: u64,
}

impl Default for SynthSpec {
    fn default() -> Self {
        Self {
            topology: Topology::Tree,
            n: 64,
            r_over_x: 0.1,
            mean_x: 0.05,
            seed: 0xC0FFEE,
        }
    }
}

pub fn generate(spec: &SynthSpec) -> crate::network::Network {
    match spec.topology {
        Topology::Tree => generate_tree(spec),
        Topology::Lattice2D => generate_lattice(spec),
        Topology::PegaseLike => generate_pegase_like(spec),
    }
}

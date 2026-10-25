//! Deterministic synthetic network generators.
//!
//! Compatibility facade for the historical `powerio_matrix::synth` API.
//! The implementation lives in `powerio-synth`, while these public types
//! remain defined in this crate so downstream code retains its original type
//! identity, constructor signatures, defaults, and Serde representation.

use powerio_tx::BalancedNetwork;
use serde::{Deserialize, Serialize};

/// Topology produced by a synthetic network generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Topology {
    Tree,
    Lattice2D,
    PegaseLike,
}

/// Parameters for a deterministic synthetic network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthSpec {
    pub topology: Topology,
    pub n: usize,
    /// Branch series resistance to reactance ratio.
    pub r_over_x: f64,
    /// Mean reactance per branch (p.u.).
    pub mean_x: f64,
    /// Random seed; identical seed produces an identical case.
    pub seed: u64,
}

impl Default for SynthSpec {
    fn default() -> Self {
        Self {
            topology: Topology::Tree,
            n: 64,
            r_over_x: 0.1,
            mean_x: 0.05,
            seed: 0x00C0_FFEE,
        }
    }
}

fn implementation_spec(
    spec: &SynthSpec,
    topology: powerio_synth::Topology,
) -> powerio_synth::SynthSpec {
    powerio_synth::SynthSpec {
        topology,
        n: spec.n,
        r_over_x: spec.r_over_x,
        mean_x: spec.mean_x,
        seed: spec.seed,
    }
}

/// Generate a deterministic network using the requested topology.
pub fn generate(spec: &SynthSpec) -> BalancedNetwork {
    match spec.topology {
        Topology::Tree => generate_tree(spec),
        Topology::Lattice2D => generate_lattice(spec),
        Topology::PegaseLike => generate_pegase_like(spec),
    }
}

/// Generate a tree-shaped network.
pub fn generate_tree(spec: &SynthSpec) -> BalancedNetwork {
    powerio_synth::generate_tree(&implementation_spec(spec, powerio_synth::Topology::Tree))
}

/// Generate a two-dimensional lattice network.
pub fn generate_lattice(spec: &SynthSpec) -> BalancedNetwork {
    powerio_synth::generate_lattice(&implementation_spec(
        spec,
        powerio_synth::Topology::Lattice2D,
    ))
}

/// Generate a PEGASE-like synthetic network.
pub fn generate_pegase_like(spec: &SynthSpec) -> BalancedNetwork {
    powerio_synth::generate_pegase_like(&implementation_spec(
        spec,
        powerio_synth::Topology::PegaseLike,
    ))
}

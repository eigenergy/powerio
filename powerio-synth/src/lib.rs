//! Deterministic synthetic power-system network generators.
//!
//! The generators produce topology-only `powerio::BalancedNetwork` values.

mod lattice;
mod pegase_like;
mod tree;

pub use lattice::generate_lattice;
pub use pegase_like::generate_pegase_like;
pub use tree::generate_tree;

use powerio_tx::{BalancedNetwork, Branch, Bus, BusId, BusType};
use rand::RngExt;
use rand_chacha::ChaCha8Rng;
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

pub fn generate(spec: &SynthSpec) -> BalancedNetwork {
    match spec.topology {
        Topology::Tree => generate_tree(spec),
        Topology::Lattice2D => generate_lattice(spec),
        Topology::PegaseLike => generate_pegase_like(spec),
    }
}

pub(crate) fn net(name: String, buses: Vec<Bus>, branches: Vec<Branch>) -> BalancedNetwork {
    BalancedNetwork::in_memory(name, 100.0, buses, branches)
}

pub(crate) fn make_buses(n: usize) -> Vec<Bus> {
    let mut buses: Vec<Bus> = (0..n).map(|i| make_bus(i + 1)).collect();
    buses[0].kind = BusType::Ref;
    buses
}

pub(crate) fn make_bus(id: usize) -> Bus {
    Bus::new(BusId(id), BusType::Pq, 345.0)
}

pub(crate) fn make_branch(
    from: usize,
    to: usize,
    spec: &SynthSpec,
    rng: &mut ChaCha8Rng,
) -> Branch {
    let log_low = (spec.mean_x * 0.5).ln();
    let log_high = (spec.mean_x * 2.0).ln();
    let log_x: f64 = rng.random_range(log_low..log_high);
    let x = log_x.exp().max(1e-6);
    let r = spec.r_over_x * x;
    Branch::new(BusId(from), BusId(to), r, x)
}

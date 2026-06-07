//! IEEE/PEGASE-like topology: a spanning tree with extra random edges to
//! match the average degree of a typical transmission grid (~2.5).

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::network::Network;

use super::SynthSpec;
use super::tree::{make_branch, make_bus, net};

pub fn generate_pegase_like(spec: &SynthSpec) -> Network {
    let n = spec.n.max(2);
    let mut rng = ChaCha8Rng::seed_from_u64(spec.seed);

    let buses = (0..n).map(|i| make_bus(i + 1)).collect();
    let mut branches = Vec::with_capacity((n as f64 * 1.3) as usize);

    // Spanning tree backbone.
    for k in 1..n {
        let parent = rng.random_range(0..k);
        branches.push(make_branch(parent + 1, k + 1, spec, &mut rng));
    }
    // Extra ~0.3 * n cross-edges to bump avg degree.
    let extra = n / 3;
    for _ in 0..extra {
        let i = rng.random_range(0..n);
        let mut j = rng.random_range(0..n);
        if i == j {
            j = (j + 1) % n;
        }
        branches.push(make_branch(i + 1, j + 1, spec, &mut rng));
    }

    net(format!("synth_pegase_n{n}"), buses, branches)
}

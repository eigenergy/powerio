//! Random spanning tree topology.

use rand::RngExt;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use powerio_tx::BalancedNetwork;

use super::{SynthSpec, make_branch, make_buses, net};

pub fn generate_tree(spec: &SynthSpec) -> BalancedNetwork {
    let n = spec.n.max(2);
    let mut rng = ChaCha8Rng::seed_from_u64(spec.seed);
    let buses = make_buses(n);

    let mut branches = Vec::with_capacity(n - 1);
    for k in 1..n {
        let parent = rng.random_range(0..k);
        branches.push(make_branch(parent + 1, k + 1, spec, &mut rng));
    }

    net(format!("synth_tree_n{n}"), buses, branches)
}

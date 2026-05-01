//! Random spanning tree topology. Produces a singular Laplacian B' (rank n-1).

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::case::{Branch, Bus, BusType, MpcCase};

use super::SynthSpec;

pub fn generate_tree(spec: &SynthSpec) -> MpcCase {
    let n = spec.n.max(2);
    let mut rng = ChaCha8Rng::seed_from_u64(spec.seed);
    let buses: Vec<Bus> = (0..n).map(|i| make_bus(i + 1)).collect();

    // For each new node k in [1, n), connect it to a uniformly chosen ancestor.
    let mut branches = Vec::with_capacity(n - 1);
    for k in 1..n {
        let parent = rng.random_range(0..k);
        branches.push(make_branch(parent + 1, k + 1, spec, &mut rng));
    }

    MpcCase::new(format!("synth_tree_n{n}"), 100.0, buses, branches)
}

pub(crate) fn make_bus(id: usize) -> Bus {
    Bus {
        id,
        kind: BusType::Pq,
        pd: 0.0,
        qd: 0.0,
        gs: 0.0,
        bs: 0.0,
        area: 1,
        vm: 1.0,
        va: 0.0,
        base_kv: 345.0,
        zone: 1,
        vmax: 1.1,
        vmin: 0.9,
    }
}

pub(crate) fn make_branch(
    from_id: usize,
    to_id: usize,
    spec: &SynthSpec,
    rng: &mut ChaCha8Rng,
) -> Branch {
    // Log-uniform reactance around mean_x; resistance = r_over_x * x.
    let log_low = (spec.mean_x * 0.5).ln();
    let log_high = (spec.mean_x * 2.0).ln();
    let log_x: f64 = rng.random_range(log_low..log_high);
    let x = log_x.exp().max(1e-6);
    let r = spec.r_over_x * x;
    Branch {
        from_id,
        to_id,
        r,
        x,
        b: 0.0,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap: 0.0,
        shift: 0.0,
        status: 1.0,
        angmin: -360.0,
        angmax: 360.0,
    }
}

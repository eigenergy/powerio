//! Random spanning tree topology. Produces a singular Laplacian B' (rank n-1).

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::network::{Branch, Bus, BusId, BusType, Extras, Network};

use super::SynthSpec;

pub fn generate_tree(spec: &SynthSpec) -> Network {
    let n = spec.n.max(2);
    let mut rng = ChaCha8Rng::seed_from_u64(spec.seed);
    let buses = make_buses(n);

    // For each new node k in [1, n), connect it to a uniformly chosen ancestor.
    let mut branches = Vec::with_capacity(n - 1);
    for k in 1..n {
        let parent = rng.random_range(0..k);
        branches.push(make_branch(parent + 1, k + 1, spec, &mut rng));
    }

    net(format!("synth_tree_n{n}"), buses, branches)
}

/// Wrap synthesized buses and branches into an in-memory [`Network`]: no loads,
/// shunts, generators, or source document.
pub(super) fn net(name: String, buses: Vec<Bus>, branches: Vec<Branch>) -> Network {
    Network::in_memory(name, 100.0, buses, branches)
}

/// Build `n` synthetic buses with ids `1..=n` and bus 1 designated reference.
/// Every synthetic case needs exactly one reference bus, or the DC sensitivity
/// and DC-OPF paths reject it with `ReferenceBusCount { found: 0 }`. Centralized
/// here so each topology generator can't forget it. Requires `n >= 1`.
pub(crate) fn make_buses(n: usize) -> Vec<Bus> {
    let mut buses: Vec<Bus> = (0..n).map(|i| make_bus(i + 1)).collect();
    buses[0].kind = BusType::Ref;
    buses
}

pub(crate) fn make_bus(id: usize) -> Bus {
    Bus {
        id: BusId(id),
        kind: BusType::Pq,
        vm: 1.0,
        va: 0.0,
        base_kv: 345.0,
        vmax: 1.1,
        vmin: 0.9,
        evhi: None,
        evlo: None,
        area: 1,
        zone: 1,
        name: None,
        extras: Extras::new(),
    }
}

pub(crate) fn make_branch(
    from: usize,
    to: usize,
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
        from: BusId(from),
        to: BusId(to),
        r,
        x,
        b: 0.0,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap: 0.0,
        shift: 0.0,
        in_service: true,
        angmin: -360.0,
        angmax: 360.0,
        control: None,
        extras: Extras::new(),
    }
}

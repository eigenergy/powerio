//! Structural regression tests shared by all synthetic topology generators.

use std::collections::{HashMap, HashSet};

use powerio_synth::{SynthSpec, Topology, generate};
use powerio_tx::{BusId, BusType};

fn spec(topology: Topology, n: usize, seed: u64) -> SynthSpec {
    SynthSpec {
        topology,
        n,
        r_over_x: 0.1,
        mean_x: 0.05,
        seed,
    }
}

fn expected_dimensions(topology: Topology, requested_n: usize) -> (usize, usize) {
    match topology {
        Topology::Tree => {
            let n = requested_n.max(2);
            (n, n - 1)
        }
        Topology::Lattice2D => {
            let side = ((requested_n as f64).sqrt().ceil() as usize).max(2);
            (side * side, 2 * side * (side - 1))
        }
        Topology::PegaseLike => {
            let n = requested_n.max(2);
            (n, n - 1 + n / 3)
        }
    }
}

fn assert_valid_connected_network(topology: Topology, requested_n: usize, seed: u64) {
    let network = generate(&spec(topology, requested_n, seed));
    let buses = network.buses();
    let branches = network.branches();
    let (expected_bus_count, expected_branch_count) = expected_dimensions(topology, requested_n);

    assert_eq!(buses.len(), expected_bus_count, "{topology:?}: bus count");
    assert_eq!(
        branches.len(),
        expected_branch_count,
        "{topology:?}: branch count"
    );

    let bus_ids: HashSet<BusId> = buses.iter().map(|bus| bus.id).collect();
    assert_eq!(
        bus_ids.len(),
        buses.len(),
        "{topology:?}: duplicate bus IDs"
    );
    assert_eq!(
        buses.iter().filter(|bus| bus.kind == BusType::Ref).count(),
        1,
        "{topology:?}: expected exactly one reference bus"
    );
    assert_eq!(
        buses.first().map(|bus| bus.kind),
        Some(BusType::Ref),
        "{topology:?}: first bus must be the reference bus"
    );

    let mut adjacency: HashMap<BusId, Vec<BusId>> =
        bus_ids.iter().copied().map(|id| (id, Vec::new())).collect();

    for (index, branch) in branches.iter().enumerate() {
        assert!(
            bus_ids.contains(&branch.from),
            "{topology:?}: branch {index} has unknown from-bus {}",
            branch.from
        );
        assert!(
            bus_ids.contains(&branch.to),
            "{topology:?}: branch {index} has unknown to-bus {}",
            branch.to
        );
        assert_ne!(
            branch.from, branch.to,
            "{topology:?}: branch {index} is a self-loop"
        );
        assert!(
            branch.r.is_finite() && branch.r > 0.0,
            "{topology:?}: branch {index} has invalid resistance {}",
            branch.r
        );
        assert!(
            branch.x.is_finite() && branch.x > 0.0,
            "{topology:?}: branch {index} has invalid reactance {}",
            branch.x
        );

        adjacency.get_mut(&branch.from).unwrap().push(branch.to);
        adjacency.get_mut(&branch.to).unwrap().push(branch.from);
    }

    let start = buses.first().expect("generator must return buses").id;
    let mut visited = HashSet::from([start]);
    let mut pending = vec![start];
    while let Some(bus) = pending.pop() {
        for neighbour in &adjacency[&bus] {
            if visited.insert(*neighbour) {
                pending.push(*neighbour);
            }
        }
    }
    assert_eq!(
        visited.len(),
        buses.len(),
        "{topology:?}: generated network is disconnected"
    );
}

#[test]
fn every_topology_obeys_structural_invariants() {
    for topology in [Topology::Tree, Topology::Lattice2D, Topology::PegaseLike] {
        for requested_n in [1, 2, 6, 17, 64] {
            assert_valid_connected_network(topology, requested_n, 0x00C0_FFEE);
        }
    }
}

#[test]
fn identical_specs_produce_identical_networks() {
    for topology in [Topology::Tree, Topology::Lattice2D, Topology::PegaseLike] {
        let spec = spec(topology, 17, 0xA11C_E55);
        let first = generate(&spec);
        let second = generate(&spec);

        let first_buses: Vec<_> = first.buses().iter().map(|bus| (bus.id, bus.kind)).collect();
        let second_buses: Vec<_> = second
            .buses()
            .iter()
            .map(|bus| (bus.id, bus.kind))
            .collect();
        assert_eq!(
            first_buses, second_buses,
            "{topology:?}: bus sequence changed for an identical spec"
        );
        assert_eq!(first.branches().len(), second.branches().len());
        for (index, (left, right)) in first.branches().iter().zip(second.branches()).enumerate() {
            assert_eq!(
                (left.from, left.to, left.r.to_bits(), left.x.to_bits()),
                (right.from, right.to, right.r.to_bits(), right.x.to_bits()),
                "{topology:?}: branch {index} changed for an identical spec"
            );
        }
    }
}

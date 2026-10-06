//! Calculation instances require an explicit source on each physical bus
//! island. This does not assert phase-by-phase energization or matrix rank.

use powerio_dist::{
    Configuration, DistBus, DistLoad, DistShunt, DistSwitch, MulticonductorNetwork, VoltageSource,
};
use powerio_prob::{McAcOpfInstance, McAcPfInstance};

fn terms(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn network(open: bool) -> MulticonductorNetwork {
    let mut net = MulticonductorNetwork::new();
    let mut load_bus = DistBus::new("load", terms(&["1", "0"]));
    load_bus.grounded.push("0".into());
    net.buses_mut()
        .extend([DistBus::new("grid", terms(&["1"])), load_bus]);
    net.sources_mut().push(VoltageSource::new(
        "grid-source",
        "grid",
        terms(&["1"]),
        vec![230.0],
        vec![0.0],
    ));
    net.loads_mut().push(DistLoad::new(
        "demand",
        "load",
        terms(&["1", "0"]),
        Configuration::SinglePhase,
        vec![1000.0],
        vec![200.0],
    ));
    net.switches_mut().push(DistSwitch::new(
        "terminal",
        "grid",
        "load",
        terms(&["1"]),
        terms(&["1"]),
        open,
    ));
    net
}

fn require_rejection(net: &MulticonductorNetwork, bus: &str) {
    for error in [
        McAcPfInstance::from_network(net.clone()).unwrap_err(),
        McAcOpfInstance::from_network(net.clone()).unwrap_err(),
    ] {
        let message = error.to_string();
        assert!(
            message.contains(bus) && message.contains("has no voltage source"),
            "{message}"
        );
    }
}

#[test]
fn open_terminal_refuses_uncovered_island_without_deleting_or_zeroing_load() {
    let net = network(true);
    let original = serde_json::to_value(&net).unwrap();
    powerio_dist::require_electrical_readiness(&net).unwrap();
    require_rejection(&net, "load");
    assert_eq!(serde_json::to_value(&net).unwrap(), original);
    let closed = network(false);
    let pf = McAcPfInstance::from_network(closed.clone()).unwrap();
    assert_eq!(pf.loads()[0].p_w, [1000.0]);
    assert_eq!(pf.network().loads().as_ptr(), closed.loads().as_ptr());
    assert!(McAcOpfInstance::from_network(closed).is_ok());
}

#[test]
fn network_replacement_rechecks_newly_open_terminals() {
    let pf = McAcPfInstance::from_network(network(false)).unwrap();
    let opf = McAcOpfInstance::from_network(network(false)).unwrap();
    assert!(pf.with_network(network(true)).is_err());
    assert!(opf.with_network(network(true)).is_err());
}

#[test]
fn independent_islands_each_accept_their_own_source() {
    let mut net = network(true);
    net.sources_mut().push(VoltageSource::new(
        "local-source",
        "LOAD",
        terms(&["1"]),
        vec![220.0],
        vec![0.1],
    ));
    assert!(McAcPfInstance::from_network(net.clone()).is_ok());
    assert!(McAcOpfInstance::from_network(net).is_ok());
}

#[test]
fn passive_or_empty_islands_require_explicit_policy_too() {
    let mut net = network(true);
    net.loads_mut().clear();
    require_rejection(&net, "load");
    // A grounded coordinate is a voltage reference for that coordinate,
    // but does not establish a source on the island's other conductors.
    net.shunts_mut().push(DistShunt::new(
        "passive",
        "load",
        terms(&["1"]),
        vec![vec![0.01]],
        vec![vec![0.0]],
    ));
    require_rejection(&net, "load");
}

#[test]
fn coupled_shunt_auxiliary_bus_and_closed_ports_share_source_coverage() {
    let mut net = network(false);
    net.switches_mut().clear();
    net.buses_mut()
        .push(DistBus::new("internal", terms(&["p", "s"])));
    net.shunts_mut().push(DistShunt::new(
        "coupled",
        "internal",
        terms(&["p", "s"]),
        vec![vec![1.0, -1.0], vec![-1.0, 1.0]],
        vec![vec![0.0; 2]; 2],
    ));
    net.switches_mut().extend([
        DistSwitch::new(
            "primary",
            "GRID",
            "internal",
            terms(&["1"]),
            terms(&["p"]),
            false,
        ),
        DistSwitch::new(
            "secondary",
            "internal",
            "load",
            terms(&["s"]),
            terms(&["1"]),
            false,
        ),
    ]);
    assert!(McAcPfInstance::from_network(net.clone()).is_ok());
    assert!(McAcOpfInstance::from_network(net.clone()).is_ok());
    net.switches_mut()[1].open = true;
    require_rejection(&net, "load");
}

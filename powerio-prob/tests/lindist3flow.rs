use powerio_dist::{
    Configuration, DistBus, DistCapacitor, DistGenerator, DistIbr, DistLine, DistLineCode,
    DistLoadVoltageModel, DistSwitch, IbrPrimeMover, IbrTopology,
    LinDist3FlowPreparationActionKind, MulticonductorNetwork, NeutralKronOptions, VoltageSource,
    neutral_kron_reduce,
};
use powerio_prob::{
    LinDist3FlowBuildOptions, LinDist3FlowOpfInstance, LinDist3FlowPfInstance,
    LinDist3FlowReferencePolicy, LinDist3FlowReferenceProvenance, LinDist3FlowUnsupported,
    McAcOpfInstance, MulticonductorOperatingPointBuilder, Objective,
    check_lindist3flow_applicability,
};

fn terminals(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn three_phase_network(reverse_line: bool) -> MulticonductorNetwork {
    let phases = terminals(&["1", "2", "3"]);
    let mut network = MulticonductorNetwork::named("radial");
    network
        .buses_mut()
        .push(DistBus::new("source", phases.clone()));
    network
        .buses_mut()
        .push(DistBus::new("load", phases.clone()));
    network.line_codes_mut().push(DistLineCode::new(
        "three-phase",
        vec![
            vec![0.4, 0.05, 0.05],
            vec![0.05, 0.4, 0.05],
            vec![0.05, 0.05, 0.4],
        ],
        vec![
            vec![0.3, 0.02, 0.02],
            vec![0.02, 0.3, 0.02],
            vec![0.02, 0.02, 0.3],
        ],
    ));
    let (from, to) = if reverse_line {
        ("load", "source")
    } else {
        ("source", "load")
    };
    network.lines_mut().push(DistLine::new(
        "feeder",
        from,
        to,
        phases.clone(),
        phases.clone(),
        "three-phase",
        100.0,
    ));
    network.sources_mut().push(VoltageSource::new(
        "grid",
        "source",
        phases,
        vec![230.0; 3],
        vec![
            0.0,
            -2.0 * std::f64::consts::PI / 3.0,
            2.0 * std::f64::consts::PI / 3.0,
        ],
    ));
    network
}

fn one_phase_mesh_with_lines(lines: &[(&str, &str, &str)]) -> MulticonductorNetwork {
    let terminal = terminals(&["1"]);
    let mut network = MulticonductorNetwork::named("mesh");
    for bus in ["a", "b", "c"] {
        network
            .buses_mut()
            .push(DistBus::new(bus, terminal.clone()));
    }
    network
        .line_codes_mut()
        .push(DistLineCode::new("one", vec![vec![0.1]], vec![vec![0.1]]));
    for &(name, from, to) in lines {
        network.lines_mut().push(DistLine::new(
            name,
            from,
            to,
            terminal.clone(),
            terminal.clone(),
            "one",
            1.0,
        ));
    }
    network.sources_mut().push(VoltageSource::new(
        "grid",
        "a",
        terminal,
        vec![230.0],
        vec![0.0],
    ));
    network
}

fn one_phase_mesh() -> MulticonductorNetwork {
    one_phase_mesh_with_lines(&[("ab", "a", "b"), ("bc", "b", "c"), ("ca", "c", "a")])
}

fn four_wire_network() -> MulticonductorNetwork {
    let wires = terminals(&["1", "2", "3", "4"]);
    let mut network = MulticonductorNetwork::named("four-wire");
    for id in ["source", "load"] {
        let mut bus = DistBus::new(id, wires.clone());
        bus.grounded.push("4".to_owned());
        network.buses_mut().push(bus);
    }
    network.line_codes_mut().push(DistLineCode::new(
        "four",
        vec![
            vec![0.4, 0.02, 0.02, 0.1],
            vec![0.02, 0.4, 0.02, 0.1],
            vec![0.02, 0.02, 0.4, 0.1],
            vec![0.1, 0.1, 0.1, 0.5],
        ],
        vec![
            vec![0.3, 0.01, 0.01, 0.05],
            vec![0.01, 0.3, 0.01, 0.05],
            vec![0.01, 0.01, 0.3, 0.05],
            vec![0.05, 0.05, 0.05, 0.2],
        ],
    ));
    network.lines_mut().push(DistLine::new(
        "feeder",
        "source",
        "load",
        wires.clone(),
        wires.clone(),
        "four",
        10.0,
    ));
    network.sources_mut().push(VoltageSource::new(
        "grid",
        "source",
        wires,
        vec![230.0, 230.0, 230.0, 0.0],
        vec![0.0, -2.094, 2.094, 0.0],
    ));
    network
}

#[test]
fn source_propagated_reference_and_conductor_forest_are_stable() {
    let instance = LinDist3FlowOpfInstance::from_network(
        three_phase_network(false),
        LinDist3FlowBuildOptions::default(),
    )
    .unwrap();

    assert!(instance.applicability().is_applicable());
    assert_eq!(instance.topology().nodes.len(), 6);
    assert_eq!(instance.topology().conductors.len(), 3);
    assert_eq!(instance.topology().roots.len(), 3);
    assert_eq!(instance.topology().islands.len(), 3);
    assert_eq!(
        instance.reference().provenance,
        LinDist3FlowReferenceProvenance::SourcePropagated
    );
    for terminal in ["1", "2", "3"] {
        let source = instance.reference().voltage("source", terminal).unwrap();
        let load = instance.reference().voltage("load", terminal).unwrap();
        assert!((source.magnitude - load.magnitude).abs() < f64::EPSILON);
        assert!((source.angle - load.angle).abs() < f64::EPSILON);
    }
}

#[test]
fn topology_is_oriented_from_the_source_not_the_input_line_direction() {
    let instance = LinDist3FlowOpfInstance::from_network(
        three_phase_network(true),
        LinDist3FlowBuildOptions::default(),
    )
    .unwrap();

    assert!(
        instance.topology().conductors.iter().all(|edge| {
            edge.parent.bus == "source" && edge.child.bus == "load" && edge.reversed
        })
    );
}

#[test]
fn auto_uses_a_complete_initial_voltage_point_when_present() {
    let network = three_phase_network(false);
    let point = MulticonductorOperatingPointBuilder::for_point(network.clone())
        .terminal_voltage_magnitudes(vec![231.0, 232.0, 233.0, 221.0, 222.0, 223.0])
        .terminal_voltage_angles(vec![0.01, -2.08, 2.10, 0.02, -2.07, 2.11])
        .build_point()
        .unwrap();
    let base = McAcOpfInstance::from_network(network)
        .unwrap()
        .with_initial_point(point);
    let instance =
        LinDist3FlowOpfInstance::from_mc_ac(base, LinDist3FlowBuildOptions::default()).unwrap();

    assert_eq!(
        instance.reference().provenance,
        LinDist3FlowReferenceProvenance::InitialPoint
    );
    assert!(
        (instance.reference().voltage("load", "2").unwrap().magnitude - 222.0).abs() < f64::EPSILON
    );
}

#[test]
fn explicit_policy_requires_an_initial_voltage_point() {
    let options = LinDist3FlowBuildOptions::default()
        .with_reference_policy(LinDist3FlowReferencePolicy::Explicit);
    let error =
        LinDist3FlowOpfInstance::from_network(three_phase_network(false), options).unwrap_err();

    assert_eq!(
        error.info().map(|info| info.code),
        Some("BUILD.LINDIST3FLOW.REFERENCE_INVALID")
    );
}

#[test]
fn kron_reduced_network_satisfies_the_optional_provenance_gate() {
    let reduction =
        neutral_kron_reduce(&four_wire_network(), &NeutralKronOptions::default()).unwrap();
    let options = LinDist3FlowBuildOptions::default().with_required_neutral_provenance(true);
    let instance =
        LinDist3FlowOpfInstance::from_network(reduction.network().clone(), options).unwrap();

    assert!(instance.applicability().kron_reduced);
    assert!(
        instance
            .network()
            .buses()
            .iter()
            .all(|bus| bus.terminals.len() == 3)
    );
}

#[test]
fn a_conductor_cycle_is_retained_and_reported_as_an_approximation() {
    let instance = LinDist3FlowOpfInstance::from_network(
        one_phase_mesh(),
        LinDist3FlowBuildOptions::default(),
    )
    .unwrap();

    assert!(instance.applicability().is_applicable());
    assert!(instance.topology().meshed);
    assert_eq!(instance.topology().conductors.len(), 3);
    assert_eq!(instance.topology().roots.len(), 1);
    assert_eq!(instance.reference().voltages.len(), 3);
    assert!(
        instance
            .applicability()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == "BUILD.LINDIST3FLOW.MESH_APPROXIMATION")
    );
}

#[test]
fn mesh_orientation_is_stable_under_line_reversal_and_reordering() {
    let forward = LinDist3FlowOpfInstance::from_network(
        one_phase_mesh_with_lines(&[("ab", "a", "b"), ("bc", "b", "c"), ("ca", "c", "a")]),
        LinDist3FlowBuildOptions::default(),
    )
    .unwrap();
    let changed = LinDist3FlowOpfInstance::from_network(
        one_phase_mesh_with_lines(&[("ca", "a", "c"), ("bc", "c", "b"), ("ab", "b", "a")]),
        LinDist3FlowBuildOptions::default(),
    )
    .unwrap();

    let signature = |instance: &LinDist3FlowOpfInstance| {
        let mut edges = instance
            .topology()
            .conductors
            .iter()
            .map(|edge| {
                (
                    edge.line.clone(),
                    edge.parent.bus.clone(),
                    edge.child.bus.clone(),
                )
            })
            .collect::<Vec<_>>();
        edges.sort();
        edges
    };
    assert_eq!(signature(&forward), signature(&changed));
}

#[test]
fn parallel_lines_remain_distinct() {
    let instance = LinDist3FlowOpfInstance::from_network(
        one_phase_mesh_with_lines(&[
            ("first", "a", "b"),
            ("second", "a", "b"),
            ("tail", "b", "c"),
        ]),
        LinDist3FlowBuildOptions::default(),
    )
    .unwrap();

    assert!(instance.topology().meshed);
    assert_eq!(instance.topology().conductors.len(), 3);
    assert_eq!(
        instance
            .topology()
            .conductors
            .iter()
            .filter(|edge| edge.line != "tail")
            .map(|edge| edge.line.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
}

#[test]
fn multiple_source_records_on_one_physical_island_are_rejected() {
    let phases = terminals(&["1", "2"]);
    let mut network = MulticonductorNetwork::named("two-source");
    network
        .buses_mut()
        .push(DistBus::new("source", phases.clone()));
    network
        .buses_mut()
        .push(DistBus::new("load", phases.clone()));
    network.line_codes_mut().push(DistLineCode::new(
        "two-phase",
        vec![vec![0.4, 0.05], vec![0.05, 0.4]],
        vec![vec![0.3, 0.02], vec![0.02, 0.3]],
    ));
    network.lines_mut().push(DistLine::new(
        "feeder",
        "source",
        "load",
        phases.clone(),
        phases,
        "two-phase",
        10.0,
    ));
    network.sources_mut().push(VoltageSource::new(
        "first-grid",
        "source",
        terminals(&["1"]),
        vec![230.0],
        vec![0.0],
    ));
    network.sources_mut().push(VoltageSource::new(
        "second-grid",
        "source",
        terminals(&["2"]),
        vec![230.0],
        vec![-2.0 * std::f64::consts::PI / 3.0],
    ));
    let error = LinDist3FlowOpfInstance::from_network(network, LinDist3FlowBuildOptions::default())
        .unwrap_err();

    assert_eq!(
        error.info().map(|info| info.code),
        Some("BUILD.LINDIST3FLOW.TOPOLOGY_INVALID")
    );
}

#[test]
fn strict_slice_reports_unsupported_components_and_load_models() {
    let mut network = three_phase_network(false);
    network.switches_mut().push(DistSwitch::new(
        "tie",
        "source",
        "load",
        terminals(&["1"]),
        terminals(&["1"]),
        true,
    ));
    let mut load = powerio_dist::DistLoad::new(
        "demand",
        "load",
        terminals(&["1", "2", "3"]),
        Configuration::Wye,
        vec![1_000.0; 3],
        vec![200.0; 3],
    );
    load.voltage_model = DistLoadVoltageModel::ConstantCurrent {
        v_nom: vec![230.0; 3],
    };
    network.loads_mut().push(load);
    let base = McAcOpfInstance::from_network(network).unwrap();
    let report = check_lindist3flow_applicability(&base, LinDist3FlowBuildOptions::default());

    assert!(!report.is_applicable());
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code() == "BUILD.LINDIST3FLOW.UNSUPPORTED_COMPONENT"
            })
            .count(),
        2
    );
}

#[test]
fn lower_policy_prepares_static_switches_and_capacitors_without_mutating_source() {
    let mut network = three_phase_network(false);
    let mut switch = DistSwitch::new(
        "tie",
        "source",
        "load",
        terminals(&["1"]),
        terminals(&["1"]),
        false,
    );
    switch.i_max = Some(vec![100.0]);
    network.switches_mut().push(switch);
    network.capacitors_mut().push(DistCapacitor::new(
        "bank",
        "load",
        terminals(&["1"]),
        Configuration::Wye,
        1_000.0,
        230.0,
    ));
    let options =
        LinDist3FlowBuildOptions::default().with_unsupported(LinDist3FlowUnsupported::Lower);
    let instance = LinDist3FlowOpfInstance::from_network(network, options).unwrap();

    assert_eq!(instance.source_network().switches().len(), 1);
    assert_eq!(instance.source_network().capacitors().len(), 1);
    assert!(instance.network().switches().is_empty());
    assert!(instance.network().capacitors().is_empty());
    assert!(instance.applicability().lowered);
    assert!(
        instance.preparation().actions.iter().any(|action| {
            action.kind == LinDist3FlowPreparationActionKind::ClosedSwitchLowered
        })
    );
    assert!(
        instance
            .preparation()
            .actions
            .iter()
            .any(|action| { action.kind == LinDist3FlowPreparationActionKind::CapacitorLowered })
    );
}

#[test]
fn approximate_policy_prepares_current_loads_and_static_ibrs() {
    let mut network = three_phase_network(false);
    let mut load = powerio_dist::DistLoad::new(
        "demand",
        "load",
        terminals(&["1"]),
        Configuration::Wye,
        vec![100.0],
        vec![20.0],
    );
    load.voltage_model = DistLoadVoltageModel::ConstantCurrent { v_nom: vec![230.0] };
    network.loads_mut().push(load);
    let mut ibr = DistIbr::new(
        "pv",
        "load",
        terminals(&["2"]),
        IbrTopology::SinglePhase,
        IbrPrimeMover::Pv,
        vec![500.0],
    );
    ibr.p_avail = Some(400.0);
    network.ibrs_mut().push(ibr);
    let options =
        LinDist3FlowBuildOptions::default().with_unsupported(LinDist3FlowUnsupported::Approximate);
    let instance = LinDist3FlowOpfInstance::from_network(network, options).unwrap();

    assert!(instance.network().ibrs().is_empty());
    assert_eq!(instance.network().generators().len(), 1);
    assert!(
        instance
            .preparation()
            .actions
            .iter()
            .any(|action| { action.kind == LinDist3FlowPreparationActionKind::LoadApproximated })
    );
    assert!(
        instance
            .preparation()
            .actions
            .iter()
            .any(|action| { action.kind == LinDist3FlowPreparationActionKind::IbrApproximated })
    );
}

#[test]
fn provenance_gate_rejects_an_unprojected_three_wire_network() {
    let options = LinDist3FlowBuildOptions::default().with_required_neutral_provenance(true);
    let error =
        LinDist3FlowOpfInstance::from_network(three_phase_network(false), options).unwrap_err();

    assert_eq!(
        error.info().map(|info| info.code),
        Some("BUILD.LINDIST3FLOW.EXPLICIT_NEUTRAL")
    );
}

#[test]
fn unsupported_objectives_are_rejected_during_applicability() {
    let base = McAcOpfInstance::from_network(three_phase_network(false))
        .unwrap()
        .with_objective(Objective::network_generator_cost());
    let report = check_lindist3flow_applicability(&base, LinDist3FlowBuildOptions::default());

    assert!(!report.is_applicable());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code() == "BUILD.LINDIST3FLOW.OBJECTIVE_UNSUPPORTED" })
    );
    let error =
        LinDist3FlowOpfInstance::from_mc_ac(base, LinDist3FlowBuildOptions::default()).unwrap_err();
    assert_eq!(
        error.info().map(|info| info.code),
        Some("BUILD.LINDIST3FLOW.OBJECTIVE_UNSUPPORTED")
    );
}

#[test]
fn absent_dispatch_costs_are_visible_nonblocking_findings() {
    let mut network = three_phase_network(false);
    network.generators_mut().push(DistGenerator::new(
        "pv",
        "load",
        terminals(&["1", "2", "3"]),
        Configuration::Wye,
        vec![100.0; 3],
        vec![0.0; 3],
    ));
    let instance =
        LinDist3FlowOpfInstance::from_network(network, LinDist3FlowBuildOptions::default())
            .unwrap();
    let missing = instance
        .applicability()
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code() == "BUILD.LINDIST3FLOW.COST_MISSING")
        .collect::<Vec<_>>();

    assert_eq!(missing.len(), 2);
    assert!(instance.applicability().is_applicable());
    assert!(
        missing
            .iter()
            .all(|diagnostic| diagnostic.severity() == powerio_core::DiagnosticSeverity::Warning)
    );
}

#[test]
fn incomplete_generator_bounds_fail_before_numerical_preparation() {
    let mut network = three_phase_network(false);
    let mut generator = DistGenerator::new(
        "pv",
        "load",
        terminals(&["1", "2", "3"]),
        Configuration::Wye,
        vec![100.0; 3],
        vec![0.0; 3],
    );
    generator.p_min = Some(vec![0.0; 3]);
    generator.p_max = Some(vec![200.0; 3]);
    generator.q_min = Some(vec![-50.0; 3]);
    network.generators_mut().push(generator);
    let base = McAcOpfInstance::from_network(network).unwrap();
    let report = check_lindist3flow_applicability(&base, LinDist3FlowBuildOptions::default());

    assert!(!report.is_applicable());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == "BUILD.LINDIST3FLOW.DEVICE_INVALID")
    );
}

#[test]
fn fixed_dispatch_has_zero_objective_and_monitors_conductor_limits() {
    let mut network = three_phase_network(false);
    network.lines_mut()[0].i_max = Some(vec![100.0; 3]);
    network.generators_mut().push(DistGenerator::new(
        "pv",
        "load",
        terminals(&["1", "2", "3"]),
        Configuration::Wye,
        vec![100.0; 3],
        vec![0.0; 3],
    ));
    let instance =
        LinDist3FlowPfInstance::from_network(network, LinDist3FlowBuildOptions::default()).unwrap();

    assert!(
        instance
            .formulation()
            .base_instance()
            .objective()
            .terms()
            .is_empty()
    );
    assert_eq!(
        instance
            .formulation()
            .base_instance()
            .constraints()
            .conductor_limits,
        powerio_prob::ConstraintSelection::None
    );
    assert_eq!(
        instance
            .formulation()
            .base_instance()
            .constraints()
            .generator_capability,
        powerio_prob::ConstraintSelection::All
    );
}

#[test]
fn fixed_dispatch_rejects_a_dispatch_range() {
    let mut network = three_phase_network(false);
    let mut generator = DistGenerator::new(
        "pv",
        "load",
        terminals(&["1"]),
        Configuration::Wye,
        vec![100.0],
        vec![0.0],
    );
    generator.p_min = Some(vec![0.0]);
    generator.p_max = Some(vec![200.0]);
    network.generators_mut().push(generator);

    let error = LinDist3FlowPfInstance::from_network(network, LinDist3FlowBuildOptions::default())
        .unwrap_err();
    assert_eq!(
        error.info().map(|info| info.code),
        Some("BUILD.LINDIST3FLOW.FIXED_DISPATCH_REQUIRED")
    );
}

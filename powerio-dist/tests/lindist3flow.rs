use powerio_dist::{
    Configuration, DistBus, DistCapacitor, DistIbr, DistLine, DistLineCode, DistLoad,
    DistLoadVoltageModel, DistSwitch, IbrPrimeMover, IbrTopology,
    LinDist3FlowPreparationActionKind, LinDist3FlowPreparationPolicy, MulticonductorNetwork,
    UntypedObject, VoltageSource, prepare_lindist3flow_network,
};

fn terminals(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn network() -> MulticonductorNetwork {
    let terminal = terminals(&["1"]);
    let mut network = MulticonductorNetwork::named("preparation");
    for bus in ["source", "load"] {
        network
            .buses_mut()
            .push(DistBus::new(bus, terminal.clone()));
    }
    network.line_codes_mut().push(DistLineCode::new(
        "linecode",
        vec![vec![0.1]],
        vec![vec![0.05]],
    ));
    network.lines_mut().push(DistLine::new(
        "line",
        "source",
        "load",
        terminal.clone(),
        terminal.clone(),
        "linecode",
        10.0,
    ));
    network.sources_mut().push(VoltageSource::new(
        "grid",
        "source",
        terminal,
        vec![230.0],
        vec![0.0],
    ));
    network
}

#[test]
fn switch_contacts_keep_independent_flows_and_limits() {
    let mut source = network();
    for (name, open, limit) in [
        ("first", false, Some(10.0)),
        ("second", false, Some(20.0)),
        ("open", true, None),
    ] {
        let mut switch = DistSwitch::new(
            name,
            "source",
            "load",
            terminals(&["1"]),
            terminals(&["1"]),
            open,
        );
        switch.i_max = limit.map(|value| vec![value]);
        source.switches_mut().push(switch);
    }

    let prepared =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Lower).unwrap();
    let contact_lines = prepared
        .network()
        .lines()
        .iter()
        .filter(|line| line.name.starts_with("__l3f-switch-"))
        .collect::<Vec<_>>();

    assert_eq!(source.switches().len(), 3, "the input remains unchanged");
    assert_eq!(prepared.network().switches().as_slice(), []);
    assert_eq!(contact_lines.len(), 2);
    assert_eq!(contact_lines[0].i_max.as_deref(), Some(&[10.0][..]));
    assert_eq!(contact_lines[1].i_max.as_deref(), Some(&[20.0][..]));
    assert_eq!(
        prepared
            .report()
            .actions
            .iter()
            .filter(|action| {
                action.kind == LinDist3FlowPreparationActionKind::ClosedSwitchLowered
            })
            .count(),
        2
    );
    assert!(
        prepared
            .report()
            .actions
            .iter()
            .any(|action| { action.kind == LinDist3FlowPreparationActionKind::OpenSwitchOmitted })
    );
}

#[test]
fn line_charging_and_capacitors_become_explicit_shunts() {
    let mut source = network();
    source.line_codes_mut()[0].b_from = vec![vec![0.001]];
    source.capacitors_mut().push(DistCapacitor::new(
        "bank",
        "load",
        terminals(&["1"]),
        Configuration::Wye,
        1_000.0,
        230.0,
    ));

    let prepared =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Lower).unwrap();

    let shunt = |prefix: &str| {
        prepared
            .network()
            .shunts()
            .iter()
            .find(|shunt| shunt.name.starts_with(prefix))
            .unwrap_or_else(|| panic!("no `{prefix}` shunt"))
    };
    assert_eq!(prepared.network().shunts().len(), 2);
    assert_eq!(prepared.network().capacitors().as_slice(), []);
    assert_eq!(prepared.network().line_codes()[0].b_from, vec![vec![0.0]]);
    assert_eq!(
        shunt("__l3f-line-").b,
        vec![vec![0.01]],
        "per-metre charging is scaled by line length"
    );
    assert_eq!(
        shunt("__l3f-capacitor-").b,
        vec![vec![1_000.0 / 230.0f64.powi(2)]]
    );
}

#[test]
fn a_grounded_wye_bank_neutral_is_reduced_with_the_network() {
    let wires = terminals(&["1", "2", "3", "4"]);
    let mut source = MulticonductorNetwork::named("four-wire");
    for id in ["source", "load"] {
        let mut bus = DistBus::new(id, wires.clone());
        bus.grounded.push("4".to_owned());
        source.buses_mut().push(bus);
    }
    source.line_codes_mut().push(DistLineCode::new(
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
    source.lines_mut().push(DistLine::new(
        "feeder",
        "source",
        "load",
        wires.clone(),
        wires.clone(),
        "four",
        10.0,
    ));
    source.sources_mut().push(VoltageSource::new(
        "grid",
        "source",
        wires.clone(),
        vec![230.0, 230.0, 230.0, 0.0],
        vec![0.0, -2.094, 2.094, 0.0],
    ));
    source.capacitors_mut().push(DistCapacitor::new(
        "bank",
        "load",
        wires,
        Configuration::Wye,
        600e3,
        4160.0,
    ));

    // The bank lowers to a shunt before neutral reduction, so the grounded
    // neutral is eliminated with the rest of the network's.
    let prepared =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Lower).unwrap();
    assert_eq!(prepared.network().capacitors().as_slice(), []);
    let shunt = prepared
        .network()
        .shunts()
        .iter()
        .find(|shunt| shunt.name.starts_with("__l3f-capacitor-"))
        .unwrap();
    assert_eq!(shunt.terminal_map, terminals(&["1", "2", "3"]));
    let phase = 600e3 / 3.0 / (4160.0f64.powi(2) / 3.0);
    for row in 0..3 {
        for column in 0..3 {
            let expected = if row == column { phase } else { 0.0 };
            assert!((shunt.b[row][column] - expected).abs() < 1e-9);
        }
    }
}

#[test]
fn approximate_policy_linearizes_loads_and_static_ibrs() {
    let mut source = network();
    let mut load = DistLoad::new(
        "demand",
        "load",
        terminals(&["1"]),
        Configuration::Wye,
        vec![100.0],
        vec![20.0],
    );
    load.voltage_model = DistLoadVoltageModel::ConstantCurrent { v_nom: vec![230.0] };
    source.loads_mut().push(load);
    let mut ibr = DistIbr::new(
        "pv",
        "load",
        terminals(&["1"]),
        IbrTopology::SinglePhase,
        IbrPrimeMover::Pv,
        vec![500.0],
    );
    ibr.p_avail = Some(400.0);
    source.ibrs_mut().push(ibr);

    let prepared =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Approximate).unwrap();

    let DistLoadVoltageModel::Zip {
        alpha_z,
        alpha_i,
        alpha_p,
        ..
    } = &prepared.network().loads()[0].voltage_model
    else {
        panic!("constant-current load was not linearized");
    };
    assert_eq!(alpha_z, &[0.5]);
    assert_eq!(alpha_i, &[0.0]);
    assert_eq!(alpha_p, &[0.5]);
    assert_eq!(prepared.network().ibrs().as_slice(), []);
    let generator = &prepared.network().generators()[0];
    assert_eq!(generator.p_min.as_deref(), Some(&[0.0][..]));
    assert_eq!(generator.p_max.as_deref(), Some(&[400.0][..]));
    assert_eq!(generator.s_max.as_deref(), Some(&[500.0][..]));
}

#[test]
fn permissive_policy_omits_untyped_records_but_lower_retains_them() {
    let mut source = network();
    source
        .untyped_objects_mut()
        .push(UntypedObject::new("storage", "battery", Vec::new()));

    let lower =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Lower).unwrap();
    let permissive =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Permissive).unwrap();

    assert_eq!(lower.network().untyped_objects().len(), 1);
    assert_eq!(permissive.network().untyped_objects().as_slice(), []);
    assert!(
        permissive.report().actions.iter().any(|action| {
            action.kind == LinDist3FlowPreparationActionKind::UntypedObjectOmitted
        })
    );
}

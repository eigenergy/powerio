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
    assert!(prepared.network().switches().is_empty());
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

    assert_eq!(prepared.network().shunts().len(), 2);
    assert!(prepared.network().capacitors().is_empty());
    assert_eq!(prepared.network().line_codes()[0].b_from, vec![vec![0.0]]);
    assert_eq!(
        prepared.network().shunts()[0].b,
        vec![vec![0.01]],
        "per-metre charging is scaled by line length"
    );
    assert_eq!(
        prepared.network().shunts()[1].b,
        vec![vec![1_000.0 / 230.0f64.powi(2)]]
    );
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
    assert!(prepared.network().ibrs().is_empty());
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
    assert!(permissive.network().untyped_objects().is_empty());
    assert!(
        permissive.report().actions.iter().any(|action| {
            action.kind == LinDist3FlowPreparationActionKind::UntypedObjectOmitted
        })
    );
}

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
    let location = component_location(powerio_dist::DistLocationTarget::Capacitor, "bank", 12.0);
    source.component_locations_mut().push(location.clone());

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
    let point = &prepared.network().component_locations()[0];
    assert_eq!(point.target, powerio_dist::DistLocationTarget::Shunt);
    assert_eq!(point.name, shunt.name);
    assert_eq!(point.point, location.point);
    prepared.network().validate_component_locations().unwrap();
    let phase = 600e3 / 3.0 / (4160.0f64.powi(2) / 3.0);
    for row in 0..3 {
        for column in 0..3 {
            let expected = if row == column { phase } else { 0.0 };
            assert!((shunt.b[row][column] - expected).abs() < 1e-9);
        }
    }
}

fn component_location(
    target: powerio_dist::DistLocationTarget,
    name: &str,
    latitude: f64,
) -> powerio_dist::DistComponentLocation {
    let mut location = powerio_dist::DistComponentLocation::new(
        target,
        name,
        powerio_dist::DistGeographicPoint::new(Some(latitude), None),
    );
    location.kind = Some(powerio_dist::DistCoordsKind::Manual);
    location
}

fn network_with_static_devices() -> MulticonductorNetwork {
    use powerio_dist::DistShunt;
    let mut source = network();
    for (name, open) in [("contact./A", false), ("open", true)] {
        source.switches_mut().push(DistSwitch::new(
            name,
            "source",
            "load",
            terminals(&["1"]),
            terminals(&["1"]),
            open,
        ));
    }
    source.capacitors_mut().push(DistCapacitor::new(
        "bank",
        "load",
        terminals(&["1"]),
        Configuration::Wye,
        1000.0,
        230.0,
    ));
    // A generated replacement must avoid an existing identity and its point.
    source.shunts_mut().push(DistShunt::new(
        "__l3f-capacitor-bank",
        "load",
        terminals(&["1"]),
        vec![vec![0.0]],
        vec![vec![0.0]],
    ));
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
    source
}

#[test]
fn component_points_follow_lowered_devices_and_omissions_are_audited() {
    use powerio_dist::DistLocationTarget as Target;
    let mut source = network_with_static_devices();
    let bare =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Approximate).unwrap();
    for (target, name, latitude) in [
        (Target::Bus, "load", 0.0),
        (Target::Switch, "contact./A", 1.0),
        (Target::Switch, "open", 2.0),
        (Target::Capacitor, "bank", 3.0),
        (Target::Ibr, "pv", 4.0),
        (Target::Shunt, "__l3f-capacitor-bank", 5.0),
        (Target::Line, "line", 6.0),
        (Target::LineCode, "linecode", 7.0),
    ] {
        source
            .component_locations_mut()
            .push(component_location(target, name, latitude));
    }
    source.component_locations_mut()[3].point.datum = Some("declared datum".into());
    let original = serde_json::to_value(&source).unwrap();
    let prepared =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Approximate).unwrap();
    let points = prepared.network().component_locations();
    assert_eq!(points.len(), 7);
    for (target, name, latitude) in [
        (Target::Bus, "load", 0.0),
        (Target::Line, "__l3f-switch-contact./A", 1.0),
        (Target::Shunt, "__l3f-capacitor-bank-2", 3.0),
        (Target::Generator, "__l3f-ibr-pv", 4.0),
        (Target::Shunt, "__l3f-capacitor-bank", 5.0),
        (Target::Line, "line", 6.0),
        (Target::LineCode, "linecode", 7.0),
    ] {
        let point = points
            .iter()
            .find(|p| p.target == target && p.name == name)
            .unwrap();
        assert_eq!(point.point.latitude, Some(latitude));
        assert_eq!(point.point.longitude, None);
        assert_eq!(point.kind, Some(powerio_dist::DistCoordsKind::Manual));
    }
    assert_eq!(
        points
            .iter()
            .find(|p| p.name == "__l3f-capacitor-bank-2")
            .unwrap()
            .point
            .datum
            .as_deref(),
        Some("declared datum")
    );
    let omitted = prepared
        .report()
        .actions
        .iter()
        .find(|a| a.kind == LinDist3FlowPreparationActionKind::OpenSwitchOmitted)
        .unwrap();
    assert_eq!(
        omitted.details["source_location"],
        serde_json::to_value(&source.component_locations()[2]).unwrap()
    );
    let stored = serde_json::to_value(prepared.network()).unwrap();
    let restored: MulticonductorNetwork = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), stored);
    assert_eq!(serde_json::to_value(&source).unwrap(), original);
    // Metadata changes cannot change the resulting electrical network.
    let mut located = prepared.network().clone();
    let mut bare = bare.network().clone();
    located.component_locations_mut().clear();
    located.extras_mut().clear();
    bare.extras_mut().clear();
    assert_eq!(
        serde_json::to_value(located).unwrap(),
        serde_json::to_value(bare).unwrap()
    );
}

#[test]
fn invalid_point_attachments_are_rejected_even_without_lowering() {
    let mut source = network();
    source.component_locations_mut().push(component_location(
        powerio_dist::DistLocationTarget::Bus,
        "LOAD",
        0.0,
    ));
    for policy in [
        LinDist3FlowPreparationPolicy::Reject,
        LinDist3FlowPreparationPolicy::Lower,
    ] {
        let error = prepare_lindist3flow_network(&source, policy).unwrap_err();
        assert!(error.to_string().contains("no exact component identity"));
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

fn assert_bounded_projection(law: &DistLoadVoltageModel) {
    let mut source = network();
    let mut load = DistLoad::new(
        "demand",
        "load",
        terminals(&["1"]),
        Configuration::Wye,
        vec![100.0],
        vec![20.0],
    );
    load.voltage_model = law.clone();
    source.loads_mut().push(load);
    for policy in [
        LinDist3FlowPreparationPolicy::Reject,
        LinDist3FlowPreparationPolicy::Lower,
    ] {
        let prepared = prepare_lindist3flow_network(&source, policy).unwrap();
        assert_eq!(&prepared.network().loads()[0].voltage_model, law);
        assert!(
            !prepared
                .report()
                .actions
                .iter()
                .any(|a| a.kind == LinDist3FlowPreparationActionKind::LoadApproximated)
        );
    }
    let prepared =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Approximate).unwrap();
    assert_eq!(&source.loads()[0].voltage_model, law);
    let action = prepared
        .report()
        .actions
        .iter()
        .find(|a| a.kind == LinDist3FlowPreparationActionKind::LoadApproximated)
        .unwrap();
    assert_eq!(
        action.details["source_voltage_model"],
        serde_json::json!(law)
    );
    let projected = &prepared.network().loads()[0].voltage_model;
    for (actual, expected) in projected
        .power_scale(0, 230.0)
        .unwrap()
        .into_iter()
        .zip(law.power_scale(0, 230.0).unwrap())
    {
        assert!((actual - expected).abs() <= f64::EPSILON);
    }
    let step = 1e-4_f64;
    let slope = |model: &DistLoadVoltageModel| {
        let hi = model.power_scale(0, 230.0 * (1.0 + step).sqrt()).unwrap();
        let lo = model.power_scale(0, 230.0 * (1.0 - step).sqrt()).unwrap();
        [
            (hi[0] - lo[0]) / (2.0 * step),
            (hi[1] - lo[1]) / (2.0 * step),
        ]
    };
    for (actual, expected) in slope(projected).into_iter().zip(slope(law)) {
        assert!((actual - expected).abs() < 1e-8);
    }
    match &mut source.loads_mut()[0].voltage_model {
        DistLoadVoltageModel::BoundedZip { v_min_pu, .. }
        | DistLoadVoltageModel::BoundedExponential { v_min_pu, .. }
        | DistLoadVoltageModel::BoundedConstantPower { v_min_pu, .. } => *v_min_pu = 1.0,
        _ => unreachable!(),
    }
    let unchanged =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Approximate).unwrap();
    assert_eq!(
        unchanged.network().loads()[0].voltage_model,
        source.loads()[0].voltage_model
    );
    assert_eq!(
        unchanged.report().actions,
        [] as [powerio_dist::LinDist3FlowPreparationAction; 0]
    );
}

#[test]
fn bounded_zip_projection_is_explicit_and_preserves_nominal_value_and_slope() {
    for (z, i, p) in [
        (1.0, 0.0, 0.0),
        (0.0, 1.0, 0.0),
        (0.0, 0.0, 1.0),
        (0.2, 0.3, 0.5),
    ] {
        for cutoff_pu in [0.0, 0.2] {
            assert_bounded_projection(&DistLoadVoltageModel::BoundedZip {
                v_nom: vec![230.0],
                alpha_z: vec![z],
                alpha_i: vec![i],
                alpha_p: vec![p],
                beta_z: vec![z],
                beta_i: vec![i],
                beta_p: vec![p],
                v_low_pu: 0.5,
                v_min_pu: 0.95,
                v_max_pu: 1.05,
                cutoff_pu,
            });
        }
    }
}

#[test]
fn bounded_exponential_projection_preserves_independent_fractional_slopes() {
    assert_bounded_projection(&DistLoadVoltageModel::BoundedExponential {
        v_nom: vec![230.0],
        gamma_p: vec![-0.5],
        gamma_q: vec![1.7],
        v_low_pu: 0.5,
        v_min_pu: 0.95,
        v_max_pu: 1.05,
    });
}

#[test]
fn bounded_constant_power_projection_requires_a_smooth_nominal_current_region() {
    for current_limit_pu in [None, Some(1.2)] {
        assert_bounded_projection(&DistLoadVoltageModel::BoundedConstantPower {
            v_nom: vec![230.0],
            v_min_pu: 0.95,
            v_max_pu: 1.05,
            current_limit_pu,
        });
    }
    let mut source = network();
    let mut load = DistLoad::new(
        "limited",
        "load",
        terminals(&["1"]),
        Configuration::Wye,
        vec![100.0],
        vec![20.0],
    );
    load.voltage_model = DistLoadVoltageModel::BoundedConstantPower {
        v_nom: vec![230.0],
        v_min_pu: 0.95,
        v_max_pu: 1.05,
        current_limit_pu: Some(1.0),
    };
    source.loads_mut().push(load);
    let prepared =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Approximate).unwrap();
    assert!(prepared.report().actions.is_empty());
    assert_eq!(
        prepared.network().loads()[0].voltage_model,
        source.loads()[0].voltage_model
    );
}

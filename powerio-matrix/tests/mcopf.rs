use powerio_dist::{
    Configuration, DistBus, DistGenerator, DistLine, DistLineCode, DistLoad, DistSwitch,
    MulticonductorNetwork, VoltageSource,
};
use powerio_matrix::{McAcOpfAssemblyOptions, McOpfDeviceKind, build_mc_ac_opf_preparation};
use powerio_prob::{ConstraintSelection, McAcOpfInstance};
fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}
fn network() -> MulticonductorNetwork {
    let mut n = MulticonductorNetwork::new();
    let mut s = DistBus::new("source", names(&["a", "n"]));
    s.grounded = names(&["n"]);
    n.buses_mut().push(s);
    let mut l = DistBus::new("load", names(&["a", "n"]));
    l.vpn_min = Some(vec![200.]);
    l.vpn_max = Some(vec![250.]);
    l.vn_max = Some(10.);
    n.buses_mut().push(l);
    let mut code = DistLineCode::new(
        "c",
        vec![vec![0.1, 0.02], vec![0.02, 0.1]],
        vec![vec![0.; 2]; 2],
    );
    code.i_max = Some(vec![10.; 2]);
    n.line_codes_mut().push(code);
    n.lines_mut().push(DistLine::new(
        "l",
        "source",
        "load",
        names(&["a", "n"]),
        names(&["a", "n"]),
        "c",
        1.,
    ));
    let mut source = VoltageSource::new(
        "grid",
        "source",
        names(&["a", "n"]),
        vec![230., 0.],
        vec![0.; 2],
    );
    source.energy_cost_rate = Some(vec![0.2]);
    n.sources_mut().push(source);
    n.loads_mut().push(DistLoad::new(
        "d",
        "load",
        names(&["a", "n"]),
        Configuration::Wye,
        vec![100.],
        vec![20.],
    ));
    n
}
fn prepare(n: MulticonductorNetwork) -> powerio_matrix::McAcOpfPreparation {
    build_mc_ac_opf_preparation(
        &McAcOpfInstance::from_network(n).unwrap(),
        &McAcOpfAssemblyOptions::default(),
    )
    .unwrap()
}
#[test]
fn preparation_preserves_coupling_units_source_cost_and_neutral() {
    let p = prepare(network());
    assert_eq!(p.terminals.len(), 4);
    assert!(!p.terminals[3].grounded);
    assert!((p.branches[0].r[0][1] - 0.02 / 52.9).abs() < 1e-15);
    assert_eq!(p.devices[0].coils[0].negative, Some(3));
    assert_eq!(p.devices[0].coils[0].prescribed, Some([0.1, 0.02]));
    let src = p
        .devices
        .iter()
        .find(|d| d.kind == McOpfDeviceKind::Source)
        .unwrap();
    assert!((src.coils[0].cost - 0.2).abs() < 1e-15);
    assert_eq!(p.voltage_limits.len(), 2);
}
#[test]
fn disabled_limits_and_unknown_selection_have_explicit_semantics() {
    let i = McAcOpfInstance::from_network(network()).unwrap();
    let mut c = i.constraints().clone();
    c.conductor_limits = ConstraintSelection::None;
    c.terminal_voltage_bounds = ConstraintSelection::None;
    let p = build_mc_ac_opf_preparation(
        &i.clone().with_constraints(c.clone()),
        &McAcOpfAssemblyOptions::default(),
    )
    .unwrap();
    assert!(p.voltage_limits.is_empty());
    assert_eq!(p.branches[0].current_max, vec![None; 2]);
    c.conductor_limits = ConstraintSelection::Only(vec!["line:missing".into()]);
    assert!(
        build_mc_ac_opf_preparation(&i.with_constraints(c), &McAcOpfAssemblyOptions::default())
            .is_err()
    );
}
#[test]
fn malformed_data_never_truncates_a_conductor_or_cost() {
    for case in 0..6 {
        let mut n = network();
        match case {
            0 => n.lines_mut()[0].terminal_map_to.pop().map(|_| ()).unwrap(),
            1 => {
                n.line_codes_mut()[0].r_series[0].pop();
            }
            2 => n.loads_mut()[0].p_nom.push(2.),
            3 => n.sources_mut()[0].energy_cost_rate = Some(vec![]),
            4 => n.line_codes_mut()[0].r_series[0][0] = f64::NAN,
            _ => n.buses_mut()[1].grounded.push("missing".into()),
        }
        let i = McAcOpfInstance::from_network(n);
        assert!(
            i.is_err()
                || build_mc_ac_opf_preparation(&i.unwrap(), &McAcOpfAssemblyOptions::default())
                    .is_err(),
            "case {case}"
        );
    }
}
#[test]
fn zero_absent_and_infinite_caps_are_distinct() {
    let mut n = network();
    n.lines_mut()[0].i_max = Some(vec![0., f64::INFINITY]);
    let p = prepare(n);
    assert_eq!(p.branches[0].current_max, vec![Some(0.), None]);
    let mut n = network();
    n.lines_mut()[0].i_max = Some(vec![-1., 1.]);
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .is_err()
    );
}
#[test]
fn exact_zero_impedance_is_retained_but_ideal_cycles_are_rejected() {
    let mut n = network();
    n.line_codes_mut()[0].r_series = vec![vec![0.; 2]; 2];
    let p = prepare(n.clone());
    assert!(p.branches[0].r.iter().flatten().all(|x| *x == 0.));
    n.switches_mut().push(DistSwitch::new(
        "parallel",
        "source",
        "load",
        names(&["a"]),
        names(&["a"]),
        false,
    ));
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .unwrap_err()
        .to_string()
        .contains("cycle")
    );
}
#[test]
fn unhandled_physics_and_reference_conflicts_fail_closed() {
    let mut n = network();
    n.loads_mut()[0]
        .extras
        .insert("custom_control".into(), serde_json::json!(1));
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .is_err()
    );
    let mut n = network();
    n.sources_mut()[0].v_magnitude[1] = 1.;
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .is_err()
    );
    let mut n = network();
    let duplicate = n.sources()[0].clone();
    n.sources_mut().push(duplicate);
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .is_err()
    );
}
#[test]
fn delta_two_terminal_device_is_one_coil_and_limits_use_coils() {
    let mut n = network();
    n.loads_mut()[0].configuration = Configuration::Delta;
    let p = prepare(n);
    assert_eq!(p.devices[0].coils.len(), 1);
}
#[test]
fn generator_bounds_are_optional_and_single_phase_ratings_collapse() {
    let mut n = network();
    let mut g = DistGenerator::new(
        "g",
        "load",
        names(&["a", "n"]),
        Configuration::Wye,
        vec![0.],
        vec![0.],
    );
    g.i_max = Some(vec![3., 4.]);
    g.p_max = Some(vec![50.]);
    g.cost = Some(vec![0.1]);
    n.generators_mut().push(g);
    let p = prepare(n);
    let d = p
        .devices
        .iter()
        .find(|d| d.kind == McOpfDeviceKind::Generator)
        .unwrap();
    assert_eq!(d.coils[0].p_min, None);
    assert_eq!(d.coils[0].p_max, Some(0.05));
    assert_eq!(d.neutral_current_max, None);
    assert!((d.coils[0].current_max.unwrap() - 0.69).abs() < 1e-15);
}
#[test]
fn explicit_bases_change_coefficients_not_physical_meaning() {
    let i = McAcOpfInstance::from_network(network()).unwrap();
    let b = McAcOpfAssemblyOptions::new(100., 2000.);
    let p = build_mc_ac_opf_preparation(&i, &b).unwrap();
    assert!((p.branches[0].r[0][0] * 5. - 0.1).abs() < 1e-15);
    assert_eq!(p.devices[0].coils[0].prescribed, Some([0.05, 0.01]));
    for x in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(
            build_mc_ac_opf_preparation(&i, &McAcOpfAssemblyOptions::new(x, b.power_base_va))
                .is_err()
        );
    }
}

#[test]
fn infinite_voltage_floor_is_not_an_unrated_upper_limit() {
    let mut n = network();
    n.buses_mut()[1].vpn_min = Some(vec![f64::INFINITY]);
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .is_err()
    );
}

#[test]
fn starts_are_physical_source_phasors_on_every_base() {
    let i = McAcOpfInstance::from_network(network()).unwrap();
    for vb in [100.0, 230.0, 1000.0] {
        let p = build_mc_ac_opf_preparation(&i, &McAcOpfAssemblyOptions::new(vb, 2000.0)).unwrap();
        assert!((p.terminals[2].start[0] * vb - 230.0).abs() < 1e-12);
        assert!(p.terminals[3].start.iter().all(|v| v.abs() < f64::EPSILON));
    }
}

#[test]
fn zero_terminal_is_ground_not_a_power_coil() {
    let mut n = network();
    for b in n.buses_mut() {
        b.terminals[1] = "0".into();
        b.grounded.clear();
    }
    n.lines_mut()[0].terminal_map_from[1] = "0".into();
    n.lines_mut()[0].terminal_map_to[1] = "0".into();
    n.loads_mut()[0].terminal_map[1] = "0".into();
    n.sources_mut()[0].terminal_map[1] = "0".into();
    let p = prepare(n);
    assert!(p.terminals[1].grounded && p.terminals[3].grounded);
    assert_eq!(p.devices[0].coils.len(), 1);
    assert_eq!(p.devices[0].coils[0].negative, Some(3));
}

#[test]
fn a_source_cannot_hide_a_floating_neutral_by_omitting_it() {
    let mut n = network();
    n.buses_mut()[0].grounded.clear();
    n.sources_mut()[0].terminal_map.pop();
    n.sources_mut()[0].v_magnitude.pop();
    n.sources_mut()[0].v_angle.pop();
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .unwrap_err()
        .to_string()
        .contains("neutral")
    );
}

#[test]
fn canonical_bmopf_metadata_and_custom_terminal_roles_are_respected() {
    let mut n = network();
    for bus in n.buses_mut() {
        bus.terminals[1] = "return".into();
        if !bus.grounded.is_empty() {
            bus.grounded[0] = "return".into();
        }
    }
    n.lines_mut()[0].terminal_map_from[1] = "return".into();
    n.lines_mut()[0].terminal_map_to[1] = "return".into();
    n.loads_mut()[0].terminal_map[1] = "return".into();
    n.sources_mut()[0].terminal_map[1] = "return".into();
    n.extras_mut().insert(
        "bmopf_terminal_conventions".into(),
        serde_json::json!({"phase":["a"], "neutral":["return"], "earth":[]}),
    );
    n.extras_mut().insert(
        "bmopf_meta".into(),
        serde_json::json!({"frequency":60, "license":"MIT"}),
    );
    let p = prepare(n);
    assert_eq!(p.devices[0].coils.len(), 1);
    assert_eq!(p.devices[0].coils[0].negative, Some(3));
    assert!(!p.terminals[3].grounded);
}

#[test]
fn voltage_dependent_load_terms_and_nominal_units_are_explicit() {
    use powerio_dist::DistLoadVoltageModel;
    let mut n = network();
    n.loads_mut()[0].voltage_model = DistLoadVoltageModel::Zip {
        v_nom: vec![230.0],
        alpha_z: vec![0.2],
        alpha_i: vec![0.3],
        alpha_p: vec![0.5],
        beta_z: vec![0.1],
        beta_i: vec![0.6],
        beta_p: vec![0.3],
    };
    let p = prepare(n.clone());
    let law = p.devices[0].coils[0].load_law.as_ref().unwrap();
    assert!((law.nominal - 1.0).abs() < 1e-14);
    assert!(
        (law.terms[0]
            .iter()
            .map(|t| t[0] * 0.9_f64.powf(t[1]))
            .sum::<f64>()
            - 0.0932)
            .abs()
            < 1e-14
    );
    n.loads_mut()[0].voltage_model = DistLoadVoltageModel::ConstantCurrent { v_nom: vec![0.0] };
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .is_err()
    );
}

#[test]
fn capacitor_bank_units_and_two_terminal_incidence() {
    let mut n = network();
    n.capacitors_mut().push(powerio_dist::DistCapacitor::new(
        "cap",
        "load",
        names(&["a", "n"]),
        Configuration::SinglePhase,
        50.0,
        230.0,
    ));
    let p = prepare(n);
    let sh = &p.shunts[0];
    assert_eq!(sh.terminals, vec![2, 3]);
    for (actual, want) in sh.b.iter().flatten().zip([0.05, -0.05, -0.05, 0.05]) {
        assert!((*actual - want).abs() < 1e-14);
    }
}

#[test]
fn inverter_topology_zero_caps_and_dc_link_are_preserved() {
    use powerio_dist::{DistIbr, IbrPrimeMover, IbrTopology};
    let mut n = network();
    let mut inv = DistIbr::new(
        "inv",
        "load",
        names(&["a", "n"]),
        IbrTopology::SinglePhase,
        IbrPrimeMover::Battery,
        vec![80.0],
    );
    inv.i_max = Some(vec![2.0, 0.0]);
    inv.p_min = Some(vec![-60.0]);
    inv.p_max = Some(vec![60.0]);
    inv.extras.insert("dc_link_coupled".into(), true.into());
    n.ibrs_mut().push(inv);
    let p = prepare(n.clone());
    let inv = p
        .devices
        .iter()
        .find(|d| d.kind == McOpfDeviceKind::Ibr)
        .unwrap();
    assert_eq!(inv.coils[0].current_max, Some(0.0));
    assert_eq!(inv.neutral_current_max, None);
    assert_eq!(inv.net_active_bounds, Some([0.0, 0.0]));
    n.ibrs_mut()[0].control_profile = Some("missing".into());
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .is_err()
    );
}

#[test]
fn transformer_descriptor_matches_referred_impedance_without_an_inverse() {
    use powerio_dist::{DistTransformer, DistWinding, DistWindingConn};
    let mut n = network();
    n.lines_mut().clear();
    n.buses_mut()[1].grounded = names(&["n"]);
    let mut primary = DistWinding::new(
        "source",
        names(&["a", "n"]),
        DistWindingConn::Wye,
        230.0,
        1000.0,
    );
    primary.r_pct = 1.0;
    let mut secondary = DistWinding::new(
        "load",
        names(&["a", "n"]),
        DistWindingConn::Wye,
        115.0,
        1000.0,
    );
    secondary.r_pct = 2.0;
    n.transformers_mut().push(DistTransformer::new(
        "tx",
        vec![primary, secondary],
        vec![3.0],
        1,
    ));
    let p = prepare(n.clone());
    let tx = &p.transformers[0];
    assert_eq!(
        tx.equations[0].current,
        vec![(0, [1.0, 0.0]), (1, [0.5, 0.0])]
    );
    // Z referred to primary = .01*52.9 + .02*13.225/.5^2 + j*.03*52.9.
    // On system Zbase 52.9, multiplying the referred secondary current gives .015+j*.015.
    let z = tx.equations[1].current[0].1;
    assert!((z[0] - 0.015).abs() < 1e-14);
    assert!((z[1] - 0.015).abs() < 1e-14);
    assert!((p.terminals[2].start[0] - 0.5).abs() < 1e-14);
    n.transformers_mut()[0]
        .windings
        .iter_mut()
        .for_each(|w| w.r_pct = 0.0);
    n.transformers_mut()[0].xsc_pct[0] = 0.0;
    let ideal = prepare(n);
    assert!(
        ideal.transformers[0].equations[1]
            .current
            .iter()
            .all(|(_, z)| *z == [0.0, 0.0])
    );
}

#[test]
fn sequence_and_angle_limits_validate_domains_before_solve() {
    let mut n = network();
    n.buses_mut()[1].vpos_max = Some(250.0);
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .is_err()
    );
    let mut n = network();
    n.lines_mut()[0]
        .extras
        .insert("va_diff_min".into(), (-0.1).into());
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(n.clone()).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .is_err()
    );
    n.lines_mut()[0]
        .extras
        .insert("va_diff_max".into(), 0.1.into());
    assert_eq!(prepare(n).angle_limits.len(), 2);
}

fn with_transformer() -> MulticonductorNetwork {
    use powerio_dist::{DistTransformer, DistWinding, DistWindingConn};
    let mut net = network();
    let windings = ["source", "load"]
        .map(|bus| DistWinding::new(bus, names(&["a", "n"]), DistWindingConn::Wye, 230.0, 1000.0));
    net.transformers_mut()
        .push(DistTransformer::new("tx", windings.to_vec(), vec![1.0], 1));
    net
}

#[test]
fn transformers_do_not_disable_ideal_cycle_validation() {
    let mut net = with_transformer();
    net.line_codes_mut()[0].r_series = vec![vec![0.0; 2]; 2];
    net.switches_mut().push(DistSwitch::new(
        "parallel",
        "source",
        "load",
        names(&["a"]),
        names(&["a"]),
        false,
    ));
    let error = build_mc_ac_opf_preparation(
        &McAcOpfInstance::from_network(net).unwrap(),
        &McAcOpfAssemblyOptions::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("cycle"));
}

#[test]
fn malformed_transformer_metadata_and_ambiguous_grounding_are_rejected() {
    for (key, value) in [
        ("bmopf_subtype", serde_json::json!(17)),
        ("bmopf_winding_metadata", serde_json::json!([])),
        ("bmopf_winding_metadata", serde_json::json!({"0":17})),
        ("bmopf_power_base", serde_json::json!(0)),
        (
            "no_load_shunt",
            serde_json::json!({"winding":0,"g":0,"b":0}),
        ),
        ("tap_min", serde_json::json!(0.9)),
    ] {
        let mut net = with_transformer();
        net.transformers_mut()[0].extras.insert(key.into(), value);
        assert!(
            build_mc_ac_opf_preparation(
                &McAcOpfInstance::from_network(net).unwrap(),
                &McAcOpfAssemblyOptions::default()
            )
            .is_err(),
            "{key}"
        );
    }
    let mut net = with_transformer();
    net.transformers_mut()[0].windings[1].r_neutral = Some(-1.0);
    assert!(
        build_mc_ac_opf_preparation(
            &McAcOpfInstance::from_network(net).unwrap(),
            &McAcOpfAssemblyOptions::default()
        )
        .is_err()
    );
}

#[test]
fn transformer_and_inverter_ratings_obey_canonical_selections() {
    use powerio_dist::{DistIbr, IbrPrimeMover, IbrTopology};
    let mut net = with_transformer();
    net.transformers_mut()[0].extras.insert(
        "bmopf_winding_metadata".into(),
        serde_json::json!({"0":{"s_max":1000,"i_max":10}}),
    );
    let mut inv = DistIbr::new(
        "inv",
        "load",
        names(&["a", "n"]),
        IbrTopology::SinglePhase,
        IbrPrimeMover::Battery,
        vec![80.0],
    );
    inv.p_max = Some(vec![60.0]);
    net.ibrs_mut().push(inv);
    let instance = McAcOpfInstance::from_network(net).unwrap();
    let mut constraints = instance.constraints().clone();
    constraints.conductor_limits = ConstraintSelection::Only(vec!["transformer:tx".into()]);
    constraints.generator_capability = ConstraintSelection::Only(vec!["ibr:inv".into()]);
    let selected = instance.with_constraints(constraints.clone());
    let prep = build_mc_ac_opf_preparation(&selected, &McAcOpfAssemblyOptions::default()).unwrap();
    assert!(
        prep.transformers[0]
            .ports
            .iter()
            .any(|p| p.apparent_max.is_some())
    );
    assert!(
        prep.devices
            .iter()
            .find(|d| d.kind == McOpfDeviceKind::Ibr)
            .unwrap()
            .coils[0]
            .p_max
            .is_some()
    );
    constraints.conductor_limits = ConstraintSelection::None;
    constraints.generator_capability = ConstraintSelection::None;
    let prep = build_mc_ac_opf_preparation(
        &selected.with_constraints(constraints),
        &McAcOpfAssemblyOptions::default(),
    )
    .unwrap();
    assert!(
        prep.transformers[0]
            .ports
            .iter()
            .all(|p| p.current_max.is_none() && p.apparent_max.is_none())
    );
    let coil = &prep
        .devices
        .iter()
        .find(|d| d.kind == McOpfDeviceKind::Ibr)
        .unwrap()
        .coils[0];
    assert!(coil.p_max.is_none() && coil.apparent_max.is_none());
}

#[test]
fn canonical_wye_capacitor_nameplate_matches_the_admittance_builder() {
    let mut n = network();
    n.capacitors_mut().push(powerio_dist::DistCapacitor::new(
        "cap",
        "load",
        names(&["a", "n"]),
        Configuration::Wye,
        50.0,
        230.0 * 3f64.sqrt(),
    ));
    let p = prepare(n.clone());
    for (actual, want) in p.shunts[0]
        .b
        .iter()
        .flatten()
        .zip([0.05, -0.05, -0.05, 0.05])
    {
        assert!((*actual - want).abs() < 1e-14);
    }
    // A canonical nameplate edit must reach preparation, with no stale source data.
    n.capacitors_mut()[0].q_rated = 100.0;
    let p = prepare(n);
    assert!((p.shunts[0].b[0][0] - 0.1).abs() < 1e-14);
}

#[test]
fn per_coil_bmopf_capacitors_enter_ivr_as_the_same_canonical_shunt() {
    let text = r#"{"bus":{"s":{"terminal_names":["a","b","c","n"],"perfectly_grounded_terminals":["n"]}},"voltage_source":{"grid":{"bus":"s","terminal_map":["a","b","c","n"],"v_magnitude":[230,230,230,0],"v_angle":[0,-2.0943951023931953,2.0943951023931953,0]}},"capacitor":{"c":{"bus":"s","terminal_map":["a","b","c","n"],"configuration":"WYE","q_rated":[13,25,7],"v_nom":230}}}"#;
    let source = powerio_core::Source::from_memory("caps.json", text.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("bmopf").unwrap());
    let module = powerio_dist::parse(source).unwrap();
    let p = prepare(module.value().clone());
    assert_eq!(p.shunts.len(), 1);
    for (i, q) in [13.0, 25.0, 7.0].into_iter().enumerate() {
        assert!((p.shunts[0].b[i][i] - q / 1000.0).abs() < 1e-14);
        assert!((p.shunts[0].b[i][3] + q / 1000.0).abs() < 1e-14);
    }
    assert!((p.shunts[0].b[3][3] - 0.045).abs() < 1e-14);
}

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
    let mut n = three_phase_source();
    n.loads_mut().push(DistLoad::new(
        "d",
        "b",
        names(&["c", "a"]),
        Configuration::Delta,
        vec![100.],
        vec![20.],
    ));
    let p = prepare(n);
    assert_eq!(p.devices[0].coils.len(), 1);
    assert_eq!(p.devices[0].coils[0].positive, 2);
    assert_eq!(p.devices[0].coils[0].negative, Some(0));
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

/// #685: a two-terminal delta bank is one phase-to-phase coil at line-to-line
/// voltage, so both consumers stamp B = q / V^2 (50 var at 230 V), not half.
#[test]
fn two_terminal_delta_capacitor_stamps_its_full_rating_in_both_consumers() {
    let capacitor = powerio_dist::DistCapacitor::new(
        "cap",
        "b",
        names(&["a", "b"]),
        Configuration::Delta,
        50.0,
        230.0,
    );
    let siemens = 50.0 / (230.0 * 230.0);

    let mut alone = MulticonductorNetwork::new();
    alone
        .buses_mut()
        .push(DistBus::new("b", names(&["a", "b"])));
    alone.capacitors_mut().push(capacitor.clone());
    let system = powerio_matrix::calc_multiconductor_admittance_matrix(&alone).unwrap();
    let b = system.susceptance();
    for (i, j, sign) in [(0, 0, 1.0), (1, 1, 1.0), (0, 1, -1.0), (1, 0, -1.0)] {
        let stamped = b.get(i, j).copied().unwrap_or(0.0);
        assert!(
            (stamped - sign * siemens).abs() < 1e-15,
            "Y[{i},{j}] = {stamped}"
        );
    }

    // Preparation stamps the same coil on the 230 V, 1 kVA base (Z_base 52.9).
    let mut n = three_phase_source();
    n.capacitors_mut().push(capacitor);
    let p = prepare(n);
    let per_unit = siemens * 230.0 * 230.0 / 1000.0;
    for (actual, want) in p.shunts[0]
        .b
        .iter()
        .flatten()
        .zip([per_unit, -per_unit, -per_unit, per_unit])
    {
        assert!((*actual - want).abs() < 1e-14, "{actual} vs {want}");
    }
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

#[test]
fn terminal_referenced_source_is_not_silently_anchored_to_earth() {
    let mut net = MulticonductorNetwork::new();
    net.buses_mut().push(DistBus::new("s", names(&["a", "b"])));
    net.sources_mut().push(VoltageSource::new(
        "phase-to-b",
        "s",
        names(&["a"]),
        vec![230.0],
        vec![0.0],
    ));
    net.sources_mut().push(VoltageSource::new(
        "earth",
        "s",
        names(&["b"]),
        vec![230.0],
        vec![0.0],
    ));
    // The ordinary earth-referenced profile remains supported.
    let prep = prepare(net.clone());
    assert_eq!(prep.terminals[0].fixed, Some([1.0, 0.0]));
    assert_eq!(prep.terminals[1].fixed, Some([1.0, 0.0]));

    // Va - Vb = 230 V and Vb = 230 V would require Va = 460 V.
    // Treating both prescriptions as absolute would instead return Va = Vb.
    net.sources_mut()[0].reference_terminal = Some("b".into());
    let instance = McAcOpfInstance::from_network(net).unwrap();
    let error = build_mc_ac_opf_preparation(&instance, &McAcOpfAssemblyOptions::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("phase-to-b"), "{error}");
    assert!(
        error.contains("reference-terminal voltage constraints"),
        "{error}"
    );
}

fn three_phase_source() -> MulticonductorNetwork {
    let mut net = MulticonductorNetwork::new();
    let mut bus = DistBus::new("b", names(&["a", "b", "c", "n"]));
    bus.grounded = names(&["n"]);
    net.buses_mut().push(bus);
    let angle = 2.0 * std::f64::consts::PI / 3.0;
    net.sources_mut().push(VoltageSource::new(
        "grid",
        "b",
        names(&["a", "b", "c", "n"]),
        vec![230., 230., 230., 0.],
        vec![0., -angle, angle, 0.],
    ));
    net
}

#[test]
fn star_current_caps_follow_terminal_identity_for_generators_and_inverters() {
    use powerio_dist::{DistIbr, IbrPrimeMover, IbrTopology};
    for map in [
        ["n", "a", "b", "c"],
        ["c", "n", "a", "b"],
        ["b", "c", "n", "a"],
        ["a", "b", "c", "n"],
    ] {
        // Independent physical ratings: a=11 A, b=unlimited, c=17 A, n=0 A.
        let ratings: Vec<_> = map
            .iter()
            .map(|t| match *t {
                "a" => 11.,
                "b" => f64::INFINITY,
                "c" => 17.,
                _ => 0.,
            })
            .collect();
        for present in [true, false] {
            let mut net = three_phase_source();
            let mut generator = DistGenerator::new(
                "g",
                "b",
                names(&map),
                Configuration::Wye,
                vec![0.; 3],
                vec![0.; 3],
            );
            generator.i_max = present.then(|| ratings.clone());
            net.generators_mut().push(generator);
            let mut inv = DistIbr::new(
                "inv",
                "b",
                names(&map),
                IbrTopology::FourLeg,
                IbrPrimeMover::Battery,
                vec![1000.; 3],
            );
            inv.i_max = present.then(|| ratings.clone());
            net.ibrs_mut().push(inv);
            let instance = McAcOpfInstance::from_network(net).unwrap();
            for selected in [true, false] {
                let mut constraints = instance.constraints().clone();
                constraints.generator_capability = if selected {
                    ConstraintSelection::All
                } else {
                    ConstraintSelection::None
                };
                let prep = build_mc_ac_opf_preparation(
                    &instance.clone().with_constraints(constraints),
                    &McAcOpfAssemblyOptions::default(),
                )
                .unwrap();
                for device in prep
                    .devices
                    .iter()
                    .filter(|d| matches!(d.kind, McOpfDeviceKind::Generator | McOpfDeviceKind::Ibr))
                {
                    assert_eq!(device.coils.len(), 3);
                    for coil in &device.coils {
                        assert_eq!(coil.negative, Some(3));
                        let expected = match coil.positive {
                            0 if present && selected => Some(11. * 230. / 1000.),
                            2 if present && selected => Some(17. * 230. / 1000.),
                            0..=2 => None,
                            _ => panic!("neutral must not become a phase coil"),
                        };
                        match (coil.current_max, expected) {
                            (Some(a), Some(b)) => assert!((a - b).abs() < 1e-12),
                            (a, b) => assert_eq!(a, b),
                        }
                    }
                    assert_eq!(
                        device.neutral_current_max,
                        (present && selected).then_some(0.)
                    );
                }
            }
        }
    }
}

#[test]
fn implicit_ground_and_delta_current_caps_keep_their_axes() {
    use powerio_dist::{DistIbr, IbrPrimeMover, IbrTopology};
    for config in [Configuration::Wye, Configuration::Delta] {
        let mut net = three_phase_source();
        let mut generator = DistGenerator::new(
            "g",
            "b",
            names(&["c", "a", "b"]),
            config,
            vec![0.; 3],
            vec![0.; 3],
        );
        generator.i_max = Some(vec![7., 11., 13.]);
        net.generators_mut().push(generator);
        if config == Configuration::Delta {
            let mut inv = DistIbr::new(
                "inv",
                "b",
                names(&["c", "a", "b"]),
                IbrTopology::ThreeLeg,
                IbrPrimeMover::Battery,
                vec![1000.; 3],
            );
            inv.i_max = Some(vec![7., 11., 13.]);
            net.ibrs_mut().push(inv);
        }
        let prep = prepare(net);
        for d in prep
            .devices
            .iter()
            .filter(|d| matches!(d.kind, McOpfDeviceKind::Generator | McOpfDeviceKind::Ibr))
        {
            assert_eq!(d.neutral_current_max, None);
            for ((coil, positive), amps) in d.coils.iter().zip([2, 0, 1]).zip([7., 11., 13.]) {
                assert_eq!(coil.positive, positive);
                assert!((coil.current_max.unwrap() * 1000. / 230. - amps).abs() < 1e-12);
                if config == Configuration::Wye {
                    assert_eq!(coil.negative, None);
                }
            }
        }
    }
}

#[test]
fn two_wire_caps_take_the_tighter_endpoint_in_either_orientation() {
    use powerio_dist::{DistIbr, IbrPrimeMover, IbrTopology};
    for map in [["n", "a"], ["a", "n"]] {
        for (ratings, expected) in [
            (vec![7., 11.], Some(7.)),
            (vec![f64::INFINITY, 0.], Some(0.)),
            (vec![f64::INFINITY, 11.], Some(11.)),
            (vec![f64::INFINITY; 2], None),
        ] {
            for config in [Configuration::Wye, Configuration::SinglePhase] {
                let mut net = network();
                let mut generator =
                    DistGenerator::new("g", "load", names(&map), config, vec![0.], vec![0.]);
                generator.i_max = Some(ratings.clone());
                net.generators_mut().push(generator);
                let mut inv = DistIbr::new(
                    "inv",
                    "load",
                    names(&map),
                    IbrTopology::SinglePhase,
                    IbrPrimeMover::Battery,
                    vec![1000.],
                );
                inv.i_max = Some(ratings.clone());
                net.ibrs_mut().push(inv);
                for d in prepare(net)
                    .devices
                    .iter()
                    .filter(|d| matches!(d.kind, McOpfDeviceKind::Generator | McOpfDeviceKind::Ibr))
                {
                    assert_eq!(d.neutral_current_max, None);
                    match (d.coils[0].current_max, expected) {
                        (Some(a), Some(b)) => assert!((a * 1000. / 230. - b).abs() < 1e-12),
                        (a, b) => assert_eq!(a, b),
                    }
                }
            }
        }
    }
}

#[test]
fn inverter_control_laws_intersect_selected_reactive_capability() {
    use powerio_dist::{DistControlProfile, DistIbr, IbrPrimeMover, IbrTopology};
    for fixed_pf in [true, false] {
        for bound in [10., 0.] {
            let mut net = network();
            let mut inv = DistIbr::new(
                "inv",
                "load",
                names(&["a", "n"]),
                IbrTopology::SinglePhase,
                IbrPrimeMover::Battery,
                vec![1000.],
            );
            inv.p_min = Some(vec![100.]);
            inv.p_max = Some(vec![100.]);
            inv.q_min = Some(vec![-bound]);
            inv.q_max = Some(vec![bound]);
            inv.control_profile = Some("control".into());
            net.ibrs_mut().push(inv);
            let mut profile = DistControlProfile::new("control");
            if fixed_pf {
                profile.power_factor =
                    Some(serde_json::from_value(serde_json::json!({"pf":0.8})).unwrap());
            } else {
                profile.volt_var = Some(serde_json::from_value(serde_json::json!({"breakpoints":[200.,220.,240.,260.],"q_limits":[-0.5,0.5]})).unwrap());
            }
            net.control_profiles_mut().push(profile);
            let instance = McAcOpfInstance::from_network(net).unwrap();
            for selected in [true, false] {
                let mut constraints = instance.constraints().clone();
                constraints.generator_capability = if selected {
                    ConstraintSelection::All
                } else {
                    ConstraintSelection::None
                };
                let prep = build_mc_ac_opf_preparation(
                    &instance.clone().with_constraints(constraints),
                    &McAcOpfAssemblyOptions::default(),
                )
                .unwrap();
                let coil = &prep
                    .devices
                    .iter()
                    .find(|d| d.kind == McOpfDeviceKind::Ibr)
                    .unwrap()
                    .coils[0];
                assert_eq!(coil.q_min, selected.then_some(-bound / 1000.));
                assert_eq!(coil.q_max, selected.then_some(bound / 1000.));
                if fixed_pf {
                    // At P=100 W this law demands Q=-75 var, outside even the
                    // nonzero box. Keep both constraints so infeasibility is visible.
                    assert!((coil.reactive_slope.unwrap() + 0.75).abs() < 1e-12);
                } else {
                    assert_eq!(
                        coil.volt_var.as_ref().unwrap().values,
                        vec![0.5, 0., 0., -0.5]
                    );
                }
            }
        }
    }
}

#[test]
fn bounded_load_laws_are_not_silently_projected_by_ivr_preparation() {
    use powerio_dist::DistLoadVoltageModel as Law;
    let instance = McAcOpfInstance::from_network(network()).unwrap();
    assert!(build_mc_ac_opf_preparation(&instance, &McAcOpfAssemblyOptions::default()).is_ok());
    for law in [
        Law::BoundedZip {
            v_nom: vec![230.0],
            alpha_z: vec![0.0],
            alpha_i: vec![0.0],
            alpha_p: vec![1.0],
            beta_z: vec![0.0],
            beta_i: vec![0.0],
            beta_p: vec![1.0],
            v_low_pu: 0.5,
            v_min_pu: 0.95,
            v_max_pu: 1.05,
            cutoff_pu: 0.0,
        },
        Law::BoundedExponential {
            v_nom: vec![230.0],
            gamma_p: vec![1.0],
            gamma_q: vec![2.0],
            v_low_pu: 0.5,
            v_min_pu: 0.95,
            v_max_pu: 1.05,
        },
        Law::BoundedConstantPower {
            v_nom: vec![230.0],
            v_min_pu: 0.95,
            v_max_pu: 1.05,
            current_limit_pu: Some(1.1),
        },
    ] {
        let mut net = network();
        net.loads_mut()[0].voltage_model = law;
        let instance = McAcOpfInstance::from_network(net).unwrap();
        let error =
            build_mc_ac_opf_preparation(&instance, &McAcOpfAssemblyOptions::default()).unwrap_err();
        assert!(
            error.to_string().contains("piecewise IVR equations"),
            "{error}"
        );
    }
}

#[test]
fn projecting_bounded_load_laws_is_an_explicit_opt_in_with_one_diagnostic_per_load() {
    use powerio_dist::DistLoadVoltageModel as Law;
    use powerio_matrix::BoundedLoadLawPolicy;
    let bounded = |alpha_z: f64, alpha_p: f64| Law::BoundedZip {
        v_nom: vec![230.0],
        alpha_z: vec![alpha_z],
        alpha_i: vec![0.0],
        alpha_p: vec![alpha_p],
        beta_z: vec![alpha_z],
        beta_i: vec![0.0],
        beta_p: vec![alpha_p],
        v_low_pu: 0.5,
        v_min_pu: 0.95,
        v_max_pu: 1.05,
        cutoff_pu: 0.0,
    };
    let opt_in = McAcOpfAssemblyOptions::default()
        .with_bounded_load_laws(BoundedLoadLawPolicy::ProjectToNominal);
    assert_eq!(
        McAcOpfAssemblyOptions::default().bounded_load_laws,
        BoundedLoadLawPolicy::Refuse
    );

    // The OpenDSS default (model=1) is constant power inside the band.
    let mut net = network();
    net.loads_mut()[0].voltage_model = bounded(0.0, 1.0);
    let instance = McAcOpfInstance::from_network(net).unwrap();
    assert!(
        build_mc_ac_opf_preparation(&instance, &McAcOpfAssemblyOptions::default()).is_err(),
        "the default stays strict"
    );
    let prepared = build_mc_ac_opf_preparation(&instance, &opt_in).unwrap();
    assert_eq!(prepared.load_law_projections.len(), 1);
    let projection = &prepared.load_law_projections[0];
    assert_eq!(projection.source_model, "bounded_zip");
    assert_eq!(projection.projected_model, "constant_power");
    let diagnostics = prepared.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code(), "BUILD.MULTI.LOAD_LAW_PROJECTED");
    let load = prepared
        .devices
        .iter()
        .find(|d| d.kind == McOpfDeviceKind::Load)
        .unwrap();
    assert!(load.coils.iter().all(|coil| coil.load_law.is_none()));

    // A constant impedance band projects to the nominal ZIP law.
    let mut net = network();
    net.loads_mut()[0].voltage_model = bounded(1.0, 0.0);
    let instance = McAcOpfInstance::from_network(net).unwrap();
    let prepared = build_mc_ac_opf_preparation(&instance, &opt_in).unwrap();
    assert_eq!(prepared.load_law_projections[0].projected_model, "zip");
    let load = prepared
        .devices
        .iter()
        .find(|d| d.kind == McOpfDeviceKind::Load)
        .unwrap();
    assert!(load.coils.iter().all(|coil| coil.load_law.is_some()));

    // OpenDSS models 3 and 4 read as a bounded exponential law.
    let mut net = network();
    net.loads_mut()[0].voltage_model = Law::BoundedExponential {
        v_nom: vec![230.0],
        gamma_p: vec![0.0],
        gamma_q: vec![2.0],
        v_low_pu: 0.5,
        v_min_pu: 0.95,
        v_max_pu: 1.05,
    };
    let instance = McAcOpfInstance::from_network(net).unwrap();
    assert!(build_mc_ac_opf_preparation(&instance, &McAcOpfAssemblyOptions::default()).is_err());
    let prepared = build_mc_ac_opf_preparation(&instance, &opt_in).unwrap();
    assert_eq!(
        prepared.load_law_projections[0].source_model,
        "bounded_exponential"
    );
    assert_eq!(
        prepared.load_law_projections[0].projected_model,
        "exponential"
    );

    // A voltage-bounded constant-power law projects to constant power.
    let mut net = network();
    net.loads_mut()[0].voltage_model = Law::BoundedConstantPower {
        v_nom: vec![230.0],
        v_min_pu: 0.9,
        v_max_pu: 1.1,
        current_limit_pu: None,
    };
    let instance = McAcOpfInstance::from_network(net).unwrap();
    assert!(build_mc_ac_opf_preparation(&instance, &McAcOpfAssemblyOptions::default()).is_err());
    let prepared = build_mc_ac_opf_preparation(&instance, &opt_in).unwrap();
    assert_eq!(
        prepared.load_law_projections[0].source_model,
        "bounded_constant_power"
    );
    assert_eq!(
        prepared.load_law_projections[0].projected_model,
        "constant_power"
    );

    // Loads that carry no bounded law are not touched.
    let instance = McAcOpfInstance::from_network(network()).unwrap();
    let prepared = build_mc_ac_opf_preparation(&instance, &opt_in).unwrap();
    assert_eq!(prepared.load_law_projections, []);
    assert_eq!(prepared.diagnostics(), []);
}

fn assert_delta_role_error(net: MulticonductorNetwork, terminal: &str) {
    let instance = McAcOpfInstance::from_network(net).unwrap();
    let err =
        build_mc_ac_opf_preparation(&instance, &McAcOpfAssemblyOptions::default()).unwrap_err();
    assert!(
        err.diagnostics()
            .iter()
            .any(|d| d.code() == "BUILD.MULTI.OPF_INVALID")
    );
    let message = err.to_string();
    assert!(
        message.contains("bad")
            && message.contains("delta requires phase terminals")
            && message.contains(terminal),
        "{message}"
    );
}

/// Role checks must precede stamping for every consumer of shared delta geometry.
#[test]
fn delta_terminal_roles_reject_neutral_earth_and_zero_for_every_device() {
    use powerio_dist::{
        DistCapacitor, DistIbr, DistTransformer, DistWinding, DistWindingConn, IbrPrimeMover,
        IbrTopology,
    };
    for (terminal, conventions) in [
        ("n", None),
        ("0", None),
        (
            "return",
            Some(serde_json::json!({"neutral":["return"], "earth":[]})),
        ),
        (
            "soil",
            Some(serde_json::json!({"neutral":[], "earth":["soil"]})),
        ),
    ] {
        for kind in ["load", "generator", "capacitor", "inverter", "transformer"] {
            for count in [2, 3] {
                let mut net = three_phase_source();
                net.buses_mut()[0].terminals[3] = terminal.into();
                net.buses_mut()[0].grounded[0] = terminal.into();
                net.sources_mut()[0].terminal_map[3] = terminal.into();
                if let Some(roles) = &conventions {
                    net.extras_mut()
                        .insert("bmopf_terminal_conventions".into(), roles.clone());
                }
                let map = if count == 2 {
                    names(&["a", terminal])
                } else {
                    names(&["a", "b", terminal])
                };
                let coils = if count == 2 { 1 } else { 3 };
                match kind {
                    "load" => net.loads_mut().push(DistLoad::new(
                        "bad",
                        "b",
                        map,
                        Configuration::Delta,
                        vec![100.; coils],
                        vec![20.; coils],
                    )),
                    "generator" => net.generators_mut().push(DistGenerator::new(
                        "bad",
                        "b",
                        map,
                        Configuration::Delta,
                        vec![0.; coils],
                        vec![0.; coils],
                    )),
                    "capacitor" => net.capacitors_mut().push(DistCapacitor::new(
                        "bad",
                        "b",
                        map,
                        Configuration::Delta,
                        50.,
                        400.,
                    )),
                    "inverter" => net.ibrs_mut().push(DistIbr::new(
                        "bad",
                        "b",
                        map,
                        IbrTopology::ThreeLeg,
                        IbrPrimeMover::Battery,
                        vec![1000.; coils],
                    )),
                    "transformer" => {
                        net.buses_mut()
                            .push(DistBus::new("secondary", names(&["a", "b", "c"])));
                        net.transformers_mut().push(DistTransformer::new(
                            "bad",
                            vec![
                                DistWinding::new("b", map, DistWindingConn::Delta, 400., 1000.),
                                DistWinding::new(
                                    "secondary",
                                    names(&["a", "b", "c"]),
                                    DistWindingConn::Delta,
                                    400.,
                                    1000.,
                                ),
                            ],
                            vec![1.],
                            3,
                        ));
                    }
                    _ => unreachable!(),
                }
                assert_delta_role_error(net, terminal);
            }
        }
    }
}

#[test]
fn delta_terminal_roles_preserve_declared_phases_and_cyclic_order() {
    use powerio_dist::{DistIbr, IbrPrimeMover, IbrTopology};
    // Explicit roles override label heuristics: `n` is a phase here.
    let mut net = three_phase_source();
    net.buses_mut()[0].terminals = names(&["n", "y", "z", "return"]);
    net.buses_mut()[0].grounded = names(&["return"]);
    net.sources_mut()[0].terminal_map = names(&["n", "y", "z", "return"]);
    net.extras_mut().insert(
        "bmopf_terminal_conventions".into(),
        serde_json::json!({"phase":["n","y","z"], "neutral":["return"], "earth":[]}),
    );
    net.loads_mut().push(DistLoad::new(
        "d",
        "b",
        names(&["z", "n"]),
        Configuration::Delta,
        vec![100.],
        vec![20.],
    ));
    net.ibrs_mut().push(DistIbr::new(
        "inv",
        "b",
        names(&["y", "z", "n"]),
        IbrTopology::ThreeLeg,
        IbrPrimeMover::Battery,
        vec![1000.; 3],
    ));
    let prep = prepare(net);
    let pairs = |name: &str| {
        prep.devices
            .iter()
            .find(|d| d.identity == name)
            .unwrap()
            .coils
            .iter()
            .map(|c| (c.positive, c.negative))
            .collect::<Vec<_>>()
    };
    assert_eq!(pairs("d"), vec![(2, Some(0))]);
    assert_eq!(pairs("inv"), vec![(1, Some(2)), (2, Some(0)), (0, Some(1))]);
}

#[test]
fn delta_terminal_roles_distinguish_numeric_neutral_from_grounded_phase() {
    let mut net = three_phase_source();
    net.buses_mut()[0].terminals = names(&["1", "2", "3", "4"]);
    net.buses_mut()[0].grounded = names(&["4"]);
    net.sources_mut()[0].terminal_map = names(&["1", "2", "3", "4"]);
    net.loads_mut().push(DistLoad::new(
        "bad",
        "b",
        names(&["1", "4"]),
        Configuration::Delta,
        vec![100.],
        vec![20.],
    ));
    assert_delta_role_error(net, "4");

    let mut net = three_phase_source();
    net.buses_mut()[0].grounded.push("c".into());
    net.sources_mut()[0].v_magnitude[2] = 0.;
    net.loads_mut().push(DistLoad::new(
        "d",
        "b",
        names(&["a", "c"]),
        Configuration::Delta,
        vec![100.],
        vec![20.],
    ));
    let prep = prepare(net);
    assert_eq!(prep.devices[0].coils[0].negative, Some(2));
    assert!(prep.terminals[2].grounded);
}

#[test]
fn fixed_pf_coefficients_are_bounded_at_unity_and_near_zero() {
    use powerio_dist::{DistControlProfile, DistIbr, IbrPrimeMover, IbrTopology};
    for magnitude in [
        1.,
        0.8,
        f64::from_bits(1.0_f64.to_bits() - 1),
        1e-18,
        1e-300,
        f64::from_bits(1),
    ] {
        for sign in [-1., 1.] {
            let pf = magnitude * sign;
            let mut net = network();
            let mut inv = DistIbr::new(
                "inv",
                "load",
                names(&["a", "n"]),
                IbrTopology::SinglePhase,
                IbrPrimeMover::Battery,
                vec![1000.],
            );
            inv.control_profile = Some("pf".into());
            net.ibrs_mut().push(inv);
            let mut profile = DistControlProfile::new("pf");
            profile.power_factor =
                Some(serde_json::from_value(serde_json::json!({"pf":pf})).unwrap());
            net.control_profiles_mut().push(profile);
            let prep = prepare(net);
            let coil = &prep
                .devices
                .iter()
                .find(|d| d.identity == "inv")
                .unwrap()
                .coils[0];
            let [a, b] = coil.power_factor_coefficients.unwrap();
            assert_eq!(b.to_bits(), pf.to_bits());
            assert!(a.is_finite() && (0.0..=1.0).contains(&a));
            assert!((a.hypot(b) - 1.).abs() < 2e-16);
            // Choose an analytic unit-power point without dividing by PF.
            assert!((a * magnitude + b * (-sign * a)).abs() < 2e-16);
            if magnitude < 1e-15 {
                assert_eq!(a.to_bits(), 1.0_f64.to_bits());
            }
            if magnitude.to_bits() == 0.8_f64.to_bits() {
                assert!((a - 0.6).abs() < 2e-16);
            }
            if let Some(slope) = coil.reactive_slope {
                assert!(slope.is_finite());
                assert!((slope * pf + a).abs() < 2e-16);
            } else {
                assert!((-a / pf).is_infinite());
            }
            // Even the smallest positive subnormal PF yields serializable data.
            let decoded: powerio_matrix::McAcOpfPreparation =
                serde_json::from_str(&serde_json::to_string(&prep).unwrap()).unwrap();
            assert_eq!(
                decoded
                    .devices
                    .iter()
                    .find(|d| d.identity == "inv")
                    .unwrap()
                    .coils[0]
                    .power_factor_coefficients
                    .unwrap()[1]
                    .to_bits(),
                pf.to_bits()
            );
        }
    }
}

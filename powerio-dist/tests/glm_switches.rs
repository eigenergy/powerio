use num_complex::Complex64;
use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};
use powerio_dist::{DistLocationTarget, MulticonductorNetwork};

fn deck(module: &str, phases: &str, properties: &str) -> String {
    format!(
        "module powerflow {{ {module} }}
object node {{ name source; phases ABCN; bustype SWING; nominal_voltage 240; }}
object load {{ name sink; phases ABCN; nominal_voltage 240; constant_impedance_A 20+5j; }}
object switch {{ name contact; phases {phases}N; from source; to sink; {properties} }}"
    )
}

fn read(text: &str) -> MulticonductorNetwork {
    parse(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap()
    .into_value()
}

fn impedances(network: &MulticonductorNetwork) -> Vec<(String, Complex64)> {
    let mut result = Vec::new();
    for line in network.lines() {
        let code = network
            .line_codes()
            .iter()
            .find(|c| c.name == line.linecode)
            .unwrap();
        for (i, terminal) in line.terminal_map_from.iter().enumerate() {
            result.push((
                terminal.clone(),
                Complex64::new(code.r_series[i][i], code.x_series[i][i]) * line.length,
            ));
            for j in 0..code.n_conductors {
                if i != j {
                    assert_eq!((code.r_series[i][j], code.x_series[i][j]), (0.0, 0.0));
                }
            }
        }
    }
    result
}

#[test]
fn finite_contacts_preserve_native_solver_defaults_and_alias_assignment_order() {
    for (module, properties, expected) in [
        ("solver_method NR;", "", Complex64::new(1e-4, -1e-4)),
        (
            "solver_method NR;",
            "switch_impedance 0+0j;",
            Complex64::new(1e-4, 1e-4),
        ),
        (
            "solver_method NR; default_resistance 0.02;",
            "switch_impedance -1-2j;",
            Complex64::new(0.02, -0.02),
        ),
        (
            "solver_method FBS;",
            "switch_impedance 1+0.5j;",
            Complex64::new(1.0, 0.5),
        ),
        (
            "solver_method NR;",
            "switch_impedance 2+3j; switch_resistance 4000 mOhm; switch_reactance 5;",
            Complex64::new(4.0, 5.0),
        ),
        (
            "solver_method NR;",
            "switch_impedance 1+1j; switch_resistance 4; switch_impedance 6+7j;",
            Complex64::new(6.0, 7.0),
        ),
    ] {
        for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
            let source = deck(module, "AC", properties);
            let module = parse(
                Source::from_memory("case.glm", source.as_bytes()).unwrap(),
                PreprocessOptions {
                    dialect,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                impedances(module.value()),
                vec![("1".into(), expected), ("3".into(), expected)]
            );
            assert_eq!(module.value().switches().as_slice(), []);
            assert!(
                module
                    .diagnostics()
                    .iter()
                    .any(|d| d.code() == "READ.GLM.SWITCH_EQUIVALENT")
            );
        }
    }
}

#[test]
fn phase_states_follow_status_and_banked_initialization_without_controls() {
    for (phases, properties, expected_closed) in [
        (
            "ABC",
            "operating_mode INDIVIDUAL; phase_B_state OPEN;",
            vec!["1", "3"],
        ),
        ("ABC", "phase_B_state OPEN;", vec!["1", "2", "3"]),
        ("ABC", "phase_A_state OPEN; phase_B_state OPEN;", vec![]),
        ("ABC", "status OPEN; phase_A_state CLOSED;", vec![]),
        ("AB", "phase_B_state OPEN;", vec!["1", "2"]),
        ("AB", "status OPEN; phase_B_state OPEN;", vec![]),
        ("C", "", vec!["3"]),
    ] {
        let net = read(&deck("solver_method NR;", phases, properties));
        assert_eq!(
            impedances(&net)
                .iter()
                .map(|(t, _)| t.as_str())
                .collect::<Vec<_>>(),
            expected_closed
        );
        assert!(net.switches().iter().all(|s| s.open));
        let count = net
            .lines()
            .iter()
            .map(|l| l.terminal_map_from.len())
            .sum::<usize>()
            + net
                .switches()
                .iter()
                .map(|s| s.terminal_map_from.len())
                .sum::<usize>();
        assert_eq!(count, phases.len());
    }
}

#[test]
fn source_free_ir_and_edited_output_keep_contact_physics_and_points() {
    let net = read(&deck(
        "solver_method NR;",
        "ABC",
        "operating_mode INDIVIDUAL; phase_B_state OPEN; latitude 0;",
    ));
    assert_eq!(net.component_locations().len(), 2);
    assert!(
        net.component_locations()
            .iter()
            .any(|p| p.target == DistLocationTarget::Line)
    );
    assert!(
        net.component_locations()
            .iter()
            .any(|p| p.target == DistLocationTarget::Switch)
    );
    let mut restored: MulticonductorNetwork =
        serde_json::from_value(serde_json::to_value(&net).unwrap()).unwrap();
    restored.line_codes_mut()[0].r_series[0][0] = 0.25;
    let result = emit_canonical(&restored, Destination::memory("fresh.glm").unwrap()).unwrap();
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("not memory")
    };
    let reparsed = read(std::str::from_utf8(artifacts[0].bytes()).unwrap());
    let actual = impedances(&reparsed);
    for ((_, got), (_, expected)) in actual.iter().zip(impedances(&restored)) {
        assert!((*got - expected).norm() < 1e-12);
    }
    assert_eq!(reparsed.switches()[0].terminal_map_from, ["2"]);
    assert!(reparsed.switches()[0].open);
    assert_eq!(reparsed.component_locations().len(), 2);
    assert!(
        reparsed
            .component_locations()
            .iter()
            .all(|p| p.point.latitude == Some(0.0) && p.point.longitude.is_none())
    );
}

#[test]
fn unspecified_fbs_contacts_stay_ideal_in_source_free_output() {
    for module in [
        "",
        "solver_method FBS;",
        "solver_method NR; solver_method FBS;",
    ] {
        let net = read(&deck(module, "ABC", ""));
        assert_eq!(net.lines().as_slice(), []);
        assert_eq!(net.switches().len(), 1);
        assert!(!net.switches()[0].open);
        let output = canonical(&net);
        assert!(output.contains("solver_method FBS"));
        assert!(output.contains("switch_impedance 0+0j"));
        let restored = read(&output);
        assert_eq!(restored.lines().as_slice(), []);
        assert!(!restored.switches()[0].open);
    }
}

fn canonical(net: &MulticonductorNetwork) -> String {
    let result = emit_canonical(net, Destination::memory("fresh.glm").unwrap()).unwrap();
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("not memory")
    };
    String::from_utf8(artifacts[0].bytes().to_vec()).unwrap()
}

fn ideal_tree() -> MulticonductorNetwork {
    read("module powerflow { solver_method FBS; }
object node { name source; phases ABCN; bustype SWING; nominal_voltage 240; }
object node { name middle; phases ABCN; nominal_voltage 240; }
object load { name sink; phases ABCN; nominal_voltage 240; constant_impedance_A 20+5j; }
object line_configuration { name code; z11 1+0.5j; z22 1+0.5j; z33 1+0.5j; }
object overhead_line { name upstream; phases ABCN; from middle; to source; length 1 mile; configuration code; }
object switch { name contact; phases ABCN; from sink; to middle; operating_mode INDIVIDUAL; phase_B_state OPEN; latitude 0; }")
}

#[test]
fn ideal_output_orients_edges_groups_disjoint_contacts_and_retains_points() {
    let net = ideal_tree();
    let before = serde_json::to_value(&net).unwrap();
    let restored: MulticonductorNetwork = serde_json::from_value(before.clone()).unwrap();
    let output = canonical(&restored);
    let again = read(&output);
    assert_eq!(again.lines()[0].bus_from, "source");
    assert_eq!(again.lines()[0].bus_to, "middle");
    assert_eq!(again.switches().len(), 2);
    for switch in again.switches() {
        assert_eq!(switch.bus_from, "middle");
        assert_eq!(switch.bus_to, "sink");
        assert_eq!(switch.open, switch.terminal_map_from == ["2"]);
    }
    assert_eq!(again.component_locations().len(), 2);
    assert!(
        again
            .component_locations()
            .iter()
            .all(|p| p.point.latitude == Some(0.0))
    );
    let repeated = read(&canonical(&again));
    for (a, b) in again.switches().iter().zip(repeated.switches()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.terminal_map_from, b.terminal_map_from);
        assert_eq!(a.open, b.open);
    }
    assert_eq!(serde_json::to_value(&net).unwrap(), before);
}

#[test]
fn ideal_output_rejects_unqualified_topology_and_conflicting_points() {
    let base = ideal_tree();
    let mut cyclic = base.clone();
    let mut extra = cyclic.lines()[0].clone();
    extra.name = "extra".into();
    extra.bus_from = "source".into();
    extra.bus_to = "sink".into();
    cyclic.lines_mut().push(extra);
    let mut parallel = base.clone();
    let mut contact = parallel.switches()[0].clone();
    contact.name = "parallel".into();
    parallel.switches_mut().push(contact);
    let incomplete = read(&deck("solver_method FBS;", "A", ""));
    let mut conflicting = base.clone();
    conflicting.component_locations_mut()[0].point.latitude = Some(1.0);
    let mut multiple_sources = base.clone();
    let mut source = multiple_sources.sources()[0].clone();
    source.name = "middle_source".into();
    source.bus = "middle".into();
    multiple_sources.sources_mut().push(source);
    let mut open_tie = base.clone();
    open_tie.switches_mut().push(powerio_dist::DistSwitch::new(
        "open_tie",
        "source",
        "sink",
        vec!["1".into()],
        vec!["1".into()],
        true,
    ));
    for net in [
        cyclic,
        parallel,
        incomplete,
        conflicting,
        open_tie,
        multiple_sources,
    ] {
        assert!(emit_canonical(&net, Destination::memory("case.glm").unwrap()).is_err());
    }
}

#[test]
fn invalid_assignments_are_not_hidden_by_later_values_or_open_states() {
    for properties in [
        "phase_A_state invalid; phase_A_state CLOSED;",
        "operating_mode invalid; operating_mode BANKED;",
        "status invalid; status OPEN;",
        "status OPEN; switch_impedance NaN;",
        "event_schedule controller;",
    ] {
        let source = deck("solver_method NR;", "A", properties);
        assert!(
            parse(
                Source::from_memory("invalid.glm", source.as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err()
        );
    }
    for value in ["0", "-1", "NaN", "inf"] {
        let source = deck(&format!("default_resistance {value};"), "A", "");
        assert!(
            parse(
                Source::from_memory("invalid.glm", source.as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err()
        );
    }
}

#[test]
fn ideal_output_expands_noncommuting_shunts_without_dropping_capacitance() {
    let mut net = ideal_tree();
    let code = &mut net.line_codes_mut()[0];
    code.r_series[1][1] *= 2.0;
    for i in 0..3 {
        code.b_from[i][i] = 0.001;
        code.b_to[i][i] = 0.001;
    }
    let commuting = canonical(&net);
    assert!(commuting.contains("solver_method FBS"));
    assert!(commuting.contains("c11"));
    let code = &mut net.line_codes_mut()[0];
    code.b_from[0][1] = -0.0001;
    code.b_from[1][0] = -0.0001;
    code.b_to = code.b_from.clone();
    let output = canonical(&net);
    assert!(output.contains("constant_impedance_AB"));
    assert!(output.contains("__glm_shunt_from"));
    assert!(output.contains("__glm_shunt_to"));
    let again = read(&output);
    assert!(
        again.line_codes()[0]
            .b_from
            .iter()
            .flatten()
            .all(|b| *b == 0.0)
    );
    assert_eq!(
        again.component_locations().len(),
        net.component_locations().len()
    );
    assert!(
        again
            .component_locations()
            .iter()
            .all(|p| p.point.latitude == Some(0.0))
    );
}

#[test]
fn unequal_conductive_shunts_keep_physical_ends_under_fbs_reversal() {
    let mut net = ideal_tree();
    let line = net.lines()[0].clone();
    let code = &mut net.line_codes_mut()[0];
    code.g_from = vec![
        vec![0.0002, -0.0001, 0.0],
        vec![-0.0001, 0.0003, -0.00005],
        vec![0.0, -0.00005, 0.0004],
    ];
    code.b_from = vec![
        vec![0.0006, -0.0002, 0.00003],
        vec![-0.0002, -0.0007, -0.00004],
        vec![0.00003, -0.00004, 0.0008],
    ];
    code.g_to = code
        .g_from
        .iter()
        .map(|r| r.iter().map(|v| v * 0.5).collect())
        .collect();
    code.b_to = code
        .b_from
        .iter()
        .map(|r| r.iter().map(|v| v * 1.5).collect())
        .collect();
    let code = code.clone();
    let before = serde_json::to_value(&net).unwrap();
    let output = canonical(&net);
    assert_eq!(before, serde_json::to_value(&net).unwrap());
    let again = read(&output);
    assert_eq!(again.lines()[0].bus_from, line.bus_to);
    for (bus, side, g, b) in [
        (&line.bus_from, "from", &code.g_from, &code.b_from),
        (&line.bus_to, "to", &code.g_to, &code.b_to),
    ] {
        let mut y = vec![vec![Complex64::default(); 3]; 3];
        for load in again.loads().iter().filter(|l| {
            l.name
                .starts_with(&format!("{}__glm_shunt_{side}", line.name))
        }) {
            assert_eq!(&load.bus, bus);
            let powerio_dist::DistLoadVoltageModel::ConstantImpedance { v_nom } =
                &load.voltage_model
            else {
                panic!("not impedance")
            };
            let admittance = Complex64::new(load.p_nom[0], -load.q_nom[0]) / v_nom[0].powi(2);
            let mut incidence = [0.0; 3];
            incidence[load.terminal_map[0].parse::<usize>().unwrap() - 1] = 1.0;
            if load.terminal_map[1] != "n" {
                incidence[load.terminal_map[1].parse::<usize>().unwrap() - 1] = -1.0;
            }
            for i in 0..3 {
                for j in 0..3 {
                    y[i][j] += admittance * incidence[i] * incidence[j];
                }
            }
        }
        for i in 0..3 {
            for j in 0..3 {
                assert!((y[i][j] - Complex64::new(g[i][j], b[i][j]) * line.length).norm() < 1e-12);
            }
        }
    }
    net.line_codes_mut()[0].g_from[0][1] *= 2.0;
    let error = emit_canonical(&net, Destination::memory("bad.glm").unwrap()).unwrap_err();
    assert!(error.to_string().contains("nonsymmetric"));
}

#[test]
fn source_fbs_noncommuting_line_reports_native_discrepancy() {
    let text = "module powerflow { solver_method FBS; line_capacitance TRUE; }
object node { name a; phases ABN; bustype SWING; nominal_voltage 240; }
object node { name b; phases ABN; nominal_voltage 240; }
object line_configuration { name c; z11 1+0j; z22 2+0j; c11 100; c22 100; c12 -10; c21 -10; }
object overhead_line { name l; phases ABN; from a; to b; configuration c; length 1 mile; }";
    for (solver, expected) in [("FBS", true), ("NR", false)] {
        let module = parse(
            Source::from_memory("case.glm", text.replace("FBS", solver).as_bytes()).unwrap(),
            PreprocessOptions::default(),
        )
        .unwrap();
        assert_eq!(
            module
                .diagnostics()
                .iter()
                .any(|d| d.code() == "READ.GLM.NATIVE_LINE_DISCREPANCY"),
            expected
        );
    }
}

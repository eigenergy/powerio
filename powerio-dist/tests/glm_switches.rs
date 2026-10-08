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
            assert!(module.value().switches().is_empty());
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
fn unspecified_fbs_contacts_stay_ideal_without_an_unverified_writer_approximation() {
    for module in [
        "",
        "solver_method FBS;",
        "solver_method NR; solver_method FBS;",
    ] {
        let net = read(&deck(module, "A", ""));
        assert!(net.lines().is_empty());
        assert_eq!(net.switches().len(), 1);
        assert!(!net.switches()[0].open);
        let error = emit_canonical(&net, Destination::memory("case.glm").unwrap()).unwrap_err();
        assert!(error.to_string().contains("ideal closed switch"));
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

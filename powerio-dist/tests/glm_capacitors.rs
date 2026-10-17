use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{PreprocessOptions, emit_canonical, parse};

fn deck() -> &'static str {
    "module powerflow { solver_method NR; }
object node { name source; phases ABCN; bustype SWING; nominal_voltage 240; }
object capacitor { name cap; parent source; phases ABC; phases_connected ABC;
nominal_voltage 240; cap_nominal_voltage 120; control MANUAL;
capacitor_A 1440; capacitor_B 2880; capacitor_C 4320;
switchA CLOSED; switchB OPEN; switchC CLOSED; latitude 0; longitude 151; }"
}
fn read(
    text: &str,
) -> Result<powerio_core::PioModule<powerio_dist::MulticonductorNetwork>, powerio_core::Error> {
    parse(
        Source::from_memory("capacitor.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
}
#[test]
fn capacitor_coils_preserve_states_rating_voltage_and_edited_coordinates() {
    let module = read(deck()).unwrap();
    let mut net = module.into_value();
    assert_eq!(net.shunts().len(), 3);
    for (shunt, b) in net.shunts().iter().zip([0.1, 0.0, 0.3]) {
        assert!((shunt.b[0][0] - b).abs() < 1e-15);
        assert_eq!(shunt.bus, "source");
    }
    assert_eq!(net.component_locations().len(), 3);
    net.shunts_mut()[0].b[0][0] *= 1.25;
    let output = emit_canonical(&net, Destination::memory("out.glm").unwrap()).unwrap();
    let EmittedOutput::Memory { artifacts } = output.output() else {
        panic!()
    };
    let back = read(std::str::from_utf8(artifacts[0].bytes()).unwrap()).unwrap();
    let active = net
        .shunts()
        .iter()
        .filter(|s| s.name != "cap__glm_B")
        .collect::<Vec<_>>();
    assert_eq!(
        serde_json::to_value(active).unwrap(),
        serde_json::to_value(back.value().shunts()).unwrap()
    );
    let idle = back
        .value()
        .loads()
        .iter()
        .find(|l| l.name == "cap__glm_B")
        .unwrap();
    assert_eq!(idle.bus, "source");
    assert_eq!(idle.terminal_map, ["2", "n"]);
    assert_eq!(idle.p_nom[0].to_bits(), 0.0_f64.to_bits());
    assert_eq!(idle.q_nom[0].to_bits(), 0.0_f64.to_bits());
    let mut points = net.component_locations().clone();
    points
        .iter_mut()
        .find(|p| p.name == "cap__glm_B")
        .unwrap()
        .target = powerio_dist::DistLocationTarget::Load;
    assert_eq!(&points, back.value().component_locations());
}
#[test]
fn omitted_switch_is_open_and_omitted_rating_voltage_uses_node_voltage() {
    let input = deck()
        .replace("cap_nominal_voltage 120;", "")
        .replace("switchA CLOSED;", "");
    let m = read(&input).unwrap();
    assert_eq!(m.value().shunts()[0].b, [vec![0.0]]);
    assert!((m.value().shunts()[2].b[0][0] - 0.075).abs() < 1e-15);
}
#[test]
fn unqualified_capacitor_physics_and_controls_are_not_silently_ignored() {
    for (from, to) in [
        ("control MANUAL", "control UNKNOWN"),
        ("phases_connected ABC", "phases_connected ABCD"),
        ("switchA CLOSED", "switchA UNKNOWN"),
        ("capacitor_A 1440", "capacitor_A -1440"),
    ] {
        assert!(read(&deck().replace(from, to)).is_err(), "{to}");
    }
    let mut net = read(deck()).unwrap().into_value();
    net.shunts_mut()[0].g[0][0] = 1.0;
    assert!(emit_canonical(&net, Destination::memory("out.glm").unwrap()).is_err());
}

#[test]
fn controlled_capacitors_require_declared_individual_states_and_report_freezing() {
    let manual = read(deck()).unwrap();
    for control in ["VAR", "VOLT", "VARVOLT", "CURRENT"] {
        let source = deck().replace("control MANUAL", &format!("control {control}; control_level INDIVIDUAL; pt_phase ABC; voltage_set_high .25 kV; voltage_set_low 230 V; VAr_set_high 475 kVAr; VAr_set_low -350 kVAr; time_delay 480 s; lockout_time 1 s"));
        let m = read(&source).unwrap();
        assert_eq!(
            serde_json::to_value(m.value()).unwrap(),
            serde_json::to_value(manual.value()).unwrap()
        );
        assert!(
            m.diagnostics()
                .iter()
                .any(|d| d.code() == "READ.GLM.CONTROL_FROZEN")
        );
        assert!(read(&source.replace("switchB OPEN;", "")).is_err());
        assert!(read(&source.replace("control_level INDIVIDUAL", "control_level BANK")).is_err());
        assert!(read(&source.replace("480 s", "broken s")).is_err());
    }
}

#[test]
fn single_phase_manual_bank_inherits_parent_voltage_unless_rating_is_explicit() {
    let text = "module powerflow { solver_method NR; }
object node { name source; phases ABCN; bustype SWING; nominal_voltage 240; }
object capacitor { name cap; parent source; phases BN; nominal_voltage 120;
control MANUAL; control_level BANK; pt_phase B; capacitor_B 1440; switchB CLOSED; }";
    let m = read(text).unwrap();
    assert_eq!(m.value().shunts().len(), 1);
    let cap = &m.value().shunts()[0];
    assert_eq!(cap.terminal_map, ["2"]);
    assert!((cap.b[0][0] - 0.025).abs() < 1e-15);
    let explicit = read(&text.replace(
        "nominal_voltage 120;",
        "nominal_voltage 120; cap_nominal_voltage 120;",
    ))
    .unwrap();
    assert!((explicit.value().shunts()[0].b[0][0] - 0.1).abs() < 1e-15);
    assert_eq!(cap.bus, "source");
    assert!(read(&text.replace("pt_phase B", "pt_phase A")).is_err());
    assert!(read(&text.replace("nominal_voltage 120", "nominal_voltage 0")).is_err());
}

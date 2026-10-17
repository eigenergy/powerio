use std::collections::BTreeMap;

use num_complex::Complex64;
use powerio_core::{Destination, EmittedOutput, Fidelity, Source};
use powerio_dist::glm::{PreprocessOptions, emit_canonical, parse};
use powerio_dist::{
    Configuration, DistBus, DistLine, DistLineCode, DistLoad, DistLoadVoltageModel,
    MulticonductorNetwork, VoltageSource,
};

fn network() -> MulticonductorNetwork {
    let mut net = MulticonductorNetwork::new();
    for name in ["source", "sink"] {
        let mut bus = DistBus::new(name, vec!["3".into(), "1".into(), "n".into()]);
        bus.grounded.push("n".into());
        net.buses_mut().push(bus);
    }
    net.sources_mut().push(VoltageSource::new(
        "source",
        "source",
        vec!["3".into(), "1".into()],
        vec![235.0, 240.0],
        vec![2.1, 0.1],
    ));
    net.line_codes_mut().push(DistLineCode::new(
        "code",
        vec![vec![0.0011, 0.0001], vec![0.0001, 0.0012]],
        vec![vec![0.0005, 0.0002], vec![0.0002, 0.0006]],
    ));
    net.lines_mut().push(DistLine::new(
        "line",
        "source",
        "sink",
        vec!["3".into(), "1".into()],
        vec!["3".into(), "1".into()],
        "code",
        1000.0,
    ));
    for (name, model) in [
        (
            "p",
            DistLoadVoltageModel::ConstantPower {
                v_nom: vec![120.0, 240.0],
            },
        ),
        (
            "i",
            DistLoadVoltageModel::ConstantCurrent {
                v_nom: vec![120.0, 240.0],
            },
        ),
        (
            "z",
            DistLoadVoltageModel::ConstantImpedance {
                v_nom: vec![120.0, 240.0],
            },
        ),
    ] {
        let mut load = DistLoad::new(
            name,
            "sink",
            vec!["3".into(), "1".into(), "n".into()],
            Configuration::Wye,
            vec![100.0, 200.0],
            vec![20.0, 50.0],
        );
        load.voltage_model = model;
        net.loads_mut().push(load);
    }
    net
}

fn text(net: &MulticonductorNetwork) -> String {
    let emission = emit_canonical(net, Destination::memory("case.glm").unwrap()).unwrap();
    assert_eq!(emission.fidelity(), Fidelity::Canonical);
    let EmittedOutput::Memory { artifacts } = emission.output() else {
        panic!("expected memory output")
    };
    String::from_utf8(artifacts[0].bytes().to_vec()).unwrap()
}

fn reparse(text: &str) -> MulticonductorNetwork {
    parse(
        Source::from_memory("fresh.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap()
    .into_value()
}

fn laws(net: &MulticonductorNetwork) -> BTreeMap<(String, String, u8), Complex64> {
    let mut result = BTreeMap::new();
    for load in net.loads() {
        let exponent = match load.voltage_model {
            DistLoadVoltageModel::ConstantPower { .. } => 0,
            DistLoadVoltageModel::ConstantCurrent { .. } => 1,
            DistLoadVoltageModel::ConstantImpedance { .. } => 2,
            _ => panic!("unexpected law"),
        };
        for (i, terminal) in load.terminal_map[..load.p_nom.len()].iter().enumerate() {
            let coefficient = Complex64::new(load.p_nom[i], load.q_nom[i])
                / load.voltage_model.v_nom()[i].powi(i32::from(exponent));
            *result
                .entry((load.bus.clone(), terminal.clone(), exponent))
                .or_default() += coefficient;
        }
    }
    result
}

#[test]
fn source_free_output_preserves_phase_order_and_nonuniform_load_voltage_laws() {
    let net = network();
    let restored = reparse(&text(&net));
    let wanted = laws(&net);
    let got = laws(&restored);
    assert_eq!(
        wanted.keys().collect::<Vec<_>>(),
        got.keys().collect::<Vec<_>>()
    );
    for (key, value) in wanted {
        assert!((got[&key] - value).norm() < 1e-12 * value.norm().max(1.0));
    }
    assert_eq!(restored.lines()[0].terminal_map_from, ["1", "3"]);
    let code = &restored.line_codes()[0];
    assert!((code.r_series[0][0] - net.line_codes()[0].r_series[1][1]).abs() < 1e-15);
    assert!((restored.sources()[0].v_magnitude[0] - 240.0).abs() < 1e-12);
    assert!((restored.sources()[0].v_angle[0] - 0.1).abs() < 1e-12);
    assert_eq!(
        restored.loads().iter().map(|l| &l.name).collect::<Vec<_>>(),
        net.loads().iter().map(|l| &l.name).collect::<Vec<_>>()
    );
}

#[test]
fn edits_change_fresh_output_and_zero_laws_keep_their_identity() {
    let mut net = network();
    let before = laws(&reparse(&text(&net)));
    net.loads_mut()[0].p_nom[0] = 0.0;
    net.loads_mut()[0].q_nom[0] = 0.0;
    net.loads_mut()[0].p_nom[1] = 0.0;
    net.loads_mut()[0].q_nom[1] = 0.0;
    let restored = reparse(&text(&net));
    assert_eq!(restored.loads().len(), 3);
    assert_eq!(restored.loads()[0].name, "p");
    let after = laws(&restored);
    assert_ne!(before, after);
    assert_eq!(after[&("sink".into(), "3".into(), 0)], Complex64::default());
}

#[test]
fn grounded_source_reference_is_an_exact_earth_equivalent() {
    let mut net = network();
    let baseline = text(&net);
    net.sources_mut()[0].reference_terminal = Some("n".into());
    assert_eq!(text(&net), baseline);
}

#[test]
fn unrepresented_physics_fails_before_writing_any_artifact() {
    let original = network();
    let mut cases = Vec::new();
    let mut net = original.clone();
    net.line_codes_mut()[0].g_from[0][0] = 0.01;
    cases.push(net);
    let mut net = original.clone();
    net.line_codes_mut()[0].b_from[0][0] = 0.01;
    cases.push(net);
    let mut net = original.clone();
    net.sources_mut()[0].v_magnitude[0] = 0.0;
    cases.push(net);
    let mut net = original.clone();
    net.sources_mut()[0].extras.insert("r1".into(), 0.01.into());
    cases.push(net);
    let mut net = original.clone();
    net.lines_mut()[0].terminal_map_to.reverse();
    cases.push(net);
    let mut net = original.clone();
    net.buses_mut()[0].grounded.clear();
    cases.push(net);
    let mut net = original.clone();
    net.loads_mut()[1].p_nom = vec![1e-200; 2];
    net.loads_mut()[1].q_nom = vec![0.0; 2];
    net.loads_mut()[1].voltage_model = DistLoadVoltageModel::ConstantCurrent {
        v_nom: vec![1e200; 2],
    };
    cases.push(net);
    let mut net = original.clone();
    net.lines_mut()[0].length = f64::MAX;
    cases.push(net);
    for net in cases {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("case.glm");
        assert!(emit_canonical(&net, Destination::path(&path)).is_err());
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[test]
fn non_electrical_projection_losses_are_reported() {
    let mut net = network();
    net.buses_mut()[0].v_max = Some(250.0);
    net.lines_mut()[0].i_max = Some(vec![100.0, 100.0]);
    let result = emit_canonical(&net, Destination::memory("case.glm").unwrap()).unwrap();
    assert_eq!(
        result
            .diagnostics()
            .iter()
            .filter(|d| d.code() == "EMIT.GLM.FIELD_DROPPED")
            .count(),
        2
    );
}

use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::{
    Configuration, DistBus, DistLine, DistLineCode, DistLoad, DistLoadVoltageModel,
    MulticonductorNetwork, VoltageSource,
};

fn network() -> MulticonductorNetwork {
    let mut net = MulticonductorNetwork::new();
    for name in ["source", "sink"] {
        let mut b = DistBus::new(name, vec!["1".into(), "n".into()]);
        b.grounded.push("n".into());
        net.buses_mut().push(b);
    }
    net.sources_mut().push(VoltageSource::new(
        "source",
        "source",
        vec!["1".into()],
        vec![7200.0],
        vec![0.0],
    ));
    net.line_codes_mut().push(DistLineCode::new(
        "code",
        vec![vec![6.25e-7]],
        vec![vec![6.25e-7]],
    ));
    net.lines_mut().push(DistLine::new(
        "line",
        "source",
        "sink",
        vec!["1".into()],
        vec!["1".into()],
        "code",
        1.0,
    ));
    let mut load = DistLoad::new(
        "load",
        "sink",
        vec!["1".into(), "n".into()],
        Configuration::SinglePhase,
        vec![0.0],
        vec![-432_000.0],
    );
    load.voltage_model = DistLoadVoltageModel::ConstantImpedance {
        v_nom: vec![7200.0],
    };
    net.loads_mut().push(load);
    net
}
fn text(output: &powerio_core::EmitResult) -> String {
    let EmittedOutput::Memory { artifacts } = output.output() else {
        panic!()
    };
    String::from_utf8(artifacts[0].bytes().to_vec()).unwrap()
}
#[test]
fn current_form_keeps_finite_impedance_and_does_not_mutate_input() {
    let net = network();
    let before = serde_json::to_value(&net).unwrap();
    let output =
        powerio_dist::glm::emit_canonical(&net, Destination::memory("case.glm").unwrap()).unwrap();
    let fresh = text(&output);
    assert!(fresh.contains("line__glm_current"));
    assert!(fresh.contains("default_maximum_voltage_error 1e-9"));
    assert!(fresh.contains("maximum_voltage_error 0;"));

    let again = powerio_dist::glm::parse(
        Source::from_memory("case.glm", fresh.as_bytes()).unwrap(),
        powerio_dist::glm::PreprocessOptions::default(),
    )
    .unwrap();
    let auxiliary = again
        .value()
        .loads()
        .iter()
        .find(|l| l.bus == "line__glm_current")
        .unwrap();
    let DistLoadVoltageModel::ConstantImpedance { v_nom } = &auxiliary.voltage_model else {
        panic!()
    };
    let y = num_complex::Complex64::new(auxiliary.p_nom[0], -auxiliary.q_nom[0]) / v_nom[0].powi(2);
    assert!((y - num_complex::Complex64::new(-6.25e-7, -6.25e-7)).norm() < 1e-18);
    assert_eq!(again.value().lines().len(), 2);
    assert_eq!(again.value().buses().len(), 3);
    assert_eq!(serde_json::to_value(&net).unwrap(), before);
    let dss = powerio_dist::emit(
        &powerio_core::PioModule::new(net),
        powerio_dist::DistTargetFormat::Dss,
        Destination::memory("case.dss").unwrap(),
    )
    .unwrap();
    let fresh = text(&dss);
    assert!(fresh.contains("_pio_current_line"));
    assert!(fresh.contains("model=2"));
}
#[test]
fn shunts_and_unqualified_extreme_impedances_keep_original_projection() {
    for change in [0, 1] {
        let mut net = network();
        let code = &mut net.line_codes_mut()[0];
        if change == 0 {
            code.b_from[0][0] = 1e-9;
            code.b_to[0][0] = 1e-9;
        } else {
            code.r_series[0][0] = 1e-10;
            code.x_series[0][0] = 1e-10;
        }
        let output =
            powerio_dist::glm::emit_canonical(&net, Destination::memory("case.glm").unwrap())
                .unwrap();
        assert!(!text(&output).contains("line__glm_current"));
    }
}

#[test]
fn generated_current_buses_do_not_replace_existing_bus_identities() {
    for (target, collision) in [
        (
            powerio_dist::DistTargetFormat::Gridlabd,
            "line__glm_current",
        ),
        (powerio_dist::DistTargetFormat::Dss, "_pio_current_line"),
    ] {
        let mut net = network();
        net.buses_mut()[1].id = collision.into();
        net.lines_mut()[0].bus_to = collision.into();
        net.loads_mut()[0].bus = collision.into();
        let output = powerio_dist::emit(
            &powerio_core::PioModule::new(net),
            target,
            Destination::memory("case").unwrap(),
        )
        .unwrap();
        let fresh = text(&output);
        let suffix = if target == powerio_dist::DistTargetFormat::Dss {
            "dss"
        } else {
            "glm"
        };
        let again = powerio_dist::parse(
            Source::from_memory(format!("case.{suffix}"), fresh.as_bytes()).unwrap(),
        )
        .unwrap();
        let ids: std::collections::BTreeSet<_> = again
            .value()
            .buses()
            .iter()
            .map(|b| b.id.as_str())
            .collect();
        assert!(ids.contains(collision));
        assert!(ids.len() >= 3);
        assert_eq!(
            again
                .value()
                .lines()
                .iter()
                .filter(|line| line.name == "line")
                .count(),
            1
        );
    }
}

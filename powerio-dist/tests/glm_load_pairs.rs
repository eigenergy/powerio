use num_complex::Complex64;
use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};
use powerio_dist::{Configuration, DistLoadVoltageModel, MulticonductorNetwork};

fn deck(properties: &str) -> String {
    format!("module powerflow {{ solver_method NR; }}
object node {{ name source; phases ABCN; bustype SWING; nominal_voltage 240; }}
object load {{ name demand; parent source; phases ABCN; nominal_voltage 240; latitude 0; {properties} }}")
}
fn read(text: &str) -> MulticonductorNetwork {
    parse(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap()
    .into_value()
}
fn fresh(net: &MulticonductorNetwork) -> String {
    let output = emit_canonical(net, Destination::memory("case.glm").unwrap()).unwrap();
    let EmittedOutput::Memory { artifacts } = output.output() else {
        panic!("not memory")
    };
    String::from_utf8(artifacts[0].bytes().to_vec()).unwrap()
}

#[test]
fn explicit_connections_normalize_power_current_and_impedance_in_both_profiles() {
    let a = Complex64::new(240.0, 0.0);
    let b = Complex64::new(-120.0, -120.0 * 3_f64.sqrt());
    let c = Complex64::new(-120.0, 120.0 * 3_f64.sqrt());
    for (suffix, terms, voltage) in [
        ("AB", ["1", "2"], a - b),
        ("BC", ["2", "3"], b - c),
        ("CA", ["3", "1"], c - a),
        ("AN", ["1", "n"], a),
        ("BN", ["2", "n"], b),
        ("CN", ["3", "n"], c),
    ] {
        for law in ["power", "current", "impedance"] {
            for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
                let text = deck(&format!("constant_{law}_{suffix} 2-0.5j;"));
                let net = parse(
                    Source::from_memory("case.glm", text.as_bytes()).unwrap(),
                    PreprocessOptions {
                        dialect,
                        ..Default::default()
                    },
                )
                .unwrap()
                .into_value();
                let load = &net.loads()[0];
                assert_eq!(load.name, "demand");
                assert_eq!(load.configuration, Configuration::SinglePhase);
                assert_eq!(load.terminal_map, terms);
                let value = Complex64::new(2.0, -0.5);
                let expected = match law {
                    "power" => value,
                    "current" => voltage * value.conj(),
                    _ => voltage.norm_sqr() / value.conj(),
                };
                assert!((Complex64::new(load.p_nom[0], load.q_nom[0]) - expected).norm() < 1e-9);
                let restored: MulticonductorNetwork =
                    serde_json::from_value(serde_json::to_value(&net).unwrap()).unwrap();
                let again = read(&fresh(&restored));
                assert_eq!(again.loads()[0].name, "demand");
                assert_eq!(again.loads()[0].terminal_map, terms);
                assert!(
                    (Complex64::new(again.loads()[0].p_nom[0], again.loads()[0].q_nom[0])
                        - expected)
                        .norm()
                        < 1e-9
                );
                assert_eq!(again.component_locations()[0].point.latitude, Some(0.0));
            }
        }
    }
}

#[test]
fn explicit_and_legacy_terms_add_instead_of_overwriting_each_other() {
    let net = read(&deck(
        "constant_power_A 100+10j; constant_power_AN 200+20j; constant_power_AB 300+30j; constant_impedance_CA 0+0j;",
    ));
    assert_eq!(net.loads().len(), 4);
    assert_eq!(net.component_locations().len(), 4);
    assert!((net.loads().iter().flat_map(|l| l.p_nom.iter()).sum::<f64>() - 600.0).abs() < 1e-12);
    let again = read(&fresh(&net));
    assert_eq!(again.loads().len(), 4);
    assert!(
        (again
            .loads()
            .iter()
            .flat_map(|l| l.p_nom.iter())
            .sum::<f64>()
            - 600.0)
            .abs()
            < 1e-12
    );
    assert_eq!(again.component_locations().len(), 4);
}

#[test]
fn delta_channels_keep_distinct_voltage_bases_and_reversed_terminal_incidence() {
    let mut net = read(&deck("constant_current_AB 1+0j;"));
    let load = &mut net.loads_mut()[0];
    load.configuration = Configuration::Delta;
    load.terminal_map = vec!["3".into(), "2".into(), "1".into()];
    load.p_nom = vec![100.0, 200.0, 300.0];
    load.q_nom = vec![20.0, 30.0, -40.0];
    load.voltage_model = DistLoadVoltageModel::ConstantCurrent {
        v_nom: vec![400.0, 410.0, 420.0],
    };
    let again = read(&fresh(&net));
    assert_eq!(again.loads().len(), 3);
    for (pair, expected) in [
        (["2", "3"], Complex64::new(100.0, 20.0) / 400.0),
        (["1", "2"], Complex64::new(200.0, 30.0) / 410.0),
        (["3", "1"], Complex64::new(300.0, -40.0) / 420.0),
    ] {
        let load = again
            .loads()
            .iter()
            .find(|l| l.terminal_map == pair)
            .unwrap();
        let DistLoadVoltageModel::ConstantCurrent { v_nom } = &load.voltage_model else {
            panic!("wrong law")
        };
        assert!(
            (Complex64::new(load.p_nom[0], load.q_nom[0]) / v_nom[0] - expected).norm() < 1e-12
        );
    }
}

#[test]
fn absent_endpoints_and_incomplete_delta_channels_fail() {
    let invalid = deck("constant_power_AB 0+0j;").replace(
        "name demand; parent source; phases ABCN",
        "name demand; parent source; phases AN",
    );
    assert!(
        parse(
            Source::from_memory("bad.glm", invalid.as_bytes()).unwrap(),
            PreprocessOptions::default()
        )
        .is_err()
    );
    let mut net = read(&deck("constant_power_AB 1+0j;"));
    net.loads_mut()[0].configuration = Configuration::Delta;
    net.loads_mut()[0].terminal_map = vec!["1".into(), "2".into(), "3".into()];
    assert!(emit_canonical(&net, Destination::memory("bad.glm").unwrap()).is_err());
}

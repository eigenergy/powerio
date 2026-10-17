use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{PreprocessOptions, emit_canonical, parse};

fn deck() -> &'static str {
    "module powerflow { solver_method NR; }
object node { name source; phases ABCN; bustype SWING; nominal_voltage 240; }
object load { name sink; phases ABCN; nominal_voltage 240; constant_impedance_A 20+5j; }
object regulator_configuration { name cfg; connect_type 1; Control MANUAL; Type A;
raise_taps 16; lower_taps 16; regulation .1; tap_pos_A 10; tap_pos_B 8; tap_pos_C 11; }
object regulator { name r; phases ABC; from source; to sink; configuration cfg; latitude 0; longitude 151; }"
}
fn read(
    text: &str,
) -> Result<powerio_core::PioModule<powerio_dist::MulticonductorNetwork>, powerio_core::Error> {
    parse(
        Source::from_memory("r.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
}
fn ratio(t: &powerio_dist::DistTransformer) -> f64 {
    let a = &t.windings[0];
    let b = &t.windings[1];
    a.v_ref * a.tap / (b.v_ref * b.tap)
}
fn resistance(t: &powerio_dist::DistTransformer) -> f64 {
    let b = &t.windings[1];
    b.r_pct / 100.0 * b.v_ref.powi(2) / b.s_rating
}
#[test]
fn fixed_phase_taps_and_coordinates_survive_edited_canonical_output() {
    let m = read(deck()).unwrap();
    assert_eq!(m.value().transformers().len(), 3);
    assert_eq!(
        m.value().transformers()[0].extras["rating_is_normalization_base"],
        true
    );
    for (t, tap) in m.value().transformers().iter().zip([10.0, 8.0, 11.0]) {
        assert!((ratio(t) - 1.0 / (1.0 + tap * 0.1 / 16.0)).abs() < 1e-14);
        assert!((resistance(t) - 1e-4).abs() < 1e-16);
    }
    assert_eq!(m.value().component_locations().len(), 3);
    let mut edited = m.into_value();
    edited.transformers_mut()[1].windings[0].tap *= 1.03;
    edited.transformers_mut()[1].windings[1].r_pct *= 2.0;
    let out = emit_canonical(&edited, Destination::memory("r.glm").unwrap()).unwrap();
    let EmittedOutput::Memory { artifacts } = out.output() else {
        panic!()
    };
    let again = read(std::str::from_utf8(artifacts[0].bytes()).unwrap()).unwrap();
    for (a, b) in edited
        .transformers()
        .iter()
        .zip(again.value().transformers())
    {
        assert_eq!(a.name, b.name);
        assert!((ratio(a) - ratio(b)).abs() < 1e-14);
        assert!((resistance(a) - resistance(b)).abs() < 1e-16);
    }
    assert_eq!(
        edited.component_locations(),
        again.value().component_locations()
    );
}
#[test]
fn object_taps_override_configuration_and_type_b_is_distinct() {
    let m = read(&deck().replace("Type A", "Type B").replace(
        "configuration cfg; latitude",
        "configuration cfg; tap_A -2; latitude",
    ))
    .unwrap();
    assert!((ratio(&m.value().transformers()[0]) - (1.0 + 2.0 * 0.1 / 16.0)).abs() < 1e-14);
    assert!((ratio(&m.value().transformers()[1]) - (1.0 - 8.0 * 0.1 / 16.0)).abs() < 1e-14);
}
#[test]
fn active_control_requires_stated_taps_and_warns_about_frozen_behavior() {
    let active = deck().replace("Control MANUAL", "Control OUTPUT_VOLTAGE");
    let m = read(&active).unwrap();
    assert!(
        m.diagnostics()
            .iter()
            .any(|d| d.code() == "READ.GLM.CONTROL_FROZEN")
    );
    assert!(read(&active.replace("tap_pos_B 8;", "")).is_err());
    let manual = read(&deck().replace("tap_pos_B 8;", "")).unwrap();
    assert!((ratio(&manual.value().transformers()[1]) - 1.0).abs() < 1e-14);
}
#[test]
fn native_singular_or_unqualified_regulator_profiles_fail() {
    for (a, b) in [
        ("regulation .1", "regulation -1.6"),
        ("raise_taps 16", "raise_taps 0"),
        ("tap_pos_A 10", "tap_pos_A 1.5"),
        (
            "configuration cfg; latitude",
            "configuration cfg; regulator_resistance 0; latitude",
        ),
        ("connect_type 1", "connect_type OPEN_DELTA_ABBC"),
    ] {
        assert!(read(&deck().replace(a, b)).is_err(), "accepted {b}");
    }
}

#[test]
fn dss_output_preserves_zero_reactance_without_scalar_setters() {
    let m = read(deck()).unwrap();
    let out = powerio_dist::emit(
        &powerio_core::PioModule::new(m.into_value()),
        powerio_dist::DistTargetFormat::Dss,
        Destination::memory("dss").unwrap(),
    )
    .unwrap();
    let EmittedOutput::Memory { artifacts } = out.output() else {
        panic!()
    };
    let text = std::str::from_utf8(artifacts[0].bytes()).unwrap();
    assert_eq!(text.matches("xscarray=(0)").count(), 3);
    assert!(!text.contains("xhl=0"));
    let m = powerio_dist::parse(Source::from_memory("r.dss", text.as_bytes()).unwrap()).unwrap();
    assert!(m.value().transformers().iter().all(|t| t.xsc_pct == [0.0]));
}

#[test]
fn native_dss_scalar_zero_traps_differ_from_array_zeros() {
    let prefix = "New Circuit.c bus1=source\nNew Transformer.t phases=1 windings=3 buses=[source.1.0 a.1.0 b.1.0] kvs=[.24 .12 .12] kvas=[10 10 10] %rs=[1 1 1] ";
    for (properties, expected) in [
        ("xhl=0 xht=0 xlt=0", [7.0, 35.0, 30.0]),
        ("x12=0 x13=0 x23=0", [7.0, 35.0, 30.0]),
        ("xscarray=[0 0 0]", [0.0, 0.0, 0.0]),
        ("xhl=0 xscarray=[0 0 0]", [7.0, 35.0, 30.0]),
        (
            "xhl=0\nEdit Transformer.t xscarray=[0 0 0]",
            [0.0, 0.0, 0.0],
        ),
    ] {
        let text = format!("{prefix}{properties}\n");
        let m =
            powerio_dist::parse(Source::from_memory("t.dss", text.as_bytes()).unwrap()).unwrap();
        assert_eq!(
            m.value().transformers()[0].xsc_pct,
            expected,
            "{properties}"
        );
    }
}

#[test]
fn regulator_unitless_suffix_is_ignored_with_a_diagnostic() {
    let ordinary = read(deck()).unwrap();
    let suffixed = read(&deck().replace("regulation .1;", "regulation .1 kV;")).unwrap();
    assert_eq!(
        ordinary.value().transformers(),
        suffixed.value().transformers()
    );
    assert!(
        suffixed
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.GLM.UNIT_IGNORED")
    );
    assert!(read(&deck().replace("regulation .1;", "regulation \".1 kV\";")).is_err());
}

#[test]
fn arras_regulation_uses_its_published_percent_unit() {
    use powerio_dist::glm::GlmDialect;
    let read_arras = |text: String| {
        parse(
            Source::from_memory("r.glm", text.as_bytes()).unwrap(),
            PreprocessOptions {
                dialect: GlmDialect::Arras4_3,
                ..Default::default()
            },
        )
    };
    assert!(read_arras(deck().replace("regulation .1;", "regulation .1 kV;")).is_err());
    let unit = read_arras(deck().replace("regulation .1;", "regulation .001 pu;")).unwrap();
    let ordinary = read(deck()).unwrap();
    for (a, b) in ordinary
        .value()
        .transformers()
        .iter()
        .zip(unit.value().transformers())
    {
        assert!((ratio(a) - ratio(b)).abs() < 1e-14);
    }
}

#[test]
fn fbs_regulators_keep_ideal_ratios_and_ignore_declared_series_resistance() {
    let baseline = read(deck()).unwrap();
    let text = deck()
        .replace("solver_method NR", "solver_method FBS")
        .replace(
            "configuration cfg; latitude",
            "configuration cfg; regulator_resistance 7.3; latitude",
        );
    let module = read(&text).unwrap();
    for (a, b) in baseline
        .value()
        .transformers()
        .iter()
        .zip(module.value().transformers())
    {
        assert!((ratio(a) - ratio(b)).abs() < 1e-14);
        assert!(b.windings.iter().all(|w| w.r_pct == 0.0));
        assert_eq!(b.xsc_pct, [0.0]);
    }
    assert_eq!(
        baseline.value().component_locations(),
        module.value().component_locations()
    );
    assert!(emit_canonical(module.value(), Destination::memory("ideal.glm").unwrap()).is_err());
    let output = powerio_dist::emit(
        &powerio_core::PioModule::new(module.into_value()),
        powerio_dist::DistTargetFormat::Dss,
        Destination::memory("ideal.dss").unwrap(),
    )
    .unwrap();
    assert_eq!(
        output
            .diagnostics()
            .iter()
            .filter(|d| d.code() == "EMIT.DSS.IDEAL_TRANSFORMER_EQUIVALENT")
            .count(),
        3
    );
    let EmittedOutput::Memory { artifacts } = output.output() else {
        panic!()
    };
    let text = std::str::from_utf8(artifacts[0].bytes()).unwrap();
    assert_eq!(text.matches("rmatrix=(-1)").count(), 3);
}

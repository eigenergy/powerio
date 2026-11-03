use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, parse};
use powerio_dist::{DistLocationTarget, DistTargetFormat};

fn deck() -> &'static str {
    "module powerflow { solver_method FBS; }
object node { name source; phases BN; bustype SWING; nominal_voltage 7200; }
object triplex_node { name head; phases BS; nominal_voltage 120; }
object triplex_load { name sink; phases BS; nominal_voltage 120; constant_power_1 1945+487.5j; constant_power_2 800+200j; }
object transformer_configuration { name xf; connect_type SINGLE_PHASE_CENTER_TAPPED; primary_voltage 7200; secondary_voltage 120; powerB_rating 15; impedance .006+.0136j; }
object transformer { name xfmr; phases BS; from source; to head; configuration xf; }
object triplex_line_conductor { name wire; resistance 1.535 Ohm/mile; geometric_mean_radius .0111 ft; rating.summer.emergency 315 A; }
object triplex_line_conductor { name neutral; resistance 2.0 Ohm/mile; geometric_mean_radius .01 ft; }
object triplex_line_configuration { name cable; conductor_1 wire; conductor_2 wire; conductor_N neutral; diameter .368 in; insulation_thickness .08 in; latitude -27; longitude 153; }
object triplex_line { name service; from head; to sink; phases BS; length 100 ft; configuration cable; latitude -27.1; longitude 153.1; }"
}

fn read(
    text: &str,
    dialect: GlmDialect,
) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    parse(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn triplex_geometry_keeps_three_conductors_and_grounded_neutral_at_both_ends() {
    for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
        let m = read(deck(), dialect);
        let n = m.value();
        let line = &n.lines()[0];
        assert_eq!(line.terminal_map_from, ["1", "2", "n"]);
        assert_eq!(line.terminal_map_to, ["1", "2", "n"]);
        assert_eq!(line.bus_from, "head");
        assert_eq!(line.bus_to, "sink");
        assert!((line.length - 30.48).abs() < 1e-12);
        for id in ["head", "sink"] {
            let bus = n.buses().iter().find(|b| b.id == id).unwrap();
            assert_eq!(bus.grounded, ["n"]);
        }
        let code = &n.line_codes()[0];
        assert_eq!(code.n_conductors, 3);
        assert!((code.r_series[0][0] * 1609.344 - (1.535 + 0.001_588_36 * 60.0)).abs() < 1e-12);
        assert!((code.r_series[2][2] * 1609.344 - (2.0 + 0.001_588_36 * 60.0)).abs() < 1e-12);
        assert!(code.r_series[0][2] > 0.0);
        assert_eq!(code.r_series[0][2].to_bits(), code.r_series[2][0].to_bits());
        assert_eq!(code.x_series[0][2].to_bits(), code.x_series[2][0].to_bits());
        assert!(
            code.source
                .as_ref()
                .unwrap()
                .contains("neutral grounded at both endpoints")
        );
        assert!(
            n.component_locations()
                .iter()
                .any(|p| p.target == DistLocationTarget::Line && p.name == "service")
        );
        assert!(
            n.component_locations()
                .iter()
                .any(|p| p.target == DistLocationTarget::LineCode && p.name == code.name)
        );
        assert!(
            m.diagnostics()
                .iter()
                .any(|d| d.code() == "READ.GLM.SOURCE_ONLY" && d.message().contains("ampacit"))
        );
        // No guessed limit: native seasonal/cumulative limit selection remains
        // a separate mapping gap, explicitly reported at import.
        assert!(code.i_max.is_none());
    }
}

#[test]
fn triplex_geometry_units_do_not_change_the_circuit() {
    let text = deck()
        .replace("100 ft", "30.48 m")
        .replace(".368 in", "0.93472 cm")
        .replace(".08 in", "0.2032 cm")
        .replace(".0111 ft", "0.338328 cm");
    let a = read(deck(), GlmDialect::Pnnl5_3).into_value();
    let b = read(&text, GlmDialect::Pnnl5_3).into_value();
    assert!((a.lines()[0].length - b.lines()[0].length).abs() < 1e-12);
    for (a, b) in a.line_codes()[0]
        .x_series
        .iter()
        .flatten()
        .zip(b.line_codes()[0].x_series.iter().flatten())
    {
        assert!((a - b).abs() < 1e-14);
    }
}

#[test]
fn invalid_or_incomplete_triplex_geometry_fails_before_returning_a_network() {
    for (old, new) in [
        ("conductor_N neutral;", ""),
        ("conductor_N neutral", "conductor_N xf"),
        ("resistance 2.0", "resistance 0"),
        (".01 ft", "1e-320 ft"),
        ("diameter .368", "diameter -1"),
        ("insulation_thickness .08", "insulation_thickness 0"),
        ("length 100", "length 0"),
        ("name cable;", "name cable; z11 1+1j;"),
        ("name sink; phases BS", "name sink; phases CS"),
        (
            "rating.summer.emergency 315 A",
            "rating.summer.emergency -315 A",
        ),
    ] {
        let text = deck().replace(old, new);
        assert!(
            parse(
                Source::from_memory("bad.glm", text.as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err(),
            "accepted {new}"
        );
    }
}

#[test]
fn explicit_and_implicit_upstream_fbs_sources_give_the_same_connected_service() {
    let text = deck().replace("bustype SWING;", "");
    let explicit = read(deck(), GlmDialect::Pnnl5_3).into_value();
    let implicit = read(&text, GlmDialect::Pnnl5_3).into_value();
    assert_eq!(explicit.sources(), implicit.sources());
    assert_eq!(explicit.lines(), implicit.lines());
    assert_eq!(explicit.transformers(), implicit.transformers());
}

#[test]
fn triplex_source_round_trip_retains_seasonal_ratings_and_geometry_exactly() {
    let m = read(deck(), GlmDialect::Pnnl5_3);
    let result = powerio_dist::emit(
        &m,
        DistTargetFormat::Gridlabd,
        Destination::memory("case.glm").unwrap(),
    )
    .unwrap();
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!()
    };
    assert_eq!(artifacts[0].bytes(), deck().as_bytes());
}

#[test]
fn direct_triplex_matrix_preserves_native_leg_orientation_and_dss_ports() {
    let text = deck().replace(
        "name cable;",
        "name cable; z11 2.16454+.880800j; z12 .623542+.673688j; z21 .623542+.673688j; z22 2.16454+.880800j;",
    );
    for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
        let m = read(&text, dialect);
        let n = m.value();
        let line = &n.lines()[0];
        assert_eq!(line.terminal_map_from, ["1", "2"]);
        assert_eq!(line.terminal_map_to, ["1", "2"]);
        let code = &n.line_codes()[0];
        assert_eq!(code.n_conductors, 2);
        for (row, expected) in [[2.16454, 0.623_542], [-0.623_542, -2.16454]]
            .iter()
            .enumerate()
        {
            for (col, value) in expected.iter().enumerate() {
                assert!((code.r_series[row][col] * 1609.344 - value).abs() < 1e-12);
            }
        }
        assert!(
            m.diagnostics()
                .iter()
                .any(|d| d.code() == "READ.GLM.REDUCED_TRIPLEX")
        );
        let out = powerio_dist::emit(
            &m,
            DistTargetFormat::Dss,
            Destination::memory("case.dss").unwrap(),
        )
        .unwrap();
        let EmittedOutput::Memory { artifacts } = out.output() else {
            panic!("expected DSS bytes")
        };
        let dss = std::str::from_utf8(artifacts[0].bytes()).unwrap();
        assert!(dss.contains("New Vsource._pio_coupled_service phases=3 bus1=head.1.2.0"));
        assert!(dss.contains("New Line.service phases=2 bus1=_pio_pair_0.1.2 bus2=sink.1.2"));
        let mut overflow = m.clone();
        overflow.value_mut().lines_mut()[0].length = f64::MAX;
        for row in &mut overflow.value_mut().line_codes_mut()[0].r_series {
            for value in row {
                *value *= 1e6;
            }
        }
        assert!(
            powerio_dist::emit(
                &overflow,
                DistTargetFormat::Dss,
                Destination::memory("case.dss").unwrap()
            )
            .is_err()
        );
    }
}

fn canonical(
    text: &str,
    edit: impl FnOnce(&mut powerio_dist::MulticonductorNetwork),
) -> powerio_dist::MulticonductorNetwork {
    let mut net = read(text, GlmDialect::Pnnl5_3).into_value();
    edit(&mut net);
    let result =
        powerio_dist::glm::emit_canonical(&net, Destination::memory("out.glm").unwrap()).unwrap();
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!()
    };
    read(
        std::str::from_utf8(artifacts[0].bytes()).unwrap(),
        GlmDialect::Pnnl5_3,
    )
    .into_value()
}

#[test]
fn edited_triplex_coefficients_and_points_survive_fresh_output() {
    let mut expected = read(deck(), GlmDialect::Pnnl5_3).into_value();
    expected.line_codes_mut()[0].r_series[0][0] *= 1.1;
    let actual = canonical(deck(), |net| net.line_codes_mut()[0].r_series[0][0] *= 1.1);
    for (expected, actual) in [
        &expected.line_codes()[0].r_series,
        &expected.line_codes()[0].x_series,
    ]
    .iter()
    .zip([
        &actual.line_codes()[0].r_series,
        &actual.line_codes()[0].x_series,
    ]) {
        for (a, b) in expected.iter().flatten().zip(actual.iter().flatten()) {
            assert!((a - b).abs() < 1e-15);
        }
    }
    assert_eq!(expected.component_locations(), actual.component_locations());
    assert_eq!(actual.lines()[0].terminal_map_from, ["1", "2", "n"]);
}

#[test]
fn a_full_matrix_outside_native_triplex_geometry_is_not_fitted() {
    let mut net = read(deck(), GlmDialect::Pnnl5_3).into_value();
    net.line_codes_mut()[0].x_series[0][2] *= 1.1;
    net.line_codes_mut()[0].x_series[2][0] *= 1.1;
    assert!(
        powerio_dist::glm::emit_canonical(&net, Destination::memory("out.glm").unwrap()).is_err()
    );
}

#[test]
fn direct_triplex_output_preserves_the_signed_nonreciprocal_hot_matrix() {
    let text=deck().replace("conductor_1 wire; conductor_2 wire; conductor_N neutral; diameter .368 in; insulation_thickness .08 in;", "z11 2.16454+.880800j; z12 .623542+.673688j; z21 .623542+.673688j; z22 2.16454+.880800j;");
    let expected = read(&text, GlmDialect::Pnnl5_3).into_value();
    let actual = canonical(&text, |_| {});
    assert_eq!(actual.line_codes()[0].n_conductors, 2);
    for (expected, actual) in [
        &expected.line_codes()[0].r_series,
        &expected.line_codes()[0].x_series,
    ]
    .iter()
    .zip([
        &actual.line_codes()[0].r_series,
        &actual.line_codes()[0].x_series,
    ]) {
        for (a, b) in expected.iter().flatten().zip(actual.iter().flatten()) {
            assert!((a - b).abs() < 1e-15);
        }
    }
}

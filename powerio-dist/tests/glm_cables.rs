use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};

fn deck() -> String {
    "#set iteration_limit=100000;
module powerflow { solver_method NR; nominal_frequency 60; line_capacitance true; }
object node { name source; phases AN; bustype SWING; nominal_voltage 240; }
object load { name sink; phases AN; nominal_voltage 240; constant_impedance_A 20+5j; }
object underground_line_conductor { name cable;
 outer_diameter 1.06; conductor_gmr .0111; conductor_diameter .368;
 conductor_resistance .97; neutral_gmr .0111; neutral_resistance .97;
 neutral_diameter .0640837; neutral_strands 6; insulation_relative_permitivitty 2.3;
}
object line_spacing { name spacing; distance_AN .08333333333333333; }
object line_configuration { name code; conductor_A cable; conductor_N cable; spacing spacing; latitude 0; }
object underground_line { name line; phases AN; from source; to sink; length 1 mile; configuration code; longitude 151; }
object voltdump { filename voltage.csv; mode POLAR; }
object currdump { filename current.csv; }
".into()
}

fn read(
    text: &str,
    dialect: GlmDialect,
) -> Result<powerio_core::PioModule<powerio_dist::MulticonductorNetwork>, powerio_core::Error> {
    parse(
        Source::from_memory("cable.glm", text.as_bytes()).unwrap(),
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )
}

#[test]
fn native_cable_impedances_pin_the_forks_different_neutral_reductions() {
    // Values measured from the pinned engines' b matrices, not from echoing
    // conductor properties. Both use the IEEE13 CN607 parameter set.
    for (dialect, resistance, reactance) in [
        (
            GlmDialect::Pnnl5_3,
            1.111_059_634_212_958_4,
            0.173_107_834_191_719_16,
        ),
        (
            GlmDialect::Arras4_3,
            1.130_295_804_518_563,
            0.169_625_392_929_496,
        ),
    ] {
        let text = deck();
        let module = read(&text, dialect).unwrap();
        let net = module.value();
        let code = &net.line_codes()[0];
        assert!((code.r_series[0][0] * 1609.344 - resistance).abs() < 1e-12);
        assert!((code.x_series[0][0] * 1609.344 - reactance).abs() < 1e-12);
        assert!(code.b_from[0][0] > 0.0);
        assert_eq!(net.component_locations().len(), 2);
        let target = match dialect {
            GlmDialect::Pnnl5_3 => powerio_dist::DistTargetFormat::Gridlabd,
            GlmDialect::Arras4_3 => powerio_dist::DistTargetFormat::GridlabdArras,
            _ => unreachable!(),
        };
        let retained =
            powerio_dist::emit(&module, target, Destination::memory("retained").unwrap()).unwrap();
        let EmittedOutput::Memory { artifacts } = retained.output() else {
            panic!()
        };
        assert_eq!(artifacts[0].bytes(), text.as_bytes());
        let fresh = emit_canonical(net, Destination::memory("fresh.glm").unwrap()).unwrap();
        let EmittedOutput::Memory { artifacts } = fresh.output() else {
            panic!()
        };
        let again = read(
            std::str::from_utf8(artifacts[0].bytes()).unwrap(),
            GlmDialect::Pnnl5_3,
        )
        .unwrap();
        let back = &again.value().line_codes()[0];
        assert!((back.r_series[0][0] - code.r_series[0][0]).abs() < 1e-14);
        assert!((back.b_from[0][0] - code.b_from[0][0]).abs() < 1e-14);
        assert_eq!(
            again.value().component_locations(),
            net.component_locations()
        );
    }
}

#[test]
fn invalid_cable_data_cannot_return_a_partial_network() {
    for (old, new) in [
        ("neutral_strands 6", "neutral_strands 0"),
        ("neutral_strands 6", "neutral_strands 6.5"),
        ("outer_diameter 1.06", "outer_diameter .01"),
        ("distance_AN .08333333333333333", "distance_AN 0"),
        ("conductor_gmr .0111", "conductor_gmr 0"),
        (
            "insulation_relative_permitivitty 2.3",
            "insulation_relative_permitivitty -1",
        ),
        ("iteration_limit=100000", "iteration_limit=0"),
    ] {
        assert!(
            read(&deck().replace(old, new), GlmDialect::Pnnl5_3).is_err(),
            "accepted {new}"
        );
    }
}

#[test]
fn explicit_underground_matrices_take_precedence_over_unused_geometry() {
    let text = deck()
        .replace(
            "spacing spacing; latitude",
            "spacing spacing; z11 1+2j; c11 4; latitude",
        )
        .replace("conductor_gmr .0111", "conductor_gmr 0");
    let module = read(&text, GlmDialect::Pnnl5_3).unwrap();
    let code = &module.value().line_codes()[0];
    assert!((code.r_series[0][0] * 1609.344 - 1.0).abs() < 1e-12);
    assert!((code.x_series[0][0] * 1609.344 - 2.0).abs() < 1e-12);
}

#[test]
#[allow(clippy::float_cmp)] // Native absent charging is exactly zero.
fn native_invalid_geometry_is_dialect_specific() {
    let bare = deck().replace("neutral_gmr .0111", "neutral_gmr 0");
    assert!(
        read(&bare, GlmDialect::Pnnl5_3)
            .unwrap_err()
            .to_string()
            .contains("PNNL requires")
    );
    let arras = read(&bare, GlmDialect::Arras4_3).unwrap();
    assert_eq!(arras.value().line_codes()[0].b_from[0][0], 0.0);
    let shield = bare.replace("phases AN", "phases A").replace("neutral_gmr 0;", "neutral_gmr 0; shield_gmr .045; shield_resistance 1.7; shield_diameter 1.1; shield_thickness .005;");
    assert!(read(&shield, GlmDialect::Pnnl5_3).is_ok());
    assert!(
        read(&shield, GlmDialect::Arras4_3)
            .unwrap_err()
            .to_string()
            .contains("without N is singular")
    );
}

fn three_phase_deck(screen: &str) -> String {
    format!(
        "module powerflow {{ solver_method NR; nominal_frequency 60; line_capacitance true; }}
object node {{ name source; phases ABCN; bustype SWING; nominal_voltage 7200; }}
object load {{ name sink; phases ABCN; nominal_voltage 7200; constant_impedance_A 200+50j; constant_impedance_B 200+50j; constant_impedance_C 200+50j; }}
object underground_line_conductor {{ name cable;
 outer_diameter 1.29; conductor_gmr .0171; conductor_diameter .567;
 conductor_resistance .41; insulation_relative_permitivitty 2.3; {screen}
}}
object underground_line_conductor {{ name neutral; conductor_gmr .00814; conductor_diameter .2; conductor_resistance 1.38; }}
object line_spacing {{ name spacing; distance_AB .5; distance_BC .5; distance_AC 1.0; distance_AN .25; distance_BN .3; distance_CN .75; }}
object line_configuration {{ name code; conductor_A cable; conductor_B cable; conductor_C cable; conductor_N neutral; spacing spacing; }}
object underground_line {{ name line; phases ABCN; from source; to sink; length 1000 ft; configuration code; }}
"
    )
}

#[test]
fn three_phase_returns_reduce_together() {
    // Pinned against the earlier one-return-at-a-time elimination with
    // symmetric pivoting: the returns (three screens and N) reduce as one
    // block, and Arras's tape-shield N, with its zero self term, still
    // reduces through the inverse's pivoting.
    let concentric = "neutral_gmr .00208; neutral_resistance 14.8722; neutral_diameter .0641; neutral_strands 13;";
    let tape = "neutral_gmr 0; shield_gmr .0365; shield_resistance 1.7; shield_diameter .88; shield_thickness .005;";
    for (screen, dialect, expected) in [
        (concentric, GlmDialect::Pnnl5_3, PNNL_CN),
        (tape, GlmDialect::Pnnl5_3, PNNL_TS),
        (tape, GlmDialect::Arras4_3, ARRAS_TS),
    ] {
        let module = read(&three_phase_deck(screen), dialect).unwrap();
        let code = &module.value().line_codes()[0];
        let actual: Vec<f64> = [&code.r_series, &code.x_series, &code.b_from]
            .into_iter()
            .flatten()
            .flatten()
            .map(|v| v * 1609.344)
            .collect();
        assert_eq!(actual.len(), expected.len());
        // Each matrix (R, X, B) to 1e-13 of its largest entry.
        for (actual, expected) in actual.chunks(9).zip(expected.chunks(9)) {
            let scale = expected.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
            for (a, e) in actual.iter().zip(expected) {
                assert!((a - e).abs() <= 1e-13 * scale, "{a} vs {e}");
            }
        }
        let n = code.n_conductors;
        for i in 0..n {
            for j in 0..n {
                assert_eq!(code.x_series[i][j].to_bits(), code.x_series[j][i].to_bits());
            }
        }
    }
}
#[allow(clippy::unreadable_literal)] // the earlier code's output, verbatim
const PNNL_CN: &[f64] = &[
    0.732150509442567,
    0.25380178380673735,
    0.22421262959046656,
    0.25380178380673735,
    0.7251609192834172,
    0.26116706847064186,
    0.22421262959046656,
    0.26116706847064186,
    0.74794557638133,
    0.4061727985551756,
    0.001541115751733095,
    -0.02868216896991566,
    0.001541115751733095,
    0.3810654229400221,
    0.025066814156503747,
    -0.02868216896991566,
    0.025066814156503747,
    0.45036272412469197,
    4.830491682802676e-05,
    0.0,
    0.0,
    0.0,
    4.830491682802676e-05,
    0.0,
    0.0,
    0.0,
    4.830491682802676e-05,
];
#[allow(clippy::unreadable_literal)] // the earlier code's output, verbatim
const PNNL_TS: &[f64] = &[
    0.7972872645389057,
    0.32816983925632937,
    0.29996143682548726,
    0.32816983925632937,
    0.7968254357635232,
    0.3330025345012986,
    0.29996143682548726,
    0.3330025345012986,
    0.8082733384493177,
    0.4538848450004377,
    0.046209005616257376,
    0.015253292419076707,
    0.046209005616257376,
    0.43389284104186265,
    0.07834626808563443,
    0.015253292419076707,
    0.07834626808563443,
    0.5150238167667459,
    8.915443923605262e-05,
    0.0,
    0.0,
    0.0,
    8.915443923605262e-05,
    0.0,
    0.0,
    0.0,
    8.915443923605262e-05,
];
#[allow(clippy::unreadable_literal)] // the earlier code's output, verbatim
const ARRAS_TS: &[f64] = &[
    1.0820868387260298,
    0.5910684879482453,
    0.5095594276141902,
    0.5910684879482453,
    1.0378072599037,
    0.5219728367588314,
    0.5095594276141902,
    0.5219728367588314,
    0.9505639783341603,
    0.3230785175050593,
    -0.09875235023941077,
    -0.1452452003352437,
    -0.09875235023941077,
    0.27694618178975283,
    -0.08970150615755675,
    -0.1452452003352437,
    -0.08970150615755675,
    0.3441394068779825,
    8.915443923605262e-05,
    0.0,
    0.0,
    0.0,
    8.915443923605262e-05,
    0.0,
    0.0,
    0.0,
    8.915443923605262e-05,
];

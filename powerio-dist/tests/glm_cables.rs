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

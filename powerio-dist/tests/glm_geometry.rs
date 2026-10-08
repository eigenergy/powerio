use num_complex::Complex64;
use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};

fn deck(neutral: bool, capacitance: bool) -> String {
    let flags = if neutral { "AN" } else { "A" };
    format!("module powerflow {{ solver_method NR; nominal_frequency 50; line_capacitance {cap}; }}
object node {{ name source; phases {flags}; bustype SWING; nominal_voltage 240; }}
object load {{ name sink; phases {flags}; nominal_voltage 240; constant_impedance_A 20+5j; }}
object overhead_line_conductor {{ name phase_wire; resistance 0.306 Ohm/mile; geometric_mean_radius 0.0244 ft; diameter 0.721 in; }}
object overhead_line_conductor {{ name neutral_wire; resistance 0.592 Ohm/mile; geometric_mean_radius 0.00814 ft; diameter 0.563 in; }}
object line_spacing {{ name spacing; distance_AN 7.810249675906654 ft; distance_AE 32 ft; distance_NE 26 ft; }}
object line_configuration {{ name code; conductor_A phase_wire; conductor_N neutral_wire; spacing spacing; latitude 0; }}
object overhead_line {{ name line; phases {flags}; from source; to sink; length 1 mile; configuration code; longitude 151; }}", cap=if capacitance { "TRUE" } else { "FALSE" })
}

#[test]
fn overhead_geometry_materializes_native_reduction_and_survives_fresh_output() {
    for neutral in [false, true] {
        for capacitance in [false, true] {
            for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
                let text = deck(neutral, capacitance);
                let module = parse(
                    Source::from_memory("case.glm", text.as_bytes()).unwrap(),
                    PreprocessOptions {
                        dialect,
                        ..Default::default()
                    },
                )
                .unwrap();
                assert!(
                    module
                        .diagnostics()
                        .iter()
                        .any(|d| d.code() == "READ.GLM.GEOMETRY_MATERIALIZED")
                );
                let net = module.value();
                assert_eq!(net.lines()[0].terminal_map_from, ["1"]);
                let code = &net.line_codes()[0];
                assert!(code.source.as_ref().unwrap().contains(if neutral {
                    "neutral reduced"
                } else {
                    "no neutral conductor"
                }));
                let z = |r: f64, d: f64| {
                    Complex64::new(
                        r + 0.001_588_36 * 50.0,
                        0.002_022_37 * 50.0 * ((100.0_f64 / 50.0).ln() / 2.0 + 7.6786 - d.ln()),
                    )
                };
                let expected = z(0.306, 0.0244)
                    - if neutral {
                        z(0.0, 7.810_249_675_906_654).powu(2) / z(0.592, 0.00814)
                    } else {
                        Complex64::default()
                    };
                assert!(
                    (Complex64::new(code.r_series[0][0], code.x_series[0][0]) * 1609.344
                        - expected)
                        .norm()
                        < 1e-12
                );
                assert_eq!(code.b_from[0][0] > 0.0, capacitance);
                let output =
                    emit_canonical(net, Destination::memory("fresh.glm").unwrap()).unwrap();
                let EmittedOutput::Memory { artifacts } = output.output() else {
                    panic!("not memory")
                };
                let again = parse(
                    Source::from_memory("fresh.glm", artifacts[0].bytes()).unwrap(),
                    PreprocessOptions::default(),
                )
                .unwrap();
                assert_eq!(again.value().base_frequency().to_bits(), 50.0_f64.to_bits());
                let other = &again.value().line_codes()[0];
                assert!((other.r_series[0][0] - code.r_series[0][0]).abs() < 1e-14);
                assert!((other.b_from[0][0] - code.b_from[0][0]).abs() < 1e-14);
                assert_eq!(
                    again.value().component_locations(),
                    net.component_locations()
                );
            }
        }
    }
}

#[test]
fn invalid_geometry_fails_before_a_network_is_returned() {
    for (old, new) in [
        ("resistance 0.306", "resistance 0"),
        ("geometric_mean_radius 0.0244", "geometric_mean_radius -1"),
        (
            "geometric_mean_radius 0.0244",
            "geometric_mean_radius 1e-320",
        ),
        ("distance_AN 7.810249675906654", "distance_AN 0"),
        ("distance_AE 32", "distance_AE 0"),
        ("diameter 0.721", "diameter 0"),
        ("conductor_A phase_wire", "conductor_A spacing"),
        ("conductor_N neutral_wire;", ""),
        ("nominal_frequency 50", "nominal_frequency 0"),
    ] {
        let text = deck(true, true).replace(old, new);
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
fn explicit_matrices_take_native_precedence_over_geometry() {
    let text = deck(true, false).replace("name code;", "name code; z11 2+3j;");
    let module = parse(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap();
    assert!(
        !module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.GLM.GEOMETRY_MATERIALIZED")
    );
    let code = &module.value().line_codes()[0];
    assert!((code.r_series[0][0] * 1609.344 - 2.0).abs() < 1e-12);
    assert!((code.x_series[0][0] * 1609.344 - 3.0).abs() < 1e-12);
}

use num_complex::Complex64;
use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};
use powerio_dist::{DistLoadVoltageModel, MulticonductorNetwork};

fn deck(flags: &str, properties: &str) -> String {
    format!("module powerflow {{ solver_method NR; }}
object node {{ name source; phases ABCD; bustype SWING; nominal_voltage 240; }}
object load {{ name demand; parent source; phases {flags}; nominal_voltage 240; latitude 0; {properties} }}")
}

fn read(text: &str, dialect: GlmDialect) -> MulticonductorNetwork {
    parse(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )
    .unwrap()
    .into_value()
}

#[test]
fn delta_flags_use_phase_pair_voltage_bases_and_preserve_coordinates() {
    let a = Complex64::new(240.0, 0.0);
    let b = Complex64::new(-120.0, -120.0 * 3_f64.sqrt());
    let c = Complex64::new(-120.0, 120.0 * 3_f64.sqrt());
    for (phase, flags, terminals, voltage) in [
        ("A", "ABD", ["1", "2"], a - b),
        ("B", "BCD", ["2", "3"], b - c),
        ("C", "ACD", ["3", "1"], c - a),
    ] {
        for law in ["power", "current", "impedance"] {
            for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
                let net = read(
                    &deck(flags, &format!("constant_{law}_{phase} 2-0.5j;")),
                    dialect,
                );
                let load = &net.loads()[0];
                assert_eq!(load.name, "demand");
                assert_eq!(load.terminal_map, terminals);
                let value = Complex64::new(2.0, -0.5);
                let expected = match law {
                    "power" => value,
                    "current" => voltage * value.conj(),
                    _ => voltage.norm_sqr() / value.conj(),
                };
                assert!((Complex64::new(load.p_nom[0], load.q_nom[0]) - expected).norm() < 1e-9);
                let nominal = match &load.voltage_model {
                    DistLoadVoltageModel::ConstantPower { v_nom }
                    | DistLoadVoltageModel::ConstantCurrent { v_nom }
                    | DistLoadVoltageModel::ConstantImpedance { v_nom } => v_nom[0],
                    _ => panic!("unexpected voltage law"),
                };
                assert!((nominal - 240.0 * 3_f64.sqrt()).abs() < 1e-10);
                let output =
                    emit_canonical(&net, Destination::memory("fresh.glm").unwrap()).unwrap();
                let EmittedOutput::Memory { artifacts } = output.output() else {
                    panic!("not memory")
                };
                let again = read(std::str::from_utf8(artifacts[0].bytes()).unwrap(), dialect);
                assert_eq!(again.loads()[0].terminal_map, terminals);
                assert!((again.loads()[0].p_nom[0] - expected.re).abs() < 1e-9);
                assert!((again.loads()[0].q_nom[0] - expected.im).abs() < 1e-9);
                assert_eq!(again.component_locations(), net.component_locations());
                assert_eq!(again.component_locations()[0].point.latitude, Some(0.0));
            }
        }
    }
}

#[test]
fn scalar_setters_mutate_complex_parts_in_source_order_for_every_channel() {
    for flags in ["ABCN", "ABCD"] {
        for channel in ["A", "B", "C", "AB", "BC", "CA", "AN", "BN", "CN"] {
            for (law, unit) in [("power", "kW"), ("current", "kA"), ("impedance", "kOhm")] {
                let reactive_unit = if law == "power" { "kVAr" } else { unit };
                let key = format!("constant_{law}_{channel}");
                let assigned = read(
                    &deck(
                        flags,
                        &format!(
                            "{key} 1+2j; {key}_real 0.003 {unit}; {key}_reac -0.004 {reactive_unit};"
                        ),
                    ),
                    GlmDialect::Pnnl5_3,
                );
                let expected = read(&deck(flags, &format!("{key} 3-4j;")), GlmDialect::Pnnl5_3);
                assert_eq!(assigned.loads(), expected.loads());
                let overwritten = read(
                    &deck(flags, &format!("{key}_real 9; {key}_reac 10; {key} 3-4j;")),
                    GlmDialect::Arras4_3,
                );
                assert_eq!(overwritten.loads(), expected.loads());
            }
        }
    }
}

#[test]
fn delta_legacy_and_explicit_channels_add_and_root_locations_stay_on_the_bus() {
    let properties = "constant_power_A 100+10j; constant_power_AB_real 200; constant_power_AB_reac 20; constant_power_AN 300+30j; constant_impedance_C 0+0j;";
    for root in [false, true] {
        let text = if root {
            deck("ABCD", properties).replace("parent source;", "")
        } else {
            deck("ABCD", properties)
        };
        let net = read(&text, GlmDialect::Pnnl5_3);
        assert_eq!(net.loads().len(), 4);
        assert!((net.loads().iter().map(|l| l.p_nom[0]).sum::<f64>() - 600.0).abs() < 1e-12);
        assert_eq!(
            net.loads()
                .iter()
                .filter(|l| l.terminal_map == ["1", "2"])
                .count(),
            2
        );
        assert_eq!(net.component_locations().len(), if root { 1 } else { 4 });
        assert_eq!(
            net.component_locations()[0].target,
            if root {
                powerio_dist::DistLocationTarget::Bus
            } else {
                powerio_dist::DistLocationTarget::Load
            }
        );
    }
}

#[test]
fn invalid_setters_and_missing_delta_endpoints_fail_even_when_overwritten_or_zero() {
    for (flags, properties) in [
        ("AD", "constant_power_A 0+0j;"),
        ("ABD", "constant_power_C_real 0;"),
        ("ABCDD", "constant_power_A 1+0j;"),
        ("ABCD", "constant_power_A_real NaN; constant_power_A 1+0j;"),
        (
            "ABCD",
            "constant_current_B_real 1 kV; constant_current_B 1+0j;",
        ),
        ("ABCD", "constant_impedance_C 1e-320+0j;"),
        (
            "ABCD",
            "constant_power_AB_real 1+2j; constant_power_AB 1+0j;",
        ),
    ] {
        let text = deck(flags, properties);
        assert!(
            parse(
                Source::from_memory("bad.glm", text.as_bytes()).unwrap(),
                PreprocessOptions::default()
            )
            .is_err(),
            "accepted {flags}: {properties}"
        );
    }
}

use num_complex::Complex64;
use powerio_core::Source;
use powerio_dist::glm::{GlmDialect, PreprocessOptions, parse};

fn read(
    text: &str,
    dialect: GlmDialect,
) -> Result<powerio_core::PioModule<powerio_dist::MulticonductorNetwork>, powerio_core::Error> {
    parse(
        Source::from_memory("static.glm", text.as_bytes()).unwrap(),
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )
}
fn deck(solver: &str, phases: &str, properties: &str) -> String {
    format!(
        "module powerflow {{ solver_method {solver}; }}\nobject node {{ name source; bustype SWING; phases {phases}; nominal_voltage 240; }}\nobject node {{ name sink; phases {phases}; nominal_voltage 240; }}\nobject series_reactor {{ name reactor; from source; to sink; phases {phases}; {properties} latitude 0; longitude 150; }}"
    )
}
#[test]
fn reactors_preserve_alias_order_and_native_zero_impedance_rules() {
    for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
        let m=read(&deck("NR","ABC", "phase_A_resistance 9; phase_A_impedance 1+2j; phase_A_reactance 3; phase_C_impedance 4+5j;"),dialect).unwrap();
        let c = &m.value().line_codes()[0];
        assert_eq!(
            c.r_series,
            vec![
                vec![1.0, 0.0, 0.0],
                vec![0.0, 0.00005, 0.0],
                vec![0.0, 0.0, 4.0]
            ]
        );
        assert_eq!(
            c.x_series,
            vec![
                vec![3.0, 0.0, 0.0],
                vec![0.0, -0.00005, 0.0],
                vec![0.0, 0.0, 5.0]
            ]
        );
        let m = read(&deck("FBS", "ABC", "phase_B_impedance 1+2j;"), dialect).unwrap();
        assert_eq!(m.value().lines()[0].terminal_map_from, ["2"]);
        assert_eq!(m.value().switches()[0].terminal_map_from, ["1", "3"]);
        assert_eq!(m.value().component_locations().len(), 2);
        let m = read(&deck("FBS", "BC", ""), dialect).unwrap();
        assert!(m.value().lines().is_empty());
        assert_eq!(m.value().switches()[0].terminal_map_from, ["2", "3"]);
    }
}
#[test]
fn substation_sequence_rotation_overrides_explicit_phase_values() {
    let text = "module powerflow { solver_method NR; } object substation { name source; phases ABCN; bustype SWING; nominal_voltage 240; voltage_A 999; positive_sequence_voltage 200+10j; negative_sequence_voltage 5-2j; zero_sequence_voltage 3+4j; reference_phase PHASE_B; base_power 12MVA; power_convergence_value 100VA; }";
    for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
        let m = read(text, dialect).unwrap();
        let source = &m.value().sources()[0];
        let a = Complex64::from_polar(1.0, 2.0 * std::f64::consts::PI / 3.0);
        let seq = [
            Complex64::new(3.0, 4.0),
            Complex64::new(200.0, 10.0),
            Complex64::new(5.0, -2.0),
        ];
        let expected = [
            seq[0] + seq[1] + seq[2],
            seq[0] + a.conj() * seq[1] + a * seq[2],
            seq[0] + a * seq[1] + a.conj() * seq[2],
        ];
        for (i, value) in expected.into_iter().enumerate() {
            let actual = Complex64::from_polar(source.v_magnitude[i], source.v_angle[i]);
            assert!((actual - value * a).norm() < 1e-10);
        }
    }
}
#[test]
fn unsupported_reactor_states_and_substation_coupling_fail_explicitly() {
    for text in [deck("NR","ABC","status OPEN;"),deck("NR","ABC","phase_A_impedance 1j;").replace("to sink;","to missing;"),"module powerflow { solver_method NR; } object substation { name s; phases AN; bustype SWING; nominal_voltage 240; positive_sequence_voltage 240; }".into(),"module powerflow { solver_method NR; } object substation { name s; phases ABCN; nominal_voltage 240; positive_sequence_voltage 240; }".into()] {
        assert!(read(&text,GlmDialect::Pnnl5_3).is_err(),"{text}");
    }
}

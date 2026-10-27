use powerio_core::Source;
use powerio_dist::glm::{GlmDialect, PreprocessOptions, parse};

fn deck(properties: &str) -> String {
    format!(
        "module powerflow {{ solver_method NR; }}\nmodule generators;\n\
        object meter {{ name bus; phases ABCN; bustype SWING; nominal_voltage 240; }}\n\
        object diesel_dg {{ name dg; parent bus; {properties} latitude 0; longitude 151; }}"
    )
}
fn read(
    text: &str,
    dialect: GlmDialect,
) -> Result<powerio_core::PioModule<powerio_dist::MulticonductorNetwork>, powerio_core::Error> {
    parse(
        Source::from_memory("generator.glm", text.as_bytes()).unwrap(),
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )
}
#[test]
fn initialized_phase_powers_ratings_and_coordinates_are_typed() {
    let module = read(&deck("Gen_type CONSTANT_PQ; Rated_VA 9000; power_out_A 4000+3000j; power_out_B -1000+200j; power_out_C 0+0j;"), GlmDialect::Pnnl5_3).unwrap();
    let generators = module.value().generators();
    assert_eq!(generators.len(), 3);
    for (phase, (g, (p, q))) in generators
        .iter()
        .zip([(2400.0, 1800.0), (-1000.0, 200.0), (0.0, 0.0)])
        .enumerate()
    {
        assert!((g.p_nom[0] - p).abs() < 1e-10);
        assert!((g.q_nom[0] - q).abs() < 1e-10);
        assert_eq!(g.s_max.as_deref(), Some([3000.0].as_slice()));
        assert_eq!(g.terminal_map, [(phase + 1).to_string(), "n".into()]);
        assert!(g.extras.is_empty());
    }
    assert_eq!(module.value().component_locations().len(), 3);
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.GLM.GENERATOR_EQUIVALENT")
    );
}
#[test]
fn scalar_overrides_apply_at_initialization_and_minus_one_is_the_native_sentinel() {
    for properties in [
        "real_power_out_A 500; reactive_power_out_A -1; power_out_A 1000+200j;",
        "power_out_A 1000+200j; real_power_out_A 500; reactive_power_out_A -1;",
    ] {
        let m = read(&deck(properties), GlmDialect::Pnnl5_3).unwrap();
        let g = &m.value().generators()[0];
        assert!((g.p_nom[0] - 500.0).abs() < 1e-12);
        assert!((g.q_nom[0] - 200.0).abs() < 1e-12);
        assert!((g.s_max.as_ref().unwrap()[0] - 500_000.0 / 3.0).abs() < 1e-10);
    }
}
#[test]
fn unsupported_fork_mode_parent_or_voltage_is_not_silently_projected() {
    assert!(read(&deck("Gen_type CONSTANT_PQ;"), GlmDialect::Arras4_3).is_err());
    for text in [
        deck("Gen_type DYN_SYNCHRONOUS;"),
        deck("Rated_V 480;"),
        deck("overload_limit 2;"),
        deck("").replace("module generators;", ""),
        deck("").replace("phases ABCN", "phases AN"),
        deck("").replace("parent bus;", ""),
        deck("").replace("phases ABCN", "phases ABCD"),
        deck("").replace("parent bus;", "parent child;")
            + "object meter { name child; parent bus; phases AN; nominal_voltage 240; }",
        deck("Rated_V 415.69219381653056;").replace("parent bus;", "parent child;")
            + "object meter { name child; parent bus; phases ABCN; nominal_voltage 120; }",
    ] {
        assert!(read(&text, GlmDialect::Pnnl5_3).is_err(), "{text}");
    }
}

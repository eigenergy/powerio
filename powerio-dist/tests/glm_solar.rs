use powerio_core::Source;
use powerio_dist::glm::{GlmDialect, PreprocessOptions, parse};
fn deck(extra: &str) -> String {
    format!(
        "module powerflow {{ solver_method NR; }} module generators; module climate; object climate {{ name weather; latitude 45; solar_direct 93.4458; }} object meter {{ name source; phases ABCN; bustype SWING; nominal_voltage 240; }} object inverter {{ name inv; parent source; phases ABCN; inverter_type FOUR_QUADRANT; four_quadrant_control_mode CONSTANT_PF; inverter_efficiency 1; rated_power 3000; power_factor 0.8; {extra} object solar {{ name pv; rated_power 13990; panel_type SINGLE_CRYSTAL_SILICON; efficiency .2; derating 1; soiling 1; }} }}"
    )
}
fn read(
    s: &str,
    dialect: GlmDialect,
) -> Result<powerio_core::PioModule<powerio_dist::MulticonductorNetwork>, powerio_core::Error> {
    parse(
        Source::from_memory("solar.glm", s.as_bytes()).unwrap(),
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )
}
#[test]
fn irradiance_temperature_pf_clipping_and_coordinates_are_typed() {
    for dialect in [GlmDialect::Pnnl5_3, GlmDialect::Arras4_3] {
        let module = read(&deck("latitude 0; longitude 150;"), dialect).unwrap();
        assert_eq!(module.value().ibrs().len(), 3);
        for (i, ibr) in module.value().ibrs().iter().enumerate() {
            assert!((ibr.p_min.as_ref().unwrap()[0] - 2400.0).abs() < 1e-9);
            assert!((ibr.q_min.as_ref().unwrap()[0] + 1800.0).abs() < 1e-9);
            assert_eq!(ibr.p_min, ibr.p_max);
            assert_eq!(ibr.q_min, ibr.q_max);
            assert!((ibr.p_avail.unwrap() - 13_989.345_858_367_315 / 3.0).abs() < 1e-9);
            assert_eq!(ibr.terminal_map, [(i + 1).to_string(), "n".into()]);
            assert!(ibr.extras.is_empty());
        }
        assert_eq!(module.value().component_locations().len(), 3);
    }
}
#[test]
fn unsupported_weather_controls_and_dc_sources_are_not_ignored() {
    for s in [
        deck("use_multipoint_efficiency TRUE;"),
        deck("inverter_efficiency -0.2;"),
        deck("").replace("CONSTANT_PF", "VOLT_VAR"),
        deck("").replace("object solar", "object battery"),
        deck("").replace("solar_direct 93.4458;", "tmyfile weather.tmy;"),
        deck("").replace("module climate;", ""),
        deck("").replace(
            "panel_type SINGLE_CRYSTAL_SILICON;",
            "SOLAR_POWER_MODEL FLATPLATE;",
        ),
        deck("").replace("power_factor 0.8;", "power_factor 2;"),
        deck("").replace("module generators;", ""),
    ] {
        assert!(read(&s, GlmDialect::Pnnl5_3).is_err(), "{s}");
    }
}

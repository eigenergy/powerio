use powerio_core::Source;
use powerio_dist::{
    DistLoadVoltageModel,
    glm::{PreprocessOptions, parse},
};
fn deck(fields: &str) -> String {
    format!(
        "module powerflow {{ solver_method NR; }} object node {{ name source; phases AN; bustype SWING; nominal_voltage 240; }} object triplex_load {{ name sink; phases AS; nominal_voltage 120; {fields} }}"
    )
}
#[test]
fn zip_initialization_normalizes_fractions_and_overrides_explicit_terms() {
    let text = deck(
        "base_power_1 1000; power_fraction_1 .9; current_fraction_1 .2; impedance_fraction_1 .3; power_pf_1 -.8; constant_power_1 9999+8888j;",
    );
    let m = parse(
        Source::from_memory("zip.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap();
    let p = m
        .value()
        .loads()
        .iter()
        .find(|l| matches!(l.voltage_model, DistLoadVoltageModel::ConstantPower { .. }))
        .unwrap();
    assert_eq!(p.p_nom, vec![400.0]);
    assert!((p.q_nom[0] + 300.0).abs() < 1e-10);
    assert_eq!(m.value().loads().len(), 3);
}
#[test]
fn zero_base_keeps_explicit_power_and_nonphysical_pf_is_refused() {
    let text = deck("base_power_1 0; constant_power_1 300+50j;");
    let m = parse(
        Source::from_memory("zero.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
    .unwrap();
    assert_eq!(m.value().loads()[0].p_nom, vec![300.0]);
    let text = deck("base_power_1 100; power_pf_1 1.1;");
    assert!(
        parse(
            Source::from_memory("bad.glm", text.as_bytes()).unwrap(),
            PreprocessOptions::default()
        )
        .is_err()
    );
}

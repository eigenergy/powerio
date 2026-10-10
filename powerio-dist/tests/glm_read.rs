use num_complex::Complex64;
use powerio_core::{Error, PioModule, Source};
use powerio_dist::glm::{PreprocessOptions, parse};
use powerio_dist::{DistLoadVoltageModel, MulticonductorNetwork};

fn read(text: &str) -> Result<PioModule<MulticonductorNetwork>, Error> {
    parse(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        PreprocessOptions::default(),
    )
}

const SINGLE: &str = "
module powerflow { solver_method NR; }
object node { name source; phases AN; bustype SWING; nominal_voltage 240; }
object load { name sink; phases AN; nominal_voltage .24 kV;
    constant_impedance_A 20+5j; }
object line_configuration { name cfg; z11 1+.5j; }
object overhead_line { name line; phases AN; from source; to sink;
    length 1 mile; configuration cfg; }
";

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-12 * expected.abs().max(1.0),
        "{actual} != {expected}"
    );
}

#[test]
fn single_phase_import_materializes_ground_and_preserves_physical_units() {
    let module = read(SINGLE).unwrap();
    let net = module.value();
    assert_eq!(net.buses().len(), 2);
    assert_eq!(net.buses()[0].terminals, ["1", "n"]);
    assert_eq!(net.buses()[0].grounded, ["n"]);
    assert_eq!(net.sources()[0].v_magnitude, [240.0]);
    assert_eq!(net.sources()[0].reference_terminal, None);
    let line = &net.lines()[0];
    let code = &net.line_codes()[0];
    close(line.length, 1609.344);
    close(code.r_series[0][0] * line.length, 1.0);
    close(code.x_series[0][0] * line.length, 0.5);
    let load = &net.loads()[0];
    assert_eq!(load.terminal_map, ["1", "n"]);
    let s = 240.0 * 240.0 / Complex64::new(20.0, -5.0);
    close(load.p_nom[0], s.re);
    close(load.q_nom[0], s.im);
    assert!(
        matches!(&load.voltage_model, DistLoadVoltageModel::ConstantImpedance { v_nom } if v_nom == &[240.0])
    );
    assert_eq!(module.diagnostics(), []);
    assert_eq!(
        module.source().unwrap().primary_buffer().unwrap().bytes(),
        SINGLE.as_bytes()
    );
}

#[test]
fn phase_c_uses_its_native_matrix_index_and_source_angle() {
    let text = SINGLE
        .replace("AN", "CN")
        .replace("impedance_A", "impedance_C")
        .replace("z11", "z33");
    let module = read(&text).unwrap();
    let net = module.value();
    assert_eq!(net.lines()[0].terminal_map_from, ["3"]);
    assert_eq!(net.loads()[0].terminal_map, ["3", "n"]);
    close(
        net.sources()[0].v_angle[0],
        2.0 * std::f64::consts::PI / 3.0,
    );
    close(
        net.line_codes()[0].r_series[0][0] * net.lines()[0].length,
        1.0,
    );
}

#[test]
fn coupled_ac_line_keeps_off_diagonal_entries_and_capacitance_halves() {
    let text = SINGLE
        .replace("AN", "ACN")
        .replace(
            "solver_method NR;",
            "solver_method NR; line_capacitance TRUE;",
        )
        .replace(
            "z11 1+.5j;",
            "z11 1+.5j; z13 .1+.2j; z31 .1+.2j; z33 2+.6j; c13 -2; c31 -2; c11 8; c33 9;",
        );
    let module = read(&text).unwrap();
    let net = module.value();
    let code = &net.line_codes()[0];
    assert_eq!(net.lines()[0].terminal_map_from, ["1", "3"]);
    close(code.r_series[0][1] * 1609.344, 0.1);
    close(code.x_series[1][0] * 1609.344, 0.2);
    close(
        code.b_from[0][1] * 1609.344,
        -2e-9 * std::f64::consts::PI * 60.0,
    );
    assert_eq!(code.b_from, code.b_to);
}

#[test]
fn parented_loads_attach_to_the_electrical_bus_and_keep_all_three_laws() {
    let text = SINGLE
        .replace("object load { name sink;", "object node { name sink;")
        .replace("constant_impedance_A 20+5j;", "")
        + "object load { name demand; parent sink; phases AN;
             constant_power_A 100+20j; constant_current_A 2-1j;
             constant_impedance_A 20+5j; }";
    let module = read(&text).unwrap();
    let net = module.value();
    assert_eq!(net.buses().len(), 2);
    assert_eq!(net.loads().len(), 3);
    assert!(net.loads().iter().all(|load| load.bus == "sink"));
    close(net.loads()[0].p_nom[0], 100.0);
    close(net.loads()[1].p_nom[0], 480.0);
    close(net.loads()[1].q_nom[0], 240.0);
}

#[test]
fn generated_names_do_not_shadow_explicit_source_identities() {
    let text = SINGLE.to_owned()
        + "object load { name sink__glm_impedance; parent sink; phases AN; constant_power_A 1; }
           object node { parent sink; phases AN; }
           object node { name __glm_object_5; parent sink; phases AN; }";
    let module = read(&text).unwrap();
    let names: Vec<_> = module
        .value()
        .loads()
        .iter()
        .map(|load| load.name.as_str())
        .collect();
    assert_eq!(names, ["sink__glm_impedance_1", "sink__glm_impedance"]);
}

#[test]
fn quoted_unit_suffix_uses_native_conversion_and_reports_the_ignored_unit() {
    let text = SINGLE.replace(
        "constant_impedance_A 20+5j",
        "constant_impedance_A \"5 kOhm\"",
    );
    let module = read(&text).unwrap();
    close(module.value().loads()[0].p_nom[0], 240.0 * 240.0 / 5.0);
    assert_eq!(module.diagnostics().len(), 1);
    assert_eq!(module.diagnostics()[0].code(), "READ.GLM.UNIT_IGNORED");
}

#[test]
fn physical_gaps_fail_instead_of_becoming_a_partial_network() {
    for text in [
        SINGLE.replace("phases AN", "phases AS"),
        SINGLE.replace("z11 1+.5j", "z12 1+.5j"),
        SINGLE.replace("z11 1+.5j", "z11 1+.5j; c11 1"),
        SINGLE.replace(
            "nominal_voltage .24 kV",
            "nominal_voltage .24 kV; base_power_A 100",
        ),
        SINGLE.replace("solver_method NR", "solver_method NR; nominal_frequency 50"),
        SINGLE.replace("bustype SWING", "bustype PV"),
        SINGLE.replace("name sink", "name SOURCE"),
        format!("{SINGLE}\nobject regulator {{ name active; }}"),
    ] {
        let error = read(&text).unwrap_err();
        assert_eq!(
            error.diagnostics()[0].code(),
            "READ.GLM.ELECTRICAL_UNSUPPORTED",
            "{error}"
        );
    }
}

#[test]
fn numeric_assignments_are_validated_even_when_overwritten_or_on_an_absent_phase() {
    for fields in [
        "constant_power_B broken;",
        "constant_impedance_A broken; constant_impedance_A 20+5j;",
    ] {
        let text = SINGLE.replace("constant_impedance_A 20+5j;", fields);
        let error = read(&text).unwrap_err();
        assert_eq!(error.diagnostics()[0].code(), "PARSE.GLM.QUANTITY");
    }
}

#[test]
fn matrix_quantity_diagnostic_is_emitted_once_per_native_assignment() {
    let text = SINGLE.replace("z11 1+.5j", "z11 \"1 kOhm/mile\"");
    let module = read(&text).unwrap();
    assert_eq!(module.diagnostics().len(), 1);
    assert_eq!(module.diagnostics()[0].code(), "READ.GLM.UNIT_IGNORED");
    close(
        module.value().line_codes()[0].r_series[0][0] * 1609.344,
        1.0,
    );
}

#[test]
fn underflowing_load_power_is_not_mistaken_for_an_open_circuit() {
    let text = SINGLE
        .replace("nominal_voltage 240", "nominal_voltage 1e-200")
        .replace("nominal_voltage .24 kV", "nominal_voltage 1e-200");
    let error = read(&text).unwrap_err();
    assert_eq!(
        error.diagnostics()[0].code(),
        "READ.GLM.ELECTRICAL_UNSUPPORTED"
    );
    assert!(error.to_string().contains("underflows"));
}

//! Format to problem mappings: GO Challenge 3 to `AcScucInstance`, OPFData to
//! `AcOpfSolution`.

use powerio_prob::solution::Termination;
use powerio_prob::{parse_goc3_instance, parse_opfdata_solution};

fn fixture(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap()
}

#[test]
fn goc3_parses_to_the_scuc_instance() {
    let text = fixture("tests/data/goc3_small.json");
    let (instance, _diagnostics) = parse_goc3_instance(&text, "goc3_small").unwrap();

    // Both halves of the join describe the same system.
    assert_eq!(instance.network().buses().len(), 2);
    assert_eq!(instance.inputs().static_data.bus.len(), 2);
    for (row, bus) in instance
        .inputs()
        .static_data
        .bus
        .iter()
        .zip(instance.network().buses())
    {
        assert_eq!(row.i, bus.id, "bus {}", row.uid);
    }

    // The scheduling categories arrived typed: the time axis is stated.
    assert!(!instance.inputs().dt.is_empty());
}

#[test]
fn goc3_network_extraction_reports_the_discard() {
    let text = fixture("tests/data/goc3_small.json");
    let (instance, _diagnostics) = parse_goc3_instance(&text, "goc3_small").unwrap();
    let (_opf, diagnostics) = instance.to_dc_opf().unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message().contains("scheduling")),
        "{diagnostics:?}"
    );
}

#[test]
fn goc3_rejects_a_malformed_document() {
    assert!(parse_goc3_instance("{\"network\": {}}", "broken").is_err());
}

#[test]
fn opfdata_parses_to_the_solved_ac_opf() {
    let text = fixture("../tests/data/opfdataset/example_0.json");
    let (solution, _diagnostics) = parse_opfdata_solution(&text).unwrap();

    // The source claims a solution and says nothing about how it was reached.
    assert_eq!(*solution.termination(), Termination::NotReported);
    assert!((solution.objective() - 2_265.953_939_003_096).abs() < 1e-9);

    // The instance carries the document's network and the solver initial
    // point the grid section supplies.
    let instance = solution.instance();
    assert_eq!(instance.network().buses().len(), 14);
    let initial = instance.initial_state().expect("OPFData states initials");
    // Generator features [mbase, pg, pmin, pmax, qg, qmin, qmax, vg, ...]:
    // row 0 states pg = 1.7 pu on a 100 MVA base and vg = 1.0.
    assert!((initial.generator_active_power("generators:0").unwrap() - 170.0).abs() < 1e-9);
    assert!((initial.generator_voltage_setpoint("generators:0").unwrap() - 1.0).abs() < 1e-12);

    // The solved dispatch is the solution section, in MW; solved voltages
    // land on the solution's bus columns.
    assert_eq!(instance.network().generators().len(), 5);
    assert!(
        (solution
            .bus_voltage_magnitude(powerio_tx::BusId(1))
            .unwrap()
            - 1.060_000_010_369_160_5)
            .abs()
            < 1e-12
    );

    // A solved case balances: the residuals this crate computed are small.
    let residuals = solution.residuals();
    assert!(residuals.max_active_power_mismatch.unwrap() < 1.0);
    assert!(residuals.max_reactive_power_mismatch.unwrap() < 1.0);
}

#[test]
fn opfdata_rejects_a_malformed_document() {
    assert!(parse_opfdata_solution("{\"grid\": {}}").is_err());
}

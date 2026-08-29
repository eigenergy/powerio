//! Format to problem mappings: GO Challenge 3 to `AcScucInstance`, OPFData to
//! `AcOpfSolution`, BMOPF to `McAcOpfInstance`. Every entry takes the
//! retained `Source` and returns the typed module.

use powerio_core::Source;
use powerio_prob::solution::Termination;
use powerio_prob::{
    ObjectiveTerm, parse_bmopf_instance, parse_goc3_instance, parse_opfdata_solution,
};

fn fixture(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap()
}

fn memory(name: &str, text: &str) -> Source {
    Source::from_bytes(name, text.as_bytes().to_vec()).unwrap()
}

#[test]
fn goc3_parses_to_the_scuc_instance() {
    let text = fixture("tests/data/goc3_small.json");
    let module = parse_goc3_instance(memory("goc3_small.json", &text)).unwrap();
    // The module retains the source it parsed.
    assert!(module.source().is_some());
    let instance = module.value();

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
        assert_eq!(
            row.i, bus.id,
            "static bus row {} disagrees with the network",
            row.i
        );
    }

    // The scheduling categories arrived typed: the time axis is stated.
    assert!(!instance.inputs().dt.is_empty());
}

#[test]
fn goc3_network_extraction_reports_the_discard() {
    let text = fixture("tests/data/goc3_small.json");
    let module = parse_goc3_instance(memory("goc3_small.json", &text)).unwrap();
    let (_opf, diagnostics) = module.value().to_dc_opf().unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message().contains("scheduling")),
        "{diagnostics:?}"
    );
}

#[test]
fn goc3_rejects_a_malformed_document_and_retains_the_source() {
    let error = parse_goc3_instance(memory("broken.json", "{\"network\": {}}")).unwrap_err();
    assert!(error.retained_source().is_some());
}

#[test]
fn opfdata_parses_to_the_solved_ac_opf() {
    let text = fixture("../tests/data/opfdataset/example_0.json");
    let module = parse_opfdata_solution(memory("example_0.json", &text)).unwrap();
    assert!(module.source().is_some());
    let solution = module.value();

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

    // The diagnostic naming this split must describe where the initial
    // values live now, never claim they survive only in the retained
    // source: the assertions above just read them from the initial state.
    let initial_value_diagnostic = module
        .diagnostics()
        .iter()
        .find(|d| {
            d.code() == powerio_tx::diagnostics::codes::READ_OPFDATA_RETAINED_SOURCE_ONLY.code
        })
        .expect("the document's generators raise this diagnostic");
    assert!(
        initial_value_diagnostic
            .message()
            .contains("carried in the parsed solution"),
        "{}",
        initial_value_diagnostic.message()
    );
    assert!(
        !initial_value_diagnostic
            .message()
            .contains("remain only in the retained source"),
        "{}",
        initial_value_diagnostic.message()
    );

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
    assert!(parse_opfdata_solution(memory("broken.json", "{\"grid\": {}}")).is_err());
}

#[test]
fn bmopf_parses_to_the_multiconductor_opf_instance() {
    let text = r#"{
      "bus": {"a": {"terminal_names": ["1", "2", "3", "n"],
        "perfectly_grounded_terminals": ["n"],
        "vpn_min": [220.0, 220.0, 220.0], "vpn_max": [260.0, 260.0, 260.0]}},
      "voltage_source": {"s": {"bus": "a", "terminal_map": ["1", "2", "3"],
        "v_magnitude": [240.0, 240.0, 240.0], "v_angle": [0.0, -2.0944, 2.0944]}},
      "generator": {"g": {"bus": "a", "terminal_map": ["1", "2", "3"],
        "configuration": "WYE", "p_max": [1000.0, 1000.0, 1000.0],
        "cost": [0.12, 0.15, 0.18]}}
    }"#;
    let module = parse_bmopf_instance(memory("bmopf_small.json", text)).unwrap();
    assert!(module.source().is_some());
    let instance = module.value();

    // The per phase objective reference, backed by the exact stated array.
    assert_eq!(
        instance.objective().terms(),
        [ObjectiveTerm::NetworkPerPhaseCost]
    );
    assert_eq!(
        instance.network().generators()[0].cost.as_deref(),
        Some([0.12, 0.15, 0.18].as_slice())
    );

    // Every stated limit family starts active.
    let constraints = instance.constraints();
    assert!(constraints.terminal_voltage_bounds.selects("branches:0"));
    assert!(constraints.conductor_limits.selects("branches:0"));
    assert!(constraints.generator_capability.selects("generators:0"));
}

#[test]
fn bmopf_without_a_source_is_refused_as_an_instance_and_retains_the_bytes() {
    let text = r#"{
      "bus": {"a": {"terminal_names": ["1"]}},
      "generator": {"g": {"bus": "a", "terminal_map": ["1"],
        "configuration": "WYE", "cost": [0.1]}}
    }"#;
    let error = parse_bmopf_instance(memory("no_source.json", text)).unwrap_err();
    assert!(error.retained_source().is_some());
}

fn write_pypsa_folder(dir: &std::path::Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    for (name, content) in files {
        std::fs::write(dir.join(name), content).unwrap();
    }
}

const PYPSA_STATIC: [(&str, &str); 4] = [
    (
        "network.csv",
        "name
seq
",
    ),
    (
        "buses.csv",
        "name,v_nom
B1,138.0
B2,138.0
",
    ),
    (
        "loads.csv",
        "name,bus,p_set,q_set
L1,B2,5.0,1.0
",
    ),
    (
        "generators.csv",
        "name,bus,control,p_nom,p_set
G1,B1,Slack,100.0,12.0
",
    ),
];

#[test]
fn pypsa_input_series_classify_as_networks() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("inputs");
    write_pypsa_folder(&dir, &PYPSA_STATIC);
    write_pypsa_folder(
        &dir,
        &[
            ("snapshots.csv", ",snapshot\n0,now\n1,later\n"),
            ("loads-p_set.csv", "snapshot,L1\nnow,10.0\nlater,20.0\n"),
        ],
    );
    let source = powerio_core::Source::open(&dir).unwrap();
    let (sequence, _diagnostics) = powerio_prob::parse_pypsa_sequence(&source).unwrap();
    match sequence {
        powerio_prob::PypsaSequence::Networks(series) => assert_eq!(series.len(), 2),
        powerio_prob::PypsaSequence::OperatingPoints(states) => {
            panic!("expected networks, got operating points: {states:?}")
        }
    }
}

#[test]
fn pypsa_state_only_series_classify_as_operating_points() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("state");
    write_pypsa_folder(&dir, &PYPSA_STATIC);
    write_pypsa_folder(
        &dir,
        &[
            ("snapshots.csv", ",snapshot\n0,now\n1,later\n"),
            (
                "buses-v_mag_pu.csv",
                "snapshot,B1,B2\nnow,1.0,0.99\nlater,1.0,0.97\n",
            ),
        ],
    );
    let source = powerio_core::Source::open(&dir).unwrap();
    let (sequence, _diagnostics) = powerio_prob::parse_pypsa_sequence(&source).unwrap();
    let powerio_prob::PypsaSequence::OperatingPoints(states) = sequence else {
        panic!("expected operating points");
    };
    assert_eq!(states.len(), 2);
    let later = &states.values()[1];
    assert!((later.bus_voltage_magnitude(powerio_tx::BusId(2)).unwrap() - 0.97).abs() < 1e-12);
    // One shared network under every point.
    assert!(std::ptr::eq(
        states.values()[0].network().buses().as_ptr(),
        later.network().buses().as_ptr()
    ));
}

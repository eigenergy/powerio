//! The canonical (no-source-document) writer path, exercised through a synth
//! case. Lives in casemat because it needs the `synth` generators.

use casemat::synth::{SynthSpec, Topology, generate};
use casemat::{parse_matpower, write_matpower};

#[test]
fn synth_case_round_trips_via_canonical_writer() {
    let spec = SynthSpec {
        topology: Topology::Tree,
        n: 8,
        r_over_x: 0.1,
        mean_x: 0.05,
        seed: 1,
    };
    let case = generate(&spec); // no source document → canonical writer
    let reparsed = parse_matpower(&write_matpower(&case)).unwrap();
    assert_eq!(reparsed.buses.len(), case.buses.len());
    assert_eq!(reparsed.branches.len(), case.branches.len());

    // A name that isn't a legal MATLAB identifier still produces parseable `.m`.
    let mut bad = case.clone();
    bad.name = "grid-1".to_string();
    let written = write_matpower(&bad);
    assert!(written.contains("function mpc = grid_1"));
    assert!(parse_matpower(&written).is_ok());
}

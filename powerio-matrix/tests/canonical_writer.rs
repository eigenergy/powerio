//! The canonical (no-source-document) writer path, exercised through a synth
//! case. Lives in powerio-matrix because it needs the `synth` generators.
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use powerio_core::{Destination, EmittedOutput, PioModule};
use powerio_matrix::synth::{SynthSpec, Topology, generate};
use powerio_tx::{BalancedNetwork, TargetFormat};

fn emit_matpower(network: &BalancedNetwork) -> String {
    let module = PioModule::new(network.clone());
    let result = powerio_tx::emit(
        &module,
        TargetFormat::Matpower,
        Destination::memory("case.m").expect("valid memory destination"),
    )
    .expect("MATPOWER emission");
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory destination must return memory output");
    };
    assert_eq!(artifacts.len(), 1, "MATPOWER emits one artifact");
    String::from_utf8(artifacts.pop().expect("one artifact").into_bytes())
        .expect("MATPOWER is UTF-8 text")
}

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
    let reparsed = parse_matpower(&emit_matpower(&case)).unwrap();
    assert_eq!(reparsed.buses().len(), case.buses().len());
    assert_eq!(reparsed.branches().len(), case.branches().len());

    // A name that isn't a legal MATLAB identifier still produces parseable `.m`.
    let mut bad = case.clone();
    *bad.name_mut() = "grid-1".to_string();
    let written = emit_matpower(&bad);
    assert!(written.contains("function mpc = grid_1"));
    assert!(parse_matpower(&written).is_ok());
}

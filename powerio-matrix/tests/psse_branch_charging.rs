//! A MATPOWER case written to PSS/E RAW and read back states the admittance
//! matrix it started from (#940).
//!
//! case118 states 68-116 and 86-87 with a ratio of exactly 1 and line
//! charging. Written as transformer records, all of their charging moved to
//! winding 1's bus, 0.082 pu at buses 68 and 116. Written as BRANCH records,
//! half sits at each end, as the case states.

mod helpers;

use std::path::Path;

use helpers::{parse_matpower_file, parse_psse};
use powerio_core::{Destination, EmittedOutput, PioModule};
use powerio_matrix::matrix::{BuildOptions, YbusParts, calc_admittance_matrix};
use powerio_tx::{BalancedNetwork, IndexedNetwork, TargetFormat};

fn emit_psse(network: &BalancedNetwork, rev: u32) -> String {
    let result = powerio_tx::emit(
        &PioModule::new(network.clone()),
        TargetFormat::Psse { rev },
        Destination::memory("case.raw").expect("valid memory destination"),
    )
    .expect("PSS/E emission");
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory destination must return memory output");
    };
    String::from_utf8(artifacts.pop().expect("one artifact").into_bytes()).expect("UTF-8")
}

fn ybus(net: &BalancedNetwork) -> (IndexedNetwork<'_>, YbusParts) {
    let indexed = IndexedNetwork::new(net);
    let parts = calc_admittance_matrix(&indexed, &BuildOptions::default()).unwrap();
    (indexed, parts)
}

#[test]
fn case118_admittance_survives_a_psse_round_trip() {
    let case =
        parse_matpower_file(Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case118.m"))
            .expect("parse case118");
    for rev in [33, 35] {
        let back = parse_psse(&emit_psse(&case, rev)).expect("parse the emitted case");
        let (case_index, expected) = ybus(&case);
        let (back_index, actual) = ybus(&back);
        let ids: Vec<_> = case.buses().iter().map(|bus| bus.id).collect();
        let mut worst = (0.0_f64, String::new());
        for (name, a, b) in [("G", &expected.g, &actual.g), ("B", &expected.b, &actual.b)] {
            for &i in &ids {
                for &j in &ids {
                    let x = a
                        .get(
                            case_index.bus_index(i).unwrap(),
                            case_index.bus_index(j).unwrap(),
                        )
                        .copied()
                        .unwrap_or(0.0);
                    let y = b
                        .get(
                            back_index.bus_index(i).unwrap(),
                            back_index.bus_index(j).unwrap(),
                        )
                        .copied()
                        .unwrap_or(0.0);
                    if (x - y).abs() > worst.0 {
                        worst = (
                            (x - y).abs(),
                            format!("rev {rev} {name}[{i}, {j}]: {x} != {y}"),
                        );
                    }
                }
            }
        }
        assert!(worst.0 < 1e-9, "{}", worst.1);
    }
}

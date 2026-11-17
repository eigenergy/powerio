//! Synthetic transformer stamps compared with OpenDSS 0.9.4.
use powerio_core::Source;
use powerio_dist::transformer::calc_transformer_primitive_admittance;
use serde::Deserialize;

#[derive(Deserialize)]
struct Report {
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    source: String,
    nodes: Vec<String>,
    matrix: Vec<Vec<[f64; 2]>>,
}

#[test]
fn transformer_stamps_match_opendss() {
    let report: Report =
        serde_json::from_str(include_str!("data/transformer_primitives.json")).unwrap();
    for case in report.cases {
        let module =
            powerio_dist::parse(Source::from_memory("test.dss", case.source.as_bytes()).unwrap())
                .unwrap();
        let network = module.value();
        let primitive =
            calc_transformer_primitive_admittance(network, &network.transformers()[0]).unwrap();
        for (index, label) in case.nodes.iter().enumerate() {
            if case.matrix[index]
                .iter()
                .any(|value| value[0] != 0.0 || value[1] != 0.0)
            {
                assert!(
                    primitive
                        .terminals
                        .iter()
                        .any(|(bus, terminal)| format!("{bus}.{terminal}") == *label),
                    "{}: missing terminal {label}",
                    case.name
                );
            }
        }
        for (i, &(bus, terminal)) in primitive.terminals.iter().enumerate() {
            let label = format!("{bus}.{terminal}");
            let a = case
                .nodes
                .iter()
                .position(|node| node == &label)
                .unwrap_or_else(|| panic!("{}: unexpected computed terminal {label}", case.name));
            for (j, &(other_bus, other_terminal)) in primitive.terminals.iter().enumerate() {
                let other = format!("{other_bus}.{other_terminal}");
                let b = case
                    .nodes
                    .iter()
                    .position(|node| node == &other)
                    .unwrap_or_else(|| {
                        panic!("{}: unexpected computed terminal {other}", case.name)
                    });
                let expected = case.matrix[a][b];
                let actual = primitive.primitive[i][j];
                let tolerance = 1e-9 + 1e-9 * expected[0].hypot(expected[1]);
                assert!(
                    (actual.re - expected[0]).hypot(actual.im - expected[1]) < tolerance,
                    "{} {label} / {other}: {actual:?} != {expected:?} tolerance {tolerance}",
                    case.name
                );
            }
        }
    }
}

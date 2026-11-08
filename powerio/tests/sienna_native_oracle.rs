//! Runtime-generated PowerSystems fixtures exercise independent serialization.
use powerio::{PioValue, Source, parse};
use powerio_matrix::{BuildOptions, IndexedNetwork, calc_admittance_matrix};

#[test]
fn powersystems_serialized_networks_keep_admittance_and_dispatch() {
    let Some(input) = std::env::var_os("POWERIO_POWERSYSTEMS_INPUT_DIR") else {
        return;
    };
    let input = std::path::PathBuf::from(input);
    let shunt_path = input.join("switched_shunt.json");
    if shunt_path.exists() {
        let module = parse(Source::open(shunt_path).unwrap()).unwrap();
        let PioValue::BalancedNetwork(net) = module.value() else {
            panic!("network");
        };
        let shunt = net.shunts().iter().find(|s| s.control.is_some()).unwrap();
        let control = shunt.control.as_ref().unwrap();
        assert_eq!(
            control.mode,
            powerio_tx::network::SwitchedShuntMode::Discrete
        );
        assert_eq!(control.control_bus.unwrap().0, 5);
        assert!((shunt.b - 12.5).abs() < 1e-12);
    }
    let cases = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data");
    for case in ["case9", "case14", "case30", "case57", "case118"] {
        let native = parse(Source::open(input.join(format!("{case}.json"))).unwrap()).unwrap();
        let reference = parse(Source::open(cases.join(format!("{case}.m"))).unwrap()).unwrap();
        let PioValue::BalancedNetwork(actual) = native.value() else {
            panic!("network");
        };
        let PioValue::BalancedNetwork(expected) = reference.value() else {
            panic!("network");
        };
        assert_eq!(actual.buses().len(), expected.buses().len());
        compare_ybus(actual, &input.join(format!("{case}.ybus.json")));
        compare_costs(actual, &input.join(format!("{case}.costs.json")));
        let exported = input.join(format!("{case}.export-ybus.json"));
        if exported.exists() {
            compare_ybus(expected, &exported);
            compare_costs(expected, &input.join(format!("{case}.export-costs.json")));
        }
        for bus in expected.buses() {
            let a: f64 = actual
                .generators()
                .iter()
                .filter(|g| g.bus == bus.id)
                .map(|g| g.pg)
                .sum();
            let e: f64 = expected
                .generators()
                .iter()
                .filter(|g| g.bus == bus.id)
                .map(|g| g.pg)
                .sum();
            assert!((a - e).abs() < 1e-8, "{case}: generation on {:?}", bus.id);
            let a: f64 = actual
                .loads()
                .iter()
                .filter(|g| g.bus == bus.id)
                .map(|g| g.p)
                .sum();
            let e: f64 = expected
                .loads()
                .iter()
                .filter(|g| g.bus == bus.id)
                .map(|g| g.p)
                .sum();
            assert!((a - e).abs() < 1e-8, "{case}: load on {:?}", bus.id);
        }
    }
}

fn compare_ybus(net: &powerio::BalancedNetwork, path: &std::path::Path) {
    #[derive(serde::Deserialize)]
    struct Oracle {
        entries: Vec<(usize, usize, f64, f64)>,
        epsilon: f64,
    }
    let oracle: Oracle = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let native: std::collections::BTreeMap<_, _> = oracle
        .entries
        .into_iter()
        .map(|(a, b, g, bi)| ((a, b), (g, bi)))
        .collect();
    let index = IndexedNetwork::new(net);
    let matrix = calc_admittance_matrix(&index, &BuildOptions::default()).unwrap();
    for from in net.buses() {
        for to in net.buses() {
            let i = index.bus_index(from.id).unwrap();
            let j = index.bus_index(to.id).unwrap();
            let expected = native
                .get(&(from.id.0, to.id.0))
                .copied()
                .unwrap_or_default();
            for (m, e) in [(&matrix.g, expected.0), (&matrix.b, expected.1)] {
                let a = m.get(i, j).copied().unwrap_or(0.0);
                assert!(
                    (a - e).abs() < 1e-8 + 8.0 * oracle.epsilon * e.abs().max(1.0),
                    "{}: admittance {}->{}: {a} vs {e}",
                    path.display(),
                    from.id.0,
                    to.id.0
                );
            }
        }
    }
}

fn compare_costs(net: &powerio::BalancedNetwork, path: &std::path::Path) {
    let costs: Vec<(usize, f64, f64)> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (bus, power, expected) in costs {
        let actual: f64 = net
            .generators()
            .iter()
            .filter(|g| g.bus.0 == bus)
            .map(|g| {
                let (q, c, c0) = g
                    .cost
                    .as_ref()
                    .expect("reference generator cost")
                    .calc_quadratic_with_constant_tol(0.0)
                    .expect("reference quadratic cost");
                0.5 * q * power * power + c * power + c0
            })
            .sum();
        assert!(
            (actual - expected).abs() < 1e-8,
            "{}: generation cost on {bus} at {power} MW: {actual} vs {expected}",
            path.display()
        );
    }
}

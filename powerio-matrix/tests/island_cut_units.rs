//! A cut must preserve the electrical units consumed by matrix builders.

use std::collections::BTreeSet;

use powerio_matrix::matrix::{BuildOptions, calc_admittance_matrix};
use powerio_tx::{
    BalancedNetwork, Branch, Bus, BusId, BusType, Generator, IndexedNetwork, Load, Selector, Shunt,
};

fn case() -> BalancedNetwork {
    let mut buses: Vec<_> = [10, 20, 30]
        .map(|id| Bus::new(BusId(id), BusType::Pq, 230.0))
        .into();
    buses[0].kind = BusType::Ref;
    buses[0].va = 25.0;
    let mut branch = Branch::new(BusId(10), BusId(20), 0.01, 0.1);
    branch.tap = 1.1;
    branch.shift = 15.0;
    let mut net = BalancedNetwork::in_memory(
        "cut units",
        100.0,
        buses,
        vec![branch, Branch::new(BusId(20), BusId(30), 0.02, 0.2)],
    );
    net.loads_mut().push(Load::new(BusId(10), 50.0, 10.0));
    net.shunts_mut().push(Shunt::new(BusId(10), 2.0, 3.0));
    net.generators_mut().push(Generator::new(BusId(10)));
    net
}

#[test]
fn whole_and_partial_normalized_cuts_match_raw_cut_matrices() {
    let raw = case();
    let normalized = raw.to_normalized().unwrap();
    for selected in [
        vec![BusId(10), BusId(20), BusId(30)],
        vec![BusId(10), BusId(20)],
    ] {
        let selected: BTreeSet<_> = selected.into_iter().collect();
        let cut = normalized.subset_buses(&selected);
        let expected = raw.subset_buses(&selected).network.to_normalized().unwrap();
        assert!(cut.network.is_normalized());
        assert!((cut.network.loads()[0].p - 0.5).abs() < 1e-12);
        assert!((cut.network.shunts()[0].g - 0.02).abs() < 1e-12);
        assert!((cut.network.buses()[0].va - 25_f64.to_radians()).abs() < 1e-12);
        assert!((cut.network.branches()[0].shift - 15_f64.to_radians()).abs() < 1e-12);
        assert_eq!(
            cut.bus_rows,
            [Some(0), Some(1), selected.contains(&BusId(30)).then_some(2)]
        );
        assert_eq!(
            cut.branch_rows,
            [Some(0), selected.contains(&BusId(30)).then_some(1)]
        );
        let expected_y =
            calc_admittance_matrix(&IndexedNetwork::new(&expected), &BuildOptions::default())
                .unwrap();
        let actual_y =
            calc_admittance_matrix(&IndexedNetwork::new(&cut.network), &BuildOptions::default())
                .unwrap();
        for (expected, actual) in [(&expected_y.g, &actual_y.g), (&expected_y.b, &actual_y.b)] {
            assert_eq!(expected.shape(), actual.shape());
            for i in 0..expected.rows() {
                for j in 0..expected.cols() {
                    let expected = expected.get(i, j).copied().unwrap_or(0.0);
                    let actual = actual.get(i, j).copied().unwrap_or(0.0);
                    assert!(
                        (actual - expected).abs() < 1e-12,
                        "({i},{j}): {actual} != {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn selector_cuts_preserve_normalized_units_too() {
    let normalized = case().to_normalized().unwrap();
    let cut = normalized.subset(
        &Selector {
            bus: Some((10, 20)),
            ..Selector::default()
        },
        false,
    );
    assert!(cut.is_normalized());
    assert!((cut.loads()[0].p - 0.5).abs() < 1e-12);
    assert!((cut.shunts()[0].g / IndexedNetwork::new(&cut).per_unit_base() - 0.02).abs() < 1e-12);
}

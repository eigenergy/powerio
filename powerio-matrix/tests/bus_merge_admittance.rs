//! Collapsing bus voltages must preserve the corresponding Y-bus projection.

use powerio_matrix::matrix::{BuildOptions, calc_admittance_matrix};
use powerio_tx::{
    BalancedNetwork, Branch, BranchCharging, Bus, BusId, BusMergeRule, BusType, Generator,
    IndexedNetwork, MergedCharging, Shunt, Switch, ZeroImpedanceRule,
};

fn network(tap: f64, shift: f64, charging: bool) -> BalancedNetwork {
    let buses = (1..=3)
        .map(|id| Bus::new(BusId(id), BusType::Ref, 230.0))
        .collect();
    let mut transformer = Branch::new(BusId(1), BusId(2), 0.01, 0.1);
    transformer.tap = tap;
    transformer.shift = shift;
    if charging {
        transformer.charging = Some(BranchCharging::new(0.02, -0.03, 0.04, 0.05));
    }
    let mut net = BalancedNetwork::in_memory(
        "bypassed-transformer",
        100.0,
        buses,
        vec![transformer, Branch::new(BusId(2), BusId(3), 0.02, 0.2)],
    );
    net.shunts_mut().push(Shunt::new(BusId(2), 1.0, -2.0));
    net.generators_mut().push(Generator::new(BusId(1)));
    net.switches_mut()
        .push(Switch::new(BusId(1), BusId(2), true));
    net
}

fn assert_projection(net: &BalancedNetwork, rule: BusMergeRule) {
    let original =
        calc_admittance_matrix(&IndexedNetwork::new(net), &BuildOptions::default()).unwrap();
    let merge = net.merge_buses(&rule).unwrap();
    merge.network.validate().unwrap();
    let view = IndexedNetwork::new(&merge.network);
    let actual = calc_admittance_matrix(&view, &BuildOptions::default()).unwrap();
    assert_eq!(merge.network.buses().len(), 2);
    assert_eq!(merge.branch_rows[0], None);
    assert_eq!(merge.branch_rows[1], Some(0));
    assert!(merge.network.merge_buses(&rule).unwrap().is_identity());
    // Q maps each original voltage to its survivor. Check every entry of
    // Q' Y Q, including the connection to the bus outside the merged group.
    let positions: Vec<_> = net
        .buses()
        .iter()
        .map(|bus| view.bus_index(merge.survivor(bus.id)).unwrap())
        .collect();
    for (before, after) in [(&original.g, &actual.g), (&original.b, &actual.b)] {
        let mut expected = [[0.0; 2]; 2];
        for (i, row) in before.outer_iterator().enumerate() {
            for (j, value) in row.iter() {
                expected[positions[i]][positions[j]] += value;
            }
        }
        for (i, row) in expected.iter().enumerate() {
            for (j, expected) in row.iter().enumerate() {
                let actual = after.get(i, j).copied().unwrap_or(0.0);
                assert!(
                    (expected - actual).abs() < 1e-9,
                    "entry ({i}, {j}): collapsed {expected}, merged {actual}"
                );
            }
        }
    }
}

#[test]
fn bypassed_transformers_preserve_the_full_admittance() {
    for (tap, shift) in [(1.1, 0.0), (1.0, 25.0), (0.9, -20.0), (0.0, 0.0)] {
        for charging in [false, true] {
            let mut net = network(tap, shift, charging);
            assert_projection(&net, BusMergeRule::closed_switches());
            // Normalize without the switch so this also exercises an explicit
            // merge after normalization on the #576 automatic-merge path.
            net.switches_mut().clear();
            let mut normalized = net.to_normalized().unwrap();
            let (from, to) = (normalized.buses()[0].id, normalized.buses()[1].id);
            normalized.switches_mut().push(Switch::new(from, to, true));
            assert_projection(&normalized, BusMergeRule::closed_switches());
        }
    }
}

#[test]
fn a_jumper_path_preserves_a_bypassed_transformer() {
    let mut net = network(1.1, 15.0, true);
    net.switches_mut().clear();
    net.branches_mut()
        .push(Branch::new(BusId(1), BusId(2), 0.0, 1e-4));
    assert_projection(
        &net,
        BusMergeRule::new(false, Some(ZeroImpedanceRule::PsseThreshold(1e-4))),
    );
}

#[test]
fn dropping_charging_still_preserves_the_series_equivalent() {
    assert_projection(
        &network(1.1, 20.0, false),
        BusMergeRule::closed_switches().with_charging(MergedCharging::Drop),
    );
    let charged = network(1.1, 20.0, true);
    let drop = charged
        .merge_buses(&BusMergeRule::closed_switches().with_charging(MergedCharging::Drop))
        .unwrap();
    let uncharged = network(1.1, 20.0, false)
        .merge_buses(&BusMergeRule::closed_switches())
        .unwrap();
    assert_eq!(drop.network.shunts(), uncharged.network.shunts());
}

#[test]
fn out_of_service_transformers_add_no_equivalent_shunt() {
    let mut net = network(1.1, 20.0, true);
    net.branches_mut()[0].in_service = false;
    assert_projection(&net, BusMergeRule::closed_switches());
    let merge = net.merge_buses(&BusMergeRule::closed_switches()).unwrap();
    assert_eq!(merge.charging_shunts, Vec::<usize>::new());
}

#[test]
fn bypassed_ideal_transformers_are_rejected_without_changing_the_source() {
    for (tap, shift) in [(1.1, 0.0), (1.0, 20.0)] {
        let mut net = network(tap, shift, true);
        net.branches_mut()[0].r = 0.0;
        net.branches_mut()[0].x = 0.0;
        for charging in [MergedCharging::FoldToShunt, MergedCharging::Drop] {
            assert!(matches!(
                net.merge_buses(&BusMergeRule::closed_switches().with_charging(charging)),
                Err(powerio_tx::Error::ZeroImpedance { row: 0 })
            ));
        }
        assert_eq!(net.buses().len(), 3);
        assert_eq!(net.branches().len(), 2);
        assert_eq!(net.shunts().len(), 1);
        assert_eq!(net.switches().len(), 1);
    }
}

#[test]
fn nominal_ideal_jumpers_still_fold_charging() {
    for tap in [0.0, 1.0] {
        let mut net = network(tap, 0.0, true);
        net.branches_mut()[0].r = 0.0;
        net.branches_mut()[0].x = 0.0;
        let merge = net.merge_buses(&BusMergeRule::closed_switches()).unwrap();
        assert_eq!(merge.charging_shunts, [1]);
        let shunt = &merge.network.shunts()[1];
        assert!((shunt.g - 6.0).abs() < 1e-12);
        assert!((shunt.b - 2.0).abs() < 1e-12);
    }
}

#[test]
fn invalid_equivalents_return_errors_instead_of_nonfinite_shunts() {
    let mut net = network(1e-200, 0.0, false);
    assert!(matches!(
        net.merge_buses(&BusMergeRule::closed_switches()),
        Err(powerio_tx::Error::DegenerateTap { row: 0, .. })
    ));
    net.branches_mut()[0].tap = 1.1;
    net.branches_mut()[0].shift = f64::NAN;
    assert!(matches!(
        net.merge_buses(&BusMergeRule::closed_switches()),
        Err(powerio_tx::Error::NonFiniteSusceptance { row: 0 })
    ));
}

#[test]
fn impedance_correction_before_merging_folds_the_corrected_admittance() {
    use powerio_tx::{ImpedanceCorrection, ImpedanceCorrectionPoint, ImpedanceCorrectionVariable};
    let mut net = network(1.05, 20.0, true);
    net.branches_mut()[0].set_impedance_correction(Some(ImpedanceCorrection::new(
        1,
        ImpedanceCorrectionVariable::Ratio,
        vec![
            ImpedanceCorrectionPoint::new(0.9, 1.0, 0.0),
            ImpedanceCorrectionPoint::new(1.1, 2.0, 0.2),
        ],
    )));
    assert_eq!(net.apply_impedance_correction(), 1);
    assert!((net.branches()[0].r - 0.0025).abs() < 1e-12);
    assert!((net.branches()[0].x - 0.1765).abs() < 1e-12);
    assert_projection(&net, BusMergeRule::closed_switches());
    let mut merged = net
        .merge_buses(&BusMergeRule::closed_switches())
        .unwrap()
        .network;
    assert_eq!(
        merged.apply_impedance_correction(),
        0,
        "the transformer is already folded into the shunt"
    );
}

#[test]
fn automatic_switch_normalization_keeps_the_equivalent_and_source_rows() {
    let net = network(1.1, 20.0, true);
    let explicit = net
        .merge_buses(&BusMergeRule::closed_switches())
        .unwrap()
        .network
        .to_normalized()
        .unwrap();
    let (automatic, rows) = net
        .to_normalized_with_source_rows(&powerio_tx::NormalizeOptions::default())
        .unwrap();
    assert_eq!(rows.shunts, [Some(0), None]);
    assert_eq!(rows.branches, [Some(1)]);
    let expected =
        calc_admittance_matrix(&IndexedNetwork::new(&explicit), &BuildOptions::default()).unwrap();
    let actual = calc_admittance_matrix(
        &IndexedNetwork::new(&automatic.network),
        &BuildOptions::default(),
    )
    .unwrap();
    assert_eq!(expected.g, actual.g);
    assert_eq!(expected.b, actual.b);
    let mut ideal = net.clone();
    ideal.branches_mut()[0].r = 0.0;
    ideal.branches_mut()[0].x = 0.0;
    assert!(matches!(
        ideal.to_normalized(),
        Err(powerio_tx::Error::ZeroImpedance { row: 0 })
    ));
}

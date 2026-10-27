//! A refused winding-pair merge must not leave an unmodeled ideal connection.

use powerio_tx::{
    BalancedNetwork, Bus, BusId, BusType, ClosedSwitchPolicy, Error, Generator, Impedance,
    IndexedNetwork, NormalizeOptions, NormalizeSettings, Switch, Transformer3W, Winding,
};

fn network() -> BalancedNetwork {
    let mut buses: Vec<_> = (1..=5)
        .map(|id| Bus::new(BusId(id), BusType::Pq, 230.0))
        .collect();
    buses[0].kind = BusType::Ref;
    buses[4].kind = BusType::Isolated;
    let mut net = BalancedNetwork::in_memory("refused-switch", 100.0, buses, vec![]);
    net.generators_mut().push(Generator::new(BusId(1)));
    net.transformers_3w_mut().push(Transformer3W::new(
        [1, 2, 3].map(|id| Winding::new(BusId(id))),
        [Impedance::new(0.0, 0.1, 100.0); 3],
    ));
    net
}

fn assert_closed(error: Error, expected: (usize, BusId, BusId, usize)) {
    assert_eq!(error.code().code, "BUILD.SWITCH.CLOSED");
    match error {
        Error::ClosedSwitch {
            row,
            from,
            to,
            closed,
        } => {
            assert_eq!((row, from, to, closed), expected);
        }
        other => panic!("expected a closed-switch error, got {other}"),
    }
}

#[test]
fn a_closed_switch_across_a_dead_transformer_or_winding_still_merges() {
    // Only live windings couple, so these merges short nothing; 0.11.5
    // normalized both networks.
    let mut out = network();
    out.transformers_3w_mut()[0].in_service = false;
    out.switches_mut()
        .push(Switch::new(BusId(1), BusId(2), true));
    assert!(out.to_normalized().is_ok());

    let mut open_winding = network();
    open_winding.transformers_3w_mut()[0].set_winding_in_service([true, true, false]);
    open_winding
        .switches_mut()
        .push(Switch::new(BusId(2), BusId(3), true));
    assert!(open_winding.to_normalized().is_ok());
}

#[test]
fn every_merge_entry_point_rejects_a_refused_winding_pair() {
    let mut net = network();
    net.switches_mut()
        .push(Switch::new(BusId(1), BusId(2), true));
    let expected = (0, BusId(1), BusId(2), 1);
    assert_closed(net.to_normalized().unwrap_err(), expected);
    assert_closed(
        net.to_normalized_with_options(&NormalizeOptions::default())
            .unwrap_err(),
        expected,
    );
    assert_closed(
        net.to_normalized_with_source_rows(&NormalizeOptions::default())
            .unwrap_err(),
        expected,
    );
    assert_closed(
        net.to_normalized_with(&NormalizeSettings::default())
            .unwrap_err(),
        expected,
    );
    // Explicit Keep is for consumers which model the ideal connection.
    let (kept, report) = net
        .to_normalized_with(
            &NormalizeSettings::default().with_closed_switches(ClosedSwitchPolicy::Keep),
        )
        .unwrap();
    assert!(report.bus_merge.is_none());
    assert!(matches!(
        IndexedNetwork::new(&kept.network).check_closed_switches(),
        Err(Error::ClosedSwitch { closed: 1, .. })
    ));
    assert_closed(
        net.to_normalized_with(
            &NormalizeSettings::default().with_closed_switches(ClosedSwitchPolicy::Refuse),
        )
        .unwrap_err(),
        expected,
    );
    assert_eq!(net.buses().len(), 5);
    assert_eq!(net.switches().len(), 1);
    assert_eq!(net.transformers_3w()[0].windings[1].bus, BusId(2));
}

#[test]
fn a_partial_merge_reports_original_switch_rows_and_remaining_count() {
    let mut net = network();
    net.switches_mut().extend([
        Switch::new(BusId(1), BusId(4), true),  // merges successfully
        Switch::new(BusId(1), BusId(2), false), // open, not eligible
        Switch::new(BusId(4), BusId(5), true),  // isolated end, filtered out
        Switch::new(BusId(4), BusId(2), true),  // refused through bus 1
        Switch::new(BusId(2), BusId(3), true),  // second refused pair
    ]);
    assert_closed(net.to_normalized().unwrap_err(), (3, BusId(4), BusId(2), 2));
    // The tentative 1-4 merge must not mutate the caller's source.
    assert_eq!(net.buses().len(), 5);
    assert_eq!(net.switches()[3].from, BusId(4));
    assert_eq!(net.switches().len(), 5);
}

#[test]
fn switches_removed_by_bus_filtering_do_not_cause_false_refusals() {
    let mut net = network();
    net.switches_mut().extend([
        Switch::new(BusId(2), BusId(5), true),   // isolated bus
        Switch::new(BusId(3), BusId(99), true),  // undeclared bus
        Switch::new(BusId(3), BusId(99), false), // open, undeclared bus
        Switch::new(BusId(1), BusId(4), true),   // supported merge
        Switch::new(BusId(1), BusId(2), false),  // open winding pair
        Switch::new(BusId(1), BusId(1), true),   // already one bus
    ]);
    let normalized = net.to_normalized().unwrap();
    normalized.validate().unwrap();
    IndexedNetwork::new(&normalized)
        .check_closed_switches()
        .unwrap();
    assert_eq!(normalized.buses().len(), 3);
    let (_, report) = net
        .to_normalized_with(&NormalizeSettings::default())
        .unwrap();
    let merge = report.bus_merge.unwrap();
    assert_eq!(merge.removed_switches.len(), 1);
    assert_eq!(merge.removed_switches[0].row, 3);
    assert_eq!(merge.removed_switches[0].identity, "switches:3");
    assert_eq!(
        merge.switch_rows,
        [None, None, None, None, Some(0), Some(1)]
    );
    assert_eq!(merge.source.switches(), net.switches());
    let (_, rows) = net
        .to_normalized_with_source_rows(&NormalizeOptions::default())
        .unwrap();
    assert_eq!(rows.switches, [Some(4), Some(5)]);
}

#[test]
fn filtered_merges_keep_source_identities_in_diagnostics_and_results() {
    let mut net = network();
    let mut named = Switch::new(BusId(2), BusId(3), false);
    named.uid = Some("named-open".into());
    net.switches_mut().extend([
        Switch::new(BusId(1), BusId(99), false),
        Switch::new(BusId(1), BusId(4), true),
        Switch::new(BusId(1), BusId(4), false),
        Switch::new(BusId(1), BusId(2), false),
        named,
    ]);
    let (normalized, report) = net
        .to_normalized_with(&NormalizeSettings::default())
        .unwrap();
    let merge = report.bus_merge.unwrap();
    assert_eq!(merge.removed_switches[1].row, 2);
    assert_eq!(merge.removed_switches[1].identity, "switches:2");
    let diagnostic = normalized
        .diagnostics
        .iter()
        .find(|d| d.code() == "CANONICALIZE.MERGE.ELEMENT_SHORTED")
        .unwrap();
    assert!(diagnostic.message().contains("open switch `switches:2`"));
    assert!(merge.network.switches()[0].uid.is_none());
    assert_eq!(
        merge.network.switches()[1].uid.as_deref(),
        Some("named-open")
    );
    assert_eq!(merge.source.switches(), net.switches());
}

#[test]
fn per_island_normalization_also_rejects_an_incomplete_switch_merge() {
    let mut net = network();
    net.switches_mut()
        .push(Switch::new(BusId(1), BusId(2), true));
    let settings = NormalizeSettings::default()
        .with_island_references(powerio_tx::IslandReferencePolicy::PerIsland);
    assert_closed(
        net.to_normalized_with(&settings).unwrap_err(),
        (0, BusId(1), BusId(2), 1),
    );
}

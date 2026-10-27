use powerio_tx::{
    BalancedNetwork, Bus, BusId, BusType, Generator, Impedance, IndexedNetwork, NormalizeOptions,
    Transformer3W, Winding,
};

fn case(status: [bool; 3], dropped: usize) -> BalancedNetwork {
    let mut buses = [10, 20, 30].map(|id| Bus::new(BusId(id), BusType::Pq, 230.0));
    buses[0].kind = BusType::Ref;
    buses[dropped].kind = BusType::Isolated;
    let mut net = BalancedNetwork::in_memory("partial", 100.0, buses.to_vec(), vec![]);
    let mut transformer = Transformer3W::new(
        [10, 20, 30].map(|id| Winding::new(BusId(id))),
        [
            Impedance::new(0.0, 0.2, 100.0),
            Impedance::new(0.0, 0.3, 100.0),
            Impedance::new(0.0, 0.4, 100.0),
        ],
    );
    transformer.set_winding_in_service(status);
    net.transformers_3w_mut().push(transformer);
    net.generators_mut().push(Generator::new(BusId(10)));
    net
}

#[test]
fn normalization_keeps_the_two_live_windings_and_source_row() {
    let net = case([true, true, false], 2);
    let before = IndexedNetwork::new(&net)
        .network()
        .branches()
        .iter()
        .filter(|b| b.in_service)
        .count();
    assert_eq!(before, 2);
    let (normalized, rows) = net
        .to_normalized_with_source_rows(&NormalizeOptions::default())
        .unwrap();
    assert_eq!(normalized.network.transformers_3w().len(), 1);
    assert_eq!(rows.transformers_3w, vec![Some(0)]);
    assert_eq!(
        normalized.network.transformers_3w()[0].winding_in_service(),
        [true, true, false]
    );
    let view = IndexedNetwork::new(&normalized.network);
    assert_eq!(
        view.network()
            .branches()
            .iter()
            .filter(|b| b.in_service)
            .count(),
        before
    );
    view.check_reference_coverage().unwrap();
    normalized.network.validate().unwrap();
}

#[test]
fn a_removed_live_terminal_reports_the_winding_change() {
    let net = case([true, true, true], 1);
    let normalized = net
        .to_normalized_with_options(&NormalizeOptions::default())
        .unwrap();
    assert_eq!(
        normalized.network.transformers_3w()[0].winding_in_service(),
        [true, false, true]
    );
    assert!(
        normalized
            .diagnostics
            .iter()
            .any(|d| d.code() == "CANONICALIZE.NORMALIZE.WINDING_TAKEN_OUT")
    );
    normalized.network.validate().unwrap();
}

#[test]
fn fewer_than_two_live_windings_couple_nothing() {
    let net = case([true, true, false], 1);
    assert_eq!(
        net.to_normalized().unwrap().transformers_3w().as_slice(),
        []
    );
}

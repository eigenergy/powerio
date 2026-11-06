mod helpers;
use powerio_tx::{BalancedNetwork, Branch, Bus, BusId, BusType, Load, TargetFormat};
#[test]
fn balanced_ravens_line_does_not_override_to_bus_base() {
    let buses = vec![
        Bus::new(BusId(1), BusType::Ref, 220.0),
        Bus::new(BusId(2), BusType::Pq, 150.0),
    ];
    let mut net = BalancedNetwork::in_memory(
        "two-bases",
        100.0,
        buses,
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    net.loads_mut().push(Load::new(BusId(2), 10.0, 2.0));
    let out = helpers::emit_value(&net, TargetFormat::RavensJson).unwrap();
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    let b = back
        .network
        .buses()
        .iter()
        .find(|b| b.id == BusId(2))
        .unwrap();
    assert!(
        (b.base_kv - 150.0).abs() < 1e-9,
        "to-bus base became {} kV",
        b.base_kv
    );
}

use powerio::{BalancedNetwork, Bus, BusId, BusType, DynamicStudy, parse_dyr, parse_dyr_bytes};

#[test]
fn facade_pairing_keeps_sparse_source_ids_and_opaque_keys() {
    let text = "! retained comment\r\n1000 'USERMODEL' 'NOT-A-GENERATOR' 1.00 /\r\n77 'GENCLS' '1' 13.0 0.0 /\r\n10 'OTHER' 'LOAD-A' 'a,b' /\r\n";
    let network = BalancedNetwork::in_memory(
        "static",
        100.0,
        vec![
            Bus::new(BusId(1000), BusType::Ref, 230.0),
            Bus::new(BusId(10), BusType::Pq, 115.0),
        ],
        Vec::new(),
    );
    let dynamics = parse_dyr(text).unwrap();
    let study = DynamicStudy::new(network, dynamics);
    let missing = study.unresolved_buses();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].bus_id, 77);
    assert_eq!(missing[0].line, 3);
    assert_eq!(missing[0].model_name, "GENCLS");
    assert_eq!(
        study.dynamics().records()[0].device_id(),
        Some("NOT-A-GENERATOR")
    );
    assert_eq!(study.dynamics().to_dyr_string(), text);
    assert_eq!(study.network().buses()[0].id, BusId(1000));
    let (network, dynamics) = study.into_parts();
    assert_eq!(network.buses()[1].id, BusId(10));
    assert_eq!(dynamics.to_dyr_string(), text);
    assert_eq!(dynamics.unresolved_buses_in(&network), missing);
}

#[test]
fn public_byte_parser_preserves_source_and_rejects_invalid_utf8() {
    let text = "! no generic model schema\n10 'CUSTOM' 'KEY' 'x/y' 1e-3 /\n";
    let case = parse_dyr_bytes(text.as_bytes()).unwrap();
    assert_eq!(case.to_dyr_string(), text);
    assert_eq!(case.records()[0].parameters(), &["x/y", "1e-3"]);
    assert!(matches!(
        parse_dyr_bytes(&[0xff]),
        Err(powerio::DyrError::InvalidUtf8)
    ));
}

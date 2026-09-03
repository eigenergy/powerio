//! Geographic edits are PowerIO module transformations.

#[test]
fn a_geo_layer_derives_a_new_network_module() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = powerio::parse(powerio::Source::open(path).unwrap(), Some("matpower")).unwrap();
    assert!(module.source().is_some());

    let layer = powerio::GeoLayer {
        space: powerio::CoordinateSpace::Geographic { crs: None },
        kind: Some(powerio::CoordsKind::Manual),
        features: vec![powerio::GeoFeature {
            target: powerio::GeoTarget::Bus,
            key: powerio::ElementKey {
                id: Some("1".to_owned()),
                ..powerio::ElementKey::default()
            },
            geometry: powerio::GeoGeometry::Point([-83.743, 42.281]),
            from: None,
            to: None,
            kind: None,
        }],
    };

    let (placed, report) = powerio::apply_geo_layer(&module, &layer).unwrap();
    assert_eq!(report.matched_buses, 1);
    assert!(placed.source().is_none());
    assert_eq!(placed.sources(), module.sources());
    assert_eq!(placed.history().last().unwrap().name(), "apply_geo_layer");
    assert_eq!(
        placed.history().last().unwrap().input_type(),
        Some("powerio.BalancedNetwork")
    );

    let powerio::PioValue::BalancedNetwork(placed_network) = &placed.value() else {
        panic!("the transformation changed the network type")
    };
    let location = placed_network
        .buses()
        .iter()
        .find(|bus| bus.id == powerio::BusId(1))
        .and_then(|bus| bus.location)
        .unwrap();
    assert_eq!((location.x, location.y), (-83.743, 42.281));

    let powerio::PioValue::BalancedNetwork(original_network) = &module.value() else {
        unreachable!()
    };
    assert!(
        original_network
            .buses()
            .iter()
            .find(|bus| bus.id == powerio::BusId(1))
            .unwrap()
            .location
            .is_none()
    );
}

#[test]
fn geo_application_refuses_non_network_values() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let network = powerio::parse(powerio::Source::open(path).unwrap(), Some("matpower")).unwrap();
    let instance = powerio::to_dc_pf_instance(&network)
        .unwrap()
        .map_value(Into::into);
    let layer = powerio::GeoLayer {
        space: powerio::CoordinateSpace::Unknown,
        kind: None,
        features: Vec::new(),
    };

    let error = powerio::apply_geo_layer(&instance, &layer).unwrap_err();
    assert_eq!(
        error.info().unwrap().code,
        powerio::codes::REQUEST_MODULE_WRONG_MODEL_KIND.code
    );
}

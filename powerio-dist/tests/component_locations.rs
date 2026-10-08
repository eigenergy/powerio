use powerio_dist::{
    DistBus, DistComponentLocation, DistGeographicPoint, DistLocationTarget, MulticonductorNetwork,
};

fn located() -> MulticonductorNetwork {
    let mut net = MulticonductorNetwork::new();
    net.buses_mut()
        .push(DistBus::new("A", vec!["1".into(), "n".into()]));
    net.component_locations_mut()
        .push(DistComponentLocation::new(
            DistLocationTarget::Bus,
            "A",
            DistGeographicPoint::new(Some(0.0), None),
        ));
    net
}

#[test]
fn point_mutations_are_copy_on_write_and_typed_storage_keeps_partial_unknown_datum() {
    let original = located();
    let mut edited = original.clone();
    edited.component_locations_mut()[0].point.longitude = Some(153.0);
    assert_eq!(original.component_locations()[0].point.longitude, None);
    assert_eq!(edited.component_locations()[0].point.longitude, Some(153.0));
    let wire = serde_json::to_value(&original).unwrap();
    assert_eq!(wire["type"], "powerio.LocatedMulticonductorNetwork");
    assert!(
        wire["value"]["network"]
            .get("component_locations")
            .is_none()
    );
    let restored: MulticonductorNetwork = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(
        restored.component_locations(),
        original.component_locations()
    );
    assert!(restored.component_locations()[0].point.datum.is_none());
    assert_eq!(serde_json::to_value(restored).unwrap(), wire);
    let reversed = format!(
        "{{\"value\":{},\"type\":\"powerio.LocatedMulticonductorNetwork\"}}",
        wire["value"]
    );
    assert!(serde_json::from_str::<MulticonductorNetwork>(&reversed).is_ok());
}

#[test]
fn legacy_networks_keep_their_body_and_cannot_hide_tagged_coordinate_fields() {
    let mut net = located();
    net.component_locations_mut().clear();
    let body = serde_json::to_value(&net).unwrap();
    assert!(body.get("type").is_none());
    assert!(body.get("component_locations").is_none());
    assert!(serde_json::from_value::<MulticonductorNetwork>(body.clone()).is_ok());
    for field in ["type", "value", "component_locations"] {
        // Place reserved fields last as well as first; dispatch must not
        // silently accept a legacy body that hides a new representation.
        for input in [
            format!("{{\"{field}\":null,{}", &body.to_string()[1..]),
            format!(
                "{},\"{field}\":null}}",
                body.to_string().trim_end_matches('}')
            ),
        ] {
            assert!(
                serde_json::from_str::<MulticonductorNetwork>(&input).is_err(),
                "{field}"
            );
        }
    }
    let mut bad = serde_json::to_value(located()).unwrap();
    bad["type"] = "powerio.FutureNetwork".into();
    assert!(serde_json::from_value::<MulticonductorNetwork>(bad).is_err());
}

#[test]
fn invalid_points_duplicate_or_stale_attachments_fail_before_serialization() {
    for (latitude, longitude) in [
        (Some(f64::NAN), None),
        (Some(91.0), None),
        (None, Some(181.0)),
        (None, None),
    ] {
        let mut net = located();
        net.component_locations_mut()[0].point = DistGeographicPoint::new(latitude, longitude);
        assert!(net.validate_component_locations().is_err());
        assert!(serde_json::to_string(&net).is_err());
    }
    let mut net = located();
    net.buses_mut()[0].id = "a".into();
    assert!(net.validate_component_locations().is_err());
    let mut net = located();
    let point = net.component_locations()[0].clone();
    net.component_locations_mut().push(point);
    assert!(net.validate_component_locations().is_err());
    let mut bad = serde_json::to_value(located()).unwrap();
    bad["value"]["locations"][0]["name"] = "missing".into();
    assert!(serde_json::from_value::<MulticonductorNetwork>(bad).is_err());
}

#[test]
fn published_schema_accepts_legacy_body_and_rejects_located_network() {
    let old: serde_json::Value =
        serde_json::from_str(include_str!("../../docs/schema/pio-ir/2/schema.json")).unwrap();
    let current: serde_json::Value = serde_json::from_str(include_str!(
        "../../docs/schema/pio-ir/2/0.11.4/schema.json"
    ))
    .unwrap();
    let schema = |document: serde_json::Value| {
        serde_json::json!({
            "$ref": "#/$defs/MulticonductorNetwork", "$defs": document["$defs"],
        })
    };
    let old = schema(old);
    let current = schema(current);
    let net = located();
    let mut bare = net.clone();
    bare.component_locations_mut().clear();
    for (network, located) in [(&net, true), (&bare, false)] {
        let document = serde_json::to_value(network).unwrap();
        assert!(jsonschema::is_valid(&current, &document));
        assert_eq!(jsonschema::is_valid(&old, &document), !located);
    }
}

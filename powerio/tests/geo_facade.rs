use powerio::{CoordinateSpace, DisplayData, GeoGeometry, GeoTarget};

#[test]
fn aux_text_transforms_to_an_owned_substation_layer() {
    let layer = {
        let text = String::from(
            "DATA (Substation, [SubNum, Latitude, Longitude])\n{\n\
             12.0 34.2 -80.05\n\
             13 nan -80.10\n\
             14 34.4 \"\"\n\
             12 35.0 -81.00\n}\n",
        );
        powerio::to_geo_layer_from_aux_text(&text).unwrap()
    };

    assert!(matches!(
        layer.space,
        CoordinateSpace::Geographic { crs: None }
    ));
    assert_eq!(layer.features.len(), 2);
    assert!(
        layer
            .features
            .iter()
            .all(|feature| feature.target == GeoTarget::Substation)
    );
    assert_eq!(layer.features[0].key.id.as_deref(), Some("12"));
    assert_eq!(
        layer.features[0].geometry,
        GeoGeometry::Point([-80.05, 34.2])
    );
    assert_eq!(layer.features[1].key.id.as_deref(), Some("12"));
    assert_eq!(
        layer.features[1].geometry,
        GeoGeometry::Point([-81.0, 35.0])
    );
}

#[test]
fn aux_text_preserves_empty_and_malformed_results() {
    let empty = powerio::to_geo_layer_from_aux_text(
        "DATA (Substation, [SubNum, Latitude, Longitude])\n{\n7 nan -80\n}\n",
    )
    .unwrap();
    assert!(empty.features.is_empty());

    let error = powerio::to_geo_layer_from_aux_text(
        "DATA (Substation, [SubNum, Latitude, Longitude])\n{\n7 34 -80 99\n}\n",
    )
    .unwrap_err();
    assert_eq!(
        error.info().map(|entry| entry.code),
        Some("PARSE.SOURCE.MALFORMED")
    );
    assert!(error.to_string().contains("line"));
}

#[test]
fn facade_parses_a_display_memory_source() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/powerworld/ACTIVSg200.pwd"
    ))
    .unwrap();
    let source = powerio::Source::from_memory("display.pwd", bytes).unwrap();
    let DisplayData::PowerWorld(display) = powerio::parse_display(source, None).unwrap() else {
        panic!("PWD bytes did not produce a PowerWorld display");
    };
    assert!(!display.substations.is_empty());
}

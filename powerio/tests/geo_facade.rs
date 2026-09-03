use powerio::{CoordinateSpace, GeoGeometry, GeoTarget};

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

/// A display file is a value like any other case: `parse` returns it, `emit`
/// writes the canonical layer document, and PowerIO IR carries it.
#[test]
fn a_display_file_parses_serializes_and_emits_as_a_layer() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/powerworld/ACTIVSg200.pwd"
    );
    let module = powerio::parse(path).unwrap();
    assert_eq!(module.value.type_name(), "powerio.GeoLayer");
    let powerio::PioValue::GeoLayer(layer) = &module.value else {
        panic!("a .pwd reads as a layer");
    };
    assert!(!layer.features.is_empty());
    assert!(matches!(
        layer.space,
        CoordinateSpace::Diagram { canvas: Some(_) }
    ));

    // Content in memory reaches the same value when the name states the
    // format, and the declared format states it without a name.
    let bytes = std::fs::read(path).unwrap();
    let named = powerio::parse(powerio::Source::from_memory("display.pwd", bytes.clone()).unwrap())
        .unwrap();
    assert_eq!(named.value.type_name(), "powerio.GeoLayer");
    let declared = powerio::parse_with_options(
        bytes,
        &powerio::ParseOptions::default()
            .format("powerworld-pwd")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(declared.value.type_name(), "powerio.GeoLayer");

    // The layer travels through PowerIO IR and out as `geo-json`.
    let ir = powerio::serialize(&module, powerio::Destination::memory("layer").unwrap()).unwrap();
    let powerio::EmittedOutput::Memory { artifacts } = ir.into_output() else {
        panic!("a memory destination returned path output");
    };
    let text = artifacts.into_iter().next().unwrap().into_bytes();
    let decoded = powerio::deserialize(text).unwrap();
    let powerio::PioValue::GeoLayer(decoded) = &decoded.value else {
        panic!("PowerIO IR carries the layer");
    };
    assert_eq!(decoded, layer);

    let written = powerio::emit(
        &module,
        "geo-json",
        powerio::Destination::memory("layer").unwrap(),
    )
    .unwrap();
    let powerio::EmittedOutput::Memory { artifacts } = written.into_output() else {
        panic!("a memory destination returned path output");
    };
    let document = String::from_utf8(artifacts.into_iter().next().unwrap().into_bytes()).unwrap();
    let reread = powerio::parse(
        powerio::Source::from_memory("layer.geo.json", document.into_bytes()).unwrap(),
    )
    .unwrap();
    let powerio::PioValue::GeoLayer(reread) = &reread.value else {
        panic!("the canonical document reads back as a layer");
    };
    assert_eq!(reread.features.len(), layer.features.len());

    // No grid exchange format states a standalone layer.
    let refused = powerio::emit(
        &module,
        "matpower",
        powerio::Destination::memory("case").unwrap(),
    )
    .unwrap_err();
    assert_eq!(
        refused.diagnostics()[0].code(),
        "REQUEST.EMIT.UNSUPPORTED_VALUE_TYPE"
    );
}

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

#[test]
fn a_branch_endpoint_pair_is_a_durable_geo_identity() {
    let layer = powerio::GeoLayer {
        space: CoordinateSpace::Geographic { crs: None },
        kind: None,
        features: vec![powerio::GeoFeature {
            target: GeoTarget::Branch,
            key: powerio::ElementKey::default(),
            geometry: GeoGeometry::LineString(vec![[0.0, 0.0], [1.0, 1.0]]),
            from: Some("1".to_owned()),
            to: Some("2".to_owned()),
            kind: None,
        }],
    };
    let module = powerio::PioModule::new(powerio::PioValue::GeoLayer(layer.clone()));
    let emission = powerio::serialize(&module, powerio::Destination::memory("layer").unwrap())
        .expect("the endpoint pair is a complete branch identity");
    let powerio::EmittedOutput::Memory { artifacts } = emission.into_output() else {
        panic!("a memory destination returned path output");
    };
    let decoded = powerio::deserialize(artifacts.into_iter().next().unwrap().into_bytes())
        .expect("the emitted endpoint pair remains valid");
    let powerio::PioValue::GeoLayer(decoded_layer) = decoded.value() else {
        panic!("the stored value is not a geographic layer");
    };
    assert_eq!(decoded_layer, &layer);
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
    assert_eq!(module.value().type_name(), "powerio.GeoLayer");
    let powerio::PioValue::GeoLayer(layer) = &module.value() else {
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
    assert_eq!(named.value().type_name(), "powerio.GeoLayer");
    let declared = powerio::parse_with_options(
        bytes,
        &powerio::ParseOptions::default()
            .format("powerworld-pwd")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(declared.value().type_name(), "powerio.GeoLayer");

    // The layer travels through PowerIO IR and out as `geo-json`.
    let ir = powerio::serialize(&module, powerio::Destination::memory("layer").unwrap()).unwrap();
    let powerio::EmittedOutput::Memory { artifacts } = ir.into_output() else {
        panic!("a memory destination returned path output");
    };
    let text = artifacts.into_iter().next().unwrap().into_bytes();
    let decoded = powerio::deserialize(text).unwrap();
    let powerio::PioValue::GeoLayer(decoded) = &decoded.value() else {
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
    let powerio::PioValue::GeoLayer(reread) = &reread.value() else {
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

/// The `geo.json` extension is matched after a separator. A case whose stem
/// merely ends in the same letters keeps its JSON classification, which
/// matters because JSON content classification has no layer verdict. Naming
/// the display file as the emit target says the format has no writer rather
/// than calling it a grid case.
#[test]
fn the_layer_extension_needs_a_separator_and_the_display_target_names_its_gap() {
    let case = r#"{"baseMVA":100,"bus":{"1":{"bus_i":1,"bus_type":3,"pd":0,"qd":0,
        "gs":0,"bs":0,"area":1,"vm":1,"va":0,"base_kv":230,"zone":1,
        "vmax":1.1,"vmin":0.9}},"gen":{},"branch":{},"per_unit":true,
        "source_type":"matpower","name":"apogeo"}"#;
    for name in ["apogeo.json", "chicago.json"] {
        let module =
            powerio::parse(powerio::Source::from_memory(name, case.as_bytes().to_vec()).unwrap())
                .unwrap_or_else(|error| panic!("`{name}` is a case, not a layer: {error}"));
        assert_eq!(
            module.value().type_name(),
            "powerio.BalancedNetwork",
            "`{name}` must keep its case classification"
        );
    }

    // A name that carries the letters in the middle keeps the reader its own
    // extension selects.
    let matpower = "function mpc = c\nmpc.baseMVA = 100;\n\
                    mpc.bus = [1 3 0 0 0 0 1 1 0 230 1 1.1 0.9;];\n\
                    mpc.gen = [1 0 0 10 -10 1 100 1 10 0;];\nmpc.branch = [];\n";
    let module = powerio::parse(
        powerio::Source::from_memory("geo.json.m", matpower.as_bytes().to_vec()).unwrap(),
    )
    .unwrap();
    assert_eq!(module.value().type_name(), "powerio.BalancedNetwork");

    let layer = r#"{"type":"FeatureCollection","features":[{"type":"Feature",
        "geometry":{"type":"Point","coordinates":[-89.6,40.6]},
        "properties":{"powerio_target":"bus","powerio_id":"1"}}]}"#;
    for name in [
        "layer.geo.json",
        "layer_geo.json",
        "layer-geo.json",
        "geo.json",
    ] {
        let module =
            powerio::parse(powerio::Source::from_memory(name, layer.as_bytes().to_vec()).unwrap())
                .unwrap_or_else(|error| panic!("`{name}` is a layer: {error}"));
        assert_eq!(
            module.value().type_name(),
            "powerio.GeoLayer",
            "`{name}` must route to the layer reader"
        );
    }

    let module = powerio::parse(
        powerio::Source::from_memory("layer.geo.json", layer.as_bytes().to_vec()).unwrap(),
    )
    .unwrap();
    let refused = powerio::emit(
        &module,
        "powerworld-pwd",
        powerio::Destination::memory("display").unwrap(),
    )
    .unwrap_err();
    assert!(
        refused.to_string().contains("has no writer"),
        "the display target names its own gap: {refused}"
    );
}

//! GeoLayer: tolerant reads, canonical writes, extract/apply, and the
//! PowerWorld `.pwd` promotion.

use powerio::{
    Bus, BusId, BusType, CoordinateSpace, CoordsKind, GeoGeometry, GeoLayer, GeoTarget, Location,
    Network, apply_substation_points, geo_layer_from_pwd, parse_display_file,
    pwd_mercator_to_lonlat,
};

fn parse(bytes: &[u8], hint: Option<&str>) -> powerio::GeoParsed {
    GeoLayer::parse_bytes(bytes, hint).expect("parse geo layer")
}

fn small_network() -> Network {
    let mut bus1 = Bus::new(BusId(1), BusType::Ref, 230.0);
    bus1.name = Some("North".to_owned());
    let mut bus2 = Bus::new(BusId(2), BusType::Pq, 230.0);
    bus2.name = Some("South".to_owned());
    let branch = powerio::Branch::new(BusId(1), BusId(2), 0.01, 0.1);
    let mut net = Network::in_memory("small", 100.0, vec![bus1, bus2], vec![branch]);
    net.generators.push(powerio::Generator::new(BusId(1)));
    net
}

// ---------------------------------------------------------------------------
// Tolerant reads
// ---------------------------------------------------------------------------

#[test]
fn headerless_buscoords_csv_reads_as_bus_points() {
    let parsed = parse(b"b1, -89.6, 40.6\nb2, -89.2, 39.8\n", None);
    assert_eq!(parsed.layer.features.len(), 2);
    let feature = &parsed.layer.features[0];
    assert_eq!(feature.target, GeoTarget::Bus);
    assert_eq!(feature.key.id.as_deref(), Some("b1"));
    assert_eq!(feature.geometry, GeoGeometry::Point([-89.6, 40.6]));
    // All points fit lon/lat bounds, so the space reads geographic.
    assert!(matches!(
        parsed.layer.space,
        CoordinateSpace::Geographic { .. }
    ));
}

#[test]
fn whitespace_separated_buscoords_read() {
    let parsed = parse(b"b1 -89.6 40.6\nb2 -89.2 39.8\n", None);
    assert_eq!(parsed.layer.features.len(), 2);
}

#[test]
fn projected_buscoords_read_as_unknown_space() {
    let parsed = parse(b"b1, 653800.0, 3626000.0\n", None);
    assert!(matches!(parsed.layer.space, CoordinateSpace::Unknown));
}

#[test]
fn aliased_csv_header_reads_points_and_branch_segments() {
    let text = "Bus Number,Latitude,Longitude\n312,34.2,-80.05\n410,34.3,-80.10\n";
    let parsed = parse(text.as_bytes(), Some("layout.csv"));
    assert_eq!(parsed.layer.features.len(), 2);
    assert_eq!(parsed.layer.features[0].key.id.as_deref(), Some("312"));

    let branch_csv = "from_bus,to_bus,lat1,lon1,lat2,lon2\n312,410,34.2,-80.05,34.3,-80.10\n";
    let parsed = parse(branch_csv.as_bytes(), Some("routes.csv"));
    let branch = parsed
        .layer
        .features
        .iter()
        .find(|f| f.target == GeoTarget::Branch)
        .expect("branch feature");
    assert_eq!(branch.from.as_deref(), Some("312"));
    assert_eq!(branch.to.as_deref(), Some("410"));
    assert_eq!(
        branch.geometry,
        GeoGeometry::LineString(vec![[-80.05, 34.2], [-80.10, 34.3]])
    );
}

#[test]
fn json_records_read_with_aliases() {
    let text = r#"[{"bus_i": 312, "lat": "34.2", "lng": "-80.05"}]"#;
    let parsed = parse(text.as_bytes(), None);
    assert_eq!(parsed.layer.features.len(), 1);
    assert_eq!(parsed.layer.features[0].key.id.as_deref(), Some("312"));

    // Records nested under an object key (the PowerModels-style dict).
    let nested = r#"{"buses": [{"id": "1", "x": -80.0, "y": 34.0}]}"#;
    let parsed = parse(nested.as_bytes(), None);
    assert_eq!(parsed.layer.features.len(), 1);
}

#[test]
fn geojson_features_read_points_and_linestrings() {
    let text = r#"{
      "type": "FeatureCollection",
      "features": [
        {"type": "Feature",
         "geometry": {"type": "Point", "coordinates": [-80.05, 34.2]},
         "properties": {"bus": "312"}},
        {"type": "Feature",
         "geometry": {"type": "LineString", "coordinates": [[-80.05, 34.2], [-80.1, 34.3]]},
         "properties": {"from": "312", "to": "410"}}
      ]
    }"#;
    let parsed = parse(text.as_bytes(), None);
    assert_eq!(parsed.layer.features.len(), 2);
    assert_eq!(parsed.layer.features[0].target, GeoTarget::Bus);
    assert_eq!(parsed.layer.features[1].target, GeoTarget::Branch);
}

#[test]
fn positional_branch_id_is_a_read_only_row_alias() {
    let text = r#"[{"branch": 1, "lat1": 34.2, "lon1": -80.05, "lat2": 34.3, "lon2": -80.1}]"#;
    let parsed = parse(text.as_bytes(), None);
    let feature = &parsed.layer.features[0];
    assert_eq!(feature.key.index, Some(1));

    let mut net = small_network();
    let report = net.apply_geo_layer(&parsed.layer);
    assert_eq!(report.matched_branches, 1);
    assert!(net.branches[0].route.is_some());

    // Never written: the canonical form carries the payload uid instead.
    let round = parse(net.geo_layer().to_geojson().as_bytes(), None);
    let branch = round
        .layer
        .features
        .iter()
        .find(|f| f.target == GeoTarget::Branch)
        .expect("branch feature");
    assert_eq!(branch.key.index, None);
    assert_eq!(branch.key.uid.as_deref(), Some("branches:0"));
}

// ---------------------------------------------------------------------------
// Canonical write
// ---------------------------------------------------------------------------

#[test]
fn canonical_write_round_trips_space_kind_and_keys() {
    let mut net = small_network();
    net.buses[0].location = Some(Location {
        x: -80.05,
        y: 34.2,
        kind: Some(CoordsKind::Manual),
    });
    net.buses[1].location = Some(Location {
        x: -80.1,
        y: 34.3,
        kind: None,
    });
    net.branches[0].route = Some(vec![
        Location {
            x: -80.05,
            y: 34.2,
            kind: None,
        },
        Location {
            x: -80.1,
            y: 34.3,
            kind: None,
        },
    ]);
    net.geo = Some(powerio::GeoMeta {
        space: CoordinateSpace::Geographic { crs: None },
        kind: Some(CoordsKind::Synthetic),
    });

    let layer = net.geo_layer();
    assert_eq!(layer.kind, Some(CoordsKind::Synthetic));
    let text = layer.to_geojson();
    let document: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(document["type"], "FeatureCollection");
    assert_eq!(document["powerio_geo"]["space"], "geographic");
    assert_eq!(document["powerio_geo"]["kind"], "synthetic");

    let round = parse(text.as_bytes(), Some("case.geo.json"));
    assert_eq!(round.layer, layer);

    // Applying onto a coordinate-free copy restores every location.
    let mut bare = small_network();
    let report = bare.apply_geo_layer(&round.layer);
    assert_eq!(report.matched_buses, 2);
    assert_eq!(report.matched_branches, 1);
    assert_eq!(report.unmatched_features, 0);
    assert_eq!(
        bare.buses[0].location.unwrap().kind,
        Some(CoordsKind::Manual)
    );
    assert_eq!(bare.geo, net.geo);
}

#[test]
fn provenance_stamping_survives_the_wire() {
    // A consumer exporting a hand layout stamps `kind = manual`.
    let mut layer = small_network().geo_layer();
    layer.features.push(powerio::GeoFeature {
        target: GeoTarget::Bus,
        key: powerio::ElementKey {
            id: Some("1".to_owned()),
            ..Default::default()
        },
        geometry: GeoGeometry::Point([-80.0, 34.0]),
        from: None,
        to: None,
        kind: None,
    });
    layer.kind = Some(CoordsKind::Manual);
    let round = parse(layer.to_geojson().as_bytes(), None);
    assert_eq!(round.layer.kind, Some(CoordsKind::Manual));
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

#[test]
fn apply_matches_by_id_name_and_pair_and_counts_misses() {
    let text = r#"[
      {"bus": "1", "lat": 34.2, "lon": -80.05},
      {"bus_i": "south", "lat": 34.3, "lon": -80.1},
      {"bus": "77", "lat": 34.4, "lon": -80.2},
      {"from_bus": 2, "to_bus": 1, "lat1": 34.2, "lon1": -80.05, "lat2": 34.3, "lon2": -80.1}
    ]"#;
    let parsed = parse(text.as_bytes(), None);
    let mut net = small_network();
    let report = net.apply_geo_layer(&parsed.layer);
    // "1" matches by external id, "south" case insensitively by name, "77"
    // misses; the branch record matches the unordered (from, to) pair.
    assert_eq!(report.matched_buses, 2);
    assert_eq!(report.matched_branches, 1);
    assert_eq!(report.unmatched_features, 1);
    assert!(net.buses[0].location.is_some());
    assert!(net.buses[1].location.is_some());
    assert!(net.branches[0].route.is_some());
}

#[test]
fn bom_prefixed_json_reads() {
    let mut bytes = b"\xef\xbb\xbf".to_vec();
    bytes.extend_from_slice(br#"[{"bus": "1", "lat": 34.2, "lon": -80.05}]"#);
    let parsed = parse(&bytes, None);
    assert_eq!(parsed.layer.features.len(), 1);
}

#[test]
fn branch_routes_match_source_uids_arriving_as_id_or_name() {
    let mut net = small_network();
    net.branches[0].uid = Some("line-1".to_owned());
    let text =
        r#"[{"branch": "line-1", "lat1": 34.2, "lon1": -80.05, "lat2": 34.3, "lon2": -80.1}]"#;
    let report = net.apply_geo_layer(&parse(text.as_bytes(), None).layer);
    assert_eq!(report.matched_branches, 1);
    assert!(net.branches[0].route.is_some());
}

#[test]
fn apply_matches_source_uids() {
    let mut net = small_network();
    net.buses[0].uid = Some("bus_00".to_owned());
    let text = r#"[{"uid": "bus_00", "id": "999", "lat": 34.2, "lon": -80.05}]"#;
    let report = net.apply_geo_layer(&parse(text.as_bytes(), None).layer);
    assert_eq!(report.matched_buses, 1);
    assert!(net.buses[0].location.is_some());
}

// ---------------------------------------------------------------------------
// PowerWorld .pwd promotion
// ---------------------------------------------------------------------------

#[test]
fn pwd_promotes_to_a_diagram_layer_and_joins_on_subnum() {
    let display = parse_display_file("../tests/data/powerworld/ACTIVSg200.pwd", None)
        .expect("parse .pwd display");
    let powerio::DisplayData::PowerWorld(display) = display else {
        panic!("expected PowerWorld display data");
    };
    let layer = geo_layer_from_pwd(&display);
    assert!(matches!(
        layer.space,
        CoordinateSpace::Diagram { canvas: Some(_) }
    ));
    assert!(!layer.features.is_empty());
    assert!(
        layer
            .features
            .iter()
            .all(|f| f.target == GeoTarget::Substation)
    );

    // The aux sibling carries SubNum per bus; the join places every bus whose
    // substation has a symbol.
    let net = powerio::parse_file("../tests/data/powerworld/ACTIVSg200.aux", None)
        .expect("parse aux")
        .network;
    let mut net = net;
    let report = apply_substation_points(&mut net, &layer);
    assert!(report.matched_buses > 0);
    assert!(matches!(
        net.geo.as_ref().expect("geo meta").space,
        CoordinateSpace::Diagram { .. }
    ));
    // The aux reader already placed geographic locations; replacing them with
    // diagram points is reported rather than silent.
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains("replaced") || note.contains("coordinate space changed")),
        "{:?}",
        report.notes
    );
}

#[test]
fn pwd_mercator_inverse_lands_near_the_aux_coordinates() {
    let display = parse_display_file("../tests/data/powerworld/ACTIVSg200.pwd", None)
        .expect("parse .pwd display");
    let powerio::DisplayData::PowerWorld(display) = display else {
        panic!("expected PowerWorld display data");
    };
    // Substation 1 (CREVE COEUR) sits at 40.642116, -89.59956 in the aux
    // export; the auto generated diagram is Mercator scaled by K.
    let substation = display
        .substations
        .iter()
        .find(|s| s.number == 1)
        .expect("substation 1");
    let (lon, lat) = pwd_mercator_to_lonlat(substation.x, substation.y);
    assert!((lon - -89.599_56).abs() < 0.05, "lon {lon}");
    assert!((lat - 40.642_116).abs() < 0.05, "lat {lat}");
}

// ---------------------------------------------------------------------------
// Untrusted input never panics
// ---------------------------------------------------------------------------

#[test]
fn malformed_inputs_error_without_panicking() {
    let cases: &[&[u8]] = &[
        b"",
        b"{",
        b"[1, 2",
        b"\xff\xfe\x00garbage",
        b"not,a,geo\nfile,at,all\n",
        b"bus,x\nb1,1\n",
        br#"{"features": "not an array"}"#,
        br#"{"features": [{"geometry": {"type": "Point", "coordinates": "x"}}]}"#,
        br#"{"features": [{"geometry": {"type": "Polygon", "coordinates": []}}]}"#,
        br#"[{"bus": "1", "lat": "nope", "lon": "-80"}]"#,
        br#"[{"lat": 1.0, "lon": 2.0}]"#,
        br#"{"type": "FeatureCollection", "powerio_geo": 7, "features": []}"#,
        br#"[{"branch": "b", "path": [[0]]}]"#,
    ];
    for bytes in cases {
        let result = GeoLayer::parse_bytes(bytes, None);
        assert!(
            result.is_err(),
            "expected an error for {:?}",
            String::from_utf8_lossy(bytes)
        );
    }
}

#[test]
fn tolerant_reader_skips_bad_records_but_keeps_good_ones() {
    let text = r#"[
      {"bus": "1", "lat": 34.2, "lon": -80.05},
      {"bus": "2", "lat": null, "lon": -80.1},
      {"bus": "", "lat": 34.4, "lon": -80.2}
    ]"#;
    let parsed = parse(text.as_bytes(), None);
    assert_eq!(parsed.layer.features.len(), 1);
}

#[test]
fn oversized_coordinate_values_read_but_stay_unknown_space() {
    let parsed = parse(b"b1, 1e308, -1e308\n", None);
    assert!(matches!(parsed.layer.space, CoordinateSpace::Unknown));
    // Non-finite coordinates are dropped, so an all-inf file errors.
    assert!(GeoLayer::parse_bytes(b"b1, inf, nan\n", None).is_err());
}

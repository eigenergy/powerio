//! GeoLayer: tolerant reads, canonical writes, extract/apply, and the
//! PowerWorld substation promotion from `.pwd` and from `.aux`.
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use powerio_core::Source;
use powerio_tx::format::powerworld::__parse_aux;
use powerio_tx::{
    BalancedNetwork, Bus, BusId, BusType, CoordinateSpace, CoordsKind, GeoGeometry, GeoLayer,
    GeoTarget, Location, apply_substation_points, to_geo_layer_from_aux_substations,
    to_geo_layer_from_pwd, to_lonlat_from_pwd_mercator,
};

fn parse(text: &str, hint: Option<&str>) -> powerio_tx::GeoParsed {
    GeoLayer::parse(text, hint).expect("parse geo layer")
}

fn small_network() -> BalancedNetwork {
    let mut bus1 = Bus::new(BusId(1), BusType::Ref, 230.0);
    bus1.name = Some("North".to_owned());
    let mut bus2 = Bus::new(BusId(2), BusType::Pq, 230.0);
    bus2.name = Some("South".to_owned());
    let branch = powerio_tx::Branch::new(BusId(1), BusId(2), 0.01, 0.1);
    let mut net = BalancedNetwork::in_memory("small", 100.0, vec![bus1, bus2], vec![branch]);
    net.generators_mut()
        .push(powerio_tx::Generator::new(BusId(1)));
    net
}

// ---------------------------------------------------------------------------
// Tolerant reads
// ---------------------------------------------------------------------------

#[test]
fn headerless_buscoords_csv_reads_as_bus_points() {
    let parsed = parse("b1, -89.6, 40.6\nb2, -89.2, 39.8\n", None);
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
    let parsed = parse("b1 -89.6 40.6\nb2 -89.2 39.8\n", None);
    assert_eq!(parsed.layer.features.len(), 2);
}

#[test]
fn projected_buscoords_read_as_unknown_space() {
    let parsed = parse("b1, 653800.0, 3626000.0\n", None);
    assert!(matches!(parsed.layer.space, CoordinateSpace::Unknown));
}

#[test]
fn aliased_csv_header_reads_points_and_branch_segments() {
    let text = "Bus Number,Latitude,Longitude\n312,34.2,-80.05\n410,34.3,-80.10\n";
    let parsed = parse(text, Some("layout.csv"));
    assert_eq!(parsed.layer.features.len(), 2);
    assert_eq!(parsed.layer.features[0].key.id.as_deref(), Some("312"));

    let branch_csv = "from_bus,to_bus,lat1,lon1,lat2,lon2\n312,410,34.2,-80.05,34.3,-80.10\n";
    let parsed = parse(branch_csv, Some("routes.csv"));
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
    let parsed = parse(text, None);
    assert_eq!(parsed.layer.features.len(), 1);
    assert_eq!(parsed.layer.features[0].key.id.as_deref(), Some("312"));

    // Records nested under an object key (the PowerModels-style dict).
    let nested = r#"{"buses": [{"id": "1", "x": -80.0, "y": 34.0}]}"#;
    let parsed = parse(nested, None);
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
    let parsed = parse(text, None);
    assert_eq!(parsed.layer.features.len(), 2);
    assert_eq!(parsed.layer.features[0].target, GeoTarget::Bus);
    assert_eq!(parsed.layer.features[1].target, GeoTarget::Branch);
}

#[test]
fn a_bare_feature_id_does_not_place_a_branch() {
    // GIS exports and RFC 7946 tooling write a feature row counter under
    // `properties.id`; a positional match would route an unrelated branch.
    let text = r#"{
      "type": "FeatureCollection",
      "features": [
        {"type": "Feature",
         "geometry": {"type": "Point", "coordinates": [-80.05, 34.2]},
         "properties": {"bus": "1"}},
        {"type": "Feature",
         "geometry": {"type": "LineString", "coordinates": [[-80.05, 34.2], [-80.1, 34.3]]},
         "properties": {"id": 1}}
      ]
    }"#;
    let parsed = parse(text, None);
    assert!(
        parsed
            .layer
            .features
            .iter()
            .all(|f| f.target != GeoTarget::Branch),
        "{:?}",
        parsed.layer.features
    );
    let mut net = small_network();
    let report = net.apply_geo_layer(&parsed.layer);
    assert_eq!(report.matched_branches, 0);
    assert!(net.branches()[0].route.is_none());
}

#[test]
fn a_capitalized_target_reads_as_a_substation() {
    let text = r#"{
      "type": "FeatureCollection",
      "features": [
        {"type": "Feature",
         "geometry": {"type": "Point", "coordinates": [-89.6, 40.6]},
         "properties": {"target": "Substation", "id": "1"}}
      ]
    }"#;
    let parsed = parse(text, None);
    assert_eq!(parsed.layer.features[0].target, GeoTarget::Substation);

    let mut net = small_network();
    let report = net.apply_geo_layer(&parsed.layer);
    assert_eq!(report.matched_buses, 0);
    assert_eq!(report.unmatched_features, 1);
}

#[test]
fn a_named_feature_id_still_matches_a_branch_uid() {
    // Dropping the counter case must not drop the whole key: a foreign record
    // that writes a source uid under `id` matches it.
    let text = r#"[{"id": "tie-a", "lat1": 34.2, "lon1": -80.05, "lat2": 34.3, "lon2": -80.1}]"#;
    let parsed = parse(text, None);
    let feature = &parsed.layer.features[0];
    assert_eq!(feature.target, GeoTarget::Branch);
    assert_eq!(feature.key.index, None);

    let mut net = small_network();
    net.branches_mut()[0].uid = Some("tie-a".to_owned());
    let report = net.apply_geo_layer(&parsed.layer);
    assert_eq!(report.matched_branches, 1);
    assert!(net.branches()[0].route.is_some());
}

#[test]
fn positional_branch_id_is_a_read_only_row_alias() {
    let text = r#"[{"branch": 1, "lat1": 34.2, "lon1": -80.05, "lat2": 34.3, "lon2": -80.1}]"#;
    let parsed = parse(text, None);
    let feature = &parsed.layer.features[0];
    assert_eq!(feature.key.index, Some(1));

    let mut net = small_network();
    let report = net.apply_geo_layer(&parsed.layer);
    assert_eq!(report.matched_branches, 1);
    assert!(net.branches()[0].route.is_some());

    // Never written: the canonical form carries the branch's stable identity.
    let round = parse(&net.to_geo_layer().to_geojson(), None);
    let branch = round
        .layer
        .features
        .iter()
        .find(|f| f.target == GeoTarget::Branch)
        .expect("branch feature");
    assert_eq!(branch.key.index, None);
    assert_eq!(branch.key.uid.as_deref(), Some("1-2"));
}

// ---------------------------------------------------------------------------
// Canonical write
// ---------------------------------------------------------------------------

#[test]
fn canonical_write_round_trips_space_kind_and_keys() {
    let mut net = small_network();
    net.buses_mut()[0].location = Some(Location {
        x: -80.05,
        y: 34.2,
        kind: Some(CoordsKind::Manual),
    });
    net.buses_mut()[1].location = Some(Location {
        x: -80.1,
        y: 34.3,
        kind: None,
    });
    net.branches_mut()[0].route = Some(vec![
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
    *net.geo_mut() = Some(powerio_tx::GeoMeta {
        space: CoordinateSpace::Geographic { crs: None },
        kind: Some(CoordsKind::Synthetic),
    });

    let layer = net.to_geo_layer();
    assert_eq!(net.to_geo_layer().features, layer.features);
    assert_eq!(layer.kind, Some(CoordsKind::Synthetic));
    let text = layer.to_geojson();
    let document: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(document["type"], "FeatureCollection");
    assert_eq!(document["powerio_geo"]["space"], "geographic");
    assert_eq!(document["powerio_geo"]["kind"], "synthetic");

    let round = parse(&text, Some("case.geo.json"));
    assert_eq!(round.layer, layer);

    // Applying onto a coordinate-free copy restores every location.
    let mut bare = small_network();
    let report = bare.apply_geo_layer(&round.layer);
    assert_eq!(report.matched_buses, 2);
    assert_eq!(report.matched_branches, 1);
    assert_eq!(report.unmatched_features, 0);
    assert_eq!(
        bare.buses()[0].location.unwrap().kind,
        Some(CoordsKind::Manual)
    );
    assert_eq!(bare.geo(), net.geo());
}

#[test]
fn provenance_stamping_survives_the_wire() {
    // A consumer exporting a hand layout stamps `kind = manual`.
    let mut layer = small_network().to_geo_layer();
    layer.features.push(powerio_tx::GeoFeature {
        target: GeoTarget::Bus,
        key: powerio_tx::ElementKey {
            id: Some("1".to_owned()),
            ..Default::default()
        },
        geometry: GeoGeometry::Point([-80.0, 34.0]),
        from: None,
        to: None,
        kind: None,
    });
    layer.kind = Some(CoordsKind::Manual);
    let round = parse(&layer.to_geojson(), None);
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
    let parsed = parse(text, None);
    let mut net = small_network();
    let report = net.apply_geo_layer(&parsed.layer);
    // "1" matches by external id, "south" case insensitively by name, "77"
    // misses; the branch record matches the unordered (from, to) pair.
    assert_eq!(report.matched_buses, 2);
    assert_eq!(report.matched_branches, 1);
    assert_eq!(report.unmatched_features, 1);
    assert!(net.buses()[0].location.is_some());
    assert!(net.buses()[1].location.is_some());
    assert!(net.branches()[0].route.is_some());
}

#[test]
fn bom_prefixed_json_reads() {
    let text = "\u{feff}[{\"bus\": \"1\", \"lat\": 34.2, \"lon\": -80.05}]";
    let parsed = parse(text, None);
    assert_eq!(parsed.layer.features.len(), 1);
}

#[test]
fn branch_routes_match_source_uids_arriving_as_id_or_name() {
    let mut net = small_network();
    net.branches_mut()[0].uid = Some("line-1".to_owned());
    let text =
        r#"[{"branch": "line-1", "lat1": 34.2, "lon1": -80.05, "lat2": 34.3, "lon2": -80.1}]"#;
    let report = net.apply_geo_layer(&parse(text, None).layer);
    assert_eq!(report.matched_branches, 1);
    assert!(net.branches()[0].route.is_some());
}

#[test]
fn apply_matches_source_uids() {
    let mut net = small_network();
    net.buses_mut()[0].uid = Some("bus_00".to_owned());
    let text = r#"[{"uid": "bus_00", "id": "999", "lat": 34.2, "lon": -80.05}]"#;
    let report = net.apply_geo_layer(&parse(text, None).layer);
    assert_eq!(report.matched_buses, 1);
    assert!(net.buses()[0].location.is_some());
}

#[test]
fn a_layer_that_matches_nothing_still_counts_the_unlocated_model() {
    let mut net = small_network();
    let report =
        net.apply_geo_layer(&parse(r#"[{"bus": "77", "lat": 34.4, "lon": -80.2}]"#, None).layer);
    assert_eq!(report.matched_buses, 0);
    assert_eq!(report.unlocated_buses, 2);
    assert_eq!(report.unlocated_branches, 1);
    assert!(report.require_located().is_err());

    let text = r#"[
      {"bus": "1", "lat": 34.2, "lon": -80.05},
      {"bus": "2", "lat": 34.3, "lon": -80.1},
      {"from_bus": 1, "to_bus": 2, "lat1": 34.2, "lon1": -80.05, "lat2": 34.3, "lon2": -80.1}
    ]"#;
    let report = net.apply_geo_layer(&parse(text, None).layer);
    assert_eq!(report.unlocated_buses, 0);
    assert_eq!(report.unlocated_branches, 0);
    report.require_located().expect("everything placed");
}

// ---------------------------------------------------------------------------
// PowerWorld .pwd promotion
// ---------------------------------------------------------------------------

#[test]
fn pwd_promotes_to_a_diagram_layer_and_joins_on_subnum() {
    let source = Source::open("../tests/data/powerworld/ACTIVSg200.pwd").unwrap();
    let buffer = source.primary_buffer().unwrap();
    let display = powerio_tx::format::powerworld::__parse_pwd_display(buffer.content_bytes())
        .expect("read .pwd display");
    let layer = to_geo_layer_from_pwd(&display);
    assert_eq!(layer, to_geo_layer_from_pwd(&display));
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
    let net = parse_file("../tests/data/powerworld/ACTIVSg200.aux", None)
        .expect("parse aux")
        .network;
    let mut net = net;
    let report = apply_substation_points(&mut net, &layer);
    assert!(report.matched_buses > 0);
    assert!(matches!(
        net.geo().as_ref().expect("geo meta").space,
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

// ---------------------------------------------------------------------------
// PowerWorld aux Substation promotion
// ---------------------------------------------------------------------------

#[test]
fn aux_substations_lift_into_a_geographic_layer_that_joins_on_subnum() {
    let text =
        std::fs::read_to_string("../tests/data/powerworld/ACTIVSg200.aux").expect("read aux");
    let layer = to_geo_layer_from_aux_substations(&__parse_aux(&text).expect("parse aux"));
    assert!(matches!(
        layer.space,
        CoordinateSpace::Geographic { crs: None }
    ));
    assert!(!layer.features.is_empty());
    assert!(
        layer
            .features
            .iter()
            .all(|f| f.target == GeoTarget::Substation)
    );

    // Every bus of the same export carries a SubNum, so the join places all
    // of them at the coordinates the aux reader promoted itself.
    let mut net = parse_file("../tests/data/powerworld/ACTIVSg200.aux", None)
        .expect("parse aux")
        .network;
    let placed: Vec<Option<Location>> = net.buses().iter().map(|bus| bus.location).collect();
    let report = apply_substation_points(&mut net, &layer);
    assert_eq!(report.matched_buses, net.buses().len());
    assert_eq!(report.unmatched_features, 0);
    assert_eq!(report.unlocated_buses, 0);
    let joined: Vec<Option<Location>> = net.buses().iter().map(|bus| bus.location).collect();
    assert_eq!(joined, placed);
}

#[test]
fn aux_substation_rows_skip_unusable_fields_and_keep_file_order() {
    let aux = __parse_aux(
        "DATA (Substation, [SubNum, Latitude, Longitude])\n{\n\
         12.0 34.2 -80.05\n\
         13 nan -80.10\n\
         14 34.4 \"\"\n\
         12 35.0 -81.00\n}\n",
    )
    .expect("parse aux");
    let layer = to_geo_layer_from_aux_substations(&aux);
    // "12.0" and "12" name one substation; the non-finite and the empty row
    // are dropped.
    let keys: Vec<Option<&str>> = layer.features.iter().map(|f| f.key.id.as_deref()).collect();
    assert_eq!(keys, [Some("12"), Some("12")]);
    assert_eq!(
        layer.features[0].geometry,
        GeoGeometry::Point([-80.05, 34.2])
    );
    assert_eq!(
        layer.features[1].geometry,
        GeoGeometry::Point([-81.0, 35.0])
    );

    // A bus carrying the number as a JSON number joins the same way, and the
    // later duplicate wins.
    let mut net = small_network();
    net.buses_mut()[0]
        .extras
        .insert("SubNum".to_owned(), serde_json::json!(12.0));
    let report = apply_substation_points(&mut net, &layer);
    assert_eq!(report.matched_buses, 2);
    let location = net.buses()[0].location.expect("bus 1 location");
    assert_eq!((location.x, location.y), (-81.0, 35.0));
    assert!(net.buses()[1].location.is_none());
}

#[test]
fn a_substation_join_counts_the_buses_it_leaves_unplaced() {
    let aux =
        __parse_aux("DATA (Substation, [SubNum, Latitude, Longitude])\n{\n7 34.2 -80.05\n}\n")
            .expect("parse aux");
    let mut net = small_network();
    net.buses_mut()[0]
        .extras
        .insert("SubNum".to_owned(), serde_json::json!("7"));
    let report = apply_substation_points(&mut net, &to_geo_layer_from_aux_substations(&aux));
    assert_eq!(report.matched_buses, 1);
    // Bus 2 is in no substation, and the join places no route.
    assert_eq!(report.unlocated_buses, 1);
    assert_eq!(report.unlocated_branches, 1);
    assert!(report.require_located().is_err());
}

#[test]
fn pwd_mercator_inverse_lands_near_the_aux_coordinates() {
    let source = Source::open("../tests/data/powerworld/ACTIVSg200.pwd").unwrap();
    let buffer = source.primary_buffer().unwrap();
    let display = powerio_tx::format::powerworld::__parse_pwd_display(buffer.content_bytes())
        .expect("read .pwd display");
    // Substation 1 (CREVE COEUR) sits at 40.642116, -89.59956 in the aux
    // export; the auto generated diagram is Mercator scaled by K.
    let substation = display
        .substations
        .iter()
        .find(|s| s.number == 1)
        .expect("substation 1");
    let (lon, lat) = to_lonlat_from_pwd_mercator(substation.x, substation.y);
    assert!((lon - -89.599_56).abs() < 0.05, "lon {lon}");
    assert!((lat - 40.642_116).abs() < 0.05, "lat {lat}");
}

// ---------------------------------------------------------------------------
// Untrusted input never panics
// ---------------------------------------------------------------------------

#[test]
fn malformed_inputs_error_without_panicking() {
    let cases = [
        "",
        "{",
        "[1, 2",
        "not,a,geo\nfile,at,all\n",
        "bus,x\nb1,1\n",
        r#"{"features": "not an array"}"#,
        r#"{"features": [{"geometry": {"type": "Point", "coordinates": "x"}}]}"#,
        r#"{"features": [{"geometry": {"type": "Polygon", "coordinates": []}}]}"#,
        r#"[{"bus": "1", "lat": "nope", "lon": "-80"}]"#,
        r#"[{"lat": 1.0, "lon": 2.0}]"#,
        r#"{"type": "FeatureCollection", "powerio_geo": 7, "features": []}"#,
        r#"[{"branch": "b", "path": [[0]]}]"#,
    ];
    for text in cases {
        let result = GeoLayer::parse(text, None);
        assert!(result.is_err(), "expected an error for {text:?}");
    }
}

#[test]
fn tolerant_reader_skips_bad_records_but_keeps_good_ones() {
    let text = r#"[
      {"bus": "1", "lat": 34.2, "lon": -80.05},
      {"bus": "2", "lat": null, "lon": -80.1},
      {"bus": "", "lat": 34.4, "lon": -80.2}
    ]"#;
    let parsed = parse(text, None);
    assert_eq!(parsed.layer.features.len(), 1);
}

#[test]
fn oversized_coordinate_values_read_but_stay_unknown_space() {
    let parsed = parse("b1, 1e308, -1e308\n", None);
    assert!(matches!(parsed.layer.space, CoordinateSpace::Unknown));
    // Non-finite coordinates are dropped, so an all-inf file errors.
    assert!(GeoLayer::parse("b1, inf, nan\n", None).is_err());
}

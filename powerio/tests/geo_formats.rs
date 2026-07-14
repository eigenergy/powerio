//! Coordinate harvest and emit in the balanced formats (#183): PowerWorld aux
//! `Latitude:1`/`Longitude:1`, pandapower bus `geo` Point strings, PyPSA
//! `buses.csv` x/y, and the dropped-location warning for formats with no
//! geometry concept.

use std::path::PathBuf;

use powerio::{
    CoordinateSpace, Location, TargetFormat, parse_file, parse_pandapower_json, parse_str,
    read_pypsa_csv_folder, write_pandapower_json, write_powerworld, write_pypsa_csv_folder,
};

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

fn tmp_dir(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("powerio-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

#[test]
fn aux_promotes_substation_coordinates_into_locations() {
    let net = parse_file(data("powerworld/ACTIVSg200.aux"), None)
        .unwrap()
        .network;
    assert!(matches!(
        net.geo.as_ref().expect("geo meta").space,
        CoordinateSpace::Geographic { crs: None }
    ));
    // Bus 1 sits in substation 1 (CREVE COEUR, 40.642116 / -89.59956).
    let bus = &net.buses[0];
    let location = bus.location.expect("bus 1 location");
    assert!((location.y - 40.642_116).abs() < 1e-6, "{}", location.y);
    assert!((location.x - -89.599_56).abs() < 1e-6, "{}", location.x);
    // Promotion removes the raw keys; substation identity stays.
    assert!(!bus.extras.contains_key("Latitude:1"));
    assert!(!bus.extras.contains_key("Longitude:1"));
    assert!(bus.extras.contains_key("SubNum"));
}

#[test]
fn aux_writes_locations_back_and_round_trips() {
    let net = parse_file(data("powerworld/ACTIVSg200.aux"), None)
        .unwrap()
        .network;
    let conv = write_powerworld(&net);
    assert!(conv.text.contains("Latitude:1"));
    let back = parse_str(&conv.text, "powerworld").unwrap().network;
    assert_eq!(back.buses[0].location, net.buses[0].location);
}

#[test]
fn aux_without_locations_writes_the_old_header() {
    let mut net = parse_file(data("powerworld/ACTIVSg200.aux"), None)
        .unwrap()
        .network;
    for bus in &mut net.buses {
        bus.location = None;
    }
    assert!(!write_powerworld(&net).text.contains("Latitude:1"));
}

#[test]
fn pandapower_geo_points_round_trip() {
    // The vendored pandapower 3.2.2 export carries a null geo column.
    let mut net = parse_file(data("pandapower/example.json"), None)
        .unwrap()
        .network;
    assert!(net.buses.iter().all(|b| b.location.is_none()));
    assert!(net.geo.is_none());

    net.buses[0].location = Some(Location {
        x: 7.09,
        y: 50.73,
        kind: None,
    });
    // Same-format writes echo the retained source byte for byte; drop it to
    // exercise the canonical writer.
    net.source = None;
    let out = write_pandapower_json(&net);
    let back = parse_pandapower_json(&out.text).unwrap().network;
    let location = back.buses[0].location.expect("harvested location");
    assert!((location.x - 7.09).abs() < 1e-12);
    assert!((location.y - 50.73).abs() < 1e-12);
    assert!(back.buses[1].location.is_none());
    assert!(matches!(
        back.geo.as_ref().expect("geo meta").space,
        CoordinateSpace::Geographic { crs: None }
    ));
}

#[test]
fn out_of_bounds_coordinates_read_as_unknown_space() {
    let mut net = parse_file(data("pandapower/example.json"), None)
        .unwrap()
        .network;
    // Projected meters in a geo column violate the format convention; the
    // reader keeps the points and declines the geographic claim.
    net.buses[0].location = Some(Location {
        x: 350_000.0,
        y: 5_800_000.0,
        kind: None,
    });
    net.source = None;
    let out = write_pandapower_json(&net);
    let back = parse_pandapower_json(&out.text).unwrap().network;
    assert!(back.buses[0].location.is_some());
    assert!(matches!(
        back.geo.as_ref().expect("geo meta").space,
        CoordinateSpace::Unknown
    ));
}

#[test]
fn pypsa_bus_xy_round_trips() {
    let mut net = read_pypsa_csv_folder(data("pypsa/example"))
        .unwrap()
        .network;
    assert!(net.buses.iter().all(|b| b.location.is_none()));

    net.buses[0].location = Some(Location {
        x: 10.4,
        y: 63.4,
        kind: None,
    });
    let out = tmp_dir("geo-pypsa-csv");
    write_pypsa_csv_folder(&net, &out).unwrap();
    let back = read_pypsa_csv_folder(&out).unwrap().network;
    let location = back.buses[0].location.expect("harvested location");
    assert!((location.x - 10.4).abs() < 1e-12);
    assert!((location.y - 63.4).abs() < 1e-12);
    assert!(back.buses[1].location.is_none());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn formats_without_geometry_report_dropped_locations() {
    let net = parse_file(data("powerworld/ACTIVSg200.aux"), None)
        .unwrap()
        .network;
    assert!(net.buses[0].location.is_some());
    for format in [
        TargetFormat::Matpower,
        TargetFormat::Psse { rev: 33 },
        TargetFormat::PowerModelsJson,
        TargetFormat::EgretJson,
        TargetFormat::Pslf,
        TargetFormat::SurgeJson,
    ] {
        let conv = net.to_format(format).unwrap();
        assert!(
            conv.warnings
                .iter()
                .any(|w| w.contains("location") && w.contains("dropped")),
            "{format:?} did not warn: {:?}",
            conv.warnings
        );
    }
    // Formats with a coordinate representation stay silent.
    for format in [TargetFormat::PowerWorld, TargetFormat::PandapowerJson] {
        let conv = net.to_format(format).unwrap();
        assert!(
            !conv.warnings.iter().any(|w| w.contains("dropped: ")),
            "{format:?} warned: {:?}",
            conv.warnings
        );
    }
}

#[test]
fn locations_and_routes_survive_the_model_snapshot() {
    let mut net = parse_file(data("powerworld/ACTIVSg200.aux"), None)
        .unwrap()
        .network;
    net.branches[0].route = Some(vec![
        Location {
            x: -89.6,
            y: 40.6,
            kind: None,
        },
        Location {
            x: -89.2,
            y: 39.9,
            kind: None,
        },
    ]);
    let back = powerio::Network::from_json(&net.to_json().unwrap()).unwrap();
    assert_eq!(back.buses[0].location, net.buses[0].location);
    assert_eq!(back.branches[0].route, net.branches[0].route);
    assert_eq!(back.geo, net.geo);
}

//! GeoLayer glue for the multiconductor model: extract and apply through
//! `powerio-pkg`, where both model crates are visible.
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use powerio::dist_geo::{apply_dist_geo_layer, to_dist_geo_layer};
use powerio::{CoordinateSpace, CoordsKind, GeoLayer, GeoTarget};

const MASTER: &str = "New Circuit.c1 bus1=sourcebus basekv=12.47\n\
     New Line.l1 bus1=sourcebus bus2=loadbus length=1 units=km\n";

fn dist_network() -> powerio_dist::MulticonductorNetwork {
    helpers::load_multiconductor_memory(MASTER, "dss")
}

#[test]
fn dist_layer_extracts_and_applies_by_name() {
    let mut net = dist_network();
    let bus_row = net
        .buses()
        .iter()
        .position(|bus| bus.id == "sourcebus")
        .expect("sourcebus");
    net.buses_mut()[bus_row].location = Some(powerio_dist::DistLocation {
        x: -89.6,
        y: 40.6,
        kind: Some(powerio_dist::DistCoordsKind::Manual),
    });
    net.lines_mut()[0].route = Some(vec![
        powerio_dist::DistLocation {
            x: -89.6,
            y: 40.6,
            kind: None,
        },
        powerio_dist::DistLocation {
            x: -89.2,
            y: 39.9,
            kind: None,
        },
    ]);
    *net.geo_mut() = Some(powerio_dist::DistGeoMeta {
        space: powerio_dist::CoordinateSpace::Geographic { crs: None },
        kind: None,
    });

    let layer = to_dist_geo_layer(&net);
    let point = layer
        .features
        .iter()
        .find(|f| f.target == GeoTarget::Bus)
        .expect("bus feature");
    assert_eq!(point.key.id.as_deref(), Some("sourcebus"));
    assert_eq!(point.key.uid.as_deref(), Some(&*format!("buses:{bus_row}")));
    assert_eq!(point.kind, Some(CoordsKind::Manual));
    let route = layer
        .features
        .iter()
        .find(|f| f.target == GeoTarget::Branch)
        .expect("line feature");
    assert_eq!(route.key.name.as_deref(), Some("l1"));
    assert_eq!(route.from.as_deref(), Some("sourcebus"));

    // The canonical wire form round trips into a fresh parse of the same
    // master, matching case insensitively on the OpenDSS names.
    let round = GeoLayer::parse(&layer.to_geojson(), None)
        .expect("reparse")
        .layer;
    let mut bare = dist_network();
    let report = apply_dist_geo_layer(&mut bare, &round);
    assert_eq!(report.matched_buses, 1);
    assert_eq!(report.matched_branches, 1);
    assert_eq!(report.unmatched_features, 0);
    let applied = bare.buses()[bus_row].location.expect("applied location");
    assert!((applied.x - -89.6).abs() < 1e-12);
    assert_eq!(applied.kind, Some(powerio_dist::DistCoordsKind::Manual));
    assert!(bare.lines()[0].route.is_some());
    assert!(matches!(
        bare.geo().as_ref().expect("geo meta").space,
        powerio_dist::CoordinateSpace::Geographic { .. }
    ));
}

#[test]
fn dist_apply_reads_a_buscoords_sidecar() {
    let mut net = dist_network();
    let parsed = GeoLayer::parse("SourceBus, -89.6, 40.6\nLoadBus, -89.2, 39.9\n", None)
        .expect("parse buscoords");
    assert!(matches!(
        parsed.layer.space,
        CoordinateSpace::Geographic { .. }
    ));
    let report = apply_dist_geo_layer(&mut net, &parsed.layer);
    assert_eq!(report.matched_buses, 2);
    assert!(
        net.buses()
            .iter()
            .find(|bus| bus.id == "loadbus")
            .expect("loadbus")
            .location
            .is_some()
    );
}

//! Attach drawing records by native identity, preserving auxiliary topology.
use crate::{
    MulticonductorNetwork,
    geo::{CoordinateSpace, DistCoordsKind, DistGeoMeta, DistLocation},
};
use powerio_sincal::DrawingGeometry;

pub(super) fn apply(
    network: &mut MulticonductorNetwork,
    geometry: &DrawingGeometry,
) -> (usize, usize) {
    let mut points = 0;
    let mut routes = 0;
    for bus in network.buses_mut() {
        let Some(id) = bus.id.parse::<i64>().ok() else {
            continue;
        };
        if let Some(native) = geometry.points.get(&id) {
            let p = native.point();
            bus.location = Some(DistLocation {
                x: p[0],
                y: p[1],
                kind: Some(if native.derived() {
                    DistCoordsKind::Derived
                } else {
                    DistCoordsKind::Source
                }),
            });
            bus.extras
                .insert("sincal_geometry".into(), native.provenance());
            points += 1;
        }
    }
    for line in network.lines_mut() {
        let Some(id) = line
            .extras
            .get("sincal")
            .and_then(|v| v.get("element"))
            .and_then(serde_json::Value::as_i64)
        else {
            continue;
        };
        if let Some(route) = geometry.routes.get(&id) {
            line.route = Some(
                route
                    .iter()
                    .map(|p| DistLocation {
                        x: p[0],
                        y: p[1],
                        kind: Some(DistCoordsKind::Source),
                    })
                    .collect(),
            );
            routes += 1;
        }
    }
    if points > 0 || routes > 0 {
        *network.geo_mut() = Some(DistGeoMeta {
            space: if geometry.schematic {
                CoordinateSpace::Diagram { canvas: None }
            } else {
                CoordinateSpace::Unknown
            },
            kind: Some(DistCoordsKind::Source),
        });
    }
    (points, routes)
}

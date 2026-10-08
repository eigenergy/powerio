//! Attach optional drawing geometry without changing the electrical model.
use crate::{
    BalancedNetwork,
    geo::{CoordinateSpace, CoordsKind, GeoMeta, Location},
};
use powerio_sincal::DrawingGeometry;

pub(super) fn apply(network: &mut BalancedNetwork, geometry: &DrawingGeometry) -> (usize, usize) {
    let mut points = 0;
    let mut routes = 0;
    for bus in network.buses_mut() {
        let Some(id) = bus
            .uid
            .as_deref()
            .and_then(|s| s.strip_prefix("sincal:node:"))
            .and_then(|s| s.parse::<i64>().ok())
        else {
            continue;
        };
        if let Some(native) = geometry.points.get(&id) {
            let p = native.point();
            bus.location = Some(Location {
                x: p[0],
                y: p[1],
                kind: Some(if native.derived() {
                    CoordsKind::Derived
                } else {
                    CoordsKind::Source
                }),
            });
            bus.extras
                .insert("sincal_geometry".into(), native.provenance());
            points += 1;
        }
    }
    for branch in network.branches_mut() {
        let Some(id) = branch
            .uid
            .as_deref()
            .and_then(|s| s.strip_prefix("sincal:element:"))
            .and_then(|s| s.parse::<i64>().ok())
        else {
            continue;
        };
        if let Some(route) = geometry.routes.get(&id) {
            branch.route = Some(
                route
                    .iter()
                    .map(|p| Location {
                        x: p[0],
                        y: p[1],
                        kind: Some(CoordsKind::Source),
                    })
                    .collect(),
            );
            routes += 1;
        }
    }
    if points > 0 || routes > 0 {
        *network.geo_mut() = Some(GeoMeta {
            space: CoordinateSpace::Unknown,
            kind: Some(CoordsKind::Source),
        });
    }
    (points, routes)
}

pub(super) fn attach(
    network: &mut BalancedNetwork,
    geometry: &DrawingGeometry,
    diagnostics: &mut Vec<powerio_core::Diagnostic>,
) -> Result<(), powerio_core::Error> {
    let (points, routes) = apply(network, geometry);
    if !geometry.points.is_empty() || !geometry.routes.is_empty() || !geometry.findings.is_empty() {
        let mut report = geometry.diagnostic(&crate::diagnostics::codes::READ_SINCAL_GEOMETRY)?;
        report.insert_detail("mapped_points", serde_json::json!(points))?;
        report.insert_detail("mapped_routes", serde_json::json!(routes))?;
        geometry.annotate_retention(diagnostics, points, routes)?;
        diagnostics.push(report);
    }
    Ok(())
}

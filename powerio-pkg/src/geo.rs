//! Geographic layer glue for the multiconductor model.
//!
//! `powerio_dist` cannot depend on `powerio`, so the [`GeoLayer`] extract and
//! apply for [`MulticonductorNetwork`] live here, where both model crates are
//! visible. The apply pass itself (`apply_geo_features`) is shared with the
//! balanced network; this module supplies the string keyed row lookups and
//! the mirrored type conversions. The parity test in this crate keeps the
//! mirrored geo types' JSON shapes identical, so the conversions are direct
//! field maps with a serde fallback for variants added on one side first.

use std::collections::HashMap;

use powerio::geo::{GeoApplyTarget, apply_geo_features};
use powerio::{CoordinateSpace, ElementKey, GeoApplyReport, GeoFeature, GeoGeometry, GeoLayer};
use powerio_dist::MulticonductorNetwork;

/// Extract a multiconductor network's coordinates as a standalone
/// [`GeoLayer`]: one point per located bus, one route per routed line, keyed
/// by the string bus and line names plus the payload row uids.
#[must_use]
pub fn dist_geo_layer(net: &MulticonductorNetwork) -> GeoLayer {
    let mut features = Vec::new();
    for (row, bus) in net.buses.iter().enumerate() {
        let Some(location) = bus.location else {
            continue;
        };
        features.push(GeoFeature {
            target: powerio::GeoTarget::Bus,
            key: ElementKey {
                uid: Some(format!("buses:{row}")),
                id: Some(bus.id.clone()),
                name: Some(bus.id.clone()),
                index: None,
            },
            geometry: GeoGeometry::Point([location.x, location.y]),
            from: None,
            to: None,
            kind: location.kind.and_then(kind_to_balanced),
        });
    }
    for (row, line) in net.lines.iter().enumerate() {
        let Some(route) = &line.route else {
            continue;
        };
        features.push(GeoFeature {
            target: powerio::GeoTarget::Branch,
            key: ElementKey {
                uid: Some(format!("lines:{row}")),
                id: Some(line.name.clone()),
                name: Some(line.name.clone()),
                index: None,
            },
            geometry: GeoGeometry::LineString(
                route.iter().map(|point| [point.x, point.y]).collect(),
            ),
            from: Some(line.bus_from.clone()),
            to: Some(line.bus_to.clone()),
            kind: None,
        });
    }
    let meta = mirror::<_, powerio::GeoMeta>(net.geo.as_ref());
    GeoLayer {
        space: meta
            .as_ref()
            .map_or(CoordinateSpace::Unknown, |geo| geo.space.clone()),
        kind: meta.and_then(|geo| geo.kind),
        features,
    }
}

/// Apply a [`GeoLayer`] onto a multiconductor network: matched bus points
/// land in `DistBus.location`, matched line routes in `DistLine.route`, and
/// the layer's space becomes the network's `geo` when anything matched.
/// Matching follows [`ElementKey`], with the string ids matched case
/// insensitively (OpenDSS names are case insensitive).
pub fn apply_dist_geo_layer(net: &mut MulticonductorNetwork, layer: &GeoLayer) -> GeoApplyReport {
    let mut target = DistApply {
        buses: DistBusIndex::new(net),
        lines: DistLineIndex::new(net),
        net,
    };
    let report = apply_geo_features(layer, &mut target);
    if report.matched_buses > 0 || report.matched_branches > 0 {
        net.geo = mirror(Some(&powerio::GeoMeta {
            space: layer.space.clone(),
            kind: layer.kind,
        }));
    }
    report
}

/// The multiconductor network as an apply target.
struct DistApply<'a> {
    net: &'a mut MulticonductorNetwork,
    buses: DistBusIndex,
    lines: DistLineIndex,
}

impl GeoApplyTarget for DistApply<'_> {
    fn bus_row(&self, key: &ElementKey) -> Option<usize> {
        key.uid
            .as_ref()
            .and_then(|uid| self.buses.rows.get(uid))
            .or_else(|| lookup_lower(&self.buses.rows, key.id.as_deref()))
            .or_else(|| lookup_lower(&self.buses.rows, key.name.as_deref()))
            .copied()
    }

    fn branch_row(&self, feature: &GeoFeature) -> Option<usize> {
        feature
            .key
            .uid
            .as_ref()
            .and_then(|uid| self.lines.rows.get(uid).copied())
            .or_else(|| {
                lookup_lower(&self.lines.rows, feature.key.id.as_deref())
                    .or_else(|| lookup_lower(&self.lines.rows, feature.key.name.as_deref()))
                    .copied()
            })
            .or_else(|| {
                // Positional row alias, 1-based.
                feature
                    .key
                    .index
                    .and_then(|index| index.checked_sub(1))
                    .filter(|row| *row < self.net.lines.len())
            })
            .or_else(|| {
                let from = feature.from.as_deref()?;
                let to = feature.to.as_deref()?;
                self.lines.pairs.get(&name_pair(from, to)).copied()
            })
    }

    fn place_bus(&mut self, row: usize, point: [f64; 2], kind: Option<powerio::CoordsKind>) {
        self.net.buses[row].location = Some(powerio_dist::Location {
            x: point[0],
            y: point[1],
            kind: kind.and_then(kind_to_dist),
        });
    }

    fn place_branch(&mut self, row: usize, path: &[[f64; 2]], kind: Option<powerio::CoordsKind>) {
        let kind = kind.and_then(kind_to_dist);
        self.net.lines[row].route = Some(
            path.iter()
                .map(|[x, y]| powerio_dist::Location { x: *x, y: *y, kind })
                .collect(),
        );
    }

    fn substation_note(&self, count: usize) -> String {
        format!(
            "{count} substation feature(s) not applied: the multiconductor model has no \
             substation join"
        )
    }
}

/// Bus row lookups: payload row uid plus the lowercased string id.
struct DistBusIndex {
    rows: HashMap<String, usize>,
}

impl DistBusIndex {
    fn new(net: &MulticonductorNetwork) -> Self {
        let mut rows = HashMap::new();
        for (row, bus) in net.buses.iter().enumerate() {
            rows.insert(format!("buses:{row}"), row);
            rows.entry(bus.id.to_ascii_lowercase()).or_insert(row);
        }
        Self { rows }
    }
}

/// Line row lookups: payload row uid, lowercased name, and the unordered
/// endpoint pair.
struct DistLineIndex {
    rows: HashMap<String, usize>,
    pairs: HashMap<(String, String), usize>,
}

impl DistLineIndex {
    fn new(net: &MulticonductorNetwork) -> Self {
        let mut rows = HashMap::new();
        let mut pairs = HashMap::new();
        for (row, line) in net.lines.iter().enumerate() {
            rows.insert(format!("lines:{row}"), row);
            rows.entry(line.name.to_ascii_lowercase()).or_insert(row);
            pairs
                .entry(name_pair(&line.bus_from, &line.bus_to))
                .or_insert(row);
        }
        Self { rows, pairs }
    }
}

fn lookup_lower<'a>(rows: &'a HashMap<String, usize>, key: Option<&str>) -> Option<&'a usize> {
    rows.get(&key?.to_ascii_lowercase())
}

fn name_pair(a: &str, b: &str) -> (String, String) {
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    if b < a { (b, a) } else { (a, b) }
}

/// Direct variant maps between the mirrored provenance enums; both are
/// `#[non_exhaustive]`, so a variant added on one side first goes through the
/// shared JSON shape instead of being dropped silently.
fn kind_to_balanced(kind: powerio_dist::CoordsKind) -> Option<powerio::CoordsKind> {
    match kind {
        powerio_dist::CoordsKind::Source => Some(powerio::CoordsKind::Source),
        powerio_dist::CoordsKind::Synthetic => Some(powerio::CoordsKind::Synthetic),
        powerio_dist::CoordsKind::Manual => Some(powerio::CoordsKind::Manual),
        powerio_dist::CoordsKind::Derived => Some(powerio::CoordsKind::Derived),
        _ => mirror(Some(&kind)),
    }
}

fn kind_to_dist(kind: powerio::CoordsKind) -> Option<powerio_dist::CoordsKind> {
    match kind {
        powerio::CoordsKind::Source => Some(powerio_dist::CoordsKind::Source),
        powerio::CoordsKind::Synthetic => Some(powerio_dist::CoordsKind::Synthetic),
        powerio::CoordsKind::Manual => Some(powerio_dist::CoordsKind::Manual),
        powerio::CoordsKind::Derived => Some(powerio_dist::CoordsKind::Derived),
        _ => mirror(Some(&kind)),
    }
}

/// Convert between the mirrored geo types through their shared JSON shape.
/// Reserved for the once-per-call `GeoMeta` conversion and the enum fallback
/// arms; the per element paths use the direct maps above.
fn mirror<S: serde::Serialize, T: serde::de::DeserializeOwned>(value: Option<&S>) -> Option<T> {
    serde_json::to_value(value?)
        .ok()
        .and_then(|json| serde_json::from_value(json).ok())
}

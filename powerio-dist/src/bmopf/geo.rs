//! BMOPF element geometry: the typed `bus.*.geo` Point, `line.*.geo`
//! LineString and `meta.crs` slots of schema 0.2.0, which PowerIO writes, and
//! the GeoJSON FeatureCollection under `extras.geojson` that PowerIO 0.11
//! wrote, which the reader still accepts.
use crate::geo::{CoordinateSpace, DistCoordsKind, DistGeoMeta, DistLocation};
use crate::model::MulticonductorNetwork;
use serde_json::{Map, Value};

/// Coordinate reference system names that mean WGS84 longitude/latitude, the
/// system an absent `meta.crs` means.
fn is_wgs84(crs: &str) -> bool {
    matches!(
        crs.to_ascii_uppercase().as_str(),
        "EPSG:4326" | "OGC:CRS84" | "WGS84" | "URN:OGC:DEF:CRS:OGC:1.3:CRS84"
    )
}

fn point(value: &Value) -> Option<DistLocation> {
    let p = value.as_array()?;
    if p.len() != 2 && p.len() != 3 {
        return None;
    }
    let (x, y) = (p[0].as_f64()?, p[1].as_f64()?);
    (x.is_finite() && y.is_finite()).then_some(DistLocation {
        x,
        y,
        kind: Some(DistCoordsKind::Source),
    })
}

/// The network's coordinate metadata, from the first of: PowerIO's own
/// `extras.powerio_geo` record, the `powerio_geo` member of a legacy
/// `extras.geojson` collection, `meta.crs`, or the legacy collection's `crs`
/// member. Absent everywhere means WGS84, as the schema states.
fn read_meta(meta: &Value, extras: &Value, collection: &Value) -> Result<DistGeoMeta, String> {
    if let Some(declared) = extras
        .get("powerio_geo")
        .or_else(|| collection.get("powerio_geo"))
    {
        return serde_json::from_value(declared.clone())
            .map_err(|error| format!("invalid powerio_geo metadata: {error}"));
    }
    let crs = meta.get("crs").or_else(|| collection.get("crs"));
    let crs = match crs {
        None => "EPSG:4326",
        Some(value) => value
            .as_str()
            .or_else(|| value.pointer("/properties/name").and_then(Value::as_str))
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| "invalid geometry coordinate reference system".to_owned())?,
    };
    let fallback_space = if is_wgs84(crs) {
        CoordinateSpace::Geographic {
            crs: Some(crs.into()),
        }
    } else {
        CoordinateSpace::Projected {
            crs: Some(crs.into()),
        }
    };
    Ok(DistGeoMeta {
        space: fallback_space,
        kind: Some(DistCoordsKind::Source),
    })
}

/// Keeps an unusable `extras.geojson` layer verbatim on the network, so a
/// write re-emits what the reader could not type.
fn retain_collection(net: &mut MulticonductorNetwork, collection: &Value) {
    if collection.is_null() {
        return;
    }
    let stash = net
        .extras_mut()
        .entry("bmopf_extras".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(stash) = stash {
        stash.insert("geojson".into(), collection.clone());
    }
}

/// Whether any bus or line of the document carries a typed `geo` field.
fn has_embedded_geometry(doc: &Map<String, Value>) -> bool {
    ["bus", "line"].iter().any(|kind| {
        doc.get(*kind)
            .and_then(Value::as_object)
            .is_some_and(|items| items.values().any(|element| element.get("geo").is_some()))
    })
}

pub(super) fn read(
    doc: &Map<String, Value>,
    net: &mut MulticonductorNetwork,
    diagnostics: &mut crate::collect::Diagnostics,
) {
    let extras = doc.get("extras").unwrap_or(&Value::Null);
    let collection = extras.get("geojson").unwrap_or(&Value::Null);
    let geo_meta = match read_meta(doc.get("meta").unwrap_or(&Value::Null), extras, collection) {
        Ok(meta) => meta,
        Err(reason) => {
            diagnostics.push(
                &crate::diagnostics::codes::READ_BMOPF_RETAINED_SOURCE_ONLY,
                format!("{reason}; geometry retained without assigning coordinates"),
            );
            retain_collection(net, collection);
            return;
        }
    };
    let collection_valid =
        collection["type"] == "FeatureCollection" && collection["features"].is_array();
    if !collection.is_null() && !collection_valid {
        diagnostics.push(
            &crate::diagnostics::codes::READ_BMOPF_RETAINED_SOURCE_ONLY,
            "invalid GeoJSON feature collection; retained without assigning its coordinates",
        );
        retain_collection(net, collection);
    }
    // The typed `geo` fields are the schema 0.2.0 form and win; the
    // `extras.geojson` layer is read only when no element carries one.
    let embedded = has_embedded_geometry(doc);
    let geographic = matches!(geo_meta.space, CoordinateSpace::Geographic { .. });
    let buses: std::collections::HashMap<_, _> = net
        .buses()
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id.clone(), i))
        .collect();
    let lines: std::collections::HashMap<_, _> = net
        .lines()
        .iter()
        .enumerate()
        .map(|(i, l)| (l.name.clone(), i))
        .collect();
    let mut changed = false;
    let mut apply = |kind: &str, id: &str, geo: &Value| {
        let mut valid = false;
        match (kind, geo["type"].as_str()) {
            ("bus", Some("Point")) => {
                if let Some(location) = point(&geo["coordinates"])
                    && (!geographic
                        || ((-180.0..=180.0).contains(&location.x)
                            && (-90.0..=90.0).contains(&location.y)))
                    && let Some(&row) = buses.get(id)
                {
                    net.buses_mut()[row].location = Some(location);
                    valid = true;
                }
            }
            ("line", Some("LineString")) => {
                if let Some(points) = geo["coordinates"]
                    .as_array()
                    .and_then(|items| items.iter().map(point).collect::<Option<Vec<_>>>())
                    && points.len() >= 2
                    && (!geographic
                        || points.iter().all(|p| {
                            (-180.0..=180.0).contains(&p.x) && (-90.0..=90.0).contains(&p.y)
                        }))
                    && let Some(&row) = lines.get(id)
                {
                    net.lines_mut()[row].route = Some(points);
                    valid = true;
                }
            }
            _ => {}
        }
        if valid {
            changed = true;
        } else {
            diagnostics.push(&crate::diagnostics::codes::READ_BMOPF_RETAINED_SOURCE_ONLY,format!("{kind} {id}: geometry is invalid or names no matching element; not assigned as coordinates"));
        }
    };
    if embedded {
        for kind in ["bus", "line"] {
            if let Some(items) = doc.get(kind).and_then(Value::as_object) {
                for (id, element) in items {
                    if let Some(geo) = element.get("geo") {
                        apply(kind, id, geo);
                    }
                }
            }
        }
    } else if collection_valid {
        for f in collection["features"].as_array().into_iter().flatten() {
            if let (Some(kind), Some(id)) = (
                f["properties"]["kind"].as_str(),
                f["properties"]["id"].as_str(),
            ) {
                apply(kind, id, &f["geometry"]);
            } else {
                apply("feature", "(missing kind or id)", &f["geometry"]);
            }
        }
    }
    if changed {
        *net.geo_mut() = Some(geo_meta);
    }
}

/// A GeoJSON Point geometry object, built as a `Value` directly (the BMOPF
/// writer assembles the whole document as `serde_json::Value` maps, so there
/// is no separate serializer to stream into).
fn point_value(p: &DistLocation) -> Value {
    let mut object = Map::with_capacity(2);
    object.insert("type".into(), Value::String("Point".into()));
    object.insert(
        "coordinates".into(),
        Value::Array(vec![Value::from(p.x), Value::from(p.y)]),
    );
    Value::Object(object)
}

/// A GeoJSON LineString geometry object over the route's vertices.
fn line_string_value(route: &[DistLocation]) -> Value {
    let mut object = Map::with_capacity(2);
    object.insert("type".into(), Value::String("LineString".into()));
    object.insert(
        "coordinates".into(),
        Value::Array(
            route
                .iter()
                .map(|p| Value::Array(vec![Value::from(p.x), Value::from(p.y)]))
                .collect(),
        ),
    );
    Value::Object(object)
}

/// The bus's `geo` Point, when it has a finite location.
pub(super) fn bus_geo(bus: &crate::model::DistBus) -> Option<Value> {
    bus.location
        .filter(|p| p.x.is_finite() && p.y.is_finite())
        .map(|p| point_value(&p))
}

/// The line's `geo` LineString, when it has a route of two or more finite vertices.
pub(super) fn line_geo(line: &crate::model::DistLine) -> Option<Value> {
    line.route
        .as_ref()
        .filter(|r| r.len() >= 2 && r.iter().all(|p| p.x.is_finite() && p.y.is_finite()))
        .map(|r| line_string_value(r))
}

/// The value `meta.crs` takes for this coordinate space: the authority code
/// of a projected or non-WGS84 geographic system, and nothing for WGS84 (the
/// schema's default for an absent `crs`) or for a space that names no system.
pub(super) fn declared_crs(meta: &DistGeoMeta) -> Option<&str> {
    match &meta.space {
        CoordinateSpace::Geographic { crs } | CoordinateSpace::Projected { crs } => {
            crs.as_deref().filter(|crs| !is_wgs84(crs))
        }
        _ => None,
    }
}

/// Whether reading `meta.crs` back would not reproduce this metadata, so the
/// writer must also record it as `extras.powerio_geo`: a space the schema
/// cannot name (unknown, diagram, a projected system without a code, or a
/// geographic system other than WGS84, which a reader would take for a
/// projected one), or coordinates whose origin is not the source data.
pub(super) fn needs_declared_meta(meta: &DistGeoMeta) -> bool {
    let space_recoverable = match &meta.space {
        CoordinateSpace::Geographic { crs } => crs.as_deref().is_none_or(is_wgs84),
        CoordinateSpace::Projected { crs } => crs.is_some(),
        _ => false,
    };
    !space_recoverable || !matches!(meta.kind, None | Some(DistCoordsKind::Source))
}

/// The network's coordinate metadata, or an unknown space with no stated
/// origin when the network states none but carries locations.
pub(super) fn meta_or_default(net: &MulticonductorNetwork) -> DistGeoMeta {
    net.geo().clone().unwrap_or(DistGeoMeta {
        space: CoordinateSpace::Unknown,
        kind: None,
    })
}

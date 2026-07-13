//! PowerWorld `.pwd` display promotion into the geo model.
//!
//! The `.pwd` reader decodes substation symbols in diagram coordinates.
//! [`geo_layer_from_pwd`] lifts them into a diagram space [`GeoLayer`];
//! [`apply_substation_points`] joins those points onto buses through the
//! `SubNum` extras key; [`pwd_mercator_to_lonlat`] is the documented,
//! approximate inverse of the projection PowerWorld's auto generated layouts
//! use, for consumers that want to place a diagram on a map.

use std::collections::HashMap;

use serde_json::Value;

use super::layer::{ElementKey, GeoApplyReport, GeoFeature, GeoGeometry, GeoLayer, GeoTarget};
use super::{Canvas, CoordinateSpace, GeoMeta, Location};
use crate::format::PwdDisplay;
use crate::network::Network;

/// Scale of PowerWorld's auto generated layouts: `x = K·lon` and
/// `y = K·mercdeg(lat)`, with the Mercator ordinate expressed in degrees.
pub const PWD_MERCATOR_K: f64 = 535.816_08;

/// Approximate inverse of the projection PowerWorld's auto generated layouts
/// use (verified against ACTIVSg200/2000 to within ~0.02 degrees): longitude
/// is `x / K`, latitude the inverse Gudermannian of `y / K`. Hand edited
/// diagrams drift from this, so treat the result as approximate.
#[must_use]
pub fn pwd_mercator_to_lonlat(x: f64, y: f64) -> (f64, f64) {
    let lon = x / PWD_MERCATOR_K;
    let lat = ((y / PWD_MERCATOR_K).to_radians().sinh())
        .atan()
        .to_degrees();
    (lon, lat)
}

/// Lift decoded `.pwd` substation symbols into a diagram space [`GeoLayer`]
/// with substation targets keyed by substation number.
#[must_use]
pub fn geo_layer_from_pwd(display: &PwdDisplay) -> GeoLayer {
    GeoLayer {
        space: CoordinateSpace::Diagram {
            canvas: Some(Canvas {
                width: Some(f64::from(display.canvas_width)),
                height: Some(f64::from(display.canvas_height)),
                units: None,
            }),
        },
        kind: None,
        features: display
            .substations
            .iter()
            .map(|substation| GeoFeature {
                target: GeoTarget::Substation,
                key: ElementKey {
                    uid: None,
                    id: Some(substation.number.to_string()),
                    name: (!substation.name.is_empty()).then(|| substation.name.clone()),
                    index: None,
                },
                geometry: GeoGeometry::Point([substation.x, substation.y]),
                from: None,
                to: None,
                kind: None,
            })
            .collect(),
    }
}

/// Join a layer's substation points onto buses through the `SubNum` (or
/// `SubNumber`) extras key: every bus in a matched substation takes the
/// substation's point, and the layer's space becomes the network's
/// [`GeoMeta`] when anything matched. Replaced locations and a coordinate
/// space change are reported in the notes rather than happening silently.
pub fn apply_substation_points(net: &mut Network, layer: &GeoLayer) -> GeoApplyReport {
    let mut report = GeoApplyReport::default();
    // Substation number -> bus rows, built once for the whole pass.
    let mut rows_by_substation: HashMap<String, Vec<usize>> = HashMap::new();
    for (row, bus) in net.buses.iter().enumerate() {
        if let Some(substation) = bus_substation(bus) {
            rows_by_substation.entry(substation).or_default().push(row);
        }
    }
    let mut replaced = 0usize;
    for feature in &layer.features {
        let (GeoTarget::Substation, GeoGeometry::Point(point)) =
            (&feature.target, &feature.geometry)
        else {
            continue;
        };
        let rows = feature
            .key
            .id
            .as_deref()
            .and_then(|number| rows_by_substation.get(number));
        let Some(rows) = rows else {
            report.unmatched_features += 1;
            continue;
        };
        for &row in rows {
            let bus = &mut net.buses[row];
            if bus.location.is_some() {
                replaced += 1;
            }
            bus.location = Some(Location {
                x: point[0],
                y: point[1],
                kind: feature.kind,
            });
            report.matched_buses += 1;
        }
    }
    if report.matched_buses > 0 {
        if replaced > 0 {
            report
                .notes
                .push(format!("replaced {replaced} existing bus location(s)"));
        }
        super::layer::note_space_change(&mut report, net.geo.as_ref(), &layer.space);
        net.geo = Some(GeoMeta {
            space: layer.space.clone(),
            kind: layer.kind,
        });
    }
    report
}

/// The bus's substation number from extras, normalized to a string
/// (PowerWorld exports carry it as a number or a numeric string).
fn bus_substation(bus: &crate::network::Bus) -> Option<String> {
    let value = bus
        .extras
        .get("SubNum")
        .or_else(|| bus.extras.get("SubNumber"))?;
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| {
                // "12.0" and "12" name the same substation.
                trimmed
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.fract() == 0.0 && v.abs() < 1e15)
                    .map_or_else(|| trimmed.to_owned(), |v| format!("{v:.0}"))
            })
        }
        _ => None,
    }
}

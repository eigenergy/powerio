//! PowerWorld substation promotion into the geo model.
//!
//! Two PowerWorld files carry substation coordinates. The `.pwd` display
//! holds symbols in diagram coordinates, which [`geo_layer_from_pwd`] lifts
//! into a diagram space [`GeoLayer`]; [`pwd_mercator_to_lonlat`] is the
//! documented, approximate inverse of the projection PowerWorld's auto
//! generated layouts use, for consumers that want to place a diagram on a
//! map. The `.aux` `Substation` table holds latitude and longitude, which
//! [`geo_layer_from_aux_substations`] lifts into a geographic layer.
//! [`apply_substation_points`] joins either layer onto buses through the
//! `SubNum` extras key.

use std::collections::HashMap;

use serde_json::Value;

use super::layer::{ElementKey, GeoApplyReport, GeoFeature, GeoGeometry, GeoLayer, GeoTarget};
use super::{Canvas, CoordinateSpace, GeoMeta, Location};
use crate::format::PwdDisplay;
use crate::format::powerworld::AuxFile;
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

/// Lift the aux `Substation` table into a geographic [`GeoLayer`] with
/// substation targets keyed by substation number. The number comes from
/// `SubNum` or `Number` and the point from `Latitude` and `Longitude`, the
/// column names PowerWorld writes itself, so they take no aliases. A row
/// whose number or coordinate is absent or is not a finite number is
/// skipped. Rows stay in file order, so a repeated substation number keeps
/// the last point once [`apply_substation_points`] runs.
#[must_use]
pub fn geo_layer_from_aux_substations(aux: &AuxFile) -> GeoLayer {
    let mut features = Vec::new();
    for object in aux.data_of("Substation") {
        let (Some(number), Some(latitude), Some(longitude)) = (
            object
                .field_index("SubNum")
                .or_else(|| object.field_index("Number")),
            object.field_index("Latitude"),
            object.field_index("Longitude"),
        ) else {
            continue;
        };
        for row in &object.rows {
            let field = |column: usize| -> Option<(&str, f64)> {
                let text = row.values.get(column)?.trim();
                let value = text.parse::<f64>().ok().filter(|value| value.is_finite())?;
                Some((text, value))
            };
            let (Some((number, _)), Some((_, lat)), Some((_, lon))) =
                (field(number), field(latitude), field(longitude))
            else {
                continue;
            };
            features.push(GeoFeature {
                target: GeoTarget::Substation,
                key: ElementKey {
                    uid: None,
                    id: Some(substation_key(number)),
                    name: None,
                    index: None,
                },
                geometry: GeoGeometry::Point([lon, lat]),
                from: None,
                to: None,
                kind: None,
            });
        }
    }
    GeoLayer {
        space: CoordinateSpace::Geographic { crs: None },
        kind: None,
        features,
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
        Value::Number(number) => Some(substation_key(&number.to_string())),
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| substation_key(trimmed))
        }
        _ => None,
    }
}

/// The join key for one substation number. Every source of a substation
/// number goes through this: "12.0" and "12" name the same substation, and
/// the two sides of the join must spell it the same way.
fn substation_key(number: &str) -> String {
    number
        .parse::<f64>()
        .ok()
        .filter(|v| v.fract() == 0.0 && v.abs() < 1e15)
        .map_or_else(|| number.to_owned(), |v| format!("{v:.0}"))
}

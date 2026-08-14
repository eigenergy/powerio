//! Typed coordinate metadata for the distribution model.

use serde::{Deserialize, Serialize};

/// A point attached to one model element.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Location {
    /// X coordinate. In geographic space this is longitude.
    pub x: f64,
    /// Y coordinate. In geographic space this is latitude.
    pub y: f64,
    /// Per point provenance when it differs from the network default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<CoordsKind>,
}

/// Coordinate provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CoordsKind {
    Source,
    Synthetic,
    Manual,
    Derived,
}

/// Network level coordinate metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GeoMeta {
    /// Coordinate space shared by every location on the network.
    #[serde(flatten)]
    pub space: CoordinateSpace,
    /// Default provenance for points without their own `kind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<CoordsKind>,
}

/// Coordinate space for locations in a network.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "space", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CoordinateSpace {
    /// x = longitude and y = latitude in decimal degrees. `None` means EPSG:4326.
    Geographic {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crs: Option<String>,
    },
    /// Planar coordinates, with the CRS named when known.
    Projected {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crs: Option<String>,
    },
    /// Drawing coordinates with no earth referent.
    Diagram {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        canvas: Option<Canvas>,
    },
    /// The source did not declare a coordinate space.
    Unknown,
}

/// Diagram canvas metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Canvas {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
}

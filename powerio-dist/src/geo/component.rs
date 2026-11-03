//! Geographic points on individual network components, independent of drawings.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::DistCoordsKind;
use crate::MulticonductorNetwork;

/// The typed table containing a located component. Names retain exact case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DistLocationTarget {
    Bus,
    Line,
    LineCode,
    Switch,
    Transformer,
    Load,
    Generator,
    Ibr,
    Shunt,
    Capacitor,
    Source,
}

/// Source or user supplied geographic coordinates, in decimal degrees.
/// Neither axis is implied by the other; zero is a valid stated value.
/// An absent datum is unknown, never an implicit WGS84 declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DistGeographicPoint {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub datum: Option<String>,
}

impl DistGeographicPoint {
    #[must_use]
    pub const fn new(latitude: Option<f64>, longitude: Option<f64>) -> Self {
        Self {
            latitude,
            longitude,
            datum: None,
        }
    }
}

/// A geographic point attached to one exact component identity.
/// It is independent of bus drawing positions, line routes and electrical length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DistComponentLocation {
    pub target: DistLocationTarget,
    pub name: String,
    pub point: DistGeographicPoint,
    pub kind: Option<DistCoordsKind>,
}

impl DistComponentLocation {
    #[must_use]
    pub fn new(
        target: DistLocationTarget,
        name: impl Into<String>,
        point: DistGeographicPoint,
    ) -> Self {
        Self {
            target,
            name: name.into(),
            point,
            kind: None,
        }
    }
}

/// Maximum component points in one stored network; checked before collection growth.
pub const MAX_COMPONENT_LOCATIONS: usize = 1_048_576;

impl MulticonductorNetwork {
    /// Check point ranges, exact attachment identities and duplicate attachments.
    /// This validates metadata only; it never changes electrical topology.
    pub fn validate_component_locations(&self) -> Result<(), powerio_core::Error> {
        validate(self).map_err(|message| {
            powerio_core::Error::new(
                &crate::diagnostics::codes::VALIDATE_DIST_COORDINATES_INVALID,
                message,
            )
        })
    }
}

fn validate(net: &MulticonductorNetwork) -> Result<(), String> {
    let locations = net.component_locations();
    if locations.is_empty() {
        return Ok(());
    }
    if locations.len() > MAX_COMPONENT_LOCATIONS {
        return Err(format!(
            "component locations exceed {MAX_COMPONENT_LOCATIONS} records"
        ));
    }
    let mut identities = BTreeSet::new();
    macro_rules! names {
        ($kind:ident, $rows:expr, $name:ident) => {
            for row in $rows {
                identities.insert((DistLocationTarget::$kind, row.$name.as_str()));
            }
        };
    }
    names!(Bus, net.buses(), id);
    names!(Line, net.lines(), name);
    names!(LineCode, net.line_codes(), name);
    names!(Switch, net.switches(), name);
    names!(Transformer, net.transformers(), name);
    names!(Load, net.loads(), name);
    names!(Generator, net.generators(), name);
    names!(Ibr, net.ibrs(), name);
    names!(Shunt, net.shunts(), name);
    names!(Capacitor, net.capacitors(), name);
    names!(Source, net.sources(), name);
    let mut seen = BTreeSet::new();
    for location in locations {
        let key = (location.target, location.name.as_str());
        if !identities.contains(&key) {
            return Err(format!(
                "coordinate attachment {key:?} has no exact component identity"
            ));
        }
        if !seen.insert(key) {
            return Err(format!("duplicate coordinate attachment {key:?}"));
        }
        let point = &location.point;
        if point.latitude.is_none() && point.longitude.is_none() {
            return Err(format!("coordinate attachment {key:?} has no stated axis"));
        }
        for (axis, value, bound) in [
            ("latitude", point.latitude, 90.0),
            ("longitude", point.longitude, 180.0),
        ] {
            if value.is_some_and(|v| !v.is_finite() || !(-bound..=bound).contains(&v)) {
                return Err(format!("coordinate attachment {key:?} has invalid {axis}"));
            }
        }
        if point.datum.as_ref().is_some_and(|d| d.trim().is_empty()) {
            return Err(format!("coordinate attachment {key:?} has an empty datum"));
        }
    }
    Ok(())
}

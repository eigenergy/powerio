//! A human-oriented summary of the payload.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Element counts, topology, and unit conventions, for a quick read of a package
/// without deserializing the whole payload.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ObjectSummary {
    /// Element type name -> count, e.g. `{"buses": 118, "branches": 186}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub elements: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<ObjectTopology>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<ObjectUnits>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ObjectTopology {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_components: Option<u64>,
    /// Reference bus ids as strings (balanced ids are integers, multiconductor
    /// ids are strings; strings cover both).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reference_buses: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ObjectUnits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_mva: Option<f64>,
}

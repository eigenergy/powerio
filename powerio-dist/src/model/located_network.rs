//! Additive IR spelling for a network with typed component geography.
//!
//! Legacy bodies remain byte-shape compatible. A distinct mandatory wrapper
//! keeps older readers from accepting a network while silently losing points.
//! Dispatch peeks at one map key, then streams the selected representation;
//! it does not buffer an entire network in a serde untagged union.

use serde::de::{DeserializeSeed, IntoDeserializer, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{MulticonductorNetwork, MulticonductorNetworkTables};
use crate::DistComponentLocation;

const TYPE: &str = "powerio.LocatedMulticonductorNetwork";

#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
enum LocatedNetworkTag {
    #[serde(rename = "powerio.LocatedMulticonductorNetwork")]
    Located,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
struct LocatedNetworkWire {
    #[serde(rename = "type")]
    _tag: LocatedNetworkTag,
    value: LocatedNetworkFields,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
struct LocatedNetworkFields {
    #[serde(with = "MulticonductorNetworkTables")]
    #[cfg_attr(feature = "schema", schemars(with = "NetworkBodySchema"))]
    network: MulticonductorNetworkTables,
    #[serde(deserialize_with = "locations")]
    locations: Vec<DistComponentLocation>,
}

fn locations<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<DistComponentLocation>, D::Error> {
    powerio_core::limits::bounded_vec(
        d,
        "component locations",
        crate::geo::MAX_COMPONENT_LOCATIONS,
    )
}

struct Body<'a>(&'a MulticonductorNetworkTables);
impl Serialize for Body<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        MulticonductorNetworkTables::serialize(self.0, serializer)
    }
}

struct LocatedBody<'a>(&'a MulticonductorNetworkTables);
impl Serialize for LocatedBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut body = serializer.serialize_struct("LocatedNetworkFields", 2)?;
        body.serialize_field("network", &Body(self.0))?;
        body.serialize_field("locations", &self.0.component_locations)?;
        body.end()
    }
}

pub(super) fn serialize<S: Serializer>(
    net: &MulticonductorNetwork,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    net.validate_component_locations()
        .map_err(serde::ser::Error::custom)?;
    let serializer = powerio_core::__implementation::nonfinite::NonFiniteSer(serializer);
    if net.component_locations().is_empty() {
        return Body(&net.tables).serialize(serializer);
    }
    let mut wrapper = serializer.serialize_struct("LocatedNetworkWire", 2)?;
    wrapper.serialize_field("type", TYPE)?;
    wrapper.serialize_field("value", &LocatedBody(&net.tables))?;
    wrapper.end()
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<MulticonductorNetwork, D::Error> {
    struct NetworkVisitor;
    impl<'de> Visitor<'de> for NetworkVisitor {
        type Value = MulticonductorNetwork;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a multiconductor network or a tagged located network")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let first = map.next_key::<String>()?;
            let located = first
                .as_deref()
                .is_some_and(|key| matches!(key, "type" | "value"));
            let d = serde::de::value::MapAccessDeserializer::new(FirstKey {
                first,
                map,
                legacy: !located,
            });
            let tables = if located {
                let wrapper = LocatedNetworkWire::deserialize(d)?;
                let mut tables = wrapper.value.network;
                tables.component_locations = wrapper.value.locations;
                tables
            } else {
                MulticonductorNetworkTables::deserialize(d)?
            };
            let network = MulticonductorNetwork::from_tables(tables);
            network
                .validate_component_locations()
                .map_err(serde::de::Error::custom)?;
            Ok(network)
        }
    }
    powerio_core::__implementation::nonfinite::NonFiniteDe(d).deserialize_map(NetworkVisitor)
}

struct FirstKey<M> {
    first: Option<String>,
    map: M,
    legacy: bool,
}

impl<'de, M: MapAccess<'de>> MapAccess<'de> for FirstKey<M> {
    type Error = M::Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        let key = match self.first.take() {
            Some(first) => Some(first),
            None => self.map.next_key::<String>()?,
        };
        key.map(|key| {
            if self.legacy && matches!(key.as_str(), "type" | "value" | "component_locations") {
                return Err(serde::de::Error::custom(
                    "component geography requires a powerio.LocatedMulticonductorNetwork record",
                ));
            }
            seed.deserialize(key.into_deserializer())
        })
        .transpose()
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        self.map.next_value_seed(seed)
    }
    fn size_hint(&self) -> Option<usize> {
        self.map
            .size_hint()
            .map(|n| n.saturating_add(usize::from(self.first.is_some())))
    }
}

#[cfg(feature = "schema")]
struct NetworkBodySchema;

#[cfg(feature = "schema")]
impl schemars::JsonSchema for NetworkBodySchema {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MulticonductorNetworkBody".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        MulticonductorNetworkTables::json_schema(generator)
    }
}

#[cfg(feature = "schema")]
pub(super) fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    use schemars::JsonSchema;
    schemars::json_schema!({"anyOf": [
        MulticonductorNetworkTables::json_schema(generator),
        generator.subschema_for::<LocatedNetworkWire>(),
    ]})
}

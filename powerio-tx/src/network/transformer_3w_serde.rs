//! The historical transformer body and the fail-closed partial-winding record.
//!
//! Only nondefault winding states need a new representation. Nesting every
//! historical required field makes older readers reject it rather than silently
//! energizing a disconnected winding. This applies inside every IR envelope.

use std::fmt;

use serde::de::{Error as _, IgnoredAny, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(doc)]
use super::Bus;
use super::{Extras, Impedance, Transformer3W, WINDING_IN_SERVICE_EXTRA, Winding};

const PARTIAL_TYPE: &str = "powerio.PartialTransformer3W";

/// A three winding transformer with three terminal buses joined at a star point.
///
/// Series impedance is stored for winding pairs 1-2, 2-3, and 3-1. The record
/// also retains star point voltage and per winding control data. PSS/E three
/// winding records and PSLF tertiary winding records map to this type.
/// [`to_star_expansion`](Transformer3W::to_star_expansion) turns it into the synthetic
/// star bus plus three branches for a consumer that works in the bus-branch model;
/// [`IndexedNetwork`](crate::IndexedNetwork) applies it before building any matrix,
/// so a 3-winding transformer contributes to `Y_bus` and connectivity.
#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
struct Transformer3WFields {
    /// The three windings, in order (primary, secondary, tertiary).
    windings: [Winding; 3],
    /// Pairwise series impedance `[z12, z23, z31]` (primary-secondary,
    /// secondary-tertiary, tertiary-primary), each per unit on the system base
    /// with its declared MVA base.
    z: [Impedance; 3],
    /// Star-point voltage magnitude (p.u.) and angle (degrees), as solved.
    star_vm: f64,
    star_va: f64,
    /// Magnetizing shunt referred to the star point (p.u. on the system base).
    mag_g: f64,
    mag_b: f64,
    in_service: bool,
    name: Option<String>,
    /// Stable row identity; see [`Bus::uid`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uid: Option<String>,
    extras: Extras,
    // Reserved structural fields must not be smuggled into a legacy body.
    #[serde(default, rename = "type", deserialize_with = "reject_reserved")]
    #[cfg_attr(feature = "schema", schemars(skip))]
    _reserved_type: (),
    #[serde(default, rename = "value", deserialize_with = "reject_reserved")]
    #[cfg_attr(feature = "schema", schemars(skip))]
    _reserved_value: (),
}

fn reject_reserved<'de, D: Deserializer<'de>>(_: D) -> Result<(), D::Error> {
    Err(D::Error::custom(
        "a transformer body cannot contain structural type/value fields",
    ))
}

impl From<Transformer3WFields> for Transformer3W {
    fn from(fields: Transformer3WFields) -> Self {
        Self {
            windings: fields.windings,
            z: fields.z,
            star_vm: fields.star_vm,
            star_va: fields.star_va,
            mag_g: fields.mag_g,
            mag_b: fields.mag_b,
            in_service: fields.in_service,
            name: fields.name,
            uid: fields.uid,
            extras: fields.extras,
        }
    }
}

struct TransformerBody<'a>(&'a Transformer3W);

impl Serialize for TransformerBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let t = self.0;
        let mut body =
            serializer.serialize_struct("Transformer3W", 9 + usize::from(t.uid.is_some()))?;
        body.serialize_field("windings", &t.windings)?;
        body.serialize_field("z", &t.z)?;
        body.serialize_field("star_vm", &t.star_vm)?;
        body.serialize_field("star_va", &t.star_va)?;
        body.serialize_field("mag_g", &t.mag_g)?;
        body.serialize_field("mag_b", &t.mag_b)?;
        body.serialize_field("in_service", &t.in_service)?;
        body.serialize_field("name", &t.name)?;
        if let Some(uid) = &t.uid {
            body.serialize_field("uid", uid)?;
        }
        body.serialize_field("extras", &t.extras)?;
        body.end()
    }
}

impl Serialize for Transformer3W {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.winding_in_service() == [true; 3] {
            return TransformerBody(self).serialize(serializer);
        }
        let mut wrapper = serializer.serialize_struct("PartialTransformer3W", 2)?;
        wrapper.serialize_field("type", PARTIAL_TYPE)?;
        wrapper.serialize_field("value", &TransformerBody(self))?;
        wrapper.end()
    }
}

#[derive(Deserialize)]
enum PartialType {
    #[serde(rename = "powerio.PartialTransformer3W")]
    Partial,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum Field {
    Windings,
    Z,
    StarVm,
    StarVa,
    MagG,
    MagB,
    InService,
    Name,
    Uid,
    Extras,
    Type,
    Value,
    #[serde(other)]
    Other,
}

impl<'de> Deserialize<'de> for Transformer3W {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TransformerVisitor;
        impl<'de> Visitor<'de> for TransformerVisitor {
            type Value = Transformer3W;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a legacy transformer or powerio.PartialTransformer3W record")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut windings = None;
                let mut z = None;
                let mut star_vm = None;
                let mut star_va = None;
                let mut mag_g = None;
                let mut mag_b = None;
                let mut in_service = None;
                let mut name = None;
                let mut uid = None;
                let mut extras = None;
                let mut kind = None;
                let mut value: Option<Transformer3WFields> = None;
                let mut has_body = false;
                // Decode directly, without buffering a JSON value or recursively
                // decoding a Transformer3W. Winding/impedance arrays stay fixed
                // at three elements and the wrapper admits exactly one body.
                macro_rules! read_field {
                    ($field:ident) => {{
                        if $field.is_some() {
                            return Err(A::Error::duplicate_field(stringify!($field)));
                        }
                        $field = Some(map.next_value()?);
                        has_body = true;
                    }};
                }
                while let Some(field) = map.next_key()? {
                    match field {
                        Field::Windings => read_field!(windings),
                        Field::Z => read_field!(z),
                        Field::StarVm => read_field!(star_vm),
                        Field::StarVa => read_field!(star_va),
                        Field::MagG => read_field!(mag_g),
                        Field::MagB => read_field!(mag_b),
                        Field::InService => read_field!(in_service),
                        Field::Name => read_field!(name),
                        Field::Uid => read_field!(uid),
                        Field::Extras => read_field!(extras),
                        Field::Type => {
                            if kind.is_some() {
                                return Err(A::Error::duplicate_field("type"));
                            }
                            kind = Some(map.next_value::<PartialType>()?);
                        }
                        Field::Value => {
                            if value.is_some() {
                                return Err(A::Error::duplicate_field("value"));
                            }
                            value = Some(map.next_value()?);
                        }
                        Field::Other => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                if kind.is_some() || value.is_some() {
                    if has_body {
                        return Err(A::Error::custom(
                            "powerio.PartialTransformer3W requires all transformer fields inside value",
                        ));
                    }
                    kind.ok_or_else(|| A::Error::missing_field("type"))?;
                    let transformer =
                        Transformer3W::from(value.ok_or_else(|| A::Error::missing_field("value"))?);
                    let flags = transformer.extras.get(WINDING_IN_SERVICE_EXTRA)
                        .and_then(|value| <[bool; 3]>::deserialize(value).ok())
                        .ok_or_else(|| A::Error::custom("powerio.PartialTransformer3W requires three boolean winding_in_service flags"))?;
                    if flags == [true; 3] {
                        return Err(A::Error::custom(
                            "powerio.PartialTransformer3W requires a nondefault winding state",
                        ));
                    }
                    return Ok(transformer);
                }
                let transformer = Transformer3W {
                    windings: windings.ok_or_else(|| A::Error::missing_field("windings"))?,
                    z: z.ok_or_else(|| A::Error::missing_field("z"))?,
                    star_vm: star_vm.ok_or_else(|| A::Error::missing_field("star_vm"))?,
                    star_va: star_va.ok_or_else(|| A::Error::missing_field("star_va"))?,
                    mag_g: mag_g.ok_or_else(|| A::Error::missing_field("mag_g"))?,
                    mag_b: mag_b.ok_or_else(|| A::Error::missing_field("mag_b"))?,
                    in_service: in_service.ok_or_else(|| A::Error::missing_field("in_service"))?,
                    name: name.unwrap_or(None),
                    uid: uid.unwrap_or(None),
                    extras: extras.ok_or_else(|| A::Error::missing_field("extras"))?,
                };
                if transformer.winding_in_service() != [true; 3] {
                    return Err(A::Error::custom(
                        "nondefault winding states require a powerio.PartialTransformer3W record",
                    ));
                }
                Ok(transformer)
            }
        }
        deserializer.deserialize_map(TransformerVisitor)
    }
}

#[cfg(feature = "schema")]
#[derive(schemars::JsonSchema)]
#[allow(dead_code)] // The payload is only used to generate the schema.
#[serde(tag = "type", content = "value")]
enum PartialTransformer3WWire {
    #[serde(rename = "powerio.PartialTransformer3W")]
    Partial(PartialTransformer3WFields),
}

#[cfg(feature = "schema")]
struct PartialTransformer3WFields;

#[cfg(feature = "schema")]
impl schemars::JsonSchema for PartialTransformer3WFields {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PartialTransformer3WFields".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut body = serde_json::to_value(Transformer3WFields::json_schema(generator))
            .expect("transformer schema is serializable");
        // Only the new alternative requires a well-formed nondefault state;
        // the legacy body and all of its historical schemas stay untouched.
        body["properties"]["extras"]["properties"] = serde_json::json!({
            "winding_in_service": {
                "type": "array", "minItems": 3, "maxItems": 3,
                "items": {"type": "boolean"},
                "not": {"const": [true, true, true]}
            }
        });
        body["properties"]["extras"]["required"] = serde_json::json!(["winding_in_service"]);
        serde_json::from_value(body).expect("an extended object remains a schema")
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for Transformer3W {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Transformer3W".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // Preserve the complete published body as the first alternative.
        schemars::json_schema!({"anyOf": [
            Transformer3WFields::json_schema(generator),
            generator.subschema_for::<PartialTransformer3WWire>(),
        ]})
    }
}

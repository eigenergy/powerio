use std::fmt;

use serde::de::{Error as _, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Extras;

/// An ideal source prescribing terminal voltages relative to earth or one
/// explicit terminal on the same bus. A reference terminal is a conductor,
/// not an implicit grounding instruction.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct VoltageSource {
    pub name: String,
    pub bus: String,
    pub terminal_map: Vec<String>,
    /// Volts per terminal, relative to `reference_terminal` or earth.
    pub v_magnitude: Vec<f64>,
    /// Radians per terminal, with the same reference as `v_magnitude`.
    pub v_angle: Vec<f64>,
    /// None preserves the historical earth-referenced source semantics.
    /// Some names a declared terminal on `bus`; its voltage remains unknown
    /// unless the rest of the circuit fixes it. Each phase current has an
    /// equal opposite contribution at this reference terminal.
    pub reference_terminal: Option<String>,
    /// Energy cost rate in $/kWh, one entry per phase in terminal-map order.
    pub energy_cost_rate: Option<Vec<f64>>,
    pub extras: Extras,
}

impl VoltageSource {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        bus: impl Into<String>,
        terminal_map: Vec<String>,
        v_magnitude: Vec<f64>,
        v_angle: Vec<f64>,
    ) -> Self {
        Self {
            name: name.into(),
            bus: bus.into(),
            terminal_map,
            v_magnitude,
            v_angle,
            reference_terminal: None,
            energy_cost_rate: None,
            extras: Extras::new(),
        }
    }

    #[must_use]
    pub fn with_reference_terminal(mut self, terminal: impl Into<String>) -> Self {
        self.reference_terminal = Some(terminal.into());
        self
    }
}

// Keep the historical body exactly the same for earth-referenced sources.
// A new source has a tagged wrapper, so a historical reader cannot accept it
// while ignoring essential physics: its required name/bus/phasor fields are
// absent at the wrapper level. This works recursively in every IR document.
#[cfg(feature = "schema")]
#[derive(schemars::JsonSchema)]
#[allow(dead_code)] // The body is only used to generate the schema.
struct SourceFields {
    name: String,
    bus: String,
    terminal_map: Vec<String>,
    /// Volts per terminal (0.0 on grounded terminals).
    v_magnitude: Vec<f64>,
    /// Radians per terminal.
    v_angle: Vec<f64>,
    /// Energy cost rate in $/kWh, one entry per phase in terminal-map order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    energy_cost_rate: Option<Vec<f64>>,
    extras: Extras,
    #[serde(flatten)]
    #[schemars(skip)]
    unknown: std::collections::BTreeMap<String, serde_json::Value>,
}

#[cfg(feature = "schema")]
struct ReferencedSourceFields;

#[cfg(feature = "schema")]
impl schemars::JsonSchema for ReferencedSourceFields {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ReferencedSourceFields".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut value = serde_json::to_value(SourceFields::json_schema(generator))
            .expect("source schema is serializable");
        value["properties"]["reference_terminal"] = serde_json::json!({
            "type": "string", "description": "Declared reference terminal on the source bus; does not imply grounding."
        });
        value["properties"]["v_magnitude"]["description"] =
            "Volts per terminal-to-reference difference, including at grounded phase terminals."
                .into();
        value["properties"]["v_angle"]["description"] =
            "Radians per terminal-to-reference difference.".into();
        value["required"]
            .as_array_mut()
            .expect("source schema has required fields")
            .push("reference_terminal".into());
        serde_json::from_value(value).expect("an extended object remains a schema")
    }
}

#[cfg(feature = "schema")]
#[derive(schemars::JsonSchema)]
#[allow(dead_code)] // The payload is only used to generate the schema.
#[serde(tag = "type", content = "value")]
enum ReferencedSourceWire {
    #[serde(rename = "powerio.ReferencedVoltageSource")]
    Referenced(ReferencedSourceFields),
}

#[derive(Deserialize)]
enum ReferencedType {
    #[serde(rename = "powerio.ReferencedVoltageSource")]
    Referenced,
}

/// The referenced record's body: every historical field plus the reference.
#[derive(Deserialize)]
struct ReferencedBody {
    name: String,
    bus: String,
    terminal_map: Vec<String>,
    v_magnitude: Vec<f64>,
    v_angle: Vec<f64>,
    #[serde(default)]
    energy_cost_rate: Option<Vec<f64>>,
    extras: Extras,
    reference_terminal: String,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum Field {
    Name,
    Bus,
    TerminalMap,
    VMagnitude,
    VAngle,
    EnergyCostRate,
    Extras,
    ReferenceTerminal,
    Type,
    Value,
    #[serde(other)]
    Other,
}

impl<'de> Deserialize<'de> for VoltageSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SourceVisitor;
        impl<'de> Visitor<'de> for SourceVisitor {
            type Value = VoltageSource;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "an earth-referenced source or powerio.ReferencedVoltageSource record",
                )
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut name = None;
                let mut bus = None;
                let mut terminal_map = None;
                let mut v_magnitude = None;
                let mut v_angle = None;
                let mut energy_cost_rate = None;
                let mut extras = None;
                let mut kind = None;
                let mut value: Option<ReferencedBody> = None;
                let mut has_body = false;
                // Decode directly: an untagged enum or a flattened body buffers
                // the record, which loses the nonfinite float spellings.
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
                        Field::Name => read_field!(name),
                        Field::Bus => read_field!(bus),
                        Field::TerminalMap => read_field!(terminal_map),
                        Field::VMagnitude => read_field!(v_magnitude),
                        Field::VAngle => read_field!(v_angle),
                        Field::EnergyCostRate => read_field!(energy_cost_rate),
                        Field::Extras => read_field!(extras),
                        Field::ReferenceTerminal => {
                            return Err(A::Error::custom(
                                "source reference semantics require a powerio.ReferencedVoltageSource record",
                            ));
                        }
                        Field::Type => {
                            if kind.is_some() {
                                return Err(A::Error::duplicate_field("type"));
                            }
                            kind = Some(map.next_value::<ReferencedType>()?);
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
                            "powerio.ReferencedVoltageSource requires all source fields inside value",
                        ));
                    }
                    kind.ok_or_else(|| A::Error::missing_field("type"))?;
                    let body = value.ok_or_else(|| A::Error::missing_field("value"))?;
                    return Ok(VoltageSource {
                        name: body.name,
                        bus: body.bus,
                        terminal_map: body.terminal_map,
                        v_magnitude: body.v_magnitude,
                        v_angle: body.v_angle,
                        reference_terminal: Some(body.reference_terminal),
                        energy_cost_rate: body.energy_cost_rate,
                        extras: body.extras,
                    });
                }
                Ok(VoltageSource {
                    name: name.ok_or_else(|| A::Error::missing_field("name"))?,
                    bus: bus.ok_or_else(|| A::Error::missing_field("bus"))?,
                    terminal_map: terminal_map
                        .ok_or_else(|| A::Error::missing_field("terminal_map"))?,
                    v_magnitude: v_magnitude
                        .ok_or_else(|| A::Error::missing_field("v_magnitude"))?,
                    v_angle: v_angle.ok_or_else(|| A::Error::missing_field("v_angle"))?,
                    reference_terminal: None,
                    energy_cost_rate: energy_cost_rate.unwrap_or(None),
                    extras: extras.ok_or_else(|| A::Error::missing_field("extras"))?,
                })
            }
        }
        deserializer.deserialize_map(SourceVisitor)
    }
}

struct SourceBody<'a>(&'a VoltageSource);

impl Serialize for SourceBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let s = self.0;
        let count = 6
            + usize::from(s.energy_cost_rate.is_some())
            + usize::from(s.reference_terminal.is_some());
        let mut body = serializer.serialize_struct("VoltageSource", count)?;
        body.serialize_field("name", &s.name)?;
        body.serialize_field("bus", &s.bus)?;
        body.serialize_field("terminal_map", &s.terminal_map)?;
        body.serialize_field("v_magnitude", &s.v_magnitude)?;
        body.serialize_field("v_angle", &s.v_angle)?;
        if let Some(cost) = &s.energy_cost_rate {
            body.serialize_field("energy_cost_rate", cost)?;
        }
        body.serialize_field("extras", &s.extras)?;
        if let Some(reference) = &s.reference_terminal {
            body.serialize_field("reference_terminal", reference)?;
        }
        body.end()
    }
}

impl Serialize for VoltageSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        if self.reference_terminal.is_none() {
            return SourceBody(self).serialize(serializer);
        }
        let mut wrapper = serializer.serialize_struct("ReferencedVoltageSource", 2)?;
        wrapper.serialize_field("type", "powerio.ReferencedVoltageSource")?;
        wrapper.serialize_field("value", &SourceBody(self))?;
        wrapper.end()
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for VoltageSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "VoltageSource".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // Retain the published source schema verbatim as the first branch.
        // The other branch has a distinct, mandatory structural type tag.
        schemars::json_schema!({"anyOf": [
            SourceFields::json_schema(generator),
            generator.subschema_for::<ReferencedSourceWire>(),
        ]})
    }
}

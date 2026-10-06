use serde::{Deserialize, Serialize, Serializer};

use super::Extras;

/// An ideal source prescribing terminal voltages relative to earth or one
/// explicit terminal on the same bus. A reference terminal is a conductor,
/// not an implicit grounding instruction.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "SourceWire")]
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
// absent at the wrapper level. This works recursively in every IR envelope.
#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    #[cfg_attr(feature = "schema", schemars(skip))]
    unknown: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct ReferencedSourceFields {
    #[serde(flatten)]
    source: SourceFields,
    reference_terminal: String,
}

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

#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", content = "value")]
enum ReferencedSourceWire {
    #[serde(rename = "powerio.ReferencedVoltageSource")]
    Referenced(ReferencedSourceFields),
}

#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
enum SourceWire {
    Referenced(ReferencedSourceWire),
    Earth(SourceFields),
}

impl TryFrom<SourceWire> for VoltageSource {
    type Error = String;

    fn try_from(wire: SourceWire) -> Result<Self, Self::Error> {
        let (fields, reference_terminal) = match wire {
            SourceWire::Earth(fields) => {
                if ["reference_terminal", "type", "value"]
                    .iter()
                    .any(|key| fields.unknown.contains_key(*key))
                {
                    return Err("source reference semantics require a powerio.ReferencedVoltageSource record".into());
                }
                (fields, None)
            }
            SourceWire::Referenced(ReferencedSourceWire::Referenced(fields)) => {
                (fields.source, Some(fields.reference_terminal))
            }
        };
        Ok(Self {
            name: fields.name,
            bus: fields.bus,
            terminal_map: fields.terminal_map,
            v_magnitude: fields.v_magnitude,
            v_angle: fields.v_angle,
            energy_cost_rate: fields.energy_cost_rate,
            extras: fields.extras,
            reference_terminal,
        })
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

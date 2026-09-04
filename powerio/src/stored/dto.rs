//! Exact data types for PowerIO IR. Runtime types do not derive this
//! layout; the mapping in [`super::convert`] is the one bridge.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::{DeserializeSeed, Error as _, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use powerio_core::limits;

use crate::BalancedNetwork;
use powerio_dist::MulticonductorNetwork;

#[cfg(feature = "schema")]
use schemars::JsonSchema;

/// JSON has no nonfinite number literals. PowerIO spells them as
/// `"Infinity"`, `"-Infinity"`, or `"NaN"` instead of turning valid open
/// bounds into `null`; `null` is not a number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoredF64(pub f64);

impl Serialize for StoredF64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            value if value.is_finite() => serializer.serialize_f64(value),
            value if value == f64::INFINITY => serializer.serialize_str("Infinity"),
            value if value == f64::NEG_INFINITY => serializer.serialize_str("-Infinity"),
            _ => serializer.serialize_str("NaN"),
        }
    }
}

struct StoredF64Visitor;

impl Visitor<'_> for StoredF64Visitor {
    type Value = StoredF64;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON number or \"Infinity\", \"-Infinity\", or \"NaN\"")
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(StoredF64(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        #[allow(clippy::cast_precision_loss)]
        Ok(StoredF64(value as f64))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        #[allow(clippy::cast_precision_loss)]
        Ok(StoredF64(value as f64))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        match value {
            "Infinity" => Ok(StoredF64(f64::INFINITY)),
            "-Infinity" => Ok(StoredF64(f64::NEG_INFINITY)),
            "NaN" => Ok(StoredF64(f64::NAN)),
            _ => Err(E::custom(format!("invalid stored float `{value}`"))),
        }
    }
}

impl<'de> Deserialize<'de> for StoredF64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StoredF64Visitor)
    }
}

#[cfg(feature = "schema")]
impl JsonSchema for StoredF64 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("StoredF64")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // The stated union: a JSON number, or exactly the three nonfinite
        // spellings. `null` is not a number.
        schemars::json_schema!({
            "anyOf": [
                {"type": "number"},
                {"enum": ["Infinity", "-Infinity", "NaN"]}
            ]
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Producer {
    pub name: String,
    pub version: String,
}

// Decode time bounds: every sequence, map, and string in a PowerIO IR record is
// refused or truncated at its limit while it is decoded, before the full
// collection has been retained, matching the core record representation.
fn bounded_identifier<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    limits::BoundedStr {
        what: "record identifier",
        max_bytes: limits::MAX_IDENTIFIER_BYTES,
    }
    .deserialize(deserializer)
}

fn bounded_code<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    limits::BoundedStr {
        what: "diagnostic code",
        max_bytes: limits::MAX_DIAGNOSTIC_CODE_BYTES,
    }
    .deserialize(deserializer)
}

fn truncated_message<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    deserializer.deserialize_str(limits::TruncatedStr {
        max_bytes: limits::MAX_DIAGNOSTIC_MESSAGE_DECODE_BYTES,
    })
}

fn bounded_target<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    limits::BoundedStr {
        what: "record target",
        max_bytes: limits::MAX_DIAGNOSTIC_TARGET_BYTES,
    }
    .deserialize(deserializer)
}

fn bounded_opt_target<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    struct OptTarget;
    impl<'de> Visitor<'de> for OptTarget {
        type Value = Option<String>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded target string or null")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(
            self,
            deserializer: D2,
        ) -> Result<Self::Value, D2::Error> {
            bounded_target(deserializer).map(Some)
        }
    }
    deserializer.deserialize_option(OptTarget)
}

fn bounded_name<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    bounded_identifier(deserializer)
}

fn bounded_opt_action<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    struct OptAction;
    impl<'de> Visitor<'de> for OptAction {
        type Value = Option<String>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded suggested action string or null")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(
            self,
            deserializer: D2,
        ) -> Result<Self::Value, D2::Error> {
            truncated_message(deserializer).map(Some)
        }
    }
    deserializer.deserialize_option(OptAction)
}

fn bounded_btree_map<'de, D: Deserializer<'de>>(
    deserializer: D,
    what: &'static str,
    max_keys: usize,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error> {
    let map = limits::bounded_json_map(
        deserializer,
        what,
        max_keys,
        limits::MAX_IDENTIFIER_BYTES,
        |key| !key.is_empty() && !key.contains('\0'),
    )?;
    Ok(map.into_iter().collect())
}

fn bounded_details<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error> {
    bounded_btree_map(
        deserializer,
        "detail keys",
        limits::MAX_DIAGNOSTIC_DETAIL_KEYS,
    )
}

fn bounded_parameters<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error> {
    bounded_btree_map(
        deserializer,
        "parameter keys",
        limits::MAX_HISTORY_PARAMETERS,
    )
}

fn bounded_extensions<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error> {
    bounded_btree_map(
        deserializer,
        "extension keys",
        limits::MAX_MODULE_EXTENSION_KEYS,
    )
}

fn bounded_diagnostic_spans<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SourceSpan>, D::Error> {
    limits::bounded_vec(deserializer, "source spans", limits::MAX_DIAGNOSTIC_SPANS)
}

fn bounded_map_spans<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SourceSpan>, D::Error> {
    limits::bounded_vec(deserializer, "source spans", limits::MAX_SOURCE_MAP_SPANS)
}

fn bounded_related<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<DiagnosticId>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "related diagnostics",
        limits::MAX_DIAGNOSTIC_RELATED,
    )
}

fn bounded_notes<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    limits::bounded_vec(deserializer, "history notes", limits::MAX_HISTORY_NOTES)
}

fn bounded_sources<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SourceDescriptor>, D::Error> {
    limits::bounded_vec(deserializer, "sources", limits::MAX_MODULE_SOURCES)
}

fn bounded_source_map<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SourceMapEntry>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "source map entries",
        limits::MAX_MODULE_SOURCE_MAP_ENTRIES,
    )
}

fn bounded_diagnostics<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Diagnostic>, D::Error> {
    limits::bounded_vec(deserializer, "diagnostics", limits::MAX_MODULE_DIAGNOSTICS)
}

fn bounded_history<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<HistoryEntry>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "history entries",
        limits::MAX_MODULE_HISTORY_ENTRIES,
    )
}

const MAX_STORED_COLLECTION_ENTRIES: usize = 65_536;
const MAX_STORED_OPERATING_POINT_QUANTITIES: usize = 64;

fn bounded_collection_entries<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "collection entries",
        MAX_STORED_COLLECTION_ENTRIES,
    )
}

fn bounded_operating_point_identities<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<StoredIdentity>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "operating point identities",
        MAX_STORED_COLLECTION_ENTRIES,
    )
}

fn bounded_operating_point_values<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<StoredF64>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "operating point values",
        MAX_STORED_COLLECTION_ENTRIES,
    )
}

fn bounded_three_winding_transformer_terminal_powers<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ThreeWindingTransformerTerminalPower>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "three winding transformer terminal powers",
        MAX_STORED_COLLECTION_ENTRIES,
    )
}

fn bounded_three_winding_transformer_terminal_active_powers<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ThreeWindingTransformerTerminalActivePower>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "three winding transformer terminal active powers",
        MAX_STORED_COLLECTION_ENTRIES,
    )
}

fn bounded_optional_values<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<StoredF64>>, D::Error> {
    struct OptionalValues;

    impl<'de> Visitor<'de> for OptionalValues {
        type Value = Option<Vec<StoredF64>>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded value vector or null")
        }

        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2: Deserializer<'de>>(
            self,
            deserializer: D2,
        ) -> Result<Self::Value, D2::Error> {
            bounded_operating_point_values(deserializer).map(Some)
        }
    }

    deserializer.deserialize_option(OptionalValues)
}

fn bounded_operating_point_quantities<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, StoredQuantity>, D::Error> {
    struct Quantities;

    impl<'de> Visitor<'de> for Quantities {
        type Value = BTreeMap<String, StoredQuantity>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "at most {MAX_STORED_OPERATING_POINT_QUANTITIES} operating point quantities"
            )
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut access: A,
        ) -> Result<Self::Value, A::Error> {
            let mut quantities = BTreeMap::new();
            while let Some(name) = access.next_key_seed(limits::BoundedStr {
                what: "operating point quantity name",
                max_bytes: limits::MAX_IDENTIFIER_BYTES,
            })? {
                if quantities.len() == MAX_STORED_OPERATING_POINT_QUANTITIES {
                    return Err(A::Error::custom(format!(
                        "a stored operating point carries more than \
                         {MAX_STORED_OPERATING_POINT_QUANTITIES} quantities"
                    )));
                }
                let value = access.next_value::<StoredQuantity>()?;
                if quantities.insert(name.clone(), value).is_some() {
                    return Err(A::Error::custom(format!(
                        "duplicate operating point quantity `{name}`"
                    )));
                }
            }
            Ok(quantities)
        }
    }

    deserializer.deserialize_map(Quantities)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct SourceId(#[serde(deserialize_with = "bounded_identifier")] pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct DiagnosticId(#[serde(deserialize_with = "bounded_identifier")] pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct HistoryId(#[serde(deserialize_with = "bounded_identifier")] pub String);

/// A stored duration: unsigned seconds plus a nanosecond remainder below one
/// billion, exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Duration {
    pub secs: u64,
    pub nanos: u32,
}

/// A bounded stable component identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct StoredIdentity(#[serde(deserialize_with = "bounded_identifier")] pub String);

/// One bounded time point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TimePoint {
    #[serde(deserialize_with = "bounded_identifier")]
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<Duration>,
}

/// One operating point quantity. Both dimensions are bounded while
/// decoding, before the vectors are retained.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredQuantity {
    #[serde(deserialize_with = "bounded_operating_point_identities")]
    pub identities: Vec<StoredIdentity>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub values: Vec<StoredF64>,
}

/// One scalar operating point and the network whose identities and defaults
/// it uses.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredOperatingPoint<N> {
    pub network: Box<N>,
    #[serde(deserialize_with = "bounded_operating_point_quantities")]
    pub quantities: BTreeMap<String, StoredQuantity>,
}

/// Ordered complete values of one registered type.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(
    deny_unknown_fields,
    bound(serialize = "T: Serialize", deserialize = "T: Deserialize<'de>")
)]
pub struct StoredTimeSeries<T> {
    #[serde(deserialize_with = "bounded_collection_entries")]
    pub time_points: Vec<TimePoint>,
    #[serde(deserialize_with = "bounded_collection_entries")]
    pub values: Vec<T>,
}

/// A time series of operating points. The shared network is absent only for
/// an empty series, whose outer structural type still preserves the element
/// type.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredOperatingPointTimeSeries<N> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<Box<N>>,
    #[serde(deserialize_with = "bounded_collection_entries")]
    pub time_points: Vec<TimePoint>,
    #[serde(deserialize_with = "bounded_collection_entries")]
    pub values: Vec<StoredOperatingPointAssignment>,
}

/// One named scenario whose value has the type fixed by the outer structural
/// discriminator.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(
    deny_unknown_fields,
    bound(serialize = "T: Serialize", deserialize = "T: Deserialize<'de>")
)]
pub struct StoredScenario<T> {
    #[serde(deserialize_with = "bounded_identifier")]
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probability: Option<StoredF64>,
    pub value: T,
}

/// Named alternatives of one registered type.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(
    deny_unknown_fields,
    bound(serialize = "T: Serialize", deserialize = "T: Deserialize<'de>")
)]
pub struct StoredScenarioSet<T> {
    #[serde(deserialize_with = "bounded_collection_entries")]
    pub scenarios: Vec<StoredScenario<T>>,
}

/// One operating point scenario. The network is stored once on the enclosing
/// set because all alternatives assign values over the same identities.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredOperatingPointScenario {
    #[serde(deserialize_with = "bounded_identifier")]
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probability: Option<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_quantities")]
    pub quantities: BTreeMap<String, StoredQuantity>,
}

/// A scenario set of operating points. The shared network is absent only for
/// an empty set.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredOperatingPointScenarioSet<N> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<Box<N>>,
    #[serde(deserialize_with = "bounded_collection_entries")]
    pub scenarios: Vec<StoredOperatingPointScenario>,
}

/// The PowerIO objective vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case", tag = "term", deny_unknown_fields)]
pub enum ObjectiveTerm {
    NetworkGeneratorCost,
    ActivePowerDispatchCost,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Objective {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<ObjectiveTerm>,
}

/// One calculation initial operating point. Its network
/// is the enclosing instance's network.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredOperatingPointAssignment {
    #[serde(deserialize_with = "bounded_operating_point_quantities")]
    pub quantities: BTreeMap<String, StoredQuantity>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DcPfInstance {
    pub network: Box<BalancedNetwork>,
    pub approximation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_point: Option<StoredOperatingPointAssignment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcPfInstance {
    pub network: Box<BalancedNetwork>,
    pub specifications: Vec<powerio_prob::AcBusSpecification>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_point: Option<StoredOperatingPointAssignment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DcOpfInstance {
    pub network: Box<BalancedNetwork>,
    pub approximation: String,
    pub objective: Objective,
    pub constraints: powerio_prob::ActiveConstraints,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_point: Option<StoredOperatingPointAssignment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcOpfInstance {
    pub network: Box<BalancedNetwork>,
    pub objective: Objective,
    pub constraints: powerio_prob::ActiveConstraints,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_point: Option<StoredOperatingPointAssignment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct McAcPfInstance {
    pub network: Box<MulticonductorNetwork>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_point: Option<StoredOperatingPointAssignment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct McAcOpfInstance {
    pub network: Box<MulticonductorNetwork>,
    pub objective: Objective,
    pub constraints: powerio_prob::MulticonductorActiveConstraints,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_point: Option<StoredOperatingPointAssignment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcScucInstance {
    pub network: Box<BalancedNetwork>,
    /// The complete SCUC inputs, in the calculation crate's own
    /// serialization, typed through this document's schema.
    pub inputs: Box<powerio_prob::ScucInputs>,
}

/// A solution's producer stated generator dispatch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GeneratorDispatch {
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub p_mw: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub q_mvar: Vec<StoredF64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ThreeWindingTransformerTerminalPower {
    pub p_mw: [StoredF64; 3],
    pub q_mvar: [StoredF64; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ThreeWindingTransformerTerminalActivePower {
    pub p_mw: [StoredF64; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DcPfSolution {
    pub instance: DcPfInstance,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_voltage_angle: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_active_injection: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_from_active_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_to_active_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_three_winding_transformer_terminal_active_powers")]
    pub three_winding_transformer_terminal_active_powers:
        Vec<ThreeWindingTransformerTerminalActivePower>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generator_dispatch: Option<GeneratorDispatch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcPfSolution {
    pub instance: AcPfInstance,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_voltage_magnitude: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_voltage_angle: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_active_injection: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_reactive_injection: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_from_active_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_from_reactive_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_to_active_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_to_reactive_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_three_winding_transformer_terminal_powers")]
    pub three_winding_transformer_terminal_powers: Vec<ThreeWindingTransformerTerminalPower>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generator_dispatch: Option<GeneratorDispatch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DcOpfSolution {
    pub instance: DcOpfInstance,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_voltage_angle: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_active_injection: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_from_active_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_to_active_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub generator_active_power: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_three_winding_transformer_terminal_active_powers")]
    pub three_winding_transformer_terminal_active_powers:
        Vec<ThreeWindingTransformerTerminalActivePower>,
    pub objective: StoredF64,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub bus_active_power_marginal: Option<Vec<StoredF64>>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch_from_limit_multiplier: Option<Vec<StoredF64>>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch_to_limit_multiplier: Option<Vec<StoredF64>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcOpfSolution {
    pub instance: AcOpfInstance,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_voltage_magnitude: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_voltage_angle: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_active_injection: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_reactive_injection: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_from_active_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_from_reactive_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_to_active_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_to_reactive_flow: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub generator_active_power: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub generator_reactive_power: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_three_winding_transformer_terminal_powers")]
    pub three_winding_transformer_terminal_powers: Vec<ThreeWindingTransformerTerminalPower>,
    pub objective: StoredF64,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub bus_active_power_marginal: Option<Vec<StoredF64>>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub bus_reactive_power_marginal: Option<Vec<StoredF64>>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch_from_limit_multiplier: Option<Vec<StoredF64>>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch_to_limit_multiplier: Option<Vec<StoredF64>>,
}

/// Primal values of a serialized SOCWR relaxation result.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SocwrOpfValues {
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub bus_voltage_magnitude_squared: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_voltage_product_real: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_voltage_product_imaginary: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub generator_active_power: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub generator_reactive_power: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_from_active_power: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_from_reactive_power: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_to_active_power: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub branch_to_reactive_power: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_three_winding_transformer_terminal_powers")]
    pub three_winding_transformer_terminal_powers: Vec<ThreeWindingTransformerTerminalPower>,
}

/// Optional dual values of a serialized SOCWR relaxation result.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SocwrOpfDuals {
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub bus_active_power_marginal: Option<Vec<StoredF64>>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub bus_reactive_power_marginal: Option<Vec<StoredF64>>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch_from_thermal_limit_multiplier: Option<Vec<StoredF64>>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch_to_thermal_limit_multiplier: Option<Vec<StoredF64>>,
}

/// A PowerModels SOCWR relaxation result. Its objective is explicitly a lower
/// bound, not an AC feasible objective value.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SocwrOpfSolution {
    pub instance: AcOpfInstance,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    pub values: SocwrOpfValues,
    #[serde(default)]
    pub duals: SocwrOpfDuals,
    pub objective_lower_bound: StoredF64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct McAcPfSolution {
    pub instance: McAcPfInstance,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub terminal_voltage_magnitude: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub terminal_voltage_angle: Vec<StoredF64>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_current_magnitude: Option<Vec<StoredF64>>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_active_power: Option<Vec<StoredF64>>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub source_active_injection: Vec<StoredF64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct McAcOpfSolution {
    pub instance: McAcOpfInstance,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub terminal_voltage_magnitude: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub terminal_voltage_angle: Vec<StoredF64>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_current_magnitude: Option<Vec<StoredF64>>,
    #[serde(
        default,
        deserialize_with = "bounded_optional_values",
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_active_power: Option<Vec<StoredF64>>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub source_active_injection: Vec<StoredF64>,
    #[serde(deserialize_with = "bounded_operating_point_values")]
    pub generator_active_power: Vec<StoredF64>,
    pub objective: StoredF64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ScucNetworkOutputs {
    pub bus_vm: Vec<Vec<StoredF64>>,
    pub bus_va: Vec<Vec<StoredF64>>,
    pub shunt_step: Vec<Vec<i64>>,
    pub ac_line_on_status: Vec<Vec<bool>>,
    pub transformer_tm: Vec<Vec<StoredF64>>,
    pub transformer_ta: Vec<Vec<StoredF64>>,
    pub transformer_on_status: Vec<Vec<bool>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dc_line_pdc_fr: Vec<Vec<StoredF64>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dc_line_qdc_fr: Vec<Vec<StoredF64>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dc_line_qdc_to: Vec<Vec<StoredF64>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ScucDeviceOutputs {
    pub on_status: Vec<Vec<bool>>,
    pub startup_status: Vec<Vec<bool>>,
    pub shutdown_status: Vec<Vec<bool>>,
    pub p_on: Vec<Vec<StoredF64>>,
    pub q: Vec<Vec<StoredF64>>,
    pub p_reg_res_up: Vec<Vec<StoredF64>>,
    pub p_reg_res_down: Vec<Vec<StoredF64>>,
    pub p_syn_res: Vec<Vec<StoredF64>>,
    pub p_nsyn_res: Vec<Vec<StoredF64>>,
    pub p_ramp_res_up_online: Vec<Vec<StoredF64>>,
    pub p_ramp_res_up_offline: Vec<Vec<StoredF64>>,
    pub p_ramp_res_down_online: Vec<Vec<StoredF64>>,
    pub p_ramp_res_down_offline: Vec<Vec<StoredF64>>,
    pub q_res_up: Vec<Vec<StoredF64>>,
    pub q_res_down: Vec<Vec<StoredF64>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcScucSolution {
    pub instance: AcScucInstance,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    pub network_outputs: ScucNetworkOutputs,
    pub device_outputs: ScucDeviceOutputs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<StoredF64>,
}

/// PowerIO IR value representation. The discriminator is the canonical
/// structural type name used by every dynamic boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields, tag = "type", content = "data")]
pub enum StoredValue {
    #[serde(rename = "powerio.BalancedNetwork")]
    BalancedNetwork(Box<BalancedNetwork>),
    #[serde(rename = "powerio.MulticonductorNetwork")]
    MulticonductorNetwork(Box<MulticonductorNetwork>),
    #[serde(rename = "powerio.GeoLayer")]
    GeoLayer(Box<powerio_tx::GeoLayer>),
    #[serde(rename = "powerio.OperatingPoint<powerio.BalancedNetwork>")]
    BalancedOperatingPoint(StoredOperatingPoint<BalancedNetwork>),
    #[serde(rename = "powerio.OperatingPoint<powerio.MulticonductorNetwork>")]
    MulticonductorOperatingPoint(StoredOperatingPoint<MulticonductorNetwork>),
    #[serde(rename = "powerio.TimeSeries<powerio.BalancedNetwork>")]
    BalancedNetworkTimeSeries(StoredTimeSeries<BalancedNetwork>),
    #[serde(rename = "powerio.TimeSeries<powerio.MulticonductorNetwork>")]
    MulticonductorNetworkTimeSeries(StoredTimeSeries<MulticonductorNetwork>),
    #[serde(rename = "powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>")]
    BalancedOperatingPointTimeSeries(StoredOperatingPointTimeSeries<BalancedNetwork>),
    #[serde(rename = "powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>")]
    MulticonductorOperatingPointTimeSeries(StoredOperatingPointTimeSeries<MulticonductorNetwork>),
    #[serde(rename = "powerio.ScenarioSet<powerio.BalancedNetwork>")]
    BalancedNetworkScenarioSet(StoredScenarioSet<BalancedNetwork>),
    #[serde(rename = "powerio.ScenarioSet<powerio.MulticonductorNetwork>")]
    MulticonductorNetworkScenarioSet(StoredScenarioSet<MulticonductorNetwork>),
    #[serde(rename = "powerio.ScenarioSet<powerio.OperatingPoint<powerio.BalancedNetwork>>")]
    BalancedOperatingPointScenarioSet(StoredOperatingPointScenarioSet<BalancedNetwork>),
    #[serde(rename = "powerio.ScenarioSet<powerio.OperatingPoint<powerio.MulticonductorNetwork>>")]
    MulticonductorOperatingPointScenarioSet(StoredOperatingPointScenarioSet<MulticonductorNetwork>),
    #[serde(rename = "powerio.ScenarioSet<powerio.TimeSeries<powerio.BalancedNetwork>>")]
    BalancedNetworkTimeSeriesScenarioSet(StoredScenarioSet<StoredTimeSeries<BalancedNetwork>>),
    #[serde(rename = "powerio.ScenarioSet<powerio.TimeSeries<powerio.MulticonductorNetwork>>")]
    MulticonductorNetworkTimeSeriesScenarioSet(
        StoredScenarioSet<StoredTimeSeries<MulticonductorNetwork>>,
    ),
    #[serde(
        rename = "powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>>"
    )]
    BalancedOperatingPointTimeSeriesScenarioSet(
        StoredScenarioSet<StoredOperatingPointTimeSeries<BalancedNetwork>>,
    ),
    #[serde(
        rename = "powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>>"
    )]
    MulticonductorOperatingPointTimeSeriesScenarioSet(
        StoredScenarioSet<StoredOperatingPointTimeSeries<MulticonductorNetwork>>,
    ),
    #[serde(rename = "powerio.DcPfInstance")]
    DcPfInstance(DcPfInstance),
    #[serde(rename = "powerio.AcPfInstance")]
    AcPfInstance(AcPfInstance),
    #[serde(rename = "powerio.DcOpfInstance")]
    DcOpfInstance(DcOpfInstance),
    #[serde(rename = "powerio.AcOpfInstance")]
    AcOpfInstance(AcOpfInstance),
    #[serde(rename = "powerio.McAcPfInstance")]
    McAcPfInstance(McAcPfInstance),
    #[serde(rename = "powerio.McAcOpfInstance")]
    McAcOpfInstance(McAcOpfInstance),
    #[serde(rename = "powerio.AcScucInstance")]
    AcScucInstance(AcScucInstance),
    #[serde(rename = "powerio.DcPfSolution")]
    DcPfSolution(Box<DcPfSolution>),
    #[serde(rename = "powerio.AcPfSolution")]
    AcPfSolution(Box<AcPfSolution>),
    #[serde(rename = "powerio.DcOpfSolution")]
    DcOpfSolution(Box<DcOpfSolution>),
    #[serde(rename = "powerio.AcOpfSolution")]
    AcOpfSolution(Box<AcOpfSolution>),
    #[serde(rename = "powerio.SocwrOpfSolution")]
    SocwrOpfSolution(Box<SocwrOpfSolution>),
    #[serde(rename = "powerio.McAcPfSolution")]
    McAcPfSolution(Box<McAcPfSolution>),
    #[serde(rename = "powerio.McAcOpfSolution")]
    McAcOpfSolution(Box<McAcOpfSolution>),
    #[serde(rename = "powerio.AcScucSolution")]
    AcScucSolution(Box<AcScucSolution>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceDescriptor {
    pub id: SourceId,
    pub name: String,
    pub byte_length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithm {
    Sha256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Digest {
    pub algorithm: DigestAlgorithm,
    /// Lowercase hexadecimal, without a prefix.
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub source: SourceId,
    pub byte_start: u64,
    pub byte_end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceRelation {
    Exact,
    Defaulted,
    Inferred,
    ConvertedUnits,
    Aggregated,
    Split,
    Synthetic,
    Transformed,
    RetainedExtra,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceMapEntry {
    /// RFC 6901 pointer into `value.data`.
    #[serde(deserialize_with = "bounded_target")]
    pub target: String,
    pub relation: SourceRelation,
    /// Empty only for `defaulted`, `synthetic`, or `transformed`.
    #[serde(
        default,
        deserialize_with = "bounded_map_spans",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub spans: Vec<SourceSpan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Remark,
    Note,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub id: DiagnosticId,
    pub severity: Severity,
    #[serde(deserialize_with = "bounded_code")]
    pub code: String,
    #[serde(deserialize_with = "truncated_message")]
    pub message: String,
    #[serde(
        default,
        deserialize_with = "bounded_opt_target",
        skip_serializing_if = "Option::is_none"
    )]
    pub target: Option<String>,
    #[serde(
        default,
        deserialize_with = "bounded_diagnostic_spans",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub spans: Vec<SourceSpan>,
    #[serde(
        default,
        deserialize_with = "bounded_related",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub related: Vec<DiagnosticId>,
    #[serde(
        default,
        deserialize_with = "bounded_opt_action",
        skip_serializing_if = "Option::is_none"
    )]
    pub suggested_action: Option<String>,
    #[serde(
        default,
        deserialize_with = "bounded_details",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub details: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    Parse,
    Transform,
    Edit,
    Repair,
    Solve,
}

/// PowerIO IR history record. Input and output identify structural types,
/// not a parallel value registry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HistoryEntry {
    pub id: HistoryId,
    pub kind: HistoryKind,
    #[serde(deserialize_with = "bounded_name")]
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_type: Option<String>,
    #[serde(
        default,
        deserialize_with = "bounded_parameters",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub parameters: BTreeMap<String, serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "bounded_notes",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub assumptions: Vec<String>,
    #[serde(
        default,
        deserialize_with = "bounded_notes",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub losses: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields, remote = "Self")]
pub struct StoredModule {
    #[cfg_attr(
        feature = "schema",
        schemars(extend("const" = crate::IR_SCHEMA_NAME))
    )]
    pub schema: String,
    #[cfg_attr(
        feature = "schema",
        schemars(extend("const" = crate::IR_SCHEMA_VERSION))
    )]
    pub version: String,
    pub producer: Producer,
    pub value: StoredValue,
    #[serde(
        default,
        deserialize_with = "bounded_sources",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub sources: Vec<SourceDescriptor>,
    #[serde(
        default,
        deserialize_with = "bounded_source_map",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub source_map: Vec<SourceMapEntry>,
    #[serde(
        default,
        deserialize_with = "bounded_diagnostics",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub diagnostics: Vec<Diagnostic>,
    #[serde(
        default,
        deserialize_with = "bounded_history",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub history: Vec<HistoryEntry>,
    #[serde(
        default,
        deserialize_with = "bounded_extensions",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl Serialize for StoredModule {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        StoredModule::serialize(
            self,
            powerio_core::__implementation::nonfinite::NonFiniteSer(serializer),
        )
    }
}

impl<'de> Deserialize<'de> for StoredModule {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        StoredModule::deserialize(powerio_core::__implementation::nonfinite::NonFiniteDe(
            deserializer,
        ))
    }
}

/// The `version` a document states, kept as written so a refusal can name it:
/// the string this reader compares, or whatever an earlier generation wrote
/// there (the v0.10.0 document carried the integer `1`).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum StoredVersion {
    Text(String),
    Other(serde_json::Value),
}

impl fmt::Display for StoredVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.write_str(text),
            Self::Other(value) => value.fmt(f),
        }
    }
}

/// The two header fields the reader dispatches on, read from a document that
/// did not decode as the current shape.
#[derive(Debug, Deserialize)]
pub(super) struct StoredHeader {
    #[serde(default)]
    pub(super) schema: Option<String>,
    #[serde(default)]
    pub(super) version: Option<StoredVersion>,
}

/// Structural validation of one decoded document: identities, digests,
/// spans, pointers, relation rules, namespaced extensions, and value shapes.
pub fn validate(module: &StoredModule) -> Result<(), String> {
    validate_value(&module.value)?;
    validate_records(
        &module.value,
        &module.sources,
        &module.source_map,
        &module.diagnostics,
        module
            .history
            .iter()
            .map(|entry| (entry.id.0.as_str(), entry.name.as_str())),
        &module.extensions,
    )
}

fn validate_records<'a, V: Serialize>(
    value: &V,
    module_sources: &[SourceDescriptor],
    source_map: &[SourceMapEntry],
    module_diagnostics: &[Diagnostic],
    module_history: impl IntoIterator<Item = (&'a str, &'a str)>,
    extensions: &BTreeMap<String, serde_json::Value>,
) -> Result<(), String> {
    // Pointer existence checks re-inflate the value only when something
    // actually targets it.
    let needs_targets = !source_map.is_empty()
        || module_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.target.is_some());
    // One generic representation, borrowed by every target check: the tagged
    // value serializes with its `data` key, so targets resolve under a `/data`
    // prefix instead of cloning the subtree out.
    let stored_value = if needs_targets {
        Some(serde_json::to_value(value).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let value_data = stored_value.as_ref();

    let mut sources = BTreeMap::new();
    for source in module_sources {
        nonempty("source id", &source.id.0)?;
        if sources
            .insert(source.id.0.clone(), source.byte_length)
            .is_some()
        {
            return Err(format!("duplicate source id `{}`", source.id.0));
        }
        nonempty("source name", &source.name)?;
        if let Some(digest) = &source.digest
            && (digest.value.len() != 64
                || !digest
                    .value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        {
            return Err(format!("invalid SHA-256 `{}`", digest.value));
        }
    }

    let mut diagnostics = BTreeSet::new();
    for diagnostic in module_diagnostics {
        nonempty("diagnostic id", &diagnostic.id.0)?;
        if !diagnostics.insert(diagnostic.id.0.as_str()) {
            return Err(format!("duplicate diagnostic id `{}`", diagnostic.id.0));
        }
        validate_target(diagnostic.target.as_deref(), value_data)?;
        validate_spans(&diagnostic.spans, &sources)?;
    }
    for diagnostic in module_diagnostics {
        for related in &diagnostic.related {
            if !diagnostics.contains(related.0.as_str()) {
                return Err(format!(
                    "diagnostic `{}` refers to unknown diagnostic `{}`",
                    diagnostic.id.0, related.0
                ));
            }
        }
    }

    for entry in source_map {
        validate_target(Some(&entry.target), value_data)?;
        validate_spans(&entry.spans, &sources)?;
        let empty_allowed = matches!(
            entry.relation,
            SourceRelation::Defaulted | SourceRelation::Synthetic | SourceRelation::Transformed
        );
        if entry.spans.is_empty() && !empty_allowed {
            return Err(format!(
                "source map relation `{:?}` requires at least one span",
                entry.relation
            ));
        }
    }

    let mut history = BTreeSet::new();
    for (id, name) in module_history {
        nonempty("history id", id)?;
        if !history.insert(id) {
            return Err(format!("duplicate history id `{id}`"));
        }
        nonempty("history operation name", name)?;
    }

    for namespace in extensions.keys() {
        if namespace.starts_with('.') || !namespace.contains('.') {
            return Err(format!("extension key `{namespace}` is not namespaced"));
        }
    }

    Ok(())
}

// One arm per stored kind, as in the decoder.
#[allow(clippy::too_many_lines)]
fn validate_time_points(points: &[TimePoint]) -> Result<(), String> {
    for point in points {
        nonempty("time point label", &point.label)?;
        if let Some(duration) = point.duration
            && duration.nanos >= 1_000_000_000
        {
            return Err(format!(
                "time point `{}` has an invalid nanosecond remainder {}",
                point.label, duration.nanos
            ));
        }
    }
    Ok(())
}

fn validate_quantities(
    quantities: &BTreeMap<String, StoredQuantity>,
    rows: usize,
) -> Result<(), String> {
    for (name, quantity) in quantities {
        nonempty("operating point quantity name", name)?;
        let expected = rows
            .checked_mul(quantity.identities.len())
            .ok_or_else(|| format!("quantity `{name}` dimensions overflow"))?;
        if quantity.values.len() != expected {
            return Err(format!(
                "operating point quantity `{name}` has {} values; expected {expected}",
                quantity.values.len()
            ));
        }
        for identity in &quantity.identities {
            nonempty("operating point identity", &identity.0)?;
        }
    }
    Ok(())
}

fn validate_time_series<T>(series: &StoredTimeSeries<T>) -> Result<(), String> {
    if series.time_points.len() != series.values.len() {
        return Err(format!(
            "time series has {} values for {} time points",
            series.values.len(),
            series.time_points.len()
        ));
    }
    validate_time_points(&series.time_points)
}

fn validate_operating_point_series<N>(
    series: &StoredOperatingPointTimeSeries<N>,
) -> Result<(), String> {
    validate_time_points(&series.time_points)?;
    if series.time_points.len() != series.values.len() {
        return Err(format!(
            "operating point time series has {} values for {} time points",
            series.values.len(),
            series.time_points.len()
        ));
    }
    if series.time_points.is_empty() {
        if series.network.is_some() {
            return Err("an empty operating point time series cannot carry a network".to_string());
        }
        return Ok(());
    }
    if series.network.is_none() {
        return Err("a nonempty operating point time series needs its base network".to_string());
    }
    for point in &series.values {
        validate_quantities(&point.quantities, 1)?;
    }
    Ok(())
}

fn validate_scenario_set<T>(
    set: &StoredScenarioSet<T>,
    mut validate_value: impl FnMut(&T) -> Result<(), String>,
) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    for scenario in &set.scenarios {
        nonempty("scenario id", &scenario.id)?;
        if !ids.insert(scenario.id.as_str()) {
            return Err(format!("duplicate scenario id `{}`", scenario.id));
        }
        validate_value(&scenario.value)?;
    }
    Ok(())
}

fn validate_operating_point_scenario_set<N>(
    set: &StoredOperatingPointScenarioSet<N>,
) -> Result<(), String> {
    if set.scenarios.is_empty() {
        if set.network.is_some() {
            return Err("an empty operating point scenario set cannot carry a network".to_string());
        }
        return Ok(());
    }
    if set.network.is_none() {
        return Err("a nonempty operating point scenario set needs its base network".to_string());
    }
    let mut ids = BTreeSet::new();
    for scenario in &set.scenarios {
        nonempty("scenario id", &scenario.id)?;
        if !ids.insert(scenario.id.as_str()) {
            return Err(format!("duplicate scenario id `{}`", scenario.id));
        }
        validate_quantities(&scenario.quantities, 1)?;
    }
    Ok(())
}

/// A layer places a finite point or route per feature, and every feature
/// names the element it places or a branch's two endpoint buses.
fn validate_geo_layer(layer: &powerio_tx::GeoLayer) -> Result<(), String> {
    for (index, feature) in layer.features.iter().enumerate() {
        let key = &feature.key;
        let named =
            key.uid.is_some() || key.id.is_some() || key.name.is_some() || key.index.is_some();
        let endpoint_pair = feature.target == powerio_tx::GeoTarget::Branch
            && feature.from.is_some()
            && feature.to.is_some();
        if !named && !endpoint_pair {
            return Err(format!("geo feature {index} names no element"));
        }
        let finite = |point: &[f64; 2]| point.iter().all(|value| value.is_finite());
        let placed = match &feature.geometry {
            powerio_tx::GeoGeometry::Point(point) => finite(point),
            powerio_tx::GeoGeometry::LineString(points) => {
                !points.is_empty() && points.iter().all(finite)
            }
        };
        if !placed {
            return Err(format!("geo feature {index} has no finite geometry"));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_value(value: &StoredValue) -> Result<(), String> {
    match value {
        StoredValue::BalancedNetwork(_)
        | StoredValue::MulticonductorNetwork(_)
        | StoredValue::AcScucInstance(_)
        | StoredValue::AcScucSolution(_) => Ok(()),
        StoredValue::GeoLayer(layer) => validate_geo_layer(layer),
        StoredValue::BalancedOperatingPoint(point) => validate_quantities(&point.quantities, 1),
        StoredValue::MulticonductorOperatingPoint(point) => {
            validate_quantities(&point.quantities, 1)
        }
        StoredValue::BalancedNetworkTimeSeries(series) => validate_time_series(series),
        StoredValue::MulticonductorNetworkTimeSeries(series) => validate_time_series(series),
        StoredValue::BalancedOperatingPointTimeSeries(series) => {
            validate_operating_point_series(series)
        }
        StoredValue::MulticonductorOperatingPointTimeSeries(series) => {
            validate_operating_point_series(series)
        }
        StoredValue::BalancedNetworkScenarioSet(set) => validate_scenario_set(set, |_| Ok(())),
        StoredValue::MulticonductorNetworkScenarioSet(set) => {
            validate_scenario_set(set, |_| Ok(()))
        }
        StoredValue::BalancedOperatingPointScenarioSet(set) => {
            validate_operating_point_scenario_set(set)
        }
        StoredValue::MulticonductorOperatingPointScenarioSet(set) => {
            validate_operating_point_scenario_set(set)
        }
        StoredValue::BalancedNetworkTimeSeriesScenarioSet(set) => {
            validate_scenario_set(set, validate_time_series)
        }
        StoredValue::MulticonductorNetworkTimeSeriesScenarioSet(set) => {
            validate_scenario_set(set, validate_time_series)
        }
        StoredValue::BalancedOperatingPointTimeSeriesScenarioSet(set) => {
            validate_scenario_set(set, validate_operating_point_series)
        }
        StoredValue::MulticonductorOperatingPointTimeSeriesScenarioSet(set) => {
            validate_scenario_set(set, validate_operating_point_series)
        }
        StoredValue::DcPfInstance(instance) => {
            validate_stored_assignment(instance.initial_point.as_ref())
        }
        StoredValue::AcPfInstance(instance) => {
            validate_stored_assignment(instance.initial_point.as_ref())
        }
        StoredValue::DcOpfInstance(instance) => {
            validate_stored_assignment(instance.initial_point.as_ref())
        }
        StoredValue::AcOpfInstance(instance) => {
            validate_stored_assignment(instance.initial_point.as_ref())
        }
        StoredValue::McAcPfInstance(instance) => {
            validate_stored_assignment(instance.initial_point.as_ref())
        }
        StoredValue::McAcOpfInstance(instance) => {
            validate_stored_assignment(instance.initial_point.as_ref())
        }
        StoredValue::DcPfSolution(solution) => {
            validate_stored_assignment(solution.instance.initial_point.as_ref())
        }
        StoredValue::AcPfSolution(solution) => {
            validate_stored_assignment(solution.instance.initial_point.as_ref())
        }
        StoredValue::DcOpfSolution(solution) => {
            validate_stored_assignment(solution.instance.initial_point.as_ref())
        }
        StoredValue::AcOpfSolution(solution) => {
            validate_stored_assignment(solution.instance.initial_point.as_ref())
        }
        StoredValue::SocwrOpfSolution(solution) => {
            validate_stored_assignment(solution.instance.initial_point.as_ref())
        }
        StoredValue::McAcPfSolution(solution) => {
            validate_stored_assignment(solution.instance.initial_point.as_ref())
        }
        StoredValue::McAcOpfSolution(solution) => {
            validate_stored_assignment(solution.instance.initial_point.as_ref())
        }
    }
}

fn validate_stored_assignment(
    point: Option<&StoredOperatingPointAssignment>,
) -> Result<(), String> {
    point.map_or(Ok(()), |point| validate_quantities(&point.quantities, 1))
}

fn validate_spans(spans: &[SourceSpan], sources: &BTreeMap<String, u64>) -> Result<(), String> {
    for span in spans {
        let Some(byte_length) = sources.get(span.source.0.as_str()) else {
            return Err(format!("unknown source id `{}`", span.source.0));
        };
        if span.byte_start > span.byte_end {
            return Err(format!(
                "source span {}..{} is reversed",
                span.byte_start, span.byte_end
            ));
        }
        if span.byte_end > *byte_length {
            return Err(format!(
                "source span end {} exceeds source length {}",
                span.byte_end, byte_length
            ));
        }
    }
    Ok(())
}

fn validate_pointer(pointer: Option<&str>) -> Result<(), String> {
    if let Some(pointer) = pointer {
        if !pointer.is_empty() && !pointer.starts_with('/') {
            return Err(format!("`{pointer}` is not an RFC 6901 pointer"));
        }
        let bytes = pointer.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'~'
                && (index + 1 == bytes.len() || !matches!(bytes[index + 1], b'0' | b'1'))
            {
                return Err(format!("`{pointer}` is not an RFC 6901 pointer"));
            }
            index += 1;
        }
    }
    Ok(())
}

fn validate_target(
    pointer: Option<&str>,
    stored_value: Option<&serde_json::Value>,
) -> Result<(), String> {
    validate_pointer(pointer)?;
    if let (Some(pointer), Some(value)) = (pointer, stored_value)
        && value.pointer(&format!("/data{pointer}")).is_none()
    {
        return Err(format!(
            "`{pointer}` does not identify a field in value.data"
        ));
    }
    Ok(())
}

fn nonempty(what: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{what} cannot be empty"));
    }
    Ok(())
}

#[cfg(test)]
mod decode_bound_tests {
    use super::*;

    #[test]
    fn collection_entries_are_refused_at_the_decode_bound() {
        let values = vec!["0"; MAX_STORED_COLLECTION_ENTRIES + 1].join(",");
        let text = format!(r#"{{"time_points":[],"values":[{values}]}}"#);
        let error = serde_json::from_str::<StoredTimeSeries<u8>>(&text).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("more than 65536 collection entries"),
            "{error}"
        );
    }

    #[test]
    fn operating_point_quantity_names_are_refused_at_the_decode_bound() {
        let quantities = (0..=MAX_STORED_OPERATING_POINT_QUANTITIES)
            .map(|index| format!(r#""q{index}":{{"identities":[],"values":[]}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let text = format!(r#"{{"network":null,"quantities":{{{quantities}}}}}"#);
        let error =
            serde_json::from_str::<StoredOperatingPoint<serde_json::Value>>(&text).unwrap_err();
        assert!(
            error.to_string().contains("more than 64 quantities"),
            "{error}"
        );
    }
}

//! The `.pio.json` version 1 wire: one stored document version, decoded by
//! exact typed DTOs after header dispatch. Runtime types never derive this
//! layout; the mapping in [`super::convert`] is the one bridge.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::BalancedNetwork;
use powerio_dist::MulticonductorNetwork;

#[cfg(feature = "schema")]
use schemars::JsonSchema;

pub const SCHEMA_NAME: &str = "powerio.module";
pub const SCHEMA_VERSION: u32 = 1;

/// JSON has no nonfinite number literals. PowerIO keeps the spellings already
/// shipped by 0.9 (`"Infinity"`, `"-Infinity"`, `"NaN"`) instead of turning
/// valid open bounds into `null`; `null` is not a number.
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
pub struct ProducerV1 {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct SourceIdV1(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct DiagnosticIdV1(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct HistoryIdV1(pub String);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TimePointV1 {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<DurationV1>,
}

/// A stored duration: unsigned seconds plus a nanosecond remainder below one
/// billion, exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DurationV1 {
    pub secs: u64,
    pub nanos: u32,
}

/// One state quantity's dense columns: the resolved identity order, then the
/// point major values (`time_points.len() × identities.len()`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredQuantityV1 {
    pub identities: Vec<String>,
    pub values: Vec<StoredF64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BalancedOperatingPointTimeSeriesV1 {
    pub network: Box<BalancedNetwork>,
    pub time_points: Vec<TimePointV1>,
    /// Quantity name → dense columns, the balanced instantaneous vocabulary.
    pub quantities: BTreeMap<String, StoredQuantityV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BalancedNetworkTimeSeriesV1 {
    pub time_points: Vec<TimePointV1>,
    pub values: Vec<BalancedNetwork>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BalancedNetworkScenarioV1 {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probability: Option<StoredF64>,
    pub value: BalancedNetwork,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BalancedNetworkScenarioSetV1 {
    pub scenarios: Vec<BalancedNetworkScenarioV1>,
}

/// The tagged value DTO. `data` is a typed record, never an untyped JSON
/// catchall; the network payloads are the typed serializations the network
/// crates own (which carry the nonfinite spellings themselves).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(
    deny_unknown_fields,
    tag = "kind",
    content = "data",
    rename_all = "snake_case"
)]
pub enum StoredValueV1 {
    BalancedNetwork(Box<BalancedNetwork>),
    MulticonductorNetwork(Box<MulticonductorNetwork>),
    BalancedNetworkTimeSeries(BalancedNetworkTimeSeriesV1),
    BalancedOperatingPointTimeSeries(BalancedOperatingPointTimeSeriesV1),
    BalancedNetworkScenarioSet(BalancedNetworkScenarioSetV1),
}

impl StoredValueV1 {
    /// The permanent kind identifier the tag serializes.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::BalancedNetwork(_) => "balanced_network",
            Self::MulticonductorNetwork(_) => "multiconductor_network",
            Self::BalancedNetworkTimeSeries(_) => "balanced_network_time_series",
            Self::BalancedOperatingPointTimeSeries(_) => "balanced_operating_point_time_series",
            Self::BalancedNetworkScenarioSet(_) => "balanced_network_scenario_set",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceDescriptorV1 {
    pub id: SourceIdV1,
    pub name: String,
    pub byte_length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<DigestV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithmV1 {
    Sha256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DigestV1 {
    pub algorithm: DigestAlgorithmV1,
    /// Lowercase hexadecimal, without a prefix.
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceSpanV1 {
    pub source: SourceIdV1,
    pub byte_start: u64,
    pub byte_end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceRelationV1 {
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
pub struct SourceMapEntryV1 {
    /// RFC 6901 pointer into `value.data`.
    pub target: String,
    pub relation: SourceRelationV1,
    /// Empty only for `defaulted`, `synthetic`, or `transformed`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<SourceSpanV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SeverityV1 {
    Error,
    Warning,
    Remark,
    Note,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DiagnosticV1 {
    pub id: DiagnosticIdV1,
    pub severity: SeverityV1,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<SourceSpanV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<DiagnosticIdV1>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum HistoryKindV1 {
    Parse,
    Upgrade,
    Transform,
    Edit,
    Repair,
}

/// Describes an operation that produced the current value. Not a replay
/// program; replayable revisions require their own typed value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HistoryEntryV1 {
    pub id: HistoryIdV1,
    pub kind: HistoryKindV1,
    /// Stable registered operation name.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_kind: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub losses: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredModuleV1 {
    pub schema: String,
    pub version: u32,
    pub producer: ProducerV1,
    pub value: StoredValueV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceDescriptorV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_map: Vec<SourceMapEntryV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<DiagnosticV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<HistoryEntryV1>,
    /// Nonsemantic third party annotations. Keys must be namespaced.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct StoredHeader {
    #[serde(default)]
    pub(super) schema: Option<String>,
    #[serde(default)]
    pub(super) version: Option<u32>,
    /// The 0.9 legacy shape identifies itself by this field instead.
    #[serde(default)]
    pub(super) powerio_version: Option<String>,
}

/// Structural validation of one decoded document: identities, digests,
/// spans, pointers, relation rules, namespaced extensions, and value shapes.
pub fn validate(module: &StoredModuleV1) -> Result<(), String> {
    validate_value(&module.value)?;
    // Pointer existence checks re-inflate the value only when something
    // actually targets it.
    let needs_targets = !module.source_map.is_empty()
        || module
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.target.is_some());
    let value_data = if needs_targets {
        let stored_value =
            serde_json::to_value(&module.value).map_err(|error| error.to_string())?;
        stored_value.get("data").cloned()
    } else {
        None
    };

    let mut sources = BTreeMap::new();
    for source in &module.sources {
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
    for diagnostic in &module.diagnostics {
        nonempty("diagnostic id", &diagnostic.id.0)?;
        if !diagnostics.insert(diagnostic.id.0.as_str()) {
            return Err(format!("duplicate diagnostic id `{}`", diagnostic.id.0));
        }
        validate_target(diagnostic.target.as_deref(), value_data.as_ref())?;
        validate_spans(&diagnostic.spans, &sources)?;
    }
    for diagnostic in &module.diagnostics {
        for related in &diagnostic.related {
            if !diagnostics.contains(related.0.as_str()) {
                return Err(format!(
                    "diagnostic `{}` refers to unknown diagnostic `{}`",
                    diagnostic.id.0, related.0
                ));
            }
        }
    }

    for entry in &module.source_map {
        validate_target(Some(&entry.target), value_data.as_ref())?;
        validate_spans(&entry.spans, &sources)?;
        let empty_allowed = matches!(
            entry.relation,
            SourceRelationV1::Defaulted
                | SourceRelationV1::Synthetic
                | SourceRelationV1::Transformed
        );
        if entry.spans.is_empty() && !empty_allowed {
            return Err(format!(
                "source map relation `{:?}` requires at least one span",
                entry.relation
            ));
        }
    }

    let mut history = BTreeSet::new();
    for entry in &module.history {
        nonempty("history id", &entry.id.0)?;
        if !history.insert(entry.id.0.as_str()) {
            return Err(format!("duplicate history id `{}`", entry.id.0));
        }
        nonempty("history operation name", &entry.name)?;
    }

    for namespace in module.extensions.keys() {
        if namespace.starts_with('.') || !namespace.contains('.') {
            return Err(format!("extension key `{namespace}` is not namespaced"));
        }
    }

    Ok(())
}

fn validate_value(value: &StoredValueV1) -> Result<(), String> {
    let points: &[TimePointV1] = match value {
        StoredValueV1::BalancedNetworkTimeSeries(series) => {
            if series.time_points.len() != series.values.len() {
                return Err(format!(
                    "balanced network time series has {} values for {} time points",
                    series.values.len(),
                    series.time_points.len()
                ));
            }
            if series.time_points.is_empty() {
                return Err("a time series needs at least one time point".to_owned());
            }
            &series.time_points
        }
        StoredValueV1::BalancedOperatingPointTimeSeries(series) => {
            if series.time_points.is_empty() {
                return Err("a time series needs at least one time point".to_owned());
            }
            for (name, quantity) in &series.quantities {
                let expected = series
                    .time_points
                    .len()
                    .checked_mul(quantity.identities.len())
                    .ok_or_else(|| format!("quantity `{name}` dimensions overflow"))?;
                if quantity.values.len() != expected {
                    return Err(format!(
                        "quantity `{name}` has {} values; expected {expected}",
                        quantity.values.len()
                    ));
                }
            }
            &series.time_points
        }
        StoredValueV1::BalancedNetworkScenarioSet(set) => {
            let mut ids = BTreeSet::new();
            for scenario in &set.scenarios {
                nonempty("scenario id", &scenario.id)?;
                if !ids.insert(scenario.id.as_str()) {
                    return Err(format!("duplicate scenario id `{}`", scenario.id));
                }
            }
            return Ok(());
        }
        StoredValueV1::BalancedNetwork(_) | StoredValueV1::MulticonductorNetwork(_) => {
            return Ok(());
        }
    };
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

fn validate_spans(spans: &[SourceSpanV1], sources: &BTreeMap<String, u64>) -> Result<(), String> {
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
    value_data: Option<&serde_json::Value>,
) -> Result<(), String> {
    validate_pointer(pointer)?;
    if let (Some(pointer), Some(data)) = (pointer, value_data)
        && data.pointer(pointer).is_none()
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

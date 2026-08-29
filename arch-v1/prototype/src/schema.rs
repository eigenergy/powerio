//! One stored module version. Runtime types do not derive this layout.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const SCHEMA_NAME: &str = "powerio.module";
pub const SCHEMA_VERSION: u32 = 1;

/// JSON has no nonfinite number literals. PowerIO keeps the spellings already
/// shipped by 0.9 instead of turning valid open bounds into `null`.
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

impl<'de> Visitor<'de> for StoredF64Visitor {
    type Value = StoredF64;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON number or \"Infinity\", \"-Infinity\", or \"NaN\"")
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(StoredF64(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StoredF64(value as f64))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProducerV1 {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceIdV1(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DiagnosticIdV1(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HistoryIdV1(pub String);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BalancedNetworkV1 {
    pub bus_ids: Vec<u64>,
    pub load_p: Vec<StoredF64>,
    pub upper_bounds: Vec<StoredF64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MulticonductorNetworkV1 {
    pub bus_ids: Vec<u64>,
    pub load_p: Vec<StoredF64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimePointV1 {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<DurationV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurationV1 {
    pub secs: u64,
    pub nanos: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BalancedNetworkTimeSeriesV1 {
    pub time_points: Vec<TimePointV1>,
    pub values: Vec<BalancedNetworkV1>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BalancedOperatingPointTimeSeriesV1 {
    pub network: BalancedNetworkV1,
    pub time_points: Vec<TimePointV1>,
    /// Point major, then bus major. Production replaces this toy column with
    /// the complete reviewed operating point columns.
    pub load_p: Vec<StoredF64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MulticonductorOperatingPointTimeSeriesV1 {
    pub network: MulticonductorNetworkV1,
    pub time_points: Vec<TimePointV1>,
    pub load_p: Vec<StoredF64>,
}

/// A tagged value DTO. `data` is never an untyped JSON catchall.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    deny_unknown_fields,
    tag = "kind",
    content = "data",
    rename_all = "snake_case"
)]
pub enum StoredValueV1 {
    BalancedNetwork(BalancedNetworkV1),
    MulticonductorNetwork(MulticonductorNetworkV1),
    BalancedNetworkTimeSeries(BalancedNetworkTimeSeriesV1),
    BalancedOperatingPointTimeSeries(BalancedOperatingPointTimeSeriesV1),
    MulticonductorOperatingPointTimeSeries(MulticonductorOperatingPointTimeSeriesV1),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithmV1 {
    Sha256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DigestV1 {
    pub algorithm: DigestAlgorithmV1,
    /// Lowercase hexadecimal, without a prefix.
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpanV1 {
    pub source: SourceIdV1,
    pub byte_start: u64,
    pub byte_end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[serde(rename_all = "snake_case")]
pub enum SeverityV1 {
    Error,
    Warning,
    Remark,
    Note,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
#[serde(rename_all = "snake_case")]
pub enum HistoryKindV1 {
    Parse,
    Upgrade,
    Transform,
    Edit,
    Repair,
}

/// Describes an operation that produced the current value. It is not a replay
/// program; replayable revisions require their own typed value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
struct StoredHeader {
    schema: String,
    version: u32,
}

#[derive(Debug)]
pub enum DecodeError {
    Json(serde_json::Error),
    Unsupported { schema: String, version: u32 },
    Invalid(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::Unsupported { schema, version } => {
                write!(f, "unsupported stored module `{schema}` version {version}")
            }
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<serde_json::Error> for DecodeError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

/// Dispatches on the document header before applying the exact version DTO.
pub fn decode(text: &str) -> Result<StoredModuleV1, DecodeError> {
    let header: StoredHeader = serde_json::from_str(text)?;
    if header.schema != SCHEMA_NAME || header.version != SCHEMA_VERSION {
        return Err(DecodeError::Unsupported {
            schema: header.schema,
            version: header.version,
        });
    }
    let module: StoredModuleV1 = serde_json::from_str(text)?;
    validate(&module)?;
    Ok(module)
}

pub fn validate(module: &StoredModuleV1) -> Result<(), DecodeError> {
    validate_value(&module.value)?;
    let stored_value = serde_json::to_value(&module.value)?;
    let value_data = stored_value
        .get("data")
        .ok_or_else(|| DecodeError::Invalid("stored value has no data field".to_owned()))?;

    let mut sources = BTreeMap::new();
    for source in &module.sources {
        nonempty("source id", &source.id.0)?;
        if sources
            .insert(source.id.0.as_str(), source.byte_length)
            .is_some()
        {
            return invalid(format!("duplicate source id `{}`", source.id.0));
        }
        nonempty("source name", &source.name)?;
        if let Some(digest) = &source.digest
            && (digest.value.len() != 64
                || !digest
                    .value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        {
            return invalid(format!("invalid SHA-256 `{}`", digest.value));
        }
    }

    let mut diagnostics = BTreeSet::new();
    for diagnostic in &module.diagnostics {
        nonempty("diagnostic id", &diagnostic.id.0)?;
        if !diagnostics.insert(diagnostic.id.0.as_str()) {
            return invalid(format!("duplicate diagnostic id `{}`", diagnostic.id.0));
        }
        validate_target(diagnostic.target.as_deref(), value_data)?;
        validate_spans(&diagnostic.spans, &sources)?;
    }
    for diagnostic in &module.diagnostics {
        for related in &diagnostic.related {
            if !diagnostics.contains(related.0.as_str()) {
                return invalid(format!(
                    "diagnostic `{}` refers to unknown diagnostic `{}`",
                    diagnostic.id.0, related.0
                ));
            }
        }
    }

    for entry in &module.source_map {
        validate_target(Some(&entry.target), value_data)?;
        validate_spans(&entry.spans, &sources)?;
        let empty_allowed = matches!(
            entry.relation,
            SourceRelationV1::Defaulted
                | SourceRelationV1::Synthetic
                | SourceRelationV1::Transformed
        );
        if entry.spans.is_empty() && !empty_allowed {
            return invalid(format!(
                "source map relation `{:?}` has the wrong span count",
                entry.relation
            ));
        }
    }

    let mut history = BTreeSet::new();
    for entry in &module.history {
        nonempty("history id", &entry.id.0)?;
        if !history.insert(entry.id.0.as_str()) {
            return invalid(format!("duplicate history id `{}`", entry.id.0));
        }
        nonempty("history operation name", &entry.name)?;
    }

    for namespace in module.extensions.keys() {
        if namespace.starts_with('.') || !namespace.contains('.') {
            return invalid(format!("extension key `{namespace}` is not namespaced"));
        }
    }

    Ok(())
}

fn validate_value(value: &StoredValueV1) -> Result<(), DecodeError> {
    let points: &[TimePointV1] = match value {
        StoredValueV1::BalancedNetworkTimeSeries(series) => {
            equal_len(
                "balanced network time series",
                series.time_points.len(),
                series.values.len(),
            )?;
            &series.time_points
        }
        StoredValueV1::BalancedOperatingPointTimeSeries(series) => {
            flat_len(
                "balanced operating point load_p",
                series.time_points.len(),
                series.network.bus_ids.len(),
                series.load_p.len(),
            )?;
            &series.time_points
        }
        StoredValueV1::MulticonductorOperatingPointTimeSeries(series) => {
            flat_len(
                "multiconductor operating point load_p",
                series.time_points.len(),
                series.network.bus_ids.len(),
                series.load_p.len(),
            )?;
            &series.time_points
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
            return invalid(format!(
                "time point `{}` has an invalid nanosecond remainder {}",
                point.label, duration.nanos
            ));
        }
    }
    Ok(())
}

fn equal_len(what: &str, expected: usize, actual: usize) -> Result<(), DecodeError> {
    if expected != actual {
        return invalid(format!(
            "{what} has {actual} values for {expected} time points"
        ));
    }
    Ok(())
}

fn flat_len(what: &str, rows: usize, columns: usize, actual: usize) -> Result<(), DecodeError> {
    let expected = rows
        .checked_mul(columns)
        .ok_or_else(|| DecodeError::Invalid(format!("{what} dimensions overflow")))?;
    if expected != actual {
        return invalid(format!("{what} has {actual} values; expected {expected}"));
    }
    Ok(())
}

fn validate_spans(
    spans: &[SourceSpanV1],
    sources: &BTreeMap<&str, u64>,
) -> Result<(), DecodeError> {
    for span in spans {
        let Some(byte_length) = sources.get(span.source.0.as_str()) else {
            return invalid(format!("unknown source id `{}`", span.source.0));
        };
        if span.byte_start > span.byte_end {
            return invalid(format!(
                "source span {}..{} is reversed",
                span.byte_start, span.byte_end
            ));
        }
        if span.byte_end > *byte_length {
            return invalid(format!(
                "source span end {} exceeds source length {}",
                span.byte_end, byte_length
            ));
        }
    }
    Ok(())
}

fn validate_pointer(pointer: Option<&str>) -> Result<(), DecodeError> {
    if let Some(pointer) = pointer {
        if !pointer.is_empty() && !pointer.starts_with('/') {
            return invalid(format!("`{pointer}` is not an RFC 6901 pointer"));
        }
        let bytes = pointer.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'~'
                && (index + 1 == bytes.len() || !matches!(bytes[index + 1], b'0' | b'1'))
            {
                return invalid(format!("`{pointer}` is not an RFC 6901 pointer"));
            }
            index += 1;
        }
    }
    Ok(())
}

fn validate_target(
    pointer: Option<&str>,
    value_data: &serde_json::Value,
) -> Result<(), DecodeError> {
    validate_pointer(pointer)?;
    if let Some(pointer) = pointer
        && value_data.pointer(pointer).is_none()
    {
        return invalid(format!(
            "`{pointer}` does not identify a field in value.data"
        ));
    }
    Ok(())
}

fn nonempty(what: &str, value: &str) -> Result<(), DecodeError> {
    if value.is_empty() {
        return invalid(format!("{what} cannot be empty"));
    }
    Ok(())
}

fn invalid<T>(message: String) -> Result<T, DecodeError> {
    Err(DecodeError::Invalid(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> StoredModuleV1 {
        StoredModuleV1 {
            schema: SCHEMA_NAME.to_owned(),
            version: SCHEMA_VERSION,
            producer: ProducerV1 {
                name: "powerio".to_owned(),
                version: "1.0.0".to_owned(),
            },
            value: StoredValueV1::BalancedNetwork(BalancedNetworkV1 {
                bus_ids: vec![1],
                load_p: vec![StoredF64(2.0)],
                upper_bounds: vec![StoredF64(f64::INFINITY)],
            }),
            sources: Vec::new(),
            source_map: Vec::new(),
            diagnostics: Vec::new(),
            history: Vec::new(),
            extensions: BTreeMap::new(),
        }
    }

    #[test]
    fn document_uses_one_version_and_one_value_kind() {
        let text = serde_json::to_string(&example()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["schema"], SCHEMA_NAME);
        assert_eq!(json["version"], SCHEMA_VERSION);
        assert_eq!(json["value"]["kind"], "balanced_network");
        assert!(json["value"].get("version").is_none());
        assert!(json.get("source").is_none());
    }

    #[test]
    fn nonfinite_bounds_round_trip_without_null() {
        let text = serde_json::to_string(&example()).unwrap();
        assert!(text.contains("\"Infinity\""));
        assert!(!text.contains("null"));
        let decoded = decode(&text).unwrap();
        let StoredValueV1::BalancedNetwork(network) = decoded.value else {
            panic!("wrong value kind")
        };
        assert_eq!(network.upper_bounds[0].0, f64::INFINITY);
    }

    #[test]
    fn header_dispatch_precedes_exact_field_checking() {
        let newer = r#"{
            "schema":"powerio.module",
            "version":2,
            "producer":{},
            "value":{},
            "future_field":true
        }"#;
        assert!(matches!(
            decode(newer),
            Err(DecodeError::Unsupported { .. })
        ));

        let mut current = serde_json::to_value(example()).unwrap();
        current
            .as_object_mut()
            .unwrap()
            .insert("unknown".to_owned(), serde_json::Value::Bool(true));
        assert!(matches!(
            decode(&serde_json::to_string(&current).unwrap()),
            Err(DecodeError::Json(_))
        ));

        let mut current = serde_json::to_value(example()).unwrap();
        current["value"]
            .as_object_mut()
            .unwrap()
            .insert("unknown".to_owned(), serde_json::Value::Bool(true));
        assert!(matches!(
            decode(&serde_json::to_string(&current).unwrap()),
            Err(DecodeError::Json(_))
        ));
    }

    #[test]
    fn references_and_source_map_shapes_are_checked() {
        let mut module = example();
        module.source_map.push(SourceMapEntryV1 {
            target: "/upper_bounds/0".to_owned(),
            relation: SourceRelationV1::Exact,
            spans: vec![SourceSpanV1 {
                source: SourceIdV1("missing".to_owned()),
                byte_start: 5,
                byte_end: 7,
            }],
        });
        assert!(matches!(validate(&module), Err(DecodeError::Invalid(_))));

        module.sources.push(SourceDescriptorV1 {
            id: SourceIdV1("missing".to_owned()),
            name: "case.m".to_owned(),
            byte_length: 100,
            format: Some("matpower".to_owned()),
            digest: Some(DigestV1 {
                algorithm: DigestAlgorithmV1::Sha256,
                value: "a".repeat(64),
            }),
        });
        validate(&module).unwrap();

        module.source_map[0].target = "/missing".to_owned();
        assert!(matches!(validate(&module), Err(DecodeError::Invalid(_))));
        module.source_map[0].target = "/upper_bounds/0".to_owned();

        module.source_map[0].spans[0].byte_end = 101;
        assert!(matches!(validate(&module), Err(DecodeError::Invalid(_))));
        module.source_map[0].spans[0].byte_end = 7;

        module.source_map[0].relation = SourceRelationV1::Transformed;
        validate(&module).unwrap();
        module.source_map[0].spans.clear();
        validate(&module).unwrap();
        module.source_map[0].relation = SourceRelationV1::Exact;
        assert!(matches!(validate(&module), Err(DecodeError::Invalid(_))));
    }
}

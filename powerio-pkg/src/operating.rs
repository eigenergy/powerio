//! Replayable operating point overlays for `.pio.json` packages.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::ModelPayload;

/// A format neutral series of operating points over a package's static payload.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OperatingPointSeries {
    pub time_axis: TimeAxis,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<OperatingPoint>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

impl OperatingPointSeries {
    #[must_use]
    pub fn new(time_axis: TimeAxis, points: Vec<OperatingPoint>) -> Self {
        Self {
            time_axis,
            points,
            metadata: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.time_axis.is_empty() && self.points.is_empty() && self.metadata.is_empty()
    }

    /// Return the first point with `index`.
    ///
    /// Use [`OperatingPointSeries::unique_point`] when duplicate indices must be
    /// rejected instead of collapsed.
    #[must_use]
    pub fn point(&self, index: usize) -> Option<&OperatingPoint> {
        self.points.iter().find(|point| point.index == index)
    }

    /// Return the only point with `index`, rejecting duplicate period indices.
    pub fn unique_point(&self, index: usize) -> serde_json::Result<Option<&OperatingPoint>> {
        let mut matches = self.points.iter().filter(|point| point.index == index);
        let first = matches.next();
        if matches.next().is_some() {
            return Err(<serde_json::Error as serde::de::Error>::custom(format!(
                "package has multiple operating points with index {index}"
            )));
        }
        Ok(first)
    }

    #[must_use]
    pub fn with_metadata(mut self, metadata: BTreeMap<String, Value>) -> Self {
        self.metadata = metadata;
        self
    }
}

/// The time axis shared by every operating point in the series.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TimeAxis {
    pub periods: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub duration_hours: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

impl TimeAxis {
    #[must_use]
    pub fn new(periods: usize) -> Self {
        Self {
            periods,
            duration_hours: Vec::new(),
            labels: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.periods == 0 && self.duration_hours.is_empty() && self.labels.is_empty()
    }

    #[must_use]
    pub fn with_duration_hours(mut self, duration_hours: Vec<f64>) -> Self {
        self.duration_hours = duration_hours;
        self
    }

    #[must_use]
    pub fn with_labels(mut self, labels: Vec<String>) -> Self {
        self.labels = labels;
        self
    }
}

/// One replayable operating state over the package's static payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OperatingPoint {
    /// Zero based period index.
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_hours: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updates: Vec<ElementUpdate>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

impl OperatingPoint {
    #[must_use]
    pub fn new(index: usize) -> Self {
        Self {
            index,
            label: None,
            duration_hours: None,
            updates: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }
}

/// A row in one table of the static payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ElementRef {
    pub table: String,
    /// Zero based row index in `table`.
    pub row: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_uid: Option<String>,
}

impl ElementRef {
    #[must_use]
    pub fn new(table: impl Into<String>, row: usize) -> Self {
        Self {
            table: table.into(),
            row,
            source_uid: None,
        }
    }

    #[must_use]
    pub fn with_source_uid(mut self, uid: impl Into<String>) -> Self {
        self.source_uid = Some(uid.into());
        self
    }
}

/// Field values to apply to one static payload row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ElementUpdate {
    pub element: ElementRef,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

impl ElementUpdate {
    #[must_use]
    pub fn new(element: ElementRef, fields: BTreeMap<String, Value>) -> Self {
        Self {
            element,
            fields,
            metadata: BTreeMap::new(),
        }
    }
}

pub(crate) fn goc3_operating_points_from_str(
    text: &str,
) -> serde_json::Result<Option<OperatingPointSeries>> {
    let root: Value = serde_json::from_str(text)?;
    let Some(root) = root.as_object() else {
        return Ok(None);
    };
    let Some(network) = root.get("network").and_then(Value::as_object) else {
        return Ok(None);
    };
    let Some(time_series) = root.get("time_series_input").and_then(Value::as_object) else {
        return Ok(None);
    };
    let Some(general) = time_series.get("general").and_then(Value::as_object) else {
        return Ok(None);
    };
    let periods = general
        .get("time_periods")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    if periods == 0 {
        return Ok(None);
    }
    let duration_hours = general
        .get("interval_duration")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect::<Vec<_>>())
        .unwrap_or_default();
    let device_ts = uid_map(section(time_series, "simple_dispatchable_device")?);
    let output = root.get("time_series_output").and_then(Value::as_object);

    let mut points = (0..periods)
        .map(|index| {
            let mut point = OperatingPoint::new(index);
            point.duration_hours = duration_hours.get(index).copied();
            point
        })
        .collect::<Vec<_>>();

    let base_mva = network
        .get("general")
        .and_then(Value::as_object)
        .and_then(|general| number(general, "base_norm_mva"))
        .unwrap_or(100.0);

    add_goc3_device_updates(network, &device_ts, base_mva, &mut points)?;
    add_goc3_status_updates(network, output, "ac_line", "branches", 0, &mut points)?;
    let line_count = section(network, "ac_line")?.len();
    add_goc3_status_updates(
        network,
        output,
        "two_winding_transformer",
        "branches",
        line_count,
        &mut points,
    )?;
    add_goc3_status_updates(network, output, "dc_line", "hvdc", 0, &mut points)?;

    Ok(Some(OperatingPointSeries {
        time_axis: TimeAxis {
            periods,
            duration_hours,
            labels: (0..periods).map(|idx| (idx + 1).to_string()).collect(),
        },
        points,
        metadata: BTreeMap::from([("source_format".to_owned(), json!("goc3-json"))]),
    }))
}

fn add_goc3_device_updates(
    network: &Map<String, Value>,
    device_ts: &HashMap<String, &Value>,
    base_mva: f64,
    points: &mut [OperatingPoint],
) -> serde_json::Result<()> {
    let mut producer_row = 0usize;
    let mut consumer_row = 0usize;
    for item in section(network, "simple_dispatchable_device")? {
        let Some(obj) = item.value.as_object() else {
            continue;
        };
        let Some(uid) = item_uid(item, obj) else {
            continue;
        };
        let device_type = obj
            .get("device_type")
            .and_then(Value::as_str)
            .unwrap_or("producer");
        let Some(ts) = device_ts
            .get(uid.as_str())
            .and_then(|value| value.as_object())
        else {
            match device_type {
                "producer" => producer_row += 1,
                "consumer" => consumer_row += 1,
                _ => {}
            }
            continue;
        };
        match device_type {
            "producer" => {
                for point in points.iter_mut() {
                    let mut fields = BTreeMap::new();
                    insert_scaled_at(&mut fields, ts, "p_ub", "pmax", point.index, base_mva);
                    insert_scaled_at(&mut fields, ts, "p_lb", "pmin", point.index, base_mva);
                    insert_scaled_at(&mut fields, ts, "q_ub", "qmax", point.index, base_mva);
                    insert_scaled_at(&mut fields, ts, "q_lb", "qmin", point.index, base_mva);
                    if let Some(cost) = goc3_cost_at(obj, ts, point.index, base_mva)? {
                        fields.insert("cost".to_owned(), cost);
                    }
                    if !fields.is_empty() {
                        let mut update = ElementUpdate::new(
                            ElementRef::new("generators", producer_row)
                                .with_source_uid(uid.clone()),
                            fields,
                        );
                        update.metadata = per_period_metadata(ts, point.index);
                        point.updates.push(update);
                    }
                }
                producer_row += 1;
            }
            "consumer" => {
                for point in points.iter_mut() {
                    let mut fields = BTreeMap::new();
                    insert_abs_scaled_at(&mut fields, ts, "p_ub", "p", point.index, base_mva);
                    insert_abs_scaled_at(&mut fields, ts, "q_ub", "q", point.index, base_mva);
                    if !fields.is_empty() {
                        let mut update = ElementUpdate::new(
                            ElementRef::new("loads", consumer_row).with_source_uid(uid.clone()),
                            fields,
                        );
                        update.metadata = per_period_metadata(ts, point.index);
                        point.updates.push(update);
                    }
                }
                consumer_row += 1;
            }
            _ => {}
        }
    }
    Ok(())
}

fn add_goc3_status_updates(
    network: &Map<String, Value>,
    output: Option<&Map<String, Value>>,
    source_section: &'static str,
    target_table: &'static str,
    row_offset: usize,
    points: &mut [OperatingPoint],
) -> serde_json::Result<()> {
    let source_items = section(network, source_section)?;
    let Some(output) = output else {
        return Ok(());
    };
    let status_by_uid = uid_map(section(output, source_section)?);
    for (row, item) in source_items.iter().enumerate() {
        let Some(obj) = item.value.as_object() else {
            continue;
        };
        let Some(uid) = item_uid(*item, obj) else {
            continue;
        };
        let Some(status) = status_by_uid
            .get(uid.as_str())
            .and_then(|value| value.as_object())
        else {
            continue;
        };
        for point in points.iter_mut() {
            if let Some(value) = array_number_at(status, "on_status", point.index) {
                point.updates.push(ElementUpdate::new(
                    ElementRef::new(target_table, row_offset + row).with_source_uid(uid.clone()),
                    BTreeMap::from([("in_service".to_owned(), json!(value != 0.0))]),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct SectionItem<'a> {
    key: Option<&'a str>,
    value: &'a Value,
}

fn section<'a>(
    parent: &'a Map<String, Value>,
    name: &'static str,
) -> serde_json::Result<Vec<SectionItem<'a>>> {
    let Some(value) = parent.get(name) else {
        return Ok(Vec::new());
    };
    match value {
        Value::Array(items) => Ok(items
            .iter()
            .map(|value| SectionItem { key: None, value })
            .collect()),
        Value::Object(map) => {
            let mut items: Vec<_> = map
                .iter()
                .map(|(key, value)| SectionItem {
                    key: Some(key.as_str()),
                    value,
                })
                .collect();
            items.sort_by(|a, b| compare_keys(a.key.unwrap_or(""), b.key.unwrap_or("")));
            Ok(items)
        }
        other => Err(json_error(format!(
            "`{name}` is not an array or object, got {}",
            kind(other)
        ))),
    }
}

fn item_uid(item: SectionItem<'_>, obj: &Map<String, Value>) -> Option<String> {
    obj.get("uid")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| item.key.map(str::to_owned))
        .filter(|uid| !uid.is_empty())
}

fn uid_map(items: Vec<SectionItem<'_>>) -> HashMap<String, &Value> {
    let mut out = HashMap::new();
    for item in items {
        if let Some(obj) = item.value.as_object()
            && let Some(uid) = item_uid(item, obj)
        {
            out.insert(uid, item.value);
        }
    }
    out
}

fn insert_scaled_at(
    fields: &mut BTreeMap<String, Value>,
    obj: &Map<String, Value>,
    source: &str,
    target: &str,
    index: usize,
    scale: f64,
) {
    if let Some(value) = array_number_at(obj, source, index) {
        fields.insert(target.to_owned(), json!(value * scale));
    }
}

fn insert_abs_scaled_at(
    fields: &mut BTreeMap<String, Value>,
    obj: &Map<String, Value>,
    source: &str,
    target: &str,
    index: usize,
    scale: f64,
) {
    if let Some(value) = array_number_at(obj, source, index) {
        fields.insert(target.to_owned(), json!(value.abs() * scale));
    }
}

fn array_number_at(obj: &Map<String, Value>, key: &str, index: usize) -> Option<f64> {
    obj.get(key)?.as_array()?.get(index)?.as_f64()
}

fn per_period_metadata(obj: &Map<String, Value>, index: usize) -> BTreeMap<String, Value> {
    let mut metadata = BTreeMap::new();
    for (key, value) in obj {
        if key == "cost" || key.ends_with("_ub") || key.ends_with("_lb") {
            continue;
        }
        if let Some(values) = value.as_array()
            && let Some(value) = values.get(index)
        {
            metadata.insert(key.clone(), value.clone());
        }
    }
    metadata
}

fn goc3_cost_at(
    device: &Map<String, Value>,
    ts: &Map<String, Value>,
    index: usize,
    base_mva: f64,
) -> serde_json::Result<Option<Value>> {
    let Some(periods) = ts.get("cost").and_then(Value::as_array) else {
        return Ok(None);
    };
    let Some(curve) = periods.get(index).and_then(Value::as_array) else {
        return Ok(None);
    };
    let mut coeffs = vec![0.0, 0.0];
    let mut p = 0.0;
    let mut y = 0.0;
    for segment in curve {
        let Some(values) = segment.as_array() else {
            continue;
        };
        let Some(marginal) = values.first().and_then(Value::as_f64) else {
            continue;
        };
        let Some(width) = values.get(1).and_then(Value::as_f64) else {
            continue;
        };
        if !marginal.is_finite() || !width.is_finite() || width <= 0.0 {
            continue;
        }
        p += width * base_mva;
        y += marginal * width;
        coeffs.push(p);
        coeffs.push(y);
    }
    if coeffs.len() < 4 {
        return Ok(None);
    }
    Ok(Some(serde_json::to_value(powerio::GenCost::new(
        1,
        number(device, "startup_cost").unwrap_or(0.0),
        number(device, "shutdown_cost").unwrap_or(0.0),
        coeffs,
    ))?))
}

fn number(obj: &Map<String, Value>, key: &str) -> Option<f64> {
    obj.get(key).and_then(Value::as_f64)
}

fn compare_keys(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn json_error(message: impl Into<String>) -> serde_json::Error {
    <serde_json::Error as serde::de::Error>::custom(message.into())
}

pub(crate) fn apply_operating_point_to_model(
    model: &ModelPayload,
    point: &OperatingPoint,
) -> serde_json::Result<ModelPayload> {
    let mut value = serde_json::to_value(model)?;
    let root = value.as_object_mut().ok_or_else(|| {
        <serde_json::Error as serde::de::Error>::custom("model payload did not serialize to object")
    })?;
    let payload_key = payload_key(model);
    let payload = root
        .get_mut(payload_key)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            <serde_json::Error as serde::de::Error>::custom(format!(
                "model payload missing `{payload_key}` object"
            ))
        })?;

    for update in &point.updates {
        apply_update(payload, update)?;
    }

    let updated = serde_json::from_value(value)?;
    validate_update_fields_survived(&updated, &point.updates)?;
    Ok(updated)
}

pub(crate) fn operating_point_update_paths(
    model: &ModelPayload,
    point: &OperatingPoint,
) -> BTreeSet<String> {
    let payload_key = payload_key(model);
    point
        .updates
        .iter()
        .flat_map(|update| {
            update.fields.keys().map(move |field| {
                format!(
                    "/model/{payload_key}/{}/{}/{}",
                    update.element.table, update.element.row, field
                )
            })
        })
        .collect()
}

fn payload_key(model: &ModelPayload) -> &'static str {
    match model {
        ModelPayload::Balanced { .. } => "balanced_network",
        ModelPayload::Multiconductor { .. } => "multiconductor_network",
    }
}

fn apply_update(
    payload: &mut serde_json::Map<String, Value>,
    update: &ElementUpdate,
) -> serde_json::Result<()> {
    let table_name = update.element.table.as_str();
    let table = payload
        .get_mut(table_name)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            <serde_json::Error as serde::de::Error>::custom(format!(
                "operating point table `{table_name}` is not present or is not an array"
            ))
        })?;
    let row = table
        .get_mut(update.element.row)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            <serde_json::Error as serde::de::Error>::custom(format!(
                "operating point table `{table_name}` has no object row {}",
                update.element.row
            ))
        })?;

    for (field, value) in &update.fields {
        row.insert(field.clone(), value.clone());
    }
    Ok(())
}

fn validate_update_fields_survived(
    model: &ModelPayload,
    updates: &[ElementUpdate],
) -> serde_json::Result<()> {
    let value = serde_json::to_value(model)?;
    let root = value.as_object().ok_or_else(|| {
        <serde_json::Error as serde::de::Error>::custom("model payload did not serialize to object")
    })?;
    let payload_key = payload_key(model);
    let payload = root
        .get(payload_key)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            <serde_json::Error as serde::de::Error>::custom(format!(
                "model payload missing `{payload_key}` object"
            ))
        })?;

    for update in updates {
        let table_name = update.element.table.as_str();
        let table = payload
            .get(table_name)
            .and_then(Value::as_array)
            .ok_or_else(|| {
                <serde_json::Error as serde::de::Error>::custom(format!(
                    "operating point table `{table_name}` is not present after typed materialization"
                ))
            })?;
        let row = table
            .get(update.element.row)
            .and_then(Value::as_object)
            .ok_or_else(|| {
                <serde_json::Error as serde::de::Error>::custom(format!(
                    "operating point table `{table_name}` has no object row {} after typed materialization",
                    update.element.row
                ))
            })?;

        for field in update.fields.keys() {
            if !row.contains_key(field) {
                return Err(<serde_json::Error as serde::de::Error>::custom(format!(
                    "operating point field `{field}` is not present on table `{table_name}` row {}",
                    update.element.row
                )));
            }
        }
    }
    Ok(())
}

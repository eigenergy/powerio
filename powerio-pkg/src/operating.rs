//! Replayable operating point overlays for `.pio.json` packages.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use powerio::{Goc3DeviceKind, Goc3Document, Goc3Record};

use crate::model::ModelPayload;

/// A format neutral series of operating points over a package's static payload.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct OperatingPointSeries {
    /// Shared period count, durations, and labels.
    pub time_axis: TimeAxis,
    /// Ordered operating states. Each state is addressed by its `index`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<OperatingPoint>,
    /// Metadata from the source format, such as `source_format`.
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct TimeAxis {
    /// Number of periods available in the series.
    pub periods: usize,
    /// Optional duration per period, in hours.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub duration_hours: Vec<f64>,
    /// Optional display labels for the periods.
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct OperatingPoint {
    /// Zero based period index. Labels and durations live on the shared
    /// [`TimeAxis`], indexed by this.
    pub index: usize,
    /// Field updates to apply to the static payload.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updates: Vec<ElementUpdate>,
    /// Metadata from the source format for this point.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

impl OperatingPoint {
    #[must_use]
    pub fn new(index: usize) -> Self {
        Self {
            index,
            updates: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }
}

/// A row in one table of the static payload.
///
/// `source_uid` is the row's payload identity: when the referenced table
/// carries `uid` values, a present `source_uid` resolves the target row and a
/// present `row` must agree with it. In a table without uids (packages written
/// before payload identity existed), `source_uid` is advisory and `row`
/// addresses the update alone. On the wire, `row` may be omitted when
/// `source_uid` is given.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(transform = element_ref_schema))]
#[non_exhaustive]
pub struct ElementRef {
    /// Payload table name, such as `loads`, `generators`, `branches`, or `hvdc`.
    pub table: String,
    /// Zero based row index in `table`, when the producer addressed one.
    /// `None` on refs built by [`ElementRef::by_source_uid`], which address by
    /// identity alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<usize>,
    /// The row's payload identity (its `uid` field), when the producer knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_uid: Option<String>,
}

impl ElementRef {
    #[must_use]
    pub fn new(table: impl Into<String>, row: usize) -> Self {
        Self {
            table: table.into(),
            row: Some(row),
            source_uid: None,
        }
    }

    /// Address a row by payload identity alone; no `row` is serialized.
    #[must_use]
    pub fn by_source_uid(table: impl Into<String>, uid: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            row: None,
            source_uid: Some(uid.into()),
        }
    }

    #[must_use]
    pub fn with_source_uid(mut self, uid: impl Into<String>) -> Self {
        self.source_uid = Some(uid.into());
        self
    }
}

#[cfg(feature = "schema")]
fn element_ref_schema(schema: &mut schemars::Schema) {
    schema.ensure_object().insert(
        "anyOf".to_owned(),
        json!([
            {
                "required": ["row"],
                "properties": {
                    "row": {
                        "format": "uint",
                        "minimum": 0,
                        "type": "integer"
                    }
                }
            },
            {
                "required": ["source_uid"],
                "properties": {
                    "source_uid": { "type": "string" }
                }
            }
        ]),
    );
}

impl<'de> Deserialize<'de> for ElementRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        struct Wire {
            table: String,
            #[serde(default)]
            row: Option<usize>,
            #[serde(default)]
            source_uid: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.row.is_none() && wire.source_uid.is_none() {
            return Err(serde::de::Error::custom(
                "element ref needs `row` or `source_uid`",
            ));
        }
        Ok(Self {
            table: wire.table,
            row: wire.row,
            source_uid: wire.source_uid,
        })
    }
}

/// Field values to apply to one static payload row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ElementUpdate {
    /// Table row to update.
    pub element: ElementRef,
    /// JSON field values to overwrite on that row.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
    /// Metadata from the source format for this update.
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
    let document =
        powerio::Goc3Document::parse(text).map_err(|error| json_error(error.to_string()))?;
    let network = document
        .network()
        .map_err(|error| json_error(error.to_string()))?;
    let time_series = document
        .time_series_input()
        .map_err(|error| json_error(error.to_string()))?;
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
    let device_ts = uid_map(
        document
            .time_series_input_records("simple_dispatchable_device")
            .map_err(|error| json_error(error.to_string()))?,
    );

    let mut points = (0..periods).map(OperatingPoint::new).collect::<Vec<_>>();

    let base_mva = network
        .get("general")
        .and_then(Value::as_object)
        .and_then(|general| general.get("base_norm_mva"))
        .and_then(Value::as_f64)
        .unwrap_or(100.0);

    add_goc3_device_updates(&document, &device_ts, base_mva, &mut points)?;
    add_goc3_status_updates(&document, "ac_line", "branches", 0, &mut points)?;
    let line_count = document
        .network_records("ac_line")
        .map_err(|error| json_error(error.to_string()))?
        .len();
    add_goc3_status_updates(
        &document,
        "two_winding_transformer",
        "branches",
        line_count,
        &mut points,
    )?;
    add_goc3_status_updates(&document, "dc_line", "hvdc", 0, &mut points)?;

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
    document: &Goc3Document,
    device_ts: &HashMap<String, &Value>,
    base_mva: f64,
    points: &mut [OperatingPoint],
) -> serde_json::Result<()> {
    for device in document
        .dispatchable_devices()
        .map_err(|error| json_error(error.to_string()))?
    {
        let Some(uid) = device.uid else {
            continue;
        };
        let Some(ts_value) = device_ts.get(uid.as_str()) else {
            continue;
        };
        let Some(ts) = ts_value.as_object() else {
            continue;
        };
        match device.kind {
            Goc3DeviceKind::Generators => {
                for point in points.iter_mut() {
                    let mut fields = BTreeMap::new();
                    insert_scaled_at(&mut fields, ts, "p_ub", "pmax", point.index, base_mva);
                    insert_scaled_at(&mut fields, ts, "p_lb", "pmin", point.index, base_mva);
                    insert_scaled_at(&mut fields, ts, "q_ub", "qmax", point.index, base_mva);
                    insert_scaled_at(&mut fields, ts, "q_lb", "qmin", point.index, base_mva);
                    if let Some(cost) = document
                        .dispatchable_device_cost_at(
                            device.obj,
                            Some(ts_value),
                            point.index,
                            base_mva,
                        )
                        .map(serde_json::to_value)
                        .transpose()?
                    {
                        fields.insert("cost".to_owned(), cost);
                    }
                    if !fields.is_empty() {
                        let mut update = ElementUpdate::new(
                            ElementRef::new("generators", device.row).with_source_uid(uid.clone()),
                            fields,
                        );
                        update.metadata = per_period_metadata(ts, point.index);
                        point.updates.push(update);
                    }
                }
            }
            Goc3DeviceKind::Loads => {
                for point in points.iter_mut() {
                    let mut fields = BTreeMap::new();
                    insert_abs_scaled_at(&mut fields, ts, "p_ub", "p", point.index, base_mva);
                    insert_abs_scaled_at(&mut fields, ts, "q_ub", "q", point.index, base_mva);
                    if !fields.is_empty() {
                        let mut update = ElementUpdate::new(
                            ElementRef::new("loads", device.row).with_source_uid(uid.clone()),
                            fields,
                        );
                        update.metadata = per_period_metadata(ts, point.index);
                        point.updates.push(update);
                    }
                }
            }
        }
    }
    Ok(())
}

fn add_goc3_status_updates(
    document: &Goc3Document,
    source_section: &'static str,
    target_table: &'static str,
    row_offset: usize,
    points: &mut [OperatingPoint],
) -> serde_json::Result<()> {
    let source_items = document
        .network_records(source_section)
        .map_err(|error| json_error(error.to_string()))?;
    if document.time_series_output().is_none() {
        return Ok(());
    }
    let status_by_uid = uid_map(
        document
            .time_series_output_records(source_section)
            .map_err(|error| json_error(error.to_string()))?,
    );
    for (row, item) in source_items.iter().enumerate() {
        let Some(uid) = item.uid.as_ref() else {
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

fn uid_map(items: Vec<Goc3Record<'_>>) -> HashMap<String, &Value> {
    let mut out = HashMap::new();
    for item in items {
        if let Some(uid) = item.uid {
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

pub(crate) fn json_error(message: impl Into<String>) -> serde_json::Error {
    <serde_json::Error as serde::de::Error>::custom(message.into())
}

/// Apply one operating point to the payload and return the updated model plus
/// the JSON Pointer paths of every field written, computed from the resolved
/// rows so stale provenance cleanup follows identity resolution, never a stale
/// wire row.
pub(crate) fn apply_operating_point_to_model(
    model: &ModelPayload,
    point: &OperatingPoint,
) -> serde_json::Result<(ModelPayload, BTreeSet<String>)> {
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

    let mut indexes = HashMap::new();
    let mut resolved_rows = Vec::with_capacity(point.updates.len());
    for update in &point.updates {
        let row = resolve_update(payload, &mut indexes, update).map_err(json_error)?;
        apply_update_fields(payload, &update.element.table, row, &update.fields)?;
        resolved_rows.push(row);
    }

    let updated_paths = point
        .updates
        .iter()
        .zip(&resolved_rows)
        .flat_map(|(update, row)| {
            update.fields.keys().map(move |field| {
                format!(
                    "/model/{payload_key}/{}/{row}/{}",
                    update.element.table, field
                )
            })
        })
        .collect();

    let updated = serde_json::from_value(value)?;
    validate_update_fields_survived(&updated, &point.updates, &resolved_rows)?;
    Ok((updated, updated_paths))
}

/// Dry run identity resolution over a whole series, returning `(point_position,
/// update_position, message)` for every update that fails to resolve. The
/// payload is serialized once and the per table indexes are shared across the
/// series.
pub(crate) fn check_series_identities(
    model: &ModelPayload,
    series: &OperatingPointSeries,
) -> Vec<(usize, usize, String)> {
    let payload_key = payload_key(model);
    let payload = match serde_json::to_value(model) {
        Ok(Value::Object(mut root)) => match root.remove(payload_key) {
            Some(Value::Object(payload)) => payload,
            _ => {
                return vec![(
                    0,
                    0,
                    format!("model payload missing `{payload_key}` object"),
                )];
            }
        },
        _ => return vec![(0, 0, "model payload did not serialize to object".to_owned())],
    };

    let mut indexes = HashMap::new();
    let mut findings = Vec::new();
    for (point_pos, point) in series.points.iter().enumerate() {
        for (update_pos, update) in point.updates.iter().enumerate() {
            if let Err(message) = resolve_update(&payload, &mut indexes, update) {
                findings.push((point_pos, update_pos, message));
            }
        }
    }
    findings
}

pub(crate) fn payload_key(model: &ModelPayload) -> &'static str {
    match model {
        ModelPayload::Balanced { .. } => "balanced_network",
        ModelPayload::Multiconductor { .. } => "multiconductor_network",
    }
}

/// The uid -> row index for one payload table.
pub(crate) struct IdentityIndex {
    by_uid: HashMap<String, usize>,
    /// Uids on more than one row; resolving through one is ambiguous.
    duplicates: BTreeSet<String>,
    /// Whether any row carries a uid. A table with none keeps the row-only
    /// semantics packages had before payload identity existed.
    has_uids: bool,
}

fn table_identity_index(table: &[Value]) -> IdentityIndex {
    let mut by_uid = HashMap::with_capacity(table.len());
    let mut duplicates = BTreeSet::new();
    let mut has_uids = false;
    for (row, value) in table.iter().enumerate() {
        let Some(uid) = value.get("uid").and_then(Value::as_str) else {
            continue;
        };
        has_uids = true;
        if by_uid.insert(uid.to_owned(), row).is_some() {
            duplicates.insert(uid.to_owned());
        }
    }
    IdentityIndex {
        by_uid,
        duplicates,
        has_uids,
    }
}

/// Resolve one update to its payload row, first rejecting any update that would
/// rewrite `uid`. Identity is immutable: letting a field write change it would
/// invalidate the per table indexes mid application.
pub(crate) fn resolve_update(
    payload: &Map<String, Value>,
    indexes: &mut HashMap<String, IdentityIndex>,
    update: &ElementUpdate,
) -> Result<usize, String> {
    if update.fields.contains_key("uid") {
        return Err(format!(
            "operating point update on table `{}` must not overwrite `uid`",
            update.element.table
        ));
    }
    resolve_update_row(payload, indexes, &update.element)
}

/// Resolve one element ref to a payload row. A `source_uid` that resolves in a
/// uid bearing table is authoritative and a present wire `row` must agree with
/// it; an unknown or duplicated uid in such a table is an error; a table without
/// uids falls back to the wire row.
pub(crate) fn resolve_update_row(
    payload: &Map<String, Value>,
    indexes: &mut HashMap<String, IdentityIndex>,
    element: &ElementRef,
) -> Result<usize, String> {
    let table_name = element.table.as_str();
    let Some(table) = payload.get(table_name).and_then(Value::as_array) else {
        return Err(format!(
            "operating point table `{table_name}` is not present or is not an array"
        ));
    };
    let index = indexes
        .entry(table_name.to_owned())
        .or_insert_with(|| table_identity_index(table));
    let resolved = match element.source_uid.as_deref() {
        Some(uid) if index.duplicates.contains(uid) => {
            return Err(format!(
                "payload table `{table_name}` carries uid `{uid}` on more than one row; \
                 identity resolution is ambiguous"
            ));
        }
        Some(uid) => match index.by_uid.get(uid) {
            Some(&row) => {
                if let Some(wire_row) = element.row
                    && wire_row != row
                {
                    return Err(format!(
                        "update for table `{table_name}` names uid `{uid}` (row {row}) \
                         but carries row {wire_row}"
                    ));
                }
                row
            }
            None if index.has_uids => {
                return Err(format!(
                    "unknown identity: table `{table_name}` has no row with uid `{uid}`"
                ));
            }
            None => element.row.ok_or_else(|| {
                format!(
                    "update for table `{table_name}` names uid `{uid}`, but the payload rows \
                     carry no uids and the update has no row to fall back on"
                )
            })?,
        },
        None => element.row.ok_or_else(|| {
            format!("update for table `{table_name}` has neither row nor source_uid")
        })?,
    };
    if resolved >= table.len() {
        return Err(format!(
            "operating point table `{table_name}` has no row {resolved}"
        ));
    }
    Ok(resolved)
}

pub(crate) fn apply_update_fields(
    payload: &mut serde_json::Map<String, Value>,
    table_name: &str,
    row: usize,
    fields: &BTreeMap<String, Value>,
) -> serde_json::Result<()> {
    let row_object = payload
        .get_mut(table_name)
        .and_then(Value::as_array_mut)
        .and_then(|table| table.get_mut(row))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            json_error(format!(
                "operating point table `{table_name}` has no object row {row}"
            ))
        })?;
    for (field, value) in fields {
        row_object.insert(field.clone(), value.clone());
    }
    Ok(())
}

pub(crate) fn validate_update_fields_survived(
    model: &ModelPayload,
    updates: &[ElementUpdate],
    resolved_rows: &[usize],
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

    for (update, &resolved_row) in updates.iter().zip(resolved_rows) {
        let table_name = update.element.table.as_str();
        let row = payload
            .get(table_name)
            .and_then(Value::as_array)
            .and_then(|table| table.get(resolved_row))
            .and_then(Value::as_object)
            .ok_or_else(|| {
                json_error(format!(
                    "operating point table `{table_name}` has no object row {resolved_row} \
                     after typed materialization"
                ))
            })?;

        for field in update.fields.keys() {
            if !row.contains_key(field) {
                return Err(json_error(format!(
                    "operating point field `{field}` is not present on table `{table_name}` \
                     row {resolved_row}"
                )));
            }
        }
    }
    Ok(())
}

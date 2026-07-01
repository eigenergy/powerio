//! Replayable operating point overlays for `.pio.json` packages.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

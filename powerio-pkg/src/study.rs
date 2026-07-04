//! Cumulative study edits for `.pio.json` packages.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize, de::Error as _};
use serde_json::{Map, Value, json};

use crate::model::ModelPayload;
use crate::operating::{
    ElementRef, ElementUpdate, IdentityIndex, apply_update_fields, json_error, payload_key,
    resolve_update, resolve_update_row, validate_update_fields_survived,
};

/// Additive study block stored on a package envelope.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StudyBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_operating_point: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<StudyCommit>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub app: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

impl StudyBlock {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.label.is_none()
            && self.author.is_none()
            && self.created_at.is_none()
            && self.base_operating_point.is_none()
            && self.commits.is_empty()
            && self.app.is_empty()
            && self.metadata.is_empty()
    }
}

/// One cumulative commit in a study block.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StudyCommit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<StudyEdit>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

/// One study edit. Unknown edit kinds are preserved and rejected only when a
/// caller tries to materialize them.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum StudyEdit {
    DemandDelta {
        bus: ElementRef,
        p_mw: f64,
        q_mvar: Option<f64>,
    },
    RatingDelta {
        branch: ElementRef,
        delta_mw: f64,
    },
    SetFields {
        update: ElementUpdate,
    },
    Unknown {
        kind: String,
        value: Value,
    },
}

impl StudyEdit {
    #[must_use]
    pub fn kind(&self) -> &str {
        match self {
            Self::DemandDelta { .. } => "demand_delta",
            Self::RatingDelta { .. } => "rating_delta",
            Self::SetFields { .. } => "set_fields",
            Self::Unknown { kind, .. } => kind,
        }
    }
}

impl Serialize for StudyEdit {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::DemandDelta { bus, p_mw, q_mvar } => {
                #[derive(Serialize)]
                struct Wire<'a> {
                    kind: &'static str,
                    bus: &'a ElementRef,
                    p_mw: f64,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    q_mvar: Option<f64>,
                }
                Wire {
                    kind: "demand_delta",
                    bus,
                    p_mw: *p_mw,
                    q_mvar: *q_mvar,
                }
                .serialize(serializer)
            }
            Self::RatingDelta { branch, delta_mw } => {
                #[derive(Serialize)]
                struct Wire<'a> {
                    kind: &'static str,
                    branch: &'a ElementRef,
                    delta_mw: f64,
                }
                Wire {
                    kind: "rating_delta",
                    branch,
                    delta_mw: *delta_mw,
                }
                .serialize(serializer)
            }
            Self::SetFields { update } => {
                #[derive(Serialize)]
                struct Wire<'a> {
                    kind: &'static str,
                    update: &'a ElementUpdate,
                }
                Wire {
                    kind: "set_fields",
                    update,
                }
                .serialize(serializer)
            }
            Self::Unknown { value, .. } => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for StudyEdit {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("study edit must be an object"))?;
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("study edit needs string `kind`"))?;
        match kind {
            "demand_delta" => {
                #[derive(Deserialize)]
                struct Wire {
                    bus: ElementRef,
                    p_mw: f64,
                    #[serde(default)]
                    q_mvar: Option<f64>,
                }
                let wire = serde_json::from_value::<Wire>(value).map_err(D::Error::custom)?;
                Ok(Self::DemandDelta {
                    bus: wire.bus,
                    p_mw: wire.p_mw,
                    q_mvar: wire.q_mvar,
                })
            }
            "rating_delta" => {
                #[derive(Deserialize)]
                struct Wire {
                    branch: ElementRef,
                    delta_mw: f64,
                }
                let wire = serde_json::from_value::<Wire>(value).map_err(D::Error::custom)?;
                Ok(Self::RatingDelta {
                    branch: wire.branch,
                    delta_mw: wire.delta_mw,
                })
            }
            "set_fields" => {
                #[derive(Deserialize)]
                struct Wire {
                    update: ElementUpdate,
                }
                let wire = serde_json::from_value::<Wire>(value).map_err(D::Error::custom)?;
                Ok(Self::SetFields {
                    update: wire.update,
                })
            }
            other => Ok(Self::Unknown {
                kind: other.to_owned(),
                value,
            }),
        }
    }
}

/// Apply commits `0..=commit_index` to a balanced model and return the updated
/// model plus JSON Pointer paths touched by the edits.
pub(crate) fn apply_study_to_model(
    model: &ModelPayload,
    study: &StudyBlock,
    commit_index: usize,
) -> serde_json::Result<(ModelPayload, BTreeSet<String>)> {
    if !matches!(model, ModelPayload::Balanced { .. }) {
        return Err(json_error(
            "STUDY.WRONG_MODEL_KIND: study materialization requires a balanced package",
        ));
    }
    if study.commits.get(commit_index).is_none() {
        return Err(json_error(format!(
            "package has no study commit {commit_index}"
        )));
    }

    let mut value = serde_json::to_value(model)?;
    let payload_key = payload_key(model);
    let payload = value
        .as_object_mut()
        .and_then(|root| root.get_mut(payload_key))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| json_error(format!("model payload missing `{payload_key}` object")))?;

    let mut indexes = HashMap::new();
    let mut updated_paths = BTreeSet::new();
    let mut set_field_updates = Vec::new();
    let mut set_field_rows = Vec::new();
    let mut context = StudyApplyContext {
        payload,
        payload_key,
        indexes: &mut indexes,
        updated_paths: &mut updated_paths,
        set_field_updates: &mut set_field_updates,
        set_field_rows: &mut set_field_rows,
    };
    for (commit_pos, commit) in study.commits.iter().take(commit_index + 1).enumerate() {
        for (edit_pos, edit) in commit.edits.iter().enumerate() {
            context.apply_edit(edit, commit_pos, edit_pos)?;
        }
    }

    let updated = serde_json::from_value(value)?;
    validate_update_fields_survived(&updated, &set_field_updates, &set_field_rows)?;
    Ok((updated, updated_paths))
}

/// Dry run identity resolution for every known study edit.
pub(crate) fn check_study_identities(
    model: &ModelPayload,
    study: &StudyBlock,
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
    for (commit_pos, commit) in study.commits.iter().enumerate() {
        for (edit_pos, edit) in commit.edits.iter().enumerate() {
            let result = match edit {
                StudyEdit::DemandDelta { bus, .. } => {
                    resolve_update_row(&payload, &mut indexes, bus).map(|_| ())
                }
                StudyEdit::RatingDelta { branch, .. } => {
                    resolve_update_row(&payload, &mut indexes, branch).map(|_| ())
                }
                StudyEdit::SetFields { update } => {
                    resolve_update(&payload, &mut indexes, update).map(|_| ())
                }
                StudyEdit::Unknown { .. } => Ok(()),
            };
            if let Err(message) = result {
                findings.push((commit_pos, edit_pos, message));
            }
        }
    }
    findings
}

struct StudyApplyContext<'a> {
    payload: &'a mut Map<String, Value>,
    payload_key: &'a str,
    indexes: &'a mut HashMap<String, IdentityIndex>,
    updated_paths: &'a mut BTreeSet<String>,
    set_field_updates: &'a mut Vec<ElementUpdate>,
    set_field_rows: &'a mut Vec<usize>,
}

impl StudyApplyContext<'_> {
    fn apply_edit(
        &mut self,
        edit: &StudyEdit,
        commit_pos: usize,
        edit_pos: usize,
    ) -> serde_json::Result<()> {
        match edit {
            StudyEdit::DemandDelta { bus, p_mw, q_mvar } => {
                let bus_row =
                    resolve_update_row(self.payload, self.indexes, bus).map_err(json_error)?;
                let touched = apply_demand_delta(self.payload, bus_row, *p_mw, *q_mvar)?;
                for path in touched {
                    self.updated_paths
                        .insert(format!("/model/{}/{path}", self.payload_key));
                }
                self.indexes.remove("loads");
            }
            StudyEdit::RatingDelta { branch, delta_mw } => {
                let branch_row =
                    resolve_update_row(self.payload, self.indexes, branch).map_err(json_error)?;
                let branch = row_object_mut(self.payload, "branches", branch_row)?;
                let old = number_field(branch, "rate_a", "branch", branch_row)?;
                branch.insert("rate_a".to_owned(), json!(old + delta_mw));
                self.updated_paths.insert(format!(
                    "/model/{}/branches/{branch_row}/rate_a",
                    self.payload_key
                ));
            }
            StudyEdit::SetFields { update } => {
                let row = resolve_update(self.payload, self.indexes, update).map_err(json_error)?;
                apply_update_fields(self.payload, &update.element.table, row, &update.fields)?;
                for field in update.fields.keys() {
                    self.updated_paths.insert(format!(
                        "/model/{}/{}/{row}/{field}",
                        self.payload_key, update.element.table
                    ));
                }
                self.set_field_updates.push(update.clone());
                self.set_field_rows.push(row);
            }
            StudyEdit::Unknown { kind, .. } => {
                return Err(json_error(format!(
                    "STUDY.UNKNOWN_EDIT_KIND: study commit {commit_pos} edit {edit_pos} has unsupported kind `{kind}`"
                )));
            }
        }
        Ok(())
    }
}

fn apply_demand_delta(
    payload: &mut Map<String, Value>,
    bus_row: usize,
    p_delta: f64,
    q_delta: Option<f64>,
) -> serde_json::Result<Vec<String>> {
    let (bus_id, bus_uid) = {
        let bus = row_object(payload, "buses", bus_row)?;
        let bus_id = bus
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| json_error(format!("bus row {bus_row} has no numeric `id`")))?;
        let bus_uid = bus
            .get("uid")
            .and_then(Value::as_str)
            .map_or_else(|| format!("buses:{bus_row}"), str::to_owned);
        (bus_id, bus_uid)
    };

    let loads = payload
        .get_mut("loads")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| json_error("balanced payload has no `loads` array"))?;
    let mut rows = Vec::new();
    let mut total_p = 0.0;
    let mut total_q = 0.0;
    for (row, load) in loads.iter().enumerate() {
        let Some(load) = load.as_object() else {
            continue;
        };
        if load.get("bus").and_then(Value::as_u64) != Some(bus_id) {
            continue;
        }
        if !load
            .get("in_service")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            continue;
        }
        let p = load.get("p").and_then(Value::as_f64).unwrap_or(0.0);
        let q = load.get("q").and_then(Value::as_f64).unwrap_or(0.0);
        rows.push((row, p, q));
        total_p += p;
        total_q += q;
    }

    if rows.is_empty() || total_p == 0.0 {
        let q = q_delta.unwrap_or(0.0);
        loads.push(json!({
            "bus": bus_id,
            "p": p_delta,
            "q": q,
            "voltage_model": null,
            "in_service": true,
            "uid": format!("study:load:{bus_uid}"),
            "extras": {
                "study": {
                    "synthetic": true,
                    "source": "demand_delta"
                }
            }
        }));
        let row = loads.len() - 1;
        return Ok(vec![
            format!("loads/{row}/p"),
            format!("loads/{row}/q"),
            format!("loads/{row}/uid"),
            format!("loads/{row}/extras"),
        ]);
    }

    let q_delta = q_delta.unwrap_or_else(|| p_delta * total_q / total_p);
    let mut touched = Vec::new();
    for (row, p, q) in rows {
        let p_share = p / total_p;
        let q_share = if total_q == 0.0 { p_share } else { q / total_q };
        let load = loads
            .get_mut(row)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| json_error(format!("load row {row} disappeared")))?;
        load.insert("p".to_owned(), json!(p + p_delta * p_share));
        load.insert("q".to_owned(), json!(q + q_delta * q_share));
        touched.push(format!("loads/{row}/p"));
        touched.push(format!("loads/{row}/q"));
    }
    Ok(touched)
}

fn row_object<'a>(
    payload: &'a Map<String, Value>,
    table_name: &str,
    row: usize,
) -> serde_json::Result<&'a Map<String, Value>> {
    payload
        .get(table_name)
        .and_then(Value::as_array)
        .and_then(|table| table.get(row))
        .and_then(Value::as_object)
        .ok_or_else(|| json_error(format!("table `{table_name}` has no object row {row}")))
}

fn row_object_mut<'a>(
    payload: &'a mut Map<String, Value>,
    table_name: &str,
    row: usize,
) -> serde_json::Result<&'a mut Map<String, Value>> {
    payload
        .get_mut(table_name)
        .and_then(Value::as_array_mut)
        .and_then(|table| table.get_mut(row))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| json_error(format!("table `{table_name}` has no object row {row}")))
}

fn number_field(
    object: &Map<String, Value>,
    field: &str,
    label: &str,
    row: usize,
) -> serde_json::Result<f64> {
    object
        .get(field)
        .and_then(Value::as_f64)
        .ok_or_else(|| json_error(format!("{label} row {row} has no numeric `{field}` field")))
}

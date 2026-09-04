use std::collections::{BTreeSet, HashMap, HashSet};

use powerio_tx::__internal::{Goc3Document, Goc3Record};
use serde_json::{Map, Value};

use super::error::{Goc3Error, Goc3Result};

type Result<T> = Goc3Result<T>;

pub(super) fn json_error(message: impl Into<String>) -> Goc3Error {
    Goc3Error::invalid(message)
}

fn rd(err: &powerio_tx::Error) -> Goc3Error {
    json_error(err.to_string())
}

// ---------------------------------------------------------------------------
// Raw-value extraction. `src/goc3.jl` mostly reads JSON5-via-JSON3 values
// straight into `Float64`/`Int`/`String` fields; these helpers do the same
// from `serde_json::Value`, erroring with the field name on a shape mismatch
// instead of Julia's `KeyError`/`MethodError`.
// ---------------------------------------------------------------------------

pub(super) fn require_num(obj: &Map<String, Value>, key: &str) -> Result<f64> {
    obj.get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| json_error(format!("missing numeric field `{key}`")))
}

pub(super) fn require_i64(obj: &Map<String, Value>, key: &str) -> Result<i64> {
    obj.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| json_error(format!("missing integer field `{key}`")))
}

pub(super) fn require_binary(obj: &Map<String, Value>, key: &str) -> Result<bool> {
    match obj.get(key).and_then(Value::as_u64) {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(json_error(format!("missing binary integer field `{key}`"))),
    }
}

pub(super) fn require_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| json_error(format!("missing string field `{key}`")))
}

/// Look up a required field on a row identified by `what`/`uid` (e.g. `what
/// = "simple_dispatchable_device time series"`), for the fields
/// `float_vec`/`float_matrix`/`cost_cube` parse themselves so the "missing
/// field" message names the row, not just the key.
pub(super) fn require_field<'a>(
    obj: &'a Map<String, Value>,
    what: &str,
    uid: &str,
    key: &str,
) -> Result<&'a Value> {
    obj.get(key)
        .ok_or_else(|| json_error(format!("{what} `{uid}` missing `{key}`")))
}

pub(super) fn float_vec(value: &Value) -> Result<Vec<f64>> {
    value
        .as_array()
        .ok_or_else(|| json_error("expected an array of numbers"))?
        .iter()
        .map(|v| v.as_f64().ok_or_else(|| json_error("expected a number")))
        .collect()
}

pub(super) fn float_matrix(value: &Value) -> Result<Vec<Vec<f64>>> {
    value
        .as_array()
        .ok_or_else(|| json_error("expected an array of arrays"))?
        .iter()
        .map(float_vec)
        .collect()
}

pub(super) fn float_pair(value: &Value) -> Result<[f64; 2]> {
    match float_vec(value)?[..] {
        [a, b] => Ok([a, b]),
        ref other => Err(json_error(format!(
            "expected a 2-element `[c_en, p_max]` cost block, got {} elements",
            other.len()
        ))),
    }
}

/// One device's multi-period cost cube: `cost[t][m]` is the 2-element
/// `[c_en, p_max]` price block `m` of period `t` (`_float_cube` applied to a
/// GOC3 device's `cost` time series in `src/goc3.jl`). Each block is a fixed
/// 2-element pair, enforced by the return type, so the price block projection
/// reads it without a bounds check.
pub(super) fn cost_cube(value: &Value) -> Result<Vec<Vec<[f64; 2]>>> {
    value
        .as_array()
        .ok_or_else(|| json_error("expected an array of cost periods"))?
        .iter()
        .map(|period| {
            period
                .as_array()
                .ok_or_else(|| json_error("expected an array of cost blocks"))?
                .iter()
                .map(float_pair)
                .collect()
        })
        .collect()
}

pub(super) fn initial_status(obj: &Map<String, Value>) -> Result<&Map<String, Value>> {
    obj.get("initial_status")
        .and_then(Value::as_object)
        .ok_or_else(|| json_error("missing object field `initial_status`"))
}

// ---------------------------------------------------------------------------
// Document tables used while projecting the validated input into typed records.
// ---------------------------------------------------------------------------

/// One GOC3 section's rows, keyed by `uid`. `uids()` preserves the source
/// document order from [`Goc3Document`]; every projection index derives from
/// that one order.
#[derive(Clone, Debug, Default)]
pub(super) struct Goc3Section {
    order: Vec<String>,
    rows: HashMap<String, Map<String, Value>>,
}

impl Goc3Section {
    fn from_items(items: Vec<Goc3Record<'_>>, what: &str) -> Result<Self> {
        let mut order = Vec::with_capacity(items.len());
        let mut rows = HashMap::with_capacity(items.len());
        for item in items {
            let obj = item
                .value
                .as_object()
                .ok_or_else(|| json_error(format!("{what} item is not an object")))?;
            let uid = item
                .uid
                .ok_or_else(|| json_error(format!("{what} item missing `uid`")))?;
            if rows.insert(uid.clone(), obj.clone()).is_some() {
                return Err(json_error(format!("duplicate {what} uid `{uid}`")));
            }
            order.push(uid);
        }
        Ok(Self { order, rows })
    }

    pub(super) fn get(&self, uid: &str) -> Result<&Map<String, Value>> {
        self.rows
            .get(uid)
            .ok_or_else(|| json_error(format!("unknown uid `{uid}`")))
    }

    pub(super) fn uids(&self) -> &[String] {
        &self.order
    }
}

/// Reject an empty section, naming its full dotted path (e.g.
/// `network.bus`) in the error: [`Goc3Section`] itself only knows the bare
/// section name.
fn require_nonempty<'a>(items: Vec<Goc3Record<'a>>, path: &str) -> Result<Vec<Goc3Record<'a>>> {
    if items.is_empty() {
        return Err(json_error(format!("missing non-empty `{path}`")));
    }
    Ok(items)
}

#[derive(Clone, Copy)]
enum RequiredKind {
    String,
    Number,
    Integer,
    Binary,
    Object,
    Array,
    StringArray,
    NumberArray,
    BinaryArray,
}

fn require_input_field<'a>(
    object: &'a Map<String, Value>,
    path: &str,
    key: &str,
    kind: RequiredKind,
) -> Result<&'a Value> {
    let value = object
        .get(key)
        .ok_or_else(|| json_error(format!("missing required `{path}.{key}`")))?;
    let valid = match kind {
        RequiredKind::String => value.is_string(),
        RequiredKind::Number => value.as_f64().is_some_and(f64::is_finite),
        RequiredKind::Integer => value.as_i64().is_some(),
        RequiredKind::Binary => matches!(value.as_u64(), Some(0 | 1)),
        RequiredKind::Object => value.is_object(),
        RequiredKind::Array => value.is_array(),
        RequiredKind::StringArray => value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string)),
        RequiredKind::NumberArray => value.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| item.as_f64().is_some_and(f64::is_finite))
        }),
        RequiredKind::BinaryArray => value.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| matches!(item.as_u64(), Some(0 | 1)))
        }),
    };
    if valid {
        Ok(value)
    } else {
        Err(json_error(format!(
            "required `{path}.{key}` has the wrong JSON type or value"
        )))
    }
}

fn require_input_fields(
    object: &Map<String, Value>,
    path: &str,
    fields: &[(&str, RequiredKind)],
) -> Result<()> {
    for &(key, kind) in fields {
        require_input_field(object, path, key, kind)?;
    }
    Ok(())
}

fn reject_unknown_fields(object: &Map<String, Value>, path: &str, allowed: &[&str]) -> Result<()> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(json_error(format!(
            "`{path}` contains unknown field `{field}`"
        )));
    }
    Ok(())
}

fn validate_optional_field(
    object: &Map<String, Value>,
    path: &str,
    key: &str,
    kind: RequiredKind,
) -> Result<()> {
    if object.contains_key(key) {
        require_input_field(object, path, key, kind)?;
    }
    Ok(())
}

fn require_input_object<'a>(
    parent: &'a Map<String, Value>,
    parent_path: &str,
    key: &str,
) -> Result<&'a Map<String, Value>> {
    parent
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| json_error(format!("missing required object `{parent_path}.{key}`")))
}

fn require_input_section(parent: &Map<String, Value>, parent_path: &str, key: &str) -> Result<()> {
    match parent.get(key) {
        Some(Value::Array(_)) => Ok(()),
        Some(_) => Err(json_error(format!(
            "required `{parent_path}.{key}` is not an array"
        ))),
        None => Err(json_error(format!(
            "missing required section `{parent_path}.{key}`"
        ))),
    }
}

fn record_object<'a>(
    record: &'a Goc3Record<'_>,
    path: &str,
    index: usize,
) -> Result<&'a Map<String, Value>> {
    record
        .value
        .as_object()
        .ok_or_else(|| json_error(format!("`{path}[{index}]` is not an object")))
}

fn record_path(path: &str, record: &Goc3Record<'_>, index: usize) -> String {
    record.uid.as_ref().map_or_else(
        || format!("{path}[{index}]"),
        |uid| format!("{path}[{uid}]"),
    )
}

fn validate_numeric_windows(
    value: &Value,
    path: &str,
    width: usize,
    integer_last: bool,
) -> Result<()> {
    let rows = value
        .as_array()
        .ok_or_else(|| json_error(format!("`{path}` is not an array")))?;
    for (index, row) in rows.iter().enumerate() {
        let fields = row
            .as_array()
            .ok_or_else(|| json_error(format!("`{path}[{index}]` is not an array")))?;
        if fields.len() != width {
            return Err(json_error(format!(
                "`{path}[{index}]` has {} fields; expected {width}",
                fields.len()
            )));
        }
        for (field_index, field) in fields.iter().enumerate() {
            let valid = if integer_last && field_index + 1 == width {
                field.as_u64().is_some()
            } else {
                field.as_f64().is_some_and(f64::is_finite)
            };
            if !valid {
                return Err(json_error(format!(
                    "`{path}[{index}][{field_index}]` has the wrong JSON type or value"
                )));
            }
        }
    }
    Ok(())
}

fn finite_number(object: &Map<String, Value>, path: &str, field: &str) -> Result<f64> {
    require_input_field(object, path, field, RequiredKind::Number)?
        .as_f64()
        .ok_or_else(|| json_error(format!("`{path}.{field}` is not a finite number")))
}

fn require_nonnegative(object: &Map<String, Value>, path: &str, field: &str) -> Result<()> {
    if finite_number(object, path, field)? < 0.0 {
        return Err(json_error(format!("`{path}.{field}` must be nonnegative")));
    }
    Ok(())
}

fn require_nonnegative_array(object: &Map<String, Value>, path: &str, field: &str) -> Result<()> {
    let values = require_input_field(object, path, field, RequiredKind::NumberArray)?
        .as_array()
        .expect("the number array check above established an array");
    if let Some((index, value)) = values
        .iter()
        .enumerate()
        .find(|(_, value)| value.as_f64().is_none_or(|value| value < 0.0))
    {
        return Err(json_error(format!(
            "`{path}.{field}[{index}]` must be nonnegative, found {value}"
        )));
    }
    Ok(())
}

fn validate_bounded_initial(
    path: &str,
    lower_name: &str,
    lower: f64,
    initial_name: &str,
    initial: f64,
    upper_name: &str,
    upper: f64,
) -> Result<()> {
    if lower > initial || initial > upper {
        return Err(json_error(format!(
            "`{path}` requires {lower_name} <= {initial_name} <= {upper_name}; found {lower}, {initial}, {upper}"
        )));
    }
    Ok(())
}

fn validate_time_windows(value: &Value, path: &str, integer_limit: bool) -> Result<()> {
    let rows = value
        .as_array()
        .expect("the numeric window shape check established an array");
    for (index, row) in rows.iter().enumerate() {
        let fields = row
            .as_array()
            .expect("the numeric window shape check established rows");
        let start = fields[0]
            .as_f64()
            .expect("the numeric window shape check established a number");
        let end = fields[1]
            .as_f64()
            .expect("the numeric window shape check established a number");
        if start < 0.0 || end < start {
            return Err(json_error(format!(
                "`{path}[{index}]` requires 0 <= start time <= end time; found {start}, {end}"
            )));
        }
        let limit_is_nonnegative = if integer_limit {
            fields[2].as_u64().is_some()
        } else {
            fields[2].as_f64().is_some_and(|limit| limit >= 0.0)
        };
        if !limit_is_nonnegative {
            return Err(json_error(format!(
                "`{path}[{index}][2]` must be nonnegative"
            )));
        }
    }
    Ok(())
}

fn validate_conditional_numbers(
    object: &Map<String, Value>,
    path: &str,
    flag: &str,
    fields: &[&str],
) -> Result<()> {
    let enabled = require_input_field(object, path, flag, RequiredKind::Binary)?
        .as_u64()
        .expect("the binary check above established an integer")
        == 1;
    for &field in fields {
        match (enabled, object.get(field)) {
            (true, _) => {
                require_input_field(object, path, field, RequiredKind::Number)?;
            }
            (false, Some(_)) => {
                return Err(json_error(format!(
                    "`{path}.{field}` is present while `{path}.{flag}` is 0"
                )));
            }
            (false, None) => {}
        }
    }
    Ok(())
}

fn validate_bus_rows(document: &Goc3Document) -> Result<()> {
    for (index, record) in document
        .network_records("bus")
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let path = record_path("network.bus", record, index);
        let object = record_object(record, "network.bus", index)?;
        reject_unknown_fields(
            object,
            &path,
            &[
                "uid",
                "vm_ub",
                "vm_lb",
                "active_reserve_uids",
                "reactive_reserve_uids",
                "area",
                "zone",
                "longitude",
                "latitude",
                "city",
                "county",
                "state",
                "country",
                "con_loss_factor",
                "base_nom_volt",
                "type",
                "initial_status",
            ],
        )?;
        require_input_fields(
            object,
            &path,
            &[
                ("uid", RequiredKind::String),
                ("vm_ub", RequiredKind::Number),
                ("vm_lb", RequiredKind::Number),
                ("active_reserve_uids", RequiredKind::StringArray),
                ("reactive_reserve_uids", RequiredKind::StringArray),
                ("base_nom_volt", RequiredKind::Number),
                ("initial_status", RequiredKind::Object),
            ],
        )?;
        let initial = require_input_object(object, &path, "initial_status")?;
        reject_unknown_fields(initial, &format!("{path}.initial_status"), &["vm", "va"])?;
        require_input_fields(
            initial,
            &format!("{path}.initial_status"),
            &[("vm", RequiredKind::Number), ("va", RequiredKind::Number)],
        )?;
        let vm_min = finite_number(object, &path, "vm_lb")?;
        let vm_max = finite_number(object, &path, "vm_ub")?;
        let vm_initial = finite_number(initial, &format!("{path}.initial_status"), "vm")?;
        if vm_min <= 0.0 {
            return Err(json_error(format!(
                "`{path}.vm_lb` must be greater than zero"
            )));
        }
        validate_bounded_initial(
            &path,
            "vm_lb",
            vm_min,
            "initial_status.vm",
            vm_initial,
            "vm_ub",
            vm_max,
        )?;
        for field in ["area", "zone", "city", "county", "state", "country", "type"] {
            validate_optional_field(object, &path, field, RequiredKind::String)?;
        }
        for field in ["longitude", "latitude", "con_loss_factor"] {
            validate_optional_field(object, &path, field, RequiredKind::Number)?;
        }
    }
    Ok(())
}

fn validate_shunt_rows(document: &Goc3Document) -> Result<()> {
    for (index, record) in document
        .network_records("shunt")
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let path = record_path("network.shunt", record, index);
        let object = record_object(record, "network.shunt", index)?;
        reject_unknown_fields(
            object,
            &path,
            &[
                "uid",
                "bus",
                "gs",
                "bs",
                "step_ub",
                "step_lb",
                "initial_status",
            ],
        )?;
        require_input_fields(
            object,
            &path,
            &[
                ("uid", RequiredKind::String),
                ("bus", RequiredKind::String),
                ("gs", RequiredKind::Number),
                ("bs", RequiredKind::Number),
                ("step_ub", RequiredKind::Integer),
                ("step_lb", RequiredKind::Integer),
                ("initial_status", RequiredKind::Object),
            ],
        )?;
        let initial = require_input_object(object, &path, "initial_status")?;
        reject_unknown_fields(initial, &format!("{path}.initial_status"), &["step"])?;
        require_input_field(
            initial,
            &format!("{path}.initial_status"),
            "step",
            RequiredKind::Integer,
        )?;
        let lower = require_i64(object, "step_lb")?;
        let upper = require_i64(object, "step_ub")?;
        let initial_step = require_i64(initial, "step")?;
        if lower < 0 || lower > initial_step || initial_step > upper {
            return Err(json_error(format!(
                "`{path}` requires 0 <= step_lb <= initial_status.step <= step_ub"
            )));
        }
    }
    Ok(())
}

#[allow(
    clippy::float_cmp,
    clippy::items_after_statements,
    clippy::too_many_lines
)]
fn validate_device_rows(document: &Goc3Document, periods: usize) -> Result<()> {
    const STATIC_NUMBERS: &[&str] = &[
        "startup_cost",
        "shutdown_cost",
        "on_cost",
        "in_service_time_lb",
        "down_time_lb",
        "p_ramp_up_ub",
        "p_ramp_down_ub",
        "p_startup_ramp_ub",
        "p_shutdown_ramp_ub",
        "p_reg_res_up_ub",
        "p_reg_res_down_ub",
        "p_syn_res_ub",
        "p_nsyn_res_ub",
        "p_ramp_res_up_online_ub",
        "p_ramp_res_down_online_ub",
        "p_ramp_res_up_offline_ub",
        "p_ramp_res_down_offline_ub",
    ];
    for (index, record) in document
        .network_records("simple_dispatchable_device")
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let path = record_path("network.simple_dispatchable_device", record, index);
        let object = record_object(record, "network.simple_dispatchable_device", index)?;
        reject_unknown_fields(
            object,
            &path,
            &[
                "uid",
                "bus",
                "device_type",
                "description",
                "vm_setpoint",
                "nameplate_capacity",
                "startup_cost",
                "startup_states",
                "shutdown_cost",
                "startups_ub",
                "energy_req_ub",
                "energy_req_lb",
                "on_cost",
                "in_service_time_lb",
                "down_time_lb",
                "p_ramp_up_ub",
                "p_ramp_down_ub",
                "p_startup_ramp_ub",
                "p_shutdown_ramp_ub",
                "initial_status",
                "q_linear_cap",
                "q_bound_cap",
                "p_reg_res_up_ub",
                "p_reg_res_down_ub",
                "p_syn_res_ub",
                "p_nsyn_res_ub",
                "p_ramp_res_up_online_ub",
                "p_ramp_res_down_online_ub",
                "p_ramp_res_up_offline_ub",
                "p_ramp_res_down_offline_ub",
                "q_0",
                "beta",
                "q_0_ub",
                "q_0_lb",
                "beta_ub",
                "beta_lb",
            ],
        )?;
        require_input_fields(
            object,
            &path,
            &[
                ("uid", RequiredKind::String),
                ("bus", RequiredKind::String),
                ("device_type", RequiredKind::String),
                ("startup_states", RequiredKind::Array),
                ("startups_ub", RequiredKind::Array),
                ("energy_req_ub", RequiredKind::Array),
                ("energy_req_lb", RequiredKind::Array),
                ("initial_status", RequiredKind::Object),
                ("q_linear_cap", RequiredKind::Binary),
                ("q_bound_cap", RequiredKind::Binary),
            ],
        )?;
        if !matches!(
            object.get("device_type").and_then(Value::as_str),
            Some("producer" | "consumer")
        ) {
            return Err(json_error(format!(
                "`{path}.device_type` must be `producer` or `consumer`"
            )));
        }
        for &field in STATIC_NUMBERS {
            require_input_field(object, &path, field, RequiredKind::Number)?;
        }
        validate_optional_field(object, &path, "description", RequiredKind::String)?;
        validate_optional_field(object, &path, "vm_setpoint", RequiredKind::Number)?;
        validate_optional_field(object, &path, "nameplate_capacity", RequiredKind::Number)?;
        validate_numeric_windows(
            &object["startup_states"],
            &format!("{path}.startup_states"),
            2,
            false,
        )?;
        validate_numeric_windows(
            &object["startups_ub"],
            &format!("{path}.startups_ub"),
            3,
            true,
        )?;
        validate_time_windows(&object["startups_ub"], &format!("{path}.startups_ub"), true)?;
        validate_numeric_windows(
            &object["energy_req_ub"],
            &format!("{path}.energy_req_ub"),
            3,
            false,
        )?;
        validate_time_windows(
            &object["energy_req_ub"],
            &format!("{path}.energy_req_ub"),
            false,
        )?;
        validate_numeric_windows(
            &object["energy_req_lb"],
            &format!("{path}.energy_req_lb"),
            3,
            false,
        )?;
        validate_time_windows(
            &object["energy_req_lb"],
            &format!("{path}.energy_req_lb"),
            false,
        )?;
        let initial = require_input_object(object, &path, "initial_status")?;
        reject_unknown_fields(
            initial,
            &format!("{path}.initial_status"),
            &["on_status", "p", "q", "accu_down_time", "accu_up_time"],
        )?;
        require_input_fields(
            initial,
            &format!("{path}.initial_status"),
            &[
                ("on_status", RequiredKind::Binary),
                ("p", RequiredKind::Number),
                ("q", RequiredKind::Number),
                ("accu_down_time", RequiredKind::Number),
                ("accu_up_time", RequiredKind::Number),
            ],
        )?;
        for field in [
            "in_service_time_lb",
            "down_time_lb",
            "p_ramp_up_ub",
            "p_ramp_down_ub",
            "p_startup_ramp_ub",
            "p_shutdown_ramp_ub",
            "p_reg_res_up_ub",
            "p_reg_res_down_ub",
            "p_syn_res_ub",
            "p_nsyn_res_ub",
            "p_ramp_res_up_online_ub",
            "p_ramp_res_down_online_ub",
            "p_ramp_res_up_offline_ub",
            "p_ramp_res_down_offline_ub",
        ] {
            require_nonnegative(object, &path, field)?;
        }
        let on_status = require_input_field(
            initial,
            &format!("{path}.initial_status"),
            "on_status",
            RequiredKind::Binary,
        )?
        .as_u64()
        .expect("the binary check above established an integer");
        let accumulated_down =
            finite_number(initial, &format!("{path}.initial_status"), "accu_down_time")?;
        let accumulated_up =
            finite_number(initial, &format!("{path}.initial_status"), "accu_up_time")?;
        if accumulated_down < 0.0 || accumulated_up < 0.0 {
            return Err(json_error(format!(
                "`{path}.initial_status` accumulated up and down times must be nonnegative"
            )));
        }
        let commitment_consistent = match on_status {
            1 => accumulated_up > 0.0 && accumulated_down == 0.0,
            0 => accumulated_down > 0.0 && accumulated_up == 0.0,
            _ => unreachable!("the binary check above established 0 or 1"),
        };
        if !commitment_consistent {
            return Err(json_error(format!(
                "`{path}.initial_status` is inconsistent: on_status={on_status}, accu_up_time={accumulated_up}, accu_down_time={accumulated_down}"
            )));
        }
        if object["q_linear_cap"].as_u64() == Some(1) && object["q_bound_cap"].as_u64() == Some(1) {
            return Err(json_error(format!(
                "`{path}` reactive capability forms are mutually exclusive"
            )));
        }
        validate_conditional_numbers(object, &path, "q_linear_cap", &["q_0", "beta"])?;
        validate_conditional_numbers(
            object,
            &path,
            "q_bound_cap",
            &["q_0_ub", "q_0_lb", "beta_ub", "beta_lb"],
        )?;
        if object["q_bound_cap"].as_u64() == Some(1) {
            let q_at_zero_max = finite_number(object, &path, "q_0_ub")?;
            let q_at_zero_min = finite_number(object, &path, "q_0_lb")?;
            let slope_max = finite_number(object, &path, "beta_ub")?;
            let slope_min = finite_number(object, &path, "beta_lb")?;
            if slope_max == slope_min && q_at_zero_min > q_at_zero_max {
                return Err(json_error(format!(
                    "`{path}` bounded reactive capability is empty: beta_lb equals beta_ub but q_0_lb exceeds q_0_ub"
                )));
            }
        }
    }

    const SERIES_FIELDS: &[(&str, RequiredKind)] = &[
        ("uid", RequiredKind::String),
        ("on_status_ub", RequiredKind::BinaryArray),
        ("on_status_lb", RequiredKind::BinaryArray),
        ("p_ub", RequiredKind::NumberArray),
        ("p_lb", RequiredKind::NumberArray),
        ("q_ub", RequiredKind::NumberArray),
        ("q_lb", RequiredKind::NumberArray),
        ("cost", RequiredKind::Array),
        ("p_reg_res_up_cost", RequiredKind::NumberArray),
        ("p_reg_res_down_cost", RequiredKind::NumberArray),
        ("p_syn_res_cost", RequiredKind::NumberArray),
        ("p_nsyn_res_cost", RequiredKind::NumberArray),
        ("p_ramp_res_up_online_cost", RequiredKind::NumberArray),
        ("p_ramp_res_down_online_cost", RequiredKind::NumberArray),
        ("p_ramp_res_up_offline_cost", RequiredKind::NumberArray),
        ("p_ramp_res_down_offline_cost", RequiredKind::NumberArray),
        ("q_res_up_cost", RequiredKind::NumberArray),
        ("q_res_down_cost", RequiredKind::NumberArray),
    ];
    for (index, record) in document
        .time_series_input_records("simple_dispatchable_device")
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let path = record_path(
            "time_series_input.simple_dispatchable_device",
            record,
            index,
        );
        let object = record_object(
            record,
            "time_series_input.simple_dispatchable_device",
            index,
        )?;
        reject_unknown_fields(
            object,
            &path,
            &[
                "uid",
                "on_status_ub",
                "on_status_lb",
                "p_ub",
                "p_lb",
                "q_ub",
                "q_lb",
                "cost",
                "p_reg_res_up_cost",
                "p_reg_res_down_cost",
                "p_syn_res_cost",
                "p_nsyn_res_cost",
                "p_ramp_res_up_online_cost",
                "p_ramp_res_down_online_cost",
                "p_ramp_res_up_offline_cost",
                "p_ramp_res_down_offline_cost",
                "q_res_up_cost",
                "q_res_down_cost",
            ],
        )?;
        require_input_fields(object, &path, SERIES_FIELDS)?;
        for &(field, kind) in SERIES_FIELDS {
            if matches!(kind, RequiredKind::NumberArray | RequiredKind::BinaryArray)
                && object[field].as_array().map_or(0, Vec::len) != periods
            {
                return Err(json_error(format!(
                    "`{path}.{field}` has {} periods; expected {periods}",
                    object[field].as_array().map_or(0, Vec::len)
                )));
            }
        }
        if object["cost"].as_array().map_or(0, Vec::len) != periods {
            return Err(json_error(format!(
                "`{path}.cost` has {} periods; expected {periods}",
                object["cost"].as_array().map_or(0, Vec::len)
            )));
        }
        let costs = object["cost"]
            .as_array()
            .expect("the cost check above established an array");
        let active_min = object["p_lb"]
            .as_array()
            .expect("the number array check above established an array");
        let active_max = object["p_ub"]
            .as_array()
            .expect("the number array check above established an array");
        let reactive_min = object["q_lb"]
            .as_array()
            .expect("the number array check above established an array");
        let reactive_max = object["q_ub"]
            .as_array()
            .expect("the number array check above established an array");
        for period in 0..periods {
            let p_min = active_min[period]
                .as_f64()
                .expect("the number array check above established numbers");
            let p_max = active_max[period]
                .as_f64()
                .expect("the number array check above established numbers");
            let q_min = reactive_min[period]
                .as_f64()
                .expect("the number array check above established numbers");
            let q_max = reactive_max[period]
                .as_f64()
                .expect("the number array check above established numbers");
            if p_min < 0.0 || p_min > p_max {
                return Err(json_error(format!(
                    "`{path}` period {period} requires 0 <= p_lb <= p_ub; found {p_min}, {p_max}"
                )));
            }
            if q_min > q_max {
                return Err(json_error(format!(
                    "`{path}` period {period} requires q_lb <= q_ub; found {q_min}, {q_max}"
                )));
            }
            let blocks = costs[period]
                .as_array()
                .ok_or_else(|| json_error(format!("`{path}.cost[{period}]` is not an array")))?;
            let mut covered_active_power = 0.0;
            for (block_index, block) in blocks.iter().enumerate() {
                let pair = block.as_array().ok_or_else(|| {
                    json_error(format!(
                        "`{path}.cost[{period}][{block_index}]` is not an array"
                    ))
                })?;
                if pair.len() != 2
                    || pair
                        .iter()
                        .any(|value| value.as_f64().is_none_or(|value| !value.is_finite()))
                {
                    return Err(json_error(format!(
                        "`{path}.cost[{period}][{block_index}]` must contain exactly two finite numbers"
                    )));
                }
                let block_size = pair[1]
                    .as_f64()
                    .expect("the finite pair check above established a number");
                if block_size < 0.0 {
                    return Err(json_error(format!(
                        "`{path}.cost[{period}][{block_index}][1]` must be nonnegative"
                    )));
                }
                covered_active_power += block_size;
            }
            if covered_active_power < p_max {
                return Err(json_error(format!(
                    "`{path}.cost[{period}]` covers {covered_active_power} per unit active power but p_ub is {p_max}"
                )));
            }
        }
        let lower = object["on_status_lb"]
            .as_array()
            .expect("input document validation established an array");
        let upper = object["on_status_ub"]
            .as_array()
            .expect("input document validation established an array");
        if lower
            .iter()
            .zip(upper)
            .any(|(lower, upper)| lower.as_u64() > upper.as_u64())
        {
            return Err(json_error(format!(
                "`{path}` has on_status_lb greater than on_status_ub"
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_ac_branch_rows(
    document: &Goc3Document,
    section: &'static str,
    transformer: bool,
) -> Result<()> {
    for (index, record) in document
        .network_records(section)
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let base_path = format!("network.{section}");
        let path = record_path(&base_path, record, index);
        let object = record_object(record, &base_path, index)?;
        let allowed: &[&str] = if transformer {
            &[
                "uid",
                "fr_bus",
                "to_bus",
                "r",
                "x",
                "b",
                "tm_ub",
                "tm_lb",
                "ta_ub",
                "ta_lb",
                "mva_ub_nom",
                "mva_ub_sht",
                "mva_ub_em",
                "connection_cost",
                "disconnection_cost",
                "initial_status",
                "additional_shunt",
                "g_fr",
                "b_fr",
                "g_to",
                "b_to",
            ]
        } else {
            &[
                "uid",
                "fr_bus",
                "to_bus",
                "r",
                "x",
                "b",
                "mva_ub_nom",
                "mva_ub_sht",
                "mva_ub_em",
                "connection_cost",
                "disconnection_cost",
                "initial_status",
                "additional_shunt",
                "g_fr",
                "b_fr",
                "g_to",
                "b_to",
            ]
        };
        reject_unknown_fields(object, &path, allowed)?;
        require_input_fields(
            object,
            &path,
            &[
                ("uid", RequiredKind::String),
                ("fr_bus", RequiredKind::String),
                ("to_bus", RequiredKind::String),
                ("r", RequiredKind::Number),
                ("x", RequiredKind::Number),
                ("b", RequiredKind::Number),
                ("mva_ub_nom", RequiredKind::Number),
                ("mva_ub_em", RequiredKind::Number),
                ("connection_cost", RequiredKind::Number),
                ("disconnection_cost", RequiredKind::Number),
                ("initial_status", RequiredKind::Object),
                ("additional_shunt", RequiredKind::Binary),
            ],
        )?;
        validate_optional_field(object, &path, "mva_ub_sht", RequiredKind::Number)?;
        validate_conditional_numbers(
            object,
            &path,
            "additional_shunt",
            &["g_fr", "b_fr", "g_to", "b_to"],
        )?;
        let initial = require_input_object(object, &path, "initial_status")?;
        let initial_allowed: &[&str] = if transformer {
            &["on_status", "tm", "ta"]
        } else {
            &["on_status"]
        };
        reject_unknown_fields(initial, &format!("{path}.initial_status"), initial_allowed)?;
        require_input_field(
            initial,
            &format!("{path}.initial_status"),
            "on_status",
            RequiredKind::Binary,
        )?;
        let from_bus = require_str(object, "fr_bus")?;
        let to_bus = require_str(object, "to_bus")?;
        if from_bus == to_bus {
            return Err(json_error(format!(
                "`{path}` requires fr_bus and to_bus to differ"
            )));
        }
        let resistance = finite_number(object, &path, "r")?;
        let reactance = finite_number(object, &path, "x")?;
        if resistance == 0.0 && reactance == 0.0 {
            return Err(json_error(format!(
                "`{path}` requires nonzero series resistance or reactance"
            )));
        }
        let nominal_rating = finite_number(object, &path, "mva_ub_nom")?;
        let emergency_rating = finite_number(object, &path, "mva_ub_em")?;
        if nominal_rating <= 0.0 || nominal_rating > emergency_rating {
            return Err(json_error(format!(
                "`{path}` requires 0 < mva_ub_nom <= mva_ub_em; found {nominal_rating}, {emergency_rating}"
            )));
        }
        if transformer {
            require_input_fields(
                object,
                &path,
                &[
                    ("tm_ub", RequiredKind::Number),
                    ("tm_lb", RequiredKind::Number),
                    ("ta_ub", RequiredKind::Number),
                    ("ta_lb", RequiredKind::Number),
                ],
            )?;
            require_input_fields(
                initial,
                &format!("{path}.initial_status"),
                &[("tm", RequiredKind::Number), ("ta", RequiredKind::Number)],
            )?;
            let tap_min = finite_number(object, &path, "tm_lb")?;
            let tap_max = finite_number(object, &path, "tm_ub")?;
            let shift_min = finite_number(object, &path, "ta_lb")?;
            let shift_max = finite_number(object, &path, "ta_ub")?;
            let tap_initial = finite_number(initial, &format!("{path}.initial_status"), "tm")?;
            let shift_initial = finite_number(initial, &format!("{path}.initial_status"), "ta")?;
            if tap_min <= 0.0 {
                return Err(json_error(format!(
                    "`{path}.tm_lb` must be greater than zero"
                )));
            }
            validate_bounded_initial(
                &path,
                "tm_lb",
                tap_min,
                "initial_status.tm",
                tap_initial,
                "tm_ub",
                tap_max,
            )?;
            validate_bounded_initial(
                &path,
                "ta_lb",
                shift_min,
                "initial_status.ta",
                shift_initial,
                "ta_ub",
                shift_max,
            )?;
            if tap_min < tap_max && shift_min < shift_max {
                return Err(json_error(format!(
                    "`{path}` requires a fixed tap ratio or a fixed phase shift"
                )));
            }
        }
    }
    Ok(())
}

fn validate_dc_line_rows(document: &Goc3Document) -> Result<()> {
    for (index, record) in document
        .network_records("dc_line")
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let path = record_path("network.dc_line", record, index);
        let object = record_object(record, "network.dc_line", index)?;
        reject_unknown_fields(
            object,
            &path,
            &[
                "uid",
                "fr_bus",
                "to_bus",
                "pdc_ub",
                "qdc_fr_ub",
                "qdc_fr_lb",
                "qdc_to_ub",
                "qdc_to_lb",
                "initial_status",
            ],
        )?;
        require_input_fields(
            object,
            &path,
            &[
                ("uid", RequiredKind::String),
                ("fr_bus", RequiredKind::String),
                ("to_bus", RequiredKind::String),
                ("pdc_ub", RequiredKind::Number),
                ("qdc_fr_ub", RequiredKind::Number),
                ("qdc_fr_lb", RequiredKind::Number),
                ("qdc_to_ub", RequiredKind::Number),
                ("qdc_to_lb", RequiredKind::Number),
                ("initial_status", RequiredKind::Object),
            ],
        )?;
        let initial = require_input_object(object, &path, "initial_status")?;
        reject_unknown_fields(
            initial,
            &format!("{path}.initial_status"),
            &["pdc_fr", "qdc_fr", "qdc_to"],
        )?;
        require_input_fields(
            initial,
            &format!("{path}.initial_status"),
            &[
                ("pdc_fr", RequiredKind::Number),
                ("qdc_fr", RequiredKind::Number),
                ("qdc_to", RequiredKind::Number),
            ],
        )?;
        let from_bus = require_str(object, "fr_bus")?;
        let to_bus = require_str(object, "to_bus")?;
        if from_bus == to_bus {
            return Err(json_error(format!(
                "`{path}` requires fr_bus and to_bus to differ"
            )));
        }
        let p_max = finite_number(object, &path, "pdc_ub")?;
        let p_initial = finite_number(initial, &format!("{path}.initial_status"), "pdc_fr")?;
        if p_initial < -p_max || p_initial > p_max {
            return Err(json_error(format!(
                "`{path}` requires -pdc_ub <= initial_status.pdc_fr <= pdc_ub; found {p_max}, {p_initial}"
            )));
        }
        for (lower_field, initial_field, upper_field) in [
            ("qdc_fr_lb", "qdc_fr", "qdc_fr_ub"),
            ("qdc_to_lb", "qdc_to", "qdc_to_ub"),
        ] {
            let lower = finite_number(object, &path, lower_field)?;
            let upper = finite_number(object, &path, upper_field)?;
            let initial_value =
                finite_number(initial, &format!("{path}.initial_status"), initial_field)?;
            if lower > 0.0 || upper < 0.0 {
                return Err(json_error(format!(
                    "`{path}` requires {lower_field} <= 0 <= {upper_field}; found {lower}, {upper}"
                )));
            }
            validate_bounded_initial(
                &path,
                lower_field,
                lower,
                &format!("initial_status.{initial_field}"),
                initial_value,
                upper_field,
                upper,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_reserve_rows(document: &Goc3Document, periods: usize) -> Result<()> {
    for (index, record) in document
        .network_records("active_zonal_reserve")
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let path = record_path("network.active_zonal_reserve", record, index);
        let object = record_object(record, "network.active_zonal_reserve", index)?;
        reject_unknown_fields(
            object,
            &path,
            &[
                "uid",
                "REG_UP",
                "REG_DOWN",
                "SYN",
                "NSYN",
                "REG_UP_vio_cost",
                "REG_DOWN_vio_cost",
                "SYN_vio_cost",
                "NSYN_vio_cost",
                "RAMPING_RESERVE_UP_vio_cost",
                "RAMPING_RESERVE_DOWN_vio_cost",
            ],
        )?;
        require_input_fields(
            object,
            &path,
            &[
                ("uid", RequiredKind::String),
                ("REG_UP", RequiredKind::Number),
                ("REG_DOWN", RequiredKind::Number),
                ("SYN", RequiredKind::Number),
                ("NSYN", RequiredKind::Number),
                ("REG_UP_vio_cost", RequiredKind::Number),
                ("REG_DOWN_vio_cost", RequiredKind::Number),
                ("SYN_vio_cost", RequiredKind::Number),
                ("NSYN_vio_cost", RequiredKind::Number),
                ("RAMPING_RESERVE_UP_vio_cost", RequiredKind::Number),
                ("RAMPING_RESERVE_DOWN_vio_cost", RequiredKind::Number),
            ],
        )?;
        for field in [
            "REG_UP",
            "REG_DOWN",
            "SYN",
            "NSYN",
            "REG_UP_vio_cost",
            "REG_DOWN_vio_cost",
            "SYN_vio_cost",
            "NSYN_vio_cost",
            "RAMPING_RESERVE_UP_vio_cost",
            "RAMPING_RESERVE_DOWN_vio_cost",
        ] {
            require_nonnegative(object, &path, field)?;
        }
    }
    for (index, record) in document
        .time_series_input_records("active_zonal_reserve")
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let path = record_path("time_series_input.active_zonal_reserve", record, index);
        let object = record_object(record, "time_series_input.active_zonal_reserve", index)?;
        reject_unknown_fields(
            object,
            &path,
            &["uid", "RAMPING_RESERVE_UP", "RAMPING_RESERVE_DOWN"],
        )?;
        require_input_fields(
            object,
            &path,
            &[
                ("uid", RequiredKind::String),
                ("RAMPING_RESERVE_UP", RequiredKind::NumberArray),
                ("RAMPING_RESERVE_DOWN", RequiredKind::NumberArray),
            ],
        )?;
        for field in ["RAMPING_RESERVE_UP", "RAMPING_RESERVE_DOWN"] {
            if object[field].as_array().map_or(0, Vec::len) != periods {
                return Err(json_error(format!(
                    "`{path}.{field}` has {} periods; expected {periods}",
                    object[field].as_array().map_or(0, Vec::len)
                )));
            }
            require_nonnegative_array(object, &path, field)?;
        }
    }
    for (index, record) in document
        .network_records("reactive_zonal_reserve")
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let path = record_path("network.reactive_zonal_reserve", record, index);
        let object = record_object(record, "network.reactive_zonal_reserve", index)?;
        reject_unknown_fields(
            object,
            &path,
            &["uid", "REACT_UP_vio_cost", "REACT_DOWN_vio_cost"],
        )?;
        require_input_fields(
            object,
            &path,
            &[
                ("uid", RequiredKind::String),
                ("REACT_UP_vio_cost", RequiredKind::Number),
                ("REACT_DOWN_vio_cost", RequiredKind::Number),
            ],
        )?;
        require_nonnegative(object, &path, "REACT_UP_vio_cost")?;
        require_nonnegative(object, &path, "REACT_DOWN_vio_cost")?;
    }
    for (index, record) in document
        .time_series_input_records("reactive_zonal_reserve")
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let path = record_path("time_series_input.reactive_zonal_reserve", record, index);
        let object = record_object(record, "time_series_input.reactive_zonal_reserve", index)?;
        reject_unknown_fields(object, &path, &["uid", "REACT_UP", "REACT_DOWN"])?;
        require_input_fields(
            object,
            &path,
            &[
                ("uid", RequiredKind::String),
                ("REACT_UP", RequiredKind::NumberArray),
                ("REACT_DOWN", RequiredKind::NumberArray),
            ],
        )?;
        for field in ["REACT_UP", "REACT_DOWN"] {
            if object[field].as_array().map_or(0, Vec::len) != periods {
                return Err(json_error(format!(
                    "`{path}.{field}` has {} periods; expected {periods}",
                    object[field].as_array().map_or(0, Vec::len)
                )));
            }
            require_nonnegative_array(object, &path, field)?;
        }
    }
    Ok(())
}

fn uid_set(records: &[Goc3Record<'_>], path: &str) -> Result<BTreeSet<String>> {
    records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let object = record_object(record, path, index)?;
            Ok(require_input_field(
                object,
                &record_path(path, record, index),
                "uid",
                RequiredKind::String,
            )?
            .as_str()
            .expect("the string check above established a string")
            .to_owned())
        })
        .collect()
}

fn require_same_uids(
    static_records: &[Goc3Record<'_>],
    series_records: &[Goc3Record<'_>],
    static_path: &str,
    series_path: &str,
) -> Result<()> {
    let static_uids = uid_set(static_records, static_path)?;
    let series_uids = uid_set(series_records, series_path)?;
    if static_uids == series_uids {
        return Ok(());
    }
    let missing: Vec<_> = static_uids.difference(&series_uids).cloned().collect();
    let extra: Vec<_> = series_uids.difference(&static_uids).cloned().collect();
    Err(json_error(format!(
        "`{static_path}` and `{series_path}` uid sets differ; missing time series {missing:?}, extra time series {extra:?}"
    )))
}

fn validate_global_uids(document: &Goc3Document) -> Result<()> {
    let mut seen = HashMap::<String, String>::new();
    for section in [
        "bus",
        "shunt",
        "simple_dispatchable_device",
        "ac_line",
        "two_winding_transformer",
        "dc_line",
        "active_zonal_reserve",
        "reactive_zonal_reserve",
    ] {
        let path = format!("network.{section}");
        for (index, record) in document
            .network_records(section)
            .map_err(|error| rd(&error))?
            .iter()
            .enumerate()
        {
            let object = record_object(record, &path, index)?;
            let row_path = record_path(&path, record, index);
            let uid = require_input_field(object, &row_path, "uid", RequiredKind::String)?
                .as_str()
                .expect("the string check above established a string")
                .to_owned();
            if let Some(first) = seen.insert(uid.clone(), row_path.clone()) {
                return Err(json_error(format!(
                    "component uid `{uid}` is repeated at `{first}` and `{row_path}`"
                )));
            }
        }
    }

    let reliability = document
        .reliability()
        .ok_or_else(|| json_error("missing required object `reliability`"))?;
    let contingencies = reliability
        .get("contingency")
        .and_then(Value::as_array)
        .ok_or_else(|| json_error("missing required array `reliability.contingency`"))?;
    for (index, value) in contingencies.iter().enumerate() {
        let object = value.as_object().ok_or_else(|| {
            json_error(format!(
                "`reliability.contingency[{index}]` is not an object"
            ))
        })?;
        let path = format!("reliability.contingency[{index}]");
        let uid = require_input_field(object, &path, "uid", RequiredKind::String)?
            .as_str()
            .expect("the string check above established a string")
            .to_owned();
        if let Some(first) = seen.insert(uid.clone(), path.clone()) {
            return Err(json_error(format!(
                "component uid `{uid}` is repeated at `{first}` and `{path}`"
            )));
        }
    }
    Ok(())
}

fn validate_reserve_references(document: &Goc3Document) -> Result<()> {
    let active = uid_set(
        &document
            .network_records("active_zonal_reserve")
            .map_err(|error| rd(&error))?,
        "network.active_zonal_reserve",
    )?;
    let reactive = uid_set(
        &document
            .network_records("reactive_zonal_reserve")
            .map_err(|error| rd(&error))?,
        "network.reactive_zonal_reserve",
    )?;
    for (index, record) in document
        .network_records("bus")
        .map_err(|error| rd(&error))?
        .iter()
        .enumerate()
    {
        let path = record_path("network.bus", record, index);
        let object = record_object(record, "network.bus", index)?;
        for (field, known) in [
            ("active_reserve_uids", &active),
            ("reactive_reserve_uids", &reactive),
        ] {
            let values = require_input_field(object, &path, field, RequiredKind::StringArray)?
                .as_array()
                .expect("the array check above established an array");
            for value in values {
                let uid = value
                    .as_str()
                    .expect("the string array check above established strings");
                if !known.contains(uid) {
                    return Err(json_error(format!(
                        "`{path}.{field}` names unknown reserve zone `{uid}`"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_contingencies(document: &Goc3Document) -> Result<()> {
    let mut branches = HashSet::new();
    for section in ["ac_line", "two_winding_transformer", "dc_line"] {
        branches.extend(uid_set(
            &document
                .network_records(section)
                .map_err(|error| rd(&error))?,
            &format!("network.{section}"),
        )?);
    }
    let reliability = document
        .reliability()
        .ok_or_else(|| json_error("missing required object `reliability`"))?;
    let contingencies = reliability
        .get("contingency")
        .and_then(Value::as_array)
        .ok_or_else(|| json_error("missing required array `reliability.contingency`"))?;
    for (index, value) in contingencies.iter().enumerate() {
        let object = value.as_object().ok_or_else(|| {
            json_error(format!(
                "`reliability.contingency[{index}]` is not an object"
            ))
        })?;
        let path = format!("reliability.contingency[{index}]");
        reject_unknown_fields(object, &path, &["uid", "components"])?;
        require_input_field(object, &path, "uid", RequiredKind::String)?;
        let components =
            require_input_field(object, &path, "components", RequiredKind::StringArray)?
                .as_array()
                .expect("the array check above established an array");
        if components.len() != 1 {
            return Err(json_error(format!(
                "`{path}.components` has {} entries; Challenge 3 requires exactly one branch outage",
                components.len()
            )));
        }
        let component = components[0]
            .as_str()
            .expect("the string array check above established a string");
        if !branches.contains(component) {
            return Err(json_error(format!(
                "`{path}.components` names `{component}`, which is not an AC line, transformer, or DC line"
            )));
        }
    }
    Ok(())
}

/// Validate the official JSON shape and the deterministic consistency rules
/// defined by the GO Challenge 3 data model. Checks that depend on competition
/// configuration, numerical tolerances, or solving a feasibility problem do
/// not belong in parsing.
#[allow(clippy::too_many_lines)]
fn validate_challenge3_input_document(document: &Goc3Document) -> Result<()> {
    reject_unknown_fields(
        document.root(),
        "root",
        &["network", "time_series_input", "reliability"],
    )?;
    let network = document.network().map_err(|error| rd(&error))?;
    let time_series = document.time_series_input().map_err(|error| rd(&error))?;
    let reliability = document
        .reliability()
        .ok_or_else(|| json_error("missing required object `reliability`"))?;

    reject_unknown_fields(
        network,
        "network",
        &[
            "general",
            "violation_cost",
            "bus",
            "shunt",
            "simple_dispatchable_device",
            "ac_line",
            "two_winding_transformer",
            "dc_line",
            "active_zonal_reserve",
            "reactive_zonal_reserve",
            "development",
        ],
    )?;
    reject_unknown_fields(
        time_series,
        "time_series_input",
        &[
            "general",
            "simple_dispatchable_device",
            "active_zonal_reserve",
            "reactive_zonal_reserve",
            "development",
        ],
    )?;
    reject_unknown_fields(reliability, "reliability", &["contingency"])?;
    validate_optional_field(network, "network", "development", RequiredKind::Object)?;
    validate_optional_field(
        time_series,
        "time_series_input",
        "development",
        RequiredKind::Object,
    )?;

    let general = require_input_object(network, "network", "general")?;
    reject_unknown_fields(
        general,
        "network.general",
        &[
            "timestamp_start",
            "timestamp_stop",
            "season",
            "electricity_demand",
            "vre_availability",
            "solar_availability",
            "wind_availability",
            "weather_temperature",
            "day_type",
            "net_load",
            "base_norm_mva",
        ],
    )?;
    for field in [
        "timestamp_start",
        "timestamp_stop",
        "season",
        "electricity_demand",
        "vre_availability",
        "solar_availability",
        "wind_availability",
        "weather_temperature",
        "day_type",
        "net_load",
    ] {
        validate_optional_field(general, "network.general", field, RequiredKind::String)?;
    }
    if let (Some(start), Some(stop)) = (
        general.get("timestamp_start").and_then(Value::as_str),
        general.get("timestamp_stop").and_then(Value::as_str),
    ) && start >= stop
    {
        return Err(json_error(format!(
            "`network.general` requires timestamp_start < timestamp_stop; found `{start}`, `{stop}`"
        )));
    }
    let base_mva = require_input_field(
        general,
        "network.general",
        "base_norm_mva",
        RequiredKind::Number,
    )?
    .as_f64()
    .expect("the numeric check above established a number");
    if base_mva <= 0.0 {
        return Err(json_error(
            "`network.general.base_norm_mva` must be greater than zero",
        ));
    }
    let violation = require_input_object(network, "network", "violation_cost")?;
    reject_unknown_fields(
        violation,
        "network.violation_cost",
        &[
            "p_bus_vio_cost",
            "q_bus_vio_cost",
            "s_vio_cost",
            "e_vio_cost",
        ],
    )?;
    require_input_fields(
        violation,
        "network.violation_cost",
        &[
            ("p_bus_vio_cost", RequiredKind::Number),
            ("q_bus_vio_cost", RequiredKind::Number),
            ("s_vio_cost", RequiredKind::Number),
            ("e_vio_cost", RequiredKind::Number),
        ],
    )?;
    for field in [
        "p_bus_vio_cost",
        "q_bus_vio_cost",
        "s_vio_cost",
        "e_vio_cost",
    ] {
        require_nonnegative(violation, "network.violation_cost", field)?;
    }
    for section in [
        "bus",
        "shunt",
        "simple_dispatchable_device",
        "ac_line",
        "two_winding_transformer",
        "dc_line",
        "active_zonal_reserve",
        "reactive_zonal_reserve",
    ] {
        require_input_section(network, "network", section)?;
    }
    let time_general = require_input_object(time_series, "time_series_input", "general")?;
    reject_unknown_fields(
        time_general,
        "time_series_input.general",
        &["time_periods", "interval_duration"],
    )?;
    let periods_u64 = require_input_field(
        time_general,
        "time_series_input.general",
        "time_periods",
        RequiredKind::Integer,
    )?
    .as_u64()
    .ok_or_else(|| json_error("`time_series_input.general.time_periods` is negative"))?;
    let periods = usize::try_from(periods_u64).map_err(|_| {
        json_error("`time_series_input.general.time_periods` exceeds this platform's index range")
    })?;
    if periods == 0 {
        return Err(json_error(
            "`time_series_input.general.time_periods` must be greater than zero",
        ));
    }
    let durations = require_input_field(
        time_general,
        "time_series_input.general",
        "interval_duration",
        RequiredKind::NumberArray,
    )?
    .as_array()
    .expect("the array check above established an array");
    if durations.len() != periods {
        return Err(json_error(format!(
            "`time_series_input.general.interval_duration` has {} periods; expected {periods}",
            durations.len()
        )));
    }
    if durations
        .iter()
        .any(|duration| duration.as_f64().is_none_or(|duration| duration <= 0.0))
    {
        return Err(json_error(
            "`time_series_input.general.interval_duration` values must be greater than zero",
        ));
    }
    for section in [
        "simple_dispatchable_device",
        "active_zonal_reserve",
        "reactive_zonal_reserve",
    ] {
        require_input_section(time_series, "time_series_input", section)?;
    }
    match reliability.get("contingency") {
        Some(Value::Array(_)) => {}
        Some(_) => {
            return Err(json_error(
                "required `reliability.contingency` is not an array",
            ));
        }
        None => {
            return Err(json_error(
                "missing required section `reliability.contingency`",
            ));
        }
    }

    validate_bus_rows(document)?;
    validate_shunt_rows(document)?;
    validate_device_rows(document, periods)?;
    validate_ac_branch_rows(document, "ac_line", false)?;
    validate_ac_branch_rows(document, "two_winding_transformer", true)?;
    validate_dc_line_rows(document)?;
    validate_reserve_rows(document, periods)?;
    validate_global_uids(document)?;
    validate_reserve_references(document)?;
    validate_contingencies(document)?;

    for (network_section, series_section) in [
        ("simple_dispatchable_device", "simple_dispatchable_device"),
        ("active_zonal_reserve", "active_zonal_reserve"),
        ("reactive_zonal_reserve", "reactive_zonal_reserve"),
    ] {
        let static_records = document
            .network_records(network_section)
            .map_err(|error| rd(&error))?;
        let series_records = document
            .time_series_input_records(series_section)
            .map_err(|error| rd(&error))?;
        require_same_uids(
            &static_records,
            &series_records,
            &format!("network.{network_section}"),
            &format!("time_series_input.{series_section}"),
        )?;
    }
    Ok(())
}

/// Load one optional section (an array or `uid`-keyed object) named `name`
/// under `parent`, describing its rows as `what` in error messages. Absent
/// loads empty. `what` and `name` differ where the same section name is
/// read from more than one parent object (e.g. `simple_dispatchable_device`
/// under both `network` and `time_series_input`).
fn load_section(items: powerio_tx::Result<Vec<Goc3Record<'_>>>, what: &str) -> Result<Goc3Section> {
    Goc3Section::from_items(items.map_err(|error| rd(&error))?, what)
}

/// The lookup tables used to construct the AC SCUC inputs, built once by
/// [`Goc3Adapter::from_document`] and shared by every projection in this module. The
/// Validated GO Challenge 3 tables used by the typed projection.
pub(super) struct Goc3Adapter {
    /// The required `network.violation_cost` object.
    pub(super) violation_cost: Map<String, Value>,
    /// The required `reliability.contingency` array. The complete input/problem document is
    /// validated before the adapter is built.
    pub(super) contingencies: Vec<Value>,
    /// Interval durations. `dt.len()` is the period count `L_T` (validated
    /// against `time_series_input.general.time_periods` at parse time).
    pub(super) dt: Vec<f64>,
    pub(super) bus: Goc3Section,
    pub(super) shunt: Goc3Section,
    pub(super) ac_line: Goc3Section,
    pub(super) twt: Goc3Section,
    pub(super) dc_line: Goc3Section,
    pub(super) sdd: Goc3Section,
    pub(super) sdd_ts: Goc3Section,
    pub(super) azr: Goc3Section,
    pub(super) azr_ts: Goc3Section,
    pub(super) rzr: Goc3Section,
    pub(super) rzr_ts: Goc3Section,
}

impl Goc3Adapter {
    /// Read the GOC3 sections used by the AC SCUC projection.
    ///
    /// Section order, device row assignment, and bus IDs come from the shared
    /// document adapter.
    #[allow(clippy::too_many_lines)]
    pub(super) fn from_document(document: &Goc3Document) -> Result<Self> {
        validate_challenge3_input_document(document)?;
        let contingencies = document
            .reliability()
            .and_then(|r| r.get("contingency"))
            .and_then(Value::as_array)
            .cloned()
            .expect("the Challenge 3 input document validator established contingencies");
        let violation_cost = document
            .network()
            .map_err(|error| rd(&error))?
            .get("violation_cost")
            .and_then(Value::as_object)
            .cloned()
            .expect("the Challenge 3 input document validator established violation costs");
        let time_series = document.time_series_input().map_err(|error| rd(&error))?;
        let general = time_series
            .get("general")
            .and_then(Value::as_object)
            .ok_or_else(|| json_error("missing object `time_series_input.general`"))?;

        let dt =
            float_vec(general.get("interval_duration").ok_or_else(|| {
                json_error("missing `time_series_input.general.interval_duration`")
            })?)?;
        let periods = general
            .get("time_periods")
            .and_then(Value::as_u64)
            .ok_or_else(|| json_error("missing `time_series_input.general.time_periods`"))?
            as usize;
        if dt.len() != periods {
            return Err(json_error(
                "interval_duration length does not match time_periods",
            ));
        }

        let bus_items = require_nonempty(
            document
                .network_records("bus")
                .map_err(|error| rd(&error))?,
            "network.bus",
        )?;
        let bus = Goc3Section::from_items(bus_items, "bus")?;

        let shunt = load_section(document.network_records("shunt"), "shunt")?;
        let ac_line = load_section(document.network_records("ac_line"), "ac_line")?;
        let twt = load_section(
            document.network_records("two_winding_transformer"),
            "two_winding_transformer",
        )?;
        let dc_line = load_section(document.network_records("dc_line"), "dc_line")?;

        let sdd = load_section(
            document.network_records("simple_dispatchable_device"),
            "simple_dispatchable_device",
        )?;
        let sdd_ts = load_section(
            document.time_series_input_records("simple_dispatchable_device"),
            "simple_dispatchable_device time series",
        )?;

        let azr = load_section(
            document.network_records("active_zonal_reserve"),
            "active_zonal_reserve",
        )?;
        let azr_ts = load_section(
            document.time_series_input_records("active_zonal_reserve"),
            "active_zonal_reserve time series",
        )?;
        let rzr = load_section(
            document.network_records("reactive_zonal_reserve"),
            "reactive_zonal_reserve",
        )?;
        let rzr_ts = load_section(
            document.time_series_input_records("reactive_zonal_reserve"),
            "reactive_zonal_reserve time series",
        )?;
        Ok(Self {
            violation_cost,
            contingencies,
            dt,
            bus,
            shunt,
            ac_line,
            twt,
            dc_line,
            sdd,
            sdd_ts,
            azr,
            azr_ts,
            rzr,
            rzr_ts,
        })
    }

    /// All simple dispatchable device uids in source document order.
    pub(super) fn sdd_order(&self) -> Vec<String> {
        self.sdd.uids().to_vec()
    }

    pub(super) fn contingencies(&self) -> &[Value] {
        &self.contingencies
    }
}

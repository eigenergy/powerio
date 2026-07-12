use std::collections::{BTreeMap, HashMap};

use powerio::BusId;
use powerio::format::goc3::{Goc3DeviceKind, Goc3Document, Goc3Record};
use serde_json::{Map, Value};

use super::error::{ScopfError, ScopfResult};

type Result<T> = ScopfResult<T>;

pub(super) fn json_error(message: impl Into<String>) -> ScopfError {
    ScopfError::invalid(message)
}

fn rd(err: &powerio::Error) -> ScopfError {
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
// Document tables (`_goc3_json_tables` / `parse_goc3_json` in `src/goc3.jl`).
// ---------------------------------------------------------------------------

/// One GOC3 section's rows, keyed by `uid`. `uids()` preserves the source
/// document order from [`Goc3Document`]; `sorted_uids()`
/// is the lexicographic order `_goc3_ids` builds in `src/goc3.jl`
/// (`sort(collect(keys(lookup)))`).
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

    pub(super) fn sorted_uids(&self) -> Vec<String> {
        let mut ids = self.order.clone();
        ids.sort();
        ids
    }

    pub(super) fn index(&self, uid: &str) -> Result<usize> {
        self.order
            .iter()
            .position(|candidate| candidate == uid)
            .ok_or_else(|| json_error(format!("unknown uid `{uid}`")))
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

/// Load one optional section (an array or `uid`-keyed object) named `name`
/// under `parent`, describing its rows as `what` in error messages. Absent
/// loads empty. `what` and `name` differ where the same section name is
/// read from more than one parent object (e.g. `simple_dispatchable_device`
/// under both `network` and `time_series_input`).
fn load_section(items: powerio::Result<Vec<Goc3Record<'_>>>, what: &str) -> Result<Goc3Section> {
    Goc3Section::from_items(items.map_err(|error| rd(&error))?, what)
}

/// The GOC3 lookup tables a SCOPF client reads, built once by
/// [`Goc3Adapter::from_document`] and shared by every projection in this module. The
/// Rust analog of `parse_goc3_json`'s return value in `src/goc3.jl`, scoped
/// to what the SCOPF projections need (it does not carry `violation_cost`; a
/// caller that needs it reads the source GOC3 JSON directly).
pub(super) struct Goc3Adapter {
    /// The `reliability.contingency` array, if the document has one. Kept
    /// lazily validated ([`Goc3Adapter::contingencies`] errors when absent)
    /// so a document with no `reliability` section still parses
    /// successfully for the projections that do not read it; only cloned
    /// out of the parsed document, not the document itself, since it is the
    /// only part of `reliability`/`network`/`time_series_input` this module
    /// reads outside the per-section tables below.
    pub(super) contingencies: Option<Vec<Value>>,
    /// Interval durations. `dt.len()` is the period count `L_T` (validated
    /// against `time_series_input.general.time_periods` at parse time).
    pub(super) dt: Vec<f64>,
    pub(super) bus: Goc3Section,
    pub(super) bus_id_by_uid: HashMap<String, BusId>,
    pub(super) shunt: Goc3Section,
    pub(super) ac_line: Goc3Section,
    pub(super) twt: Goc3Section,
    pub(super) dc_line: Goc3Section,
    pub(super) sdd: Goc3Section,
    pub(super) sdd_ts: Goc3Section,
    pub(super) sdd_ids_producer: Vec<String>,
    pub(super) sdd_ids_consumer: Vec<String>,
    pub(super) azr: Goc3Section,
    pub(super) azr_ts: Goc3Section,
    pub(super) azr_ids: Vec<String>,
    pub(super) rzr: Goc3Section,
    pub(super) rzr_ts: Goc3Section,
    pub(super) rzr_ids: Vec<String>,
}

impl Goc3Adapter {
    /// Read the GOC3 sections used by the SCOPF projection.
    ///
    /// Section order, device row assignment, and bus IDs come from the shared
    /// document adapter.
    #[allow(clippy::too_many_lines)]
    pub(super) fn from_document(document: &Goc3Document) -> Result<Self> {
        let contingencies = document
            .reliability()
            .and_then(|r| r.get("contingency"))
            .and_then(Value::as_array)
            .cloned();
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
        let bus_id_by_uid = document.bus_ids().map_err(|error| rd(&error))?;
        let bus = Goc3Section::from_items(bus_items, "bus")?;

        let shunt = load_section(document.network_records("shunt"), "shunt")?;
        let ac_line = load_section(document.network_records("ac_line"), "ac_line")?;
        let twt = load_section(
            document.network_records("two_winding_transformer"),
            "two_winding_transformer",
        )?;
        let dc_line = load_section(document.network_records("dc_line"), "dc_line")?;

        let sdd_items = require_nonempty(
            document
                .network_records("simple_dispatchable_device")
                .map_err(|error| rd(&error))?,
            "network.simple_dispatchable_device",
        )?;
        let sdd = Goc3Section::from_items(sdd_items, "simple_dispatchable_device")?;
        let sdd_ts_items = require_nonempty(
            document
                .time_series_input_records("simple_dispatchable_device")
                .map_err(|error| rd(&error))?,
            "time_series_input.simple_dispatchable_device",
        )?;
        let sdd_ts =
            Goc3Section::from_items(sdd_ts_items, "simple_dispatchable_device time series")?;

        // Producer/consumer partition uses the row assignment stored by
        // `Goc3Document`, which is also used by the balanced reader and the
        // operating point extractor. Not
        // sorted: only read through `.len()` (`L_J_pr`/`L_J_cs`), never
        // iterated in this order.
        let mut sdd_ids_producer = Vec::new();
        let mut sdd_ids_consumer = Vec::new();
        for row in document
            .dispatchable_devices()
            .map_err(|error| rd(&error))?
        {
            let Some(uid) = row.uid else { continue };
            match row.kind {
                Goc3DeviceKind::Generators => sdd_ids_producer.push(uid),
                Goc3DeviceKind::Loads => sdd_ids_consumer.push(uid),
            }
        }

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
        let azr_ids = azr.sorted_uids();
        let rzr_ids = rzr.sorted_uids();

        Ok(Self {
            contingencies,
            dt,
            bus,
            bus_id_by_uid,
            shunt,
            ac_line,
            twt,
            dc_line,
            sdd,
            sdd_ts,
            sdd_ids_producer,
            sdd_ids_consumer,
            azr,
            azr_ts,
            azr_ids,
            rzr,
            rzr_ts,
            rzr_ids,
        })
    }

    /// Map a GOC3 bus UID to the external bus ID used by the balanced reader.
    pub(super) fn goc3_bus_id(&self, uid: &str) -> Result<BusId> {
        self.bus_id_by_uid
            .get(uid)
            .copied()
            .ok_or_else(|| json_error(format!("unknown bus uid `{uid}`")))
    }

    /// Bus UIDs ordered by their assigned external bus ID.
    // Every uid in `self.bus` was used to build `self.bus_id_by_uid` in the
    // same pass in `parse`, so the direct index below never panics.
    pub(super) fn bus_order(&self) -> Vec<String> {
        let mut uids = self.bus.uids().to_vec();
        uids.sort_by_key(|uid| self.bus_id_by_uid[uid]);
        uids
    }

    /// All simple dispatchable device uids in source document order.
    pub(super) fn sdd_order(&self) -> Vec<String> {
        self.sdd.uids().to_vec()
    }

    /// Simple dispatchable device uids grouped by their bus uid, each group
    /// in [`Goc3Adapter::sdd_order`]. The Rust equivalent of
    /// `_goc3_static_data`'s `devices_by_bus`.
    pub(super) fn devices_by_bus(&self) -> Result<BTreeMap<String, Vec<String>>> {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for uid in self.sdd_order() {
            let obj = self.sdd.get(&uid)?;
            let bus = require_str(obj, "bus")?.to_owned();
            map.entry(bus).or_default().push(uid);
        }
        Ok(map)
    }

    pub(super) fn contingencies(&self) -> Result<&[Value]> {
        self.contingencies
            .as_deref()
            .ok_or_else(|| json_error("missing `reliability.contingency`"))
    }
}

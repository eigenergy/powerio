//! GO Challenge 3 SCOPF projections: the Rust port of `src/goc3.jl`'s
//! `_goc3_*` index-set builders and `goc3_scopf_data` in PowerIO.jl (post
//! 0.6.4). This module is the general, format-neutral security-constrained
//! OPF instance a GOC3 case reduces to: buses, shunts, AC/DC branches,
//! transformer control sets, producers, consumers, zonal reserves,
//! device-zone membership, multi-period energy windows, flattened price
//! blocks, and per-contingency survivor sets.
//!
//! Every projection is a pure function of the parsed GOC3 document: no unit
//! commitment solution and no model-specific stacked variable numbering. Row
//! fields expose PER-CLASS GOC3 position indices only (`j_ln`/`j_xf`/`j_dc`,
//! `n_p`/`n_q`, `t`, `ctg`, per-window `ind`), exactly as `src/goc3.jl`
//! documents; a client threads its own solver-specific offsets on top, same
//! as it threads unit commitment status via a separate step. Row indices are
//! 1-based, matching the Julia side field for field, since the eventual
//! PowerIO.jl binding reads these numbers directly as Julia array positions.
//!
//! Document walking (section ordering, device-row assignment, and the
//! producer/consumer partition) reuses `powerio::format::goc3_bridge`, the
//! same helpers `powerio-pkg::operating`'s GOC3 operating-point extractor
//! uses, so a GOC3 document's rows land in the same place here as they do in
//! `BalancedNetwork`'s reduction and in the operating point series. Value
//! extraction (unit conversion, cost-curve shape) does not reuse
//! `goc3_bridge::cost_at`: that helper cumulates a MATPOWER-style piecewise
//! curve scaled by `base_mva` for the static `Network` snapshot, whereas
//! [`goc3_price_blocks`] flattens each raw `(marginal_cost, block_width)`
//! pair unscaled, in the GOC3 document's own per-unit convention, matching
//! `_goc3_price_blocks` in `src/goc3.jl`.
//!
//! `src/goc3.jl` iterates a handful of these lookups (`Dict`s keyed by uid)
//! in native hash order where it does not sort the result afterward
//! (`_goc3_energy_windows`'s window indices, `_goc3_static_data`'s reserve
//! membership sets, and the survivor/DC-flow row order within one
//! contingency). `Dict` iteration order is not part of that function's
//! documented contract, so this port does not attempt to reproduce Julia's
//! hash order bit for bit; it instead uses one deterministic order
//! throughout (ascending GOC3 uid number for devices and transformers,
//! ascending assigned bus index for buses, source-document order for
//! AC/DC-line survivor rows within one contingency), documented at each call
//! site below. Every row still carries the same position-index fields Julia
//! assigns, and every output Julia pins with an explicit `sort` afterward
//! matches exactly.
//!
//! Naming: Rust names this module's public API for parity with the Julia
//! function stems (`goc3_static_data`, `goc3_energy_windows`,
//! `goc3_price_blocks`, `goc3_ac_contingency_survivors`,
//! `goc3_dc_contingency_flows`, `goc3_scopf_data`), dropping the `_goc3_*`
//! leading underscore Julia uses to mark them internal: `PowerIO.jl`'s module
//! only exports `goc3_scopf_data` and `ScopfInstance`, but this crate makes
//! all five projections `pub` (Rust's visibility system, not a naming
//! convention, is how this crate marks something internal) so a later
//! `powerio-opf` crate can build its own `ScopfInstance` wrapper on top of
//! [`Goc3ScopfData`] without re-deriving these sets. [`Goc3ScopfData`] itself
//! is deliberately not named `ScopfInstance`: `powerio-opf`'s public
//! `ScopfInstance` type (eigenergy/powerio#238) is the OPF-family analog of
//! `powerio-matrix`'s `OpfInstance`, and will likely wrap or convert from
//! this GOC3-specific bundle rather than duplicate it.
//!
//! GOC3 is an input format, not a stored representation of this instance:
//! like `powerio-matrix`'s `OpfInstance`, [`Goc3ScopfData`] is a projection a
//! client reads to build a model, computed fresh from [`Goc3Tables::parse`]
//! every time. There is no writer that serializes it back to GOC3 JSON (see
//! the module-level round-trip note in `powerio-pkg`'s test suite).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use powerio::BusId;
use powerio::format::goc3_bridge::{
    self, DeviceTable, SectionItem, device_rows, item_uid, number, section, string,
};

use crate::operating::json_error;

/// This module's `Result`: every error is a [`serde_json::Error`] built with
/// a custom message, the same convention `powerio-pkg::operating` uses for
/// its GOC3 document walking.
type Result<T> = serde_json::Result<T>;

fn rd(err: &powerio::Error) -> serde_json::Error {
    json_error(err.to_string())
}

// ---------------------------------------------------------------------------
// Raw-value extraction. `src/goc3.jl` mostly reads JSON5-via-JSON3 values
// straight into `Float64`/`Int`/`String` fields; these helpers do the same
// from `serde_json::Value`, erroring with the field name on a shape mismatch
// instead of Julia's `KeyError`/`MethodError`.
// ---------------------------------------------------------------------------

fn require_num(obj: &Map<String, Value>, key: &str) -> Result<f64> {
    number(obj, key).ok_or_else(|| json_error(format!("missing numeric field `{key}`")))
}

fn require_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    string(obj, key).ok_or_else(|| json_error(format!("missing string field `{key}`")))
}

/// Look up a required field on a row identified by `what`/`uid` (e.g. `what
/// = "simple_dispatchable_device time series"`), for the fields
/// `float_vec`/`float_matrix`/`cost_cube` parse themselves so the "missing
/// field" message names the row, not just the key.
fn require_field<'a>(
    obj: &'a Map<String, Value>,
    what: &str,
    uid: &str,
    key: &str,
) -> Result<&'a Value> {
    obj.get(key)
        .ok_or_else(|| json_error(format!("{what} `{uid}` missing `{key}`")))
}

fn float_vec(value: &Value) -> Result<Vec<f64>> {
    value
        .as_array()
        .ok_or_else(|| json_error("expected an array of numbers"))?
        .iter()
        .map(|v| v.as_f64().ok_or_else(|| json_error("expected a number")))
        .collect()
}

fn float_matrix(value: &Value) -> Result<Vec<Vec<f64>>> {
    value
        .as_array()
        .ok_or_else(|| json_error("expected an array of arrays"))?
        .iter()
        .map(float_vec)
        .collect()
}

fn float_pair(value: &Value) -> Result<[f64; 2]> {
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
/// 2-element pair, enforced by the return type, so [`flatten_price_blocks`]
/// reads it without a bounds check.
fn cost_cube(value: &Value) -> Result<Vec<Vec<[f64; 2]>>> {
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

fn initial_status(obj: &Map<String, Value>) -> Result<&Map<String, Value>> {
    obj.get("initial_status")
        .and_then(Value::as_object)
        .ok_or_else(|| json_error("missing object field `initial_status`"))
}

/// First run of ASCII digits in `uid` (e.g. `"acl_07"` -> `7`), the Rust
/// equivalent of `_uidnum`'s unanchored `r"\d+"` match in `src/goc3.jl`. GOC3
/// uids carry exactly one digit run in practice, so "first" and "trailing"
/// (the wording of `_uidnum`'s docstring) coincide.
fn uid_num(uid: &str) -> Result<usize> {
    let digits: String = uid
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits
        .parse()
        .map_err(|_| json_error(format!("uid `{uid}` has no numeric suffix")))
}

/// Sort `ids` by [`uid_num`], breaking ties on the uid itself for a total
/// order. The Rust equivalent of `_uidnum_order` in `src/goc3.jl`.
fn uidnum_order(ids: &[String]) -> Result<Vec<String>> {
    let mut keyed = ids
        .iter()
        .map(|id| Ok((uid_num(id)?, id.clone())))
        .collect::<Result<Vec<_>>>()?;
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(keyed.into_iter().map(|(_, id)| id).collect())
}

// ---------------------------------------------------------------------------
// Document tables (`_goc3_json_tables` / `parse_goc3_json` in `src/goc3.jl`).
// ---------------------------------------------------------------------------

/// One GOC3 section's rows, keyed by `uid`. `uids()` preserves the document
/// order `powerio::format::goc3_bridge::section` establishes; `sorted_uids()`
/// is the lexicographic order `_goc3_ids` builds in `src/goc3.jl`
/// (`sort(collect(keys(lookup)))`).
#[derive(Clone, Debug, Default)]
struct Goc3Section {
    order: Vec<String>,
    rows: HashMap<String, Map<String, Value>>,
}

impl Goc3Section {
    fn from_items(items: Vec<SectionItem<'_>>, what: &str) -> Result<Self> {
        let mut order = Vec::with_capacity(items.len());
        let mut rows = HashMap::with_capacity(items.len());
        for item in items {
            let obj = item
                .value
                .as_object()
                .ok_or_else(|| json_error(format!("{what} item is not an object")))?;
            let uid = item_uid(item, obj)
                .ok_or_else(|| json_error(format!("{what} item missing `uid`")))?;
            if rows.insert(uid.clone(), obj.clone()).is_some() {
                return Err(json_error(format!("duplicate {what} uid `{uid}`")));
            }
            order.push(uid);
        }
        Ok(Self { order, rows })
    }

    fn get(&self, uid: &str) -> Result<&Map<String, Value>> {
        self.rows
            .get(uid)
            .ok_or_else(|| json_error(format!("unknown uid `{uid}`")))
    }

    fn uids(&self) -> &[String] {
        &self.order
    }

    fn sorted_uids(&self) -> Vec<String> {
        let mut ids = self.order.clone();
        ids.sort();
        ids
    }
}

/// Reject an empty section, naming its full dotted path (e.g.
/// `network.bus`) in the error: [`Goc3Section`] itself only knows the bare
/// section name.
fn require_nonempty<'a>(items: Vec<SectionItem<'a>>, path: &str) -> Result<Vec<SectionItem<'a>>> {
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
fn load_section(
    parent: &Map<String, Value>,
    name: &'static str,
    what: &str,
) -> Result<Goc3Section> {
    Goc3Section::from_items(section(parent, name).map_err(|e| rd(&e))?, what)
}

/// The GOC3 lookup tables a SCOPF client reads, built once by
/// [`Goc3Tables::parse`] and shared by every projection in this module. The
/// Rust analog of `parse_goc3_json`'s return value in `src/goc3.jl`, scoped
/// to what the SCOPF projections need (it does not carry `violation_cost`; a
/// caller that needs it reads the source GOC3 JSON directly).
pub struct Goc3Tables {
    /// The `reliability.contingency` array, if the document has one. Kept
    /// lazily validated ([`Goc3Tables::contingencies`] errors when absent)
    /// so a document with no `reliability` section still parses
    /// successfully for the projections that do not read it; only cloned
    /// out of the parsed document, not the document itself, since it is the
    /// only part of `reliability`/`network`/`time_series_input` this module
    /// reads outside the per-section tables below.
    contingencies: Option<Vec<Value>>,
    /// Interval durations. `dt.len()` is the period count `L_T` (validated
    /// against `time_series_input.general.time_periods` at parse time).
    dt: Vec<f64>,
    bus: Goc3Section,
    bus_id_by_uid: HashMap<String, BusId>,
    shunt: Goc3Section,
    ac_line: Goc3Section,
    twt: Goc3Section,
    dc_line: Goc3Section,
    sdd: Goc3Section,
    sdd_ts: Goc3Section,
    sdd_ids_producer: Vec<String>,
    sdd_ids_consumer: Vec<String>,
    azr: Goc3Section,
    azr_ts: Goc3Section,
    azr_ids: Vec<String>,
    rzr: Goc3Section,
    rzr_ts: Goc3Section,
    rzr_ids: Vec<String>,
}

impl Goc3Tables {
    /// Parse a full GOC3 JSON input document into the lookup tables the
    /// SCOPF projections read (`_goc3_json_tables` in `src/goc3.jl`).
    /// Section ordering, device-row assignment, and the producer/consumer
    /// partition reuse `powerio::format::goc3_bridge`, so they agree with
    /// `BalancedNetwork`'s reduction of the same document by construction.
    // One flat sequence of section reads mirroring `_goc3_json_tables` in
    // `src/goc3.jl`, which builds its named tuple the same way.
    #[allow(clippy::too_many_lines)]
    pub fn parse(text: &str) -> Result<Self> {
        let root: Value = serde_json::from_str(text)?;
        let root = root
            .as_object()
            .ok_or_else(|| json_error("top level is not a JSON object"))?;
        let contingencies = root
            .get("reliability")
            .and_then(|r| r.get("contingency"))
            .and_then(Value::as_array)
            .cloned();
        let network = root
            .get("network")
            .and_then(Value::as_object)
            .ok_or_else(|| json_error("missing object `network`"))?;
        let time_series = root
            .get("time_series_input")
            .and_then(Value::as_object)
            .ok_or_else(|| json_error("missing object `time_series_input`"))?;
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

        let bus_items =
            require_nonempty(section(network, "bus").map_err(|e| rd(&e))?, "network.bus")?;
        let bus_id_by_uid = goc3_bridge::bus_id_by_uid(&bus_items).map_err(|e| rd(&e))?;
        let bus = Goc3Section::from_items(bus_items, "bus")?;

        let shunt = load_section(network, "shunt", "shunt")?;
        let ac_line = load_section(network, "ac_line", "ac_line")?;
        let twt = load_section(
            network,
            "two_winding_transformer",
            "two_winding_transformer",
        )?;
        let dc_line = load_section(network, "dc_line", "dc_line")?;

        let sdd_items = require_nonempty(
            section(network, "simple_dispatchable_device").map_err(|e| rd(&e))?,
            "network.simple_dispatchable_device",
        )?;
        let sdd = Goc3Section::from_items(sdd_items, "simple_dispatchable_device")?;
        let sdd_ts_items = require_nonempty(
            section(time_series, "simple_dispatchable_device").map_err(|e| rd(&e))?,
            "time_series_input.simple_dispatchable_device",
        )?;
        let sdd_ts =
            Goc3Section::from_items(sdd_ts_items, "simple_dispatchable_device time series")?;

        // Producer/consumer partition reuses `device_rows`, the same walk
        // `BalancedNetwork`'s GOC3 reader and the operating-point extractor
        // use, so this partition agrees with theirs by construction. Not
        // sorted: only read through `.len()` (`L_J_pr`/`L_J_cs`), never
        // iterated in this order.
        let mut sdd_ids_producer = Vec::new();
        let mut sdd_ids_consumer = Vec::new();
        for row in device_rows(network).map_err(|e| rd(&e))? {
            let Some(uid) = row.uid else { continue };
            match row.table {
                DeviceTable::Generators => sdd_ids_producer.push(uid),
                DeviceTable::Loads => sdd_ids_consumer.push(uid),
            }
        }

        let azr = load_section(network, "active_zonal_reserve", "active_zonal_reserve")?;
        let azr_ts = load_section(
            time_series,
            "active_zonal_reserve",
            "active_zonal_reserve time series",
        )?;
        let rzr = load_section(network, "reactive_zonal_reserve", "reactive_zonal_reserve")?;
        let rzr_ts = load_section(
            time_series,
            "reactive_zonal_reserve",
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

    /// Map a GOC3 bus uid to its 1-based row index, using the same rule
    /// `BalancedNetwork`'s GOC3 reader uses for `BusId` (`goc3_bus_id` in
    /// `src/goc3.jl`).
    fn goc3_bus_id(&self, uid: &str) -> Result<BusId> {
        self.bus_id_by_uid
            .get(uid)
            .copied()
            .ok_or_else(|| json_error(format!("unknown bus uid `{uid}`")))
    }

    /// Bus uids ordered by their assigned [`Goc3Tables::goc3_bus_id`] (ascending).
    // Every uid in `self.bus` was used to build `self.bus_id_by_uid` in the
    // same pass in `parse`, so the direct index below never panics.
    fn bus_order(&self) -> Vec<String> {
        let mut uids = self.bus.uids().to_vec();
        uids.sort_by_key(|uid| self.bus_id_by_uid[uid]);
        uids
    }

    /// All simple dispatchable device uids, ordered by ascending GOC3 uid
    /// number. `src/goc3.jl` computes this once as `sdd_order` in
    /// `_goc3_static_data`; this port reuses the same order for the energy
    /// window and reserve membership builders, which iterate a `Dict` in
    /// `src/goc3.jl` (see the module-level order note).
    fn sdd_order(&self) -> Result<Vec<String>> {
        uidnum_order(self.sdd.uids())
    }

    /// Simple dispatchable device uids grouped by their bus uid, each group
    /// in [`Goc3Tables::sdd_order`]. The Rust equivalent of
    /// `_goc3_static_data`'s `devices_by_bus`.
    fn devices_by_bus(&self) -> Result<BTreeMap<String, Vec<String>>> {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for uid in self.sdd_order()? {
            let obj = self.sdd.get(&uid)?;
            let bus = require_str(obj, "bus")?.to_owned();
            map.entry(bus).or_default().push(uid);
        }
        Ok(map)
    }

    fn contingencies(&self) -> Result<&[Value]> {
        self.contingencies
            .as_deref()
            .ok_or_else(|| json_error("missing `reliability.contingency`"))
    }
}

// ---------------------------------------------------------------------------
// Static data (`_goc3_static_data` in `src/goc3.jl`).
// ---------------------------------------------------------------------------

/// One bus row: `(i, uid, v_min, v_max)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3BusRow {
    pub i: BusId,
    pub uid: String,
    pub v_min: f64,
    pub v_max: f64,
}

/// One shunt row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3ShuntRow {
    pub uid: String,
    pub bus: BusId,
    pub g_sh: f64,
    pub b_sh: f64,
}

/// One AC line row (`AclRow` in `src/goc3.jl`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3AcLineRow {
    pub j_ln: usize,
    pub uid: String,
    pub to_bus: BusId,
    pub fr_bus: BusId,
    pub c_su: f64,
    pub c_sd: f64,
    pub s_max: f64,
    pub g_sr: f64,
    pub b_sr: f64,
    pub b_ch: f64,
    pub g_fr: f64,
    pub g_to: f64,
    pub b_fr: f64,
    pub b_to: f64,
}

/// One two-winding transformer row (`AcxRow` in `src/goc3.jl`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3TransformerRow {
    pub j_xf: usize,
    pub uid: String,
    pub to_bus: BusId,
    pub fr_bus: BusId,
    pub c_su: f64,
    pub c_sd: f64,
    pub s_max: f64,
    pub g_sr: f64,
    pub b_sr: f64,
    pub b_ch: f64,
    pub g_fr: f64,
    pub g_to: f64,
    pub b_fr: f64,
    pub b_to: f64,
}

/// One DC line row (`DcRow` in `src/goc3.jl`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3DcLineRow {
    pub j_dc: usize,
    pub uid: String,
    pub pdc_max: f64,
    pub qdc_fr_min: f64,
    pub qdc_to_min: f64,
    pub qdc_fr_max: f64,
    pub qdc_to_max: f64,
    pub to_bus: BusId,
    pub fr_bus: BusId,
}

/// A transformer with a variable phase-shift control range (`vpd`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3VariablePhaseRow {
    pub j_xf: usize,
    pub phi_min: f64,
    pub phi_max: f64,
}

/// A transformer with a fixed phase shift (`fpd`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3FixedPhaseRow {
    pub j_xf: usize,
    pub phi_o: f64,
}

/// A transformer with a variable winding ratio control range (`vwr`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3VariableRatioRow {
    pub j_xf: usize,
    pub tau_min: f64,
    pub tau_max: f64,
}

/// A transformer with a fixed winding ratio (`fwr`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3FixedRatioRow {
    pub j_xf: usize,
    pub tau_o: f64,
}

/// One simple dispatchable device row: producers and consumers share this
/// layout (`SddRow` in `src/goc3.jl`), and `prod`/`cons` split by
/// `device_type`. The multi-period fields (`c_rgu`, `p_max`, `sus`, ...) are
/// the "per-device time series" `src/goc3.jl` keeps here as ordinary vectors
/// indexed by period, rather than as a separate overlay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3DeviceRow {
    pub bus: BusId,
    pub uid: String,
    pub c_on: f64,
    pub c_su: f64,
    pub c_sd: f64,
    pub p_ru: f64,
    pub p_rd: f64,
    pub p_ru_su: f64,
    pub p_rd_sd: f64,
    pub c_rgu: Vec<f64>,
    pub c_rgd: Vec<f64>,
    pub c_scr: Vec<f64>,
    pub c_nsc: Vec<f64>,
    pub c_rru_on: Vec<f64>,
    pub c_rru_off: Vec<f64>,
    pub c_rrd_on: Vec<f64>,
    pub c_rrd_off: Vec<f64>,
    pub c_qru: Vec<f64>,
    pub c_qrd: Vec<f64>,
    pub p_rgu_max: f64,
    pub p_rgd_max: f64,
    pub p_scr_max: f64,
    pub p_nsc_max: f64,
    pub p_rru_on_max: f64,
    pub p_rru_off_max: f64,
    pub p_rrd_on_max: f64,
    pub p_rrd_off_max: f64,
    pub p_0: f64,
    pub q_0: f64,
    pub p_max: Vec<f64>,
    pub p_min: Vec<f64>,
    pub q_max: Vec<f64>,
    pub q_min: Vec<f64>,
    pub sus: Vec<Vec<f64>>,
}

/// One active (real-power) zonal reserve row. The Greek `sigma_*` fields
/// serialize as `σ_*` on the wire (Julia's exact field spelling in
/// `ActiveReserveRow`) so a JSON consumer sees the same field names Julia
/// does; the Rust field names stay ASCII.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3ActiveReserveRow {
    pub n_p: usize,
    pub uid: String,
    pub c_rgu: f64,
    pub c_rgd: f64,
    pub c_scr: f64,
    pub c_nsc: f64,
    pub c_rru: f64,
    pub c_rrd: f64,
    #[serde(rename = "σ_rgu")]
    pub sigma_rgu: f64,
    #[serde(rename = "σ_rgd")]
    pub sigma_rgd: f64,
    #[serde(rename = "σ_scr")]
    pub sigma_scr: f64,
    #[serde(rename = "σ_nsc")]
    pub sigma_nsc: f64,
    pub p_rru_min: Vec<f64>,
    pub p_rrd_min: Vec<f64>,
}

/// One reactive (reactive-power) zonal reserve row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3ReactiveReserveRow {
    pub n_q: usize,
    pub uid: String,
    pub c_qru: f64,
    pub c_qrd: f64,
    pub q_qru_min: Vec<f64>,
    pub q_qrd_min: Vec<f64>,
}

/// One (bus, active reserve zone, device) membership row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3ActiveReserveSetRow {
    pub i: BusId,
    pub n_p: usize,
    pub uid: String,
}

/// One (bus, reactive reserve zone, device) membership row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3ReactiveReserveSetRow {
    pub i: BusId,
    pub n_q: usize,
    pub uid: String,
}

/// The per-class set sizes (`lengths` in `src/goc3.jl`). Field names carry
/// Julia's exact spelling on the wire via `#[serde(rename)]`; Rust source
/// uses idiomatic lowercase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Goc3Lengths {
    #[serde(rename = "L_J_xf")]
    pub l_j_xf: usize,
    #[serde(rename = "L_J_ln")]
    pub l_j_ln: usize,
    #[serde(rename = "L_J_ac")]
    pub l_j_ac: usize,
    #[serde(rename = "L_J_dc")]
    pub l_j_dc: usize,
    #[serde(rename = "L_J_br")]
    pub l_j_br: usize,
    #[serde(rename = "L_J_cs")]
    pub l_j_cs: usize,
    #[serde(rename = "L_J_pr")]
    pub l_j_pr: usize,
    #[serde(rename = "L_J_cspr")]
    pub l_j_cspr: usize,
    #[serde(rename = "L_J_sh")]
    pub l_j_sh: usize,
    /// Bus count (`I` in `src/goc3.jl`).
    #[serde(rename = "I")]
    pub i: usize,
    #[serde(rename = "L_T")]
    pub l_t: usize,
    #[serde(rename = "L_N_p")]
    pub l_n_p: usize,
    #[serde(rename = "L_N_q")]
    pub l_n_q: usize,
}

/// The static SCOPF index sets: buses, shunts, AC/DC branches, transformer
/// control sets, producers, consumers, zonal reserves, and device-zone
/// membership sets (`sc_data` in `src/goc3.jl`, `ScopfInstance.static`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Goc3Static {
    pub bus: Vec<Goc3BusRow>,
    pub shunt: Vec<Goc3ShuntRow>,
    pub acl_branch: Vec<Goc3AcLineRow>,
    pub acx_branch: Vec<Goc3TransformerRow>,
    pub vpd: Vec<Goc3VariablePhaseRow>,
    pub fpd: Vec<Goc3FixedPhaseRow>,
    pub vwr: Vec<Goc3VariableRatioRow>,
    pub fwr: Vec<Goc3FixedRatioRow>,
    pub dc_branch: Vec<Goc3DcLineRow>,
    pub prod: Vec<Goc3DeviceRow>,
    pub cons: Vec<Goc3DeviceRow>,
    pub active_reserve: Vec<Goc3ActiveReserveRow>,
    pub reactive_reserve: Vec<Goc3ReactiveReserveRow>,
    pub active_reserve_set_pr: Vec<Goc3ActiveReserveSetRow>,
    pub active_reserve_set_cs: Vec<Goc3ActiveReserveSetRow>,
    pub reactive_reserve_set_pr: Vec<Goc3ReactiveReserveSetRow>,
    pub reactive_reserve_set_cs: Vec<Goc3ReactiveReserveSetRow>,
}

/// One producer/consumer energy cost curve, keyed by bus and uid
/// (`CostRow` in `src/goc3.jl`): `cost[t][m]` is `[c_en, p_max]` for cost
/// block `m` of period `t`. An intermediate between [`goc3_static_data`] and
/// [`goc3_price_blocks`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3CostRow {
    pub bus: BusId,
    pub uid: String,
    pub cost: Vec<Vec<[f64; 2]>>,
}

/// The result of [`goc3_static_data`]: the static index sets plus the
/// producer/consumer cost vectors [`goc3_price_blocks`] flattens. Mirrors
/// the 4 values `_goc3_static_data` returns in `src/goc3.jl`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Goc3StaticProjection {
    #[serde(rename = "static")]
    pub static_data: Goc3Static,
    pub lengths: Goc3Lengths,
    pub cost_vector_pr: Vec<Goc3CostRow>,
    pub cost_vector_cs: Vec<Goc3CostRow>,
}

impl Goc3Tables {
    fn cost_vector(&self, device_type: &str) -> Result<Vec<Goc3CostRow>> {
        let mut rows = Vec::new();
        for uid in self.sdd_order()? {
            let val = self.sdd.get(&uid)?;
            if require_str(val, "device_type")? != device_type {
                continue;
            }
            let ts_val = self.sdd_ts.get(&uid)?;
            let bus = self.goc3_bus_id(require_str(val, "bus")?)?;
            let cost = ts_val.get("cost").ok_or_else(|| {
                json_error(format!(
                    "simple_dispatchable_device time series `{uid}` missing `cost`"
                ))
            })?;
            rows.push(Goc3CostRow {
                bus,
                uid,
                cost: cost_cube(cost)?,
            });
        }
        Ok(rows)
    }

    fn twt_variable_phase(&self) -> Result<Vec<Goc3VariablePhaseRow>> {
        let mut rows = Vec::new();
        for uid in uidnum_order(self.twt.uids())? {
            let val = self.twt.get(&uid)?;
            let (lb, ub) = (require_num(val, "ta_lb")?, require_num(val, "ta_ub")?);
            if lb < ub {
                rows.push(Goc3VariablePhaseRow {
                    j_xf: uid_num(&uid)? + 1,
                    phi_min: lb,
                    phi_max: ub,
                });
            }
        }
        rows.sort_by_key(|r| r.j_xf);
        Ok(rows)
    }

    fn twt_fixed_phase(&self) -> Result<Vec<Goc3FixedPhaseRow>> {
        let mut rows = Vec::new();
        for uid in uidnum_order(self.twt.uids())? {
            let val = self.twt.get(&uid)?;
            let (lb, ub) = (require_num(val, "ta_lb")?, require_num(val, "ta_ub")?);
            if lb >= ub {
                let phi_o = require_num(initial_status(val)?, "ta")?;
                rows.push(Goc3FixedPhaseRow {
                    j_xf: uid_num(&uid)? + 1,
                    phi_o,
                });
            }
        }
        rows.sort_by_key(|r| r.j_xf);
        Ok(rows)
    }

    fn twt_variable_ratio(&self) -> Result<Vec<Goc3VariableRatioRow>> {
        let mut rows = Vec::new();
        for uid in uidnum_order(self.twt.uids())? {
            let val = self.twt.get(&uid)?;
            let (lb, ub) = (require_num(val, "tm_lb")?, require_num(val, "tm_ub")?);
            if lb < ub {
                rows.push(Goc3VariableRatioRow {
                    j_xf: uid_num(&uid)? + 1,
                    tau_min: lb,
                    tau_max: ub,
                });
            }
        }
        rows.sort_by_key(|r| r.j_xf);
        Ok(rows)
    }

    fn twt_fixed_ratio(&self) -> Result<Vec<Goc3FixedRatioRow>> {
        let mut rows = Vec::new();
        for uid in uidnum_order(self.twt.uids())? {
            let val = self.twt.get(&uid)?;
            let (lb, ub) = (require_num(val, "tm_lb")?, require_num(val, "tm_ub")?);
            if lb >= ub {
                let tau_o = require_num(initial_status(val)?, "tm")?;
                rows.push(Goc3FixedRatioRow {
                    j_xf: uid_num(&uid)? + 1,
                    tau_o,
                });
            }
        }
        rows.sort_by_key(|r| r.j_xf);
        Ok(rows)
    }

    fn sdd_row(&self, uid: &str) -> Result<Goc3DeviceRow> {
        const SDD: &str = "simple_dispatchable_device";
        const SDD_TS: &str = "simple_dispatchable_device time series";
        let val = self.sdd.get(uid)?;
        let ts_val = self.sdd_ts.get(uid)?;
        let initial = initial_status(val)?;
        let ts = |key| require_field(ts_val, SDD_TS, uid, key);
        Ok(Goc3DeviceRow {
            bus: self.goc3_bus_id(require_str(val, "bus")?)?,
            uid: uid.to_owned(),
            c_on: require_num(val, "on_cost")?,
            c_su: require_num(val, "startup_cost")?,
            c_sd: require_num(val, "shutdown_cost")?,
            p_ru: require_num(val, "p_ramp_up_ub")?,
            p_rd: require_num(val, "p_ramp_down_ub")?,
            p_ru_su: require_num(val, "p_startup_ramp_ub")?,
            p_rd_sd: require_num(val, "p_shutdown_ramp_ub")?,
            c_rgu: float_vec(ts("p_reg_res_up_cost")?)?,
            c_rgd: float_vec(ts("p_reg_res_down_cost")?)?,
            c_scr: float_vec(ts("p_syn_res_cost")?)?,
            c_nsc: float_vec(ts("p_nsyn_res_cost")?)?,
            c_rru_on: float_vec(ts("p_ramp_res_up_online_cost")?)?,
            c_rru_off: float_vec(ts("p_ramp_res_up_offline_cost")?)?,
            c_rrd_on: float_vec(ts("p_ramp_res_down_online_cost")?)?,
            c_rrd_off: float_vec(ts("p_ramp_res_down_offline_cost")?)?,
            c_qru: float_vec(ts("q_res_up_cost")?)?,
            c_qrd: float_vec(ts("q_res_down_cost")?)?,
            p_rgu_max: require_num(val, "p_reg_res_up_ub")?,
            p_rgd_max: require_num(val, "p_reg_res_down_ub")?,
            p_scr_max: require_num(val, "p_syn_res_ub")?,
            p_nsc_max: require_num(val, "p_nsyn_res_ub")?,
            p_rru_on_max: require_num(val, "p_ramp_res_up_online_ub")?,
            p_rru_off_max: require_num(val, "p_ramp_res_up_offline_ub")?,
            p_rrd_on_max: require_num(val, "p_ramp_res_down_online_ub")?,
            p_rrd_off_max: require_num(val, "p_ramp_res_down_offline_ub")?,
            p_0: require_num(initial, "p")?,
            q_0: require_num(initial, "q")?,
            p_max: float_vec(ts("p_ub")?)?,
            p_min: float_vec(ts("p_lb")?)?,
            q_max: float_vec(ts("q_ub")?)?,
            q_min: float_vec(ts("q_lb")?)?,
            sus: float_matrix(require_field(val, SDD, uid, "startup_states")?)?,
        })
    }

    fn sdd_rows(&self, device_type: &str) -> Result<Vec<Goc3DeviceRow>> {
        let mut rows = Vec::new();
        for uid in self.sdd_order()? {
            if require_str(self.sdd.get(&uid)?, "device_type")? == device_type {
                rows.push(self.sdd_row(&uid)?);
            }
        }
        Ok(rows)
    }

    /// One (bus, zone, device) membership set: `ids` is the sorted reserve
    /// zone uid list (`azr_ids`/`rzr_ids`), `uids_key` names the bus field
    /// listing its zone uids, `device_type` filters the zone's devices. The
    /// Rust equivalent of `reserve_set` in `src/goc3.jl`, iterating buses in
    /// [`Goc3Tables::bus_order`] and devices in
    /// [`Goc3Tables::devices_by_bus`] order (see the module-level order
    /// note; `src/goc3.jl` iterates both as `Dict`s here).
    fn reserve_set<R>(
        &self,
        ids: &[String],
        uids_key: &str,
        device_type: &str,
        mkrow: impl Fn(BusId, usize, String) -> R,
    ) -> Result<Vec<R>> {
        let devices_by_bus = self.devices_by_bus()?;
        let bus_order = self.bus_order();
        let mut rows = Vec::new();
        for id in ids {
            let num = uid_num(id)?;
            for bus_uid in &bus_order {
                let bus_obj = self.bus.get(bus_uid)?;
                let member = bus_obj
                    .get(uids_key)
                    .and_then(Value::as_array)
                    .is_some_and(|zones| zones.iter().any(|z| z.as_str() == Some(id.as_str())));
                if !member {
                    continue;
                }
                let Some(devices) = devices_by_bus.get(bus_uid) else {
                    continue;
                };
                for dev_uid in devices {
                    let device = self.sdd.get(dev_uid)?;
                    if require_str(device, "device_type")? == device_type {
                        // Zone number is 1-based (`num + 1`), matching every
                        // other `n_p`/`n_q` assignment in this module.
                        rows.push(mkrow(self.goc3_bus_id(bus_uid)?, num + 1, dev_uid.clone()));
                    }
                }
            }
        }
        Ok(rows)
    }
}

/// Build the static SCOPF index sets from parsed GOC3 tables
/// (`_goc3_static_data` in `src/goc3.jl`). Pure function of `tables`; no unit
/// commitment solution is used.
// One flat builder mirroring `_goc3_static_data`'s single `sc_data` literal
// in `src/goc3.jl`; splitting it into a builder per row family would scatter
// the one-to-one correspondence with the Julia source this port is checked
// against. `additional_shunt` is a discrete 0/1 flag read straight from
// JSON, not an accumulated float, so the exact comparison is intentional.
#[allow(clippy::too_many_lines, clippy::float_cmp)]
pub fn goc3_static_data(tables: &Goc3Tables) -> Result<Goc3StaticProjection> {
    let l_j_xf = tables.twt.uids().len();
    let l_j_ln = tables.ac_line.uids().len();
    let l_j_ac = l_j_ln + l_j_xf;
    let l_j_dc = tables.dc_line.uids().len();
    let l_j_br = l_j_ac + l_j_dc;
    let l_j_cs = tables.sdd_ids_consumer.len();
    let l_j_pr = tables.sdd_ids_producer.len();
    let l_j_cspr = l_j_cs + l_j_pr;
    let l_j_sh = tables.shunt.uids().len();
    let i = tables.bus.uids().len();
    let l_t = tables.dt.len();
    let l_n_p = tables.azr.uids().len();
    let l_n_q = tables.rzr.uids().len();

    let lengths = Goc3Lengths {
        l_j_xf,
        l_j_ln,
        l_j_ac,
        l_j_dc,
        l_j_br,
        l_j_cs,
        l_j_pr,
        l_j_cspr,
        l_j_sh,
        i,
        l_t,
        l_n_p,
        l_n_q,
    };

    let mut bus: Vec<Goc3BusRow> = tables
        .bus
        .uids()
        .iter()
        .map(|uid| {
            let val = tables.bus.get(uid)?;
            Ok(Goc3BusRow {
                i: tables.goc3_bus_id(uid)?,
                uid: uid.clone(),
                v_min: require_num(val, "vm_lb")?,
                v_max: require_num(val, "vm_ub")?,
            })
        })
        .collect::<Result<_>>()?;
    bus.sort_by_key(|r| r.i);

    let mut shunt: Vec<Goc3ShuntRow> = tables
        .shunt
        .uids()
        .iter()
        .map(|uid| {
            let val = tables.shunt.get(uid)?;
            Ok(Goc3ShuntRow {
                uid: uid.clone(),
                bus: tables.goc3_bus_id(require_str(val, "bus")?)?,
                g_sh: require_num(val, "gs")?,
                b_sh: require_num(val, "bs")?,
            })
        })
        .collect::<Result<_>>()?;
    shunt.sort_by_key(|r| uid_num(&r.uid).unwrap_or(0));

    let mut acl_branch: Vec<Goc3AcLineRow> = tables
        .ac_line
        .uids()
        .iter()
        .map(|uid| {
            let val = tables.ac_line.get(uid)?;
            let (g_sr, b_sr, b_ch, g_fr, g_to, b_fr, b_to) = branch_admittance(val)?;
            Ok(Goc3AcLineRow {
                j_ln: uid_num(uid)? + 1,
                uid: uid.clone(),
                to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
                c_su: require_num(val, "connection_cost")?,
                c_sd: require_num(val, "disconnection_cost")?,
                s_max: require_num(val, "mva_ub_nom")?,
                g_sr,
                b_sr,
                b_ch,
                g_fr,
                g_to,
                b_fr,
                b_to,
            })
        })
        .collect::<Result<_>>()?;
    acl_branch.sort_by_key(|r| r.j_ln);

    let mut acx_branch: Vec<Goc3TransformerRow> = tables
        .twt
        .uids()
        .iter()
        .map(|uid| {
            let val = tables.twt.get(uid)?;
            let (g_sr, b_sr, b_ch, g_fr, g_to, b_fr, b_to) = branch_admittance(val)?;
            Ok(Goc3TransformerRow {
                j_xf: uid_num(uid)? + 1,
                uid: uid.clone(),
                to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
                c_su: require_num(val, "connection_cost")?,
                c_sd: require_num(val, "disconnection_cost")?,
                s_max: require_num(val, "mva_ub_nom")?,
                g_sr,
                b_sr,
                b_ch,
                g_fr,
                g_to,
                b_fr,
                b_to,
            })
        })
        .collect::<Result<_>>()?;
    acx_branch.sort_by_key(|r| r.j_xf);

    let mut dc_branch: Vec<Goc3DcLineRow> = tables
        .dc_line
        .uids()
        .iter()
        .map(|uid| {
            let val = tables.dc_line.get(uid)?;
            Ok(Goc3DcLineRow {
                j_dc: uid_num(uid)? + 1,
                uid: uid.clone(),
                pdc_max: require_num(val, "pdc_ub")?,
                qdc_fr_min: require_num(val, "qdc_fr_lb")?,
                qdc_to_min: require_num(val, "qdc_to_lb")?,
                qdc_fr_max: require_num(val, "qdc_fr_ub")?,
                qdc_to_max: require_num(val, "qdc_to_ub")?,
                to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
            })
        })
        .collect::<Result<_>>()?;
    dc_branch.sort_by_key(|r| r.j_dc);

    let cost_vector_pr = tables.cost_vector("producer")?;
    let cost_vector_cs = tables.cost_vector("consumer")?;
    let prod = tables.sdd_rows("producer")?;
    let cons = tables.sdd_rows("consumer")?;

    let mut active_reserve: Vec<Goc3ActiveReserveRow> = tables
        .azr
        .uids()
        .iter()
        .map(|uid| {
            let val = tables.azr.get(uid)?;
            let ts_val = tables.azr_ts.get(uid)?;
            Ok(Goc3ActiveReserveRow {
                n_p: uid_num(uid)? + 1,
                uid: uid.clone(),
                c_rgu: require_num(val, "REG_UP_vio_cost")?,
                c_rgd: require_num(val, "REG_DOWN_vio_cost")?,
                c_scr: require_num(val, "SYN_vio_cost")?,
                c_nsc: require_num(val, "NSYN_vio_cost")?,
                c_rru: require_num(val, "RAMPING_RESERVE_UP_vio_cost")?,
                c_rrd: require_num(val, "RAMPING_RESERVE_DOWN_vio_cost")?,
                sigma_rgu: require_num(val, "REG_UP")?,
                sigma_rgd: require_num(val, "REG_DOWN")?,
                sigma_scr: require_num(val, "SYN")?,
                sigma_nsc: require_num(val, "NSYN")?,
                p_rru_min: float_vec(ts_val.get("RAMPING_RESERVE_UP").ok_or_else(|| {
                    json_error(format!(
                        "active_zonal_reserve time series `{uid}` missing `RAMPING_RESERVE_UP`"
                    ))
                })?)?,
                p_rrd_min: float_vec(ts_val.get("RAMPING_RESERVE_DOWN").ok_or_else(|| {
                    json_error(format!(
                        "active_zonal_reserve time series `{uid}` missing `RAMPING_RESERVE_DOWN`"
                    ))
                })?)?,
            })
        })
        .collect::<Result<_>>()?;
    active_reserve.sort_by_key(|r| r.n_p);

    let mut reactive_reserve: Vec<Goc3ReactiveReserveRow> = tables
        .rzr
        .uids()
        .iter()
        .map(|uid| {
            let val = tables.rzr.get(uid)?;
            let ts_val = tables.rzr_ts.get(uid)?;
            Ok(Goc3ReactiveReserveRow {
                n_q: uid_num(uid)? + 1,
                uid: uid.clone(),
                c_qru: require_num(val, "REACT_UP_vio_cost")?,
                c_qrd: require_num(val, "REACT_DOWN_vio_cost")?,
                q_qru_min: float_vec(ts_val.get("REACT_UP").ok_or_else(|| {
                    json_error(format!(
                        "reactive_zonal_reserve time series `{uid}` missing `REACT_UP`"
                    ))
                })?)?,
                q_qrd_min: float_vec(ts_val.get("REACT_DOWN").ok_or_else(|| {
                    json_error(format!(
                        "reactive_zonal_reserve time series `{uid}` missing `REACT_DOWN`"
                    ))
                })?)?,
            })
        })
        .collect::<Result<_>>()?;
    reactive_reserve.sort_by_key(|r| r.n_q);

    let active_reserve_set_pr = tables.reserve_set(
        &tables.azr_ids,
        "active_reserve_uids",
        "producer",
        |i, n_p, uid| Goc3ActiveReserveSetRow { i, n_p, uid },
    )?;
    let active_reserve_set_cs = tables.reserve_set(
        &tables.azr_ids,
        "active_reserve_uids",
        "consumer",
        |i, n_p, uid| Goc3ActiveReserveSetRow { i, n_p, uid },
    )?;
    let reactive_reserve_set_pr = tables.reserve_set(
        &tables.rzr_ids,
        "reactive_reserve_uids",
        "producer",
        |i, n_q, uid| Goc3ReactiveReserveSetRow { i, n_q, uid },
    )?;
    let reactive_reserve_set_cs = tables.reserve_set(
        &tables.rzr_ids,
        "reactive_reserve_uids",
        "consumer",
        |i, n_q, uid| Goc3ReactiveReserveSetRow { i, n_q, uid },
    )?;

    let static_data = Goc3Static {
        bus,
        shunt,
        acl_branch,
        acx_branch,
        vpd: tables.twt_variable_phase()?,
        fpd: tables.twt_fixed_phase()?,
        vwr: tables.twt_variable_ratio()?,
        fwr: tables.twt_fixed_ratio()?,
        dc_branch,
        prod,
        cons,
        active_reserve,
        reactive_reserve,
        active_reserve_set_pr,
        active_reserve_set_cs,
        reactive_reserve_set_pr,
        reactive_reserve_set_cs,
    };

    Ok(Goc3StaticProjection {
        static_data,
        lengths,
        cost_vector_pr,
        cost_vector_cs,
    })
}

// ---------------------------------------------------------------------------
// Energy windows (`_goc3_energy_windows` in `src/goc3.jl`).
// ---------------------------------------------------------------------------

macro_rules! energy_window_row {
    ($name:ident, $ind_field:ident, $start_field:ident, $end_field:ident, $bound_field:ident) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        pub struct $name {
            pub $ind_field: usize,
            pub uid: String,
            pub $start_field: f64,
            pub $end_field: f64,
            pub $bound_field: f64,
        }
    };
}

energy_window_row!(
    Goc3EnergyWindowMaxPrRow,
    w_en_max_pr_ind,
    a_en_max_start,
    a_en_max_end,
    e_max
);
energy_window_row!(
    Goc3EnergyWindowMaxCsRow,
    w_en_max_cs_ind,
    a_en_max_start,
    a_en_max_end,
    e_max
);
energy_window_row!(
    Goc3EnergyWindowMinPrRow,
    w_en_min_pr_ind,
    a_en_min_start,
    a_en_min_end,
    e_min
);
energy_window_row!(
    Goc3EnergyWindowMinCsRow,
    w_en_min_cs_ind,
    a_en_min_start,
    a_en_min_end,
    e_min
);

macro_rules! energy_window_period_row {
    ($name:ident, $ind_field:ident) => {
        /// Period membership of one energy window: the period belongs when
        /// its midpoint falls within the window's `(start, end]` interval.
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        pub struct $name {
            pub $ind_field: usize,
            pub uid: String,
            pub t: usize,
            pub dt: f64,
        }
    };
}

energy_window_period_row!(Goc3EnergyWindowPeriodMaxPrRow, w_en_max_pr_ind);
energy_window_period_row!(Goc3EnergyWindowPeriodMaxCsRow, w_en_max_cs_ind);
energy_window_period_row!(Goc3EnergyWindowPeriodMinPrRow, w_en_min_pr_ind);
energy_window_period_row!(Goc3EnergyWindowPeriodMinCsRow, w_en_min_cs_ind);

/// The multi-period energy requirement window sets and their per-period
/// membership sets, split by producer/consumer and by max/min. The Rust
/// equivalent of `_goc3_energy_windows`'s return value in `src/goc3.jl`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Goc3EnergyWindows {
    #[serde(rename = "W_en_max_pr")]
    pub w_en_max_pr: Vec<Goc3EnergyWindowMaxPrRow>,
    #[serde(rename = "W_en_max_cs")]
    pub w_en_max_cs: Vec<Goc3EnergyWindowMaxCsRow>,
    #[serde(rename = "W_en_min_pr")]
    pub w_en_min_pr: Vec<Goc3EnergyWindowMinPrRow>,
    #[serde(rename = "W_en_min_cs")]
    pub w_en_min_cs: Vec<Goc3EnergyWindowMinCsRow>,
    #[serde(rename = "T_w_en_max_pr")]
    pub t_w_en_max_pr: Vec<Goc3EnergyWindowPeriodMaxPrRow>,
    #[serde(rename = "T_w_en_max_cs")]
    pub t_w_en_max_cs: Vec<Goc3EnergyWindowPeriodMaxCsRow>,
    #[serde(rename = "T_w_en_min_pr")]
    pub t_w_en_min_pr: Vec<Goc3EnergyWindowPeriodMinPrRow>,
    #[serde(rename = "T_w_en_min_cs")]
    pub t_w_en_min_cs: Vec<Goc3EnergyWindowPeriodMinCsRow>,
}

/// Interval midpoints from cumulative durations (`goc3_interval_bounds`'s
/// midpoint, precomputed once for every period as `_goc3_energy_windows`
/// does in `src/goc3.jl`).
fn interval_midpoints(dt: &[f64]) -> Vec<f64> {
    let mut a_end = 0.0;
    dt.iter()
        .map(|d| {
            let start = a_end;
            a_end += d;
            f64::midpoint(start, a_end)
        })
        .collect()
}

/// One `(window_index, uid, start, end, bound)` row, before it is packed
/// into a [`Goc3EnergyWindowMaxPrRow`]-family struct.
type EnergyWindowTuple = (usize, String, f64, f64, f64);
/// One `(window_index, uid, period, duration)` row, before it is packed into
/// a [`Goc3EnergyWindowPeriodMaxPrRow`]-family struct.
type EnergyWindowPeriodTuple = (usize, String, usize, f64);

/// One energy-requirement window set and its per-period membership rows in
/// one pass: `device_type`/`req_key` select the producer/consumer max/min
/// window list. The Rust equivalent of `windows` and `window_periods`
/// together in `src/goc3.jl`'s `_goc3_energy_windows` (there, two separate
/// passes over the same device/window set; fused here since a window row and
/// its period memberships come from the same parsed `(start, end, bound)`).
/// Device iteration uses [`Goc3Tables::sdd_order`] (see the module-level
/// order note; `src/goc3.jl` iterates `keys(data.sdd_lookup)`, a `Dict`,
/// here).
fn sdd_windows(
    tables: &Goc3Tables,
    a_mid: &[f64],
    device_type: &str,
    req_key: &str,
    eps: f64,
) -> Result<(Vec<EnergyWindowTuple>, Vec<EnergyWindowPeriodTuple>)> {
    let mut windows = Vec::new();
    let mut window_periods = Vec::new();
    let mut ind = 0usize;
    for uid in tables.sdd_order()? {
        let val = tables.sdd.get(&uid)?;
        if require_str(val, "device_type")? != device_type {
            continue;
        }
        let req = require_field(val, "simple_dispatchable_device", &uid, req_key)?
            .as_array()
            .ok_or_else(|| {
                json_error(format!(
                    "simple_dispatchable_device `{uid}` `{req_key}` is not an array"
                ))
            })?;
        for w in req {
            let w = float_vec(w)?;
            let [start, end, bound] = w[..] else {
                return Err(json_error(format!(
                    "simple_dispatchable_device `{uid}` `{req_key}` window is not a 3-element array"
                )));
            };
            ind += 1;
            windows.push((ind, uid.clone(), start, end, bound));
            for (t0, &m) in a_mid.iter().enumerate() {
                if start + eps < m && m <= end + eps {
                    window_periods.push((ind, uid.clone(), t0 + 1, tables.dt[t0]));
                }
            }
        }
    }
    Ok((windows, window_periods))
}

/// Build the multi-interval energy requirement window sets and their
/// per-period membership sets (`_goc3_energy_windows` in `src/goc3.jl`).
/// Pure function of `tables`.
// Four max/min x producer/consumer variants, each packed into its own
// distinctly-named row struct to keep Julia's exact field spelling on the
// wire (see the module doc comment); the packing is what pushes this over
// the line budget.
#[allow(clippy::too_many_lines)]
pub fn goc3_energy_windows(tables: &Goc3Tables) -> Result<Goc3EnergyWindows> {
    const EPS_TIME: f64 = 1e-6;
    let a_mid = interval_midpoints(&tables.dt);

    let (max_pr, t_max_pr) = sdd_windows(tables, &a_mid, "producer", "energy_req_ub", EPS_TIME)?;
    let (max_cs, t_max_cs) = sdd_windows(tables, &a_mid, "consumer", "energy_req_ub", EPS_TIME)?;
    let (min_pr, t_min_pr) = sdd_windows(tables, &a_mid, "producer", "energy_req_lb", EPS_TIME)?;
    let (min_cs, t_min_cs) = sdd_windows(tables, &a_mid, "consumer", "energy_req_lb", EPS_TIME)?;

    let w_en_max_pr = max_pr
        .into_iter()
        .map(
            |(w_en_max_pr_ind, uid, a_en_max_start, a_en_max_end, e_max)| {
                Goc3EnergyWindowMaxPrRow {
                    w_en_max_pr_ind,
                    uid,
                    a_en_max_start,
                    a_en_max_end,
                    e_max,
                }
            },
        )
        .collect();
    let w_en_max_cs = max_cs
        .into_iter()
        .map(
            |(w_en_max_cs_ind, uid, a_en_max_start, a_en_max_end, e_max)| {
                Goc3EnergyWindowMaxCsRow {
                    w_en_max_cs_ind,
                    uid,
                    a_en_max_start,
                    a_en_max_end,
                    e_max,
                }
            },
        )
        .collect();
    let w_en_min_pr = min_pr
        .into_iter()
        .map(
            |(w_en_min_pr_ind, uid, a_en_min_start, a_en_min_end, e_min)| {
                Goc3EnergyWindowMinPrRow {
                    w_en_min_pr_ind,
                    uid,
                    a_en_min_start,
                    a_en_min_end,
                    e_min,
                }
            },
        )
        .collect();
    let w_en_min_cs = min_cs
        .into_iter()
        .map(
            |(w_en_min_cs_ind, uid, a_en_min_start, a_en_min_end, e_min)| {
                Goc3EnergyWindowMinCsRow {
                    w_en_min_cs_ind,
                    uid,
                    a_en_min_start,
                    a_en_min_end,
                    e_min,
                }
            },
        )
        .collect();

    let t_w_en_max_pr = t_max_pr
        .into_iter()
        .map(
            |(w_en_max_pr_ind, uid, t, dt)| Goc3EnergyWindowPeriodMaxPrRow {
                w_en_max_pr_ind,
                uid,
                t,
                dt,
            },
        )
        .collect();
    let t_w_en_max_cs = t_max_cs
        .into_iter()
        .map(
            |(w_en_max_cs_ind, uid, t, dt)| Goc3EnergyWindowPeriodMaxCsRow {
                w_en_max_cs_ind,
                uid,
                t,
                dt,
            },
        )
        .collect();
    let t_w_en_min_pr = t_min_pr
        .into_iter()
        .map(
            |(w_en_min_pr_ind, uid, t, dt)| Goc3EnergyWindowPeriodMinPrRow {
                w_en_min_pr_ind,
                uid,
                t,
                dt,
            },
        )
        .collect();
    let t_w_en_min_cs = t_min_cs
        .into_iter()
        .map(
            |(w_en_min_cs_ind, uid, t, dt)| Goc3EnergyWindowPeriodMinCsRow {
                w_en_min_cs_ind,
                uid,
                t,
                dt,
            },
        )
        .collect();

    Ok(Goc3EnergyWindows {
        w_en_max_pr,
        w_en_max_cs,
        w_en_min_pr,
        w_en_min_cs,
        t_w_en_max_pr,
        t_w_en_max_cs,
        t_w_en_min_pr,
        t_w_en_min_cs,
    })
}

// ---------------------------------------------------------------------------
// Price blocks (`_goc3_price_blocks` in `src/goc3.jl`).
// ---------------------------------------------------------------------------

/// One flattened (device, period, cost-block) price row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3PriceBlockRow {
    pub flat_k: usize,
    pub uid: String,
    pub t: usize,
    pub m: usize,
    pub c_en: f64,
    pub p_max: f64,
}

/// Flattened producer/consumer price blocks (the result of
/// [`goc3_price_blocks`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Goc3PriceBlocks {
    pub producer: Vec<Goc3PriceBlockRow>,
    pub consumer: Vec<Goc3PriceBlockRow>,
}

fn flatten_price_blocks(cost_vector: &[Goc3CostRow]) -> Vec<Goc3PriceBlockRow> {
    let mut rows = Vec::new();
    let mut flat_k = 1usize;
    for pc in cost_vector {
        for (t0, cost_t) in pc.cost.iter().enumerate() {
            for (m0, cost_tm) in cost_t.iter().enumerate() {
                let (c_en, p_max) = (cost_tm[0], cost_tm[1]);
                rows.push(Goc3PriceBlockRow {
                    flat_k,
                    uid: pc.uid.clone(),
                    t: t0 + 1,
                    m: m0 + 1,
                    c_en,
                    p_max,
                });
                flat_k += 1;
            }
        }
    }
    rows
}

/// Flatten the per-device energy cost curves into one row per (device,
/// period, cost block), unscaled in the GOC3 document's own per-unit
/// convention (`_goc3_price_blocks` in `src/goc3.jl`). Pure function of the
/// cost vectors [`goc3_static_data`] returns; infallible, since
/// [`Goc3CostRow::cost`](Goc3CostRow) is already validated numeric data.
pub fn goc3_price_blocks(
    cost_vector_pr: &[Goc3CostRow],
    cost_vector_cs: &[Goc3CostRow],
) -> Goc3PriceBlocks {
    Goc3PriceBlocks {
        producer: flatten_price_blocks(cost_vector_pr),
        consumer: flatten_price_blocks(cost_vector_cs),
    }
}

// ---------------------------------------------------------------------------
// AC contingency survivors (`_goc3_ac_contingency_survivors` in
// `src/goc3.jl`).
// ---------------------------------------------------------------------------

/// One AC line surviving a contingency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3AcLineSurvivorRow {
    pub ctg: usize,
    pub j_ln: usize,
    pub uid: String,
    pub to_bus: BusId,
    pub fr_bus: BusId,
    pub b_sr: f64,
    pub s_max_ctg: f64,
}

/// One transformer surviving a contingency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3TransformerSurvivorRow {
    pub ctg: usize,
    pub j_xf: usize,
    pub uid: String,
    pub to_bus: BusId,
    pub fr_bus: BusId,
    pub b_sr: f64,
    pub s_max_ctg: f64,
}

/// Per-contingency surviving AC lines and transformers, one group per
/// contingency in `reliability.contingency`'s document order (the result of
/// [`goc3_ac_contingency_survivors`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Goc3AcContingencySurvivors {
    pub ln: Vec<Vec<Goc3AcLineSurvivorRow>>,
    pub xf: Vec<Vec<Goc3TransformerSurvivorRow>>,
}

/// Series reactance to series susceptance: `b_sr = -x / (x^2 + r^2)`, the
/// same rectangular-to-admittance step `src/goc3.jl` applies per branch
/// (`_goc3_static_data`'s `acl_branch`/`acx_branch` and
/// `_goc3_ac_contingency_survivors`'s survivor rows alike).
fn b_sr(r: f64, x: f64) -> f64 {
    -x / (x * x + r * r)
}

/// Series admittance and terminal shunt parameters shared by AC lines and
/// transformers: `(g_sr, b_sr, b_ch, g_fr, g_to, b_fr, b_to)`, from
/// `r`/`x`/`b` and, when `additional_shunt` is set, `g_fr`/`g_to`/`b_fr`/
/// `b_to`. The common body of `acl_branch`/`acx_branch` in
/// `_goc3_static_data` (`src/goc3.jl`). `additional_shunt` is a discrete 0/1
/// flag read straight from JSON, not an accumulated float, so the exact
/// comparison is intentional.
#[allow(clippy::type_complexity, clippy::float_cmp)]
fn branch_admittance(val: &Map<String, Value>) -> Result<(f64, f64, f64, f64, f64, f64, f64)> {
    let (r, x) = (require_num(val, "r")?, require_num(val, "x")?);
    let g_sr = r / (x * x + r * r);
    let additional_shunt = require_num(val, "additional_shunt")? == 1.0;
    let (g_fr, g_to, b_fr, b_to) = if additional_shunt {
        (
            require_num(val, "g_fr")?,
            require_num(val, "g_to")?,
            require_num(val, "b_fr")?,
            require_num(val, "b_to")?,
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };
    Ok((
        g_sr,
        b_sr(r, x),
        require_num(val, "b")?,
        g_fr,
        g_to,
        b_fr,
        b_to,
    ))
}

/// One contingency's 1-based index and its outaged component uids
/// (`ctg_idx`, `outaged` in `_goc3_ac_contingency_survivors` /
/// `_goc3_dc_contingency_flows` in `src/goc3.jl`).
fn contingency_outages(ctg: &Value) -> Result<(usize, HashSet<&str>)> {
    let ctg_obj = ctg
        .as_object()
        .ok_or_else(|| json_error("reliability.contingency item is not an object"))?;
    let ctg_uid = require_str(ctg_obj, "uid")?;
    let ctg_idx = uid_num(ctg_uid)? + 1;
    let outaged = require_field(ctg_obj, "contingency", ctg_uid, "components")?
        .as_array()
        .ok_or_else(|| {
            json_error(format!(
                "contingency `{ctg_uid}` `components` is not an array"
            ))
        })?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| json_error("component uid is not a string"))
        })
        .collect::<Result<_>>()?;
    Ok((ctg_idx, outaged))
}

/// Enumerate, for each contingency, the AC lines and transformers that
/// remain in service: the branch is not among the contingency's outaged
/// components (`_goc3_ac_contingency_survivors` in `src/goc3.jl`). The outer
/// vector follows `reliability.contingency`'s document order (which need not
/// match ascending `ctg`); rows within one contingency follow the section's
/// document order (see the module-level order note; `src/goc3.jl` iterates
/// `values(lookup)`, a `Dict`, here).
pub fn goc3_ac_contingency_survivors(tables: &Goc3Tables) -> Result<Goc3AcContingencySurvivors> {
    let contingencies = tables.contingencies()?;

    let mut ln = Vec::with_capacity(contingencies.len());
    let mut xf = Vec::with_capacity(contingencies.len());
    for ctg in contingencies {
        let (ctg_idx, outaged) = contingency_outages(ctg)?;

        let mut ln_rows = Vec::new();
        for uid in tables.ac_line.uids() {
            if outaged.contains(uid.as_str()) {
                continue;
            }
            let val = tables.ac_line.get(uid)?;
            let (r, x) = (require_num(val, "r")?, require_num(val, "x")?);
            ln_rows.push(Goc3AcLineSurvivorRow {
                ctg: ctg_idx,
                j_ln: uid_num(uid)? + 1,
                uid: uid.clone(),
                to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
                b_sr: b_sr(r, x),
                s_max_ctg: require_num(val, "mva_ub_em")?,
            });
        }
        ln.push(ln_rows);

        let mut xf_rows = Vec::new();
        for uid in tables.twt.uids() {
            if outaged.contains(uid.as_str()) {
                continue;
            }
            let val = tables.twt.get(uid)?;
            let (r, x) = (require_num(val, "r")?, require_num(val, "x")?);
            xf_rows.push(Goc3TransformerSurvivorRow {
                ctg: ctg_idx,
                j_xf: uid_num(uid)? + 1,
                uid: uid.clone(),
                to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
                b_sr: b_sr(r, x),
                s_max_ctg: require_num(val, "mva_ub_em")?,
            });
        }
        xf.push(xf_rows);
    }

    Ok(Goc3AcContingencySurvivors { ln, xf })
}

// ---------------------------------------------------------------------------
// DC contingency flows (`_goc3_dc_contingency_flows` in `src/goc3.jl`).
// ---------------------------------------------------------------------------

/// One (contingency, period, surviving DC line) row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Goc3DcContingencyFlowRow {
    pub flat_jtk_dc: usize,
    pub ctg: usize,
    pub j_dc: usize,
    pub to_bus: BusId,
    pub fr_bus: BusId,
    pub t: usize,
    pub dt: f64,
}

/// Enumerate the surviving DC lines for each contingency and period,
/// flattened (`_goc3_dc_contingency_flows` in `src/goc3.jl`). Fully pure: no
/// unit commitment status is involved for DC lines. Contingencies follow
/// `reliability.contingency`'s document order; DC lines within one
/// (contingency, period) follow the section's document order (see the
/// module-level order note; `src/goc3.jl` iterates `values(...)`, a `Dict`,
/// here).
pub fn goc3_dc_contingency_flows(tables: &Goc3Tables) -> Result<Vec<Goc3DcContingencyFlowRow>> {
    let contingencies = tables.contingencies()?;
    let mut rows = Vec::new();
    let mut flat_jtk_dc = 1usize;
    for ctg in contingencies {
        let (ctg_idx, outaged) = contingency_outages(ctg)?;

        for (t0, &dt) in tables.dt.iter().enumerate() {
            for uid in tables.dc_line.uids() {
                if outaged.contains(uid.as_str()) {
                    continue;
                }
                let val = tables.dc_line.get(uid)?;
                rows.push(Goc3DcContingencyFlowRow {
                    flat_jtk_dc,
                    ctg: ctg_idx,
                    j_dc: uid_num(uid)? + 1,
                    to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                    fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
                    t: t0 + 1,
                    dt,
                });
                flat_jtk_dc += 1;
            }
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// The combined SCOPF instance (`ScopfInstance` / `goc3_scopf_data` in
// `src/goc3.jl`).
// ---------------------------------------------------------------------------

/// The derived, format-neutral security-constrained OPF instance a GOC3 case
/// reduces to (`ScopfInstance` in `src/goc3.jl`). Every field is keyed by
/// `uid` and per-class GOC3 ordering, with no model-specific stacked
/// variable index: a client reads these fields and threads its own solver
/// indices and unit commitment status on top. See the module documentation
/// for the naming and layering note relative to `powerio-opf`'s future
/// public `ScopfInstance` (eigenergy/powerio#238).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Goc3ScopfData {
    /// Buses, shunts, AC/DC branches, transformer control sets, producers,
    /// consumers, zonal reserves, and device-zone membership sets. Wire name
    /// `static`, matching `ScopfInstance.static` in `src/goc3.jl` (`static`
    /// is a Rust keyword, so the Rust field is `static_data`).
    #[serde(rename = "static")]
    pub static_data: Goc3Static,
    pub lengths: Goc3Lengths,
    pub energy_windows: Goc3EnergyWindows,
    pub price_blocks: Goc3PriceBlocks,
    pub ac_contingency_survivors: Goc3AcContingencySurvivors,
    pub dc_contingency_flows: Vec<Goc3DcContingencyFlowRow>,
}

/// Build the security-constrained OPF instance from parsed GOC3 tables in
/// one call (`goc3_scopf_data` in `src/goc3.jl`). Pure function of `tables`:
/// no unit commitment solution and no model-specific variable numbering.
pub fn goc3_scopf_data(tables: &Goc3Tables) -> Result<Goc3ScopfData> {
    let Goc3StaticProjection {
        static_data,
        lengths,
        cost_vector_pr,
        cost_vector_cs,
    } = goc3_static_data(tables)?;
    Ok(Goc3ScopfData {
        static_data,
        lengths,
        energy_windows: goc3_energy_windows(tables)?,
        price_blocks: goc3_price_blocks(&cost_vector_pr, &cost_vector_cs),
        ac_contingency_survivors: goc3_ac_contingency_survivors(tables)?,
        dc_contingency_flows: goc3_dc_contingency_flows(tables)?,
    })
}

/// Parse GOC3 JSON text and build its SCOPF instance in one call: the
/// composition `Goc3Tables::parse` then [`goc3_scopf_data`] that
/// `pio_goc3_scopf_data_json` (the `powerio-capi` `pkg` feature) exposes over
/// the C ABI.
pub fn goc3_scopf_data_from_str(text: &str) -> Result<Goc3ScopfData> {
    let tables = Goc3Tables::parse(text)?;
    goc3_scopf_data(&tables)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 2-bus/2-AC-line/1-transformer/1-DC-line/1-producer/1-consumer
    /// synthetic case from PowerIO.jl's `test/test_goc3_static.jl`
    /// ("GO Challenge 3 static index sets"), transcribed to GOC3 JSON so both
    /// sides of the port can be checked against the same hand-verified
    /// numbers.
    const SMALL_FIXTURE: &str = include_str!("../tests/data/goc3_small.json");

    fn small_tables() -> Goc3Tables {
        Goc3Tables::parse(SMALL_FIXTURE).expect("parse small GOC3 fixture")
    }

    #[test]
    fn static_data_matches_hand_checked_small_fixture() {
        let tables = small_tables();
        let projection = goc3_static_data(&tables).expect("static data");
        let lengths = projection.lengths;
        assert_eq!(lengths.l_j_ln, 2);
        assert_eq!(lengths.l_j_xf, 1);
        assert_eq!(lengths.l_j_ac, 3);
        assert_eq!(lengths.l_j_dc, 1);
        assert_eq!(lengths.l_j_br, 4);
        assert_eq!(lengths.l_j_pr, 1);
        assert_eq!(lengths.l_j_cs, 1);
        assert_eq!(lengths.l_j_cspr, 2);
        assert_eq!(lengths.i, 2);
        assert_eq!(lengths.l_t, 2);
        assert_eq!(lengths.l_n_p, 1);
        assert_eq!(lengths.l_n_q, 1);

        let sc = &projection.static_data;
        assert_eq!(
            sc.bus.iter().map(|b| b.i).collect::<Vec<_>>(),
            vec![BusId(1), BusId(2)]
        );
        assert_eq!(sc.prod.len(), 1);
        assert_eq!(sc.prod[0].uid, "sd_00");
        assert_eq!(sc.cons.len(), 1);
        assert_eq!(sc.cons[0].uid, "sd_01");
        assert_eq!(sc.acl_branch[0].j_ln, 1);
        assert_eq!(sc.acl_branch[1].j_ln, 2);
        assert_eq!(sc.acx_branch[0].j_xf, 1);
        assert_eq!(sc.dc_branch[0].j_dc, 1);
        assert!(sc.vpd.is_empty() && sc.vwr.is_empty());
        assert_eq!(sc.fpd.len(), 1);
        assert_eq!(sc.fwr.len(), 1);
        assert_eq!(sc.fpd[0].j_xf, 1);
        assert_eq!(sc.fwr[0].j_xf, 1);
        assert_eq!(projection.cost_vector_pr[0].uid, "sd_00");
        assert_eq!(projection.cost_vector_cs[0].uid, "sd_01");
        assert_eq!(sc.active_reserve[0].n_p, 1);
        assert_eq!(sc.reactive_reserve[0].n_q, 1);
        assert_eq!(sc.active_reserve_set_pr[0].n_p, 1);
        assert_eq!(sc.active_reserve_set_cs[0].n_p, 1);
        assert_eq!(sc.reactive_reserve_set_pr[0].n_q, 1);
        assert_eq!(sc.reactive_reserve_set_cs[0].n_q, 1);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn energy_windows_match_hand_checked_small_fixture() {
        let tables = small_tables();
        let ew = goc3_energy_windows(&tables).expect("energy windows");
        assert_eq!(ew.w_en_max_pr.len(), 1);
        assert_eq!(ew.w_en_max_pr[0].uid, "sd_00");
        assert_eq!(ew.w_en_max_pr[0].e_max, 9.0);
        assert_eq!(ew.w_en_max_pr[0].a_en_max_end, 2.0);
        assert!(ew.w_en_max_cs.is_empty());
        assert_eq!(ew.w_en_min_pr.len(), 1);
        assert_eq!(ew.w_en_min_pr[0].e_min, 1.0);
        // Both period midpoints (0.5, 1.5) fall inside (0, 2].
        assert_eq!(ew.t_w_en_max_pr.len(), 2);
        assert_eq!(
            ew.t_w_en_max_pr.iter().map(|r| r.t).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(ew.t_w_en_min_pr.len(), 2);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn price_blocks_match_hand_checked_small_fixture() {
        let tables = small_tables();
        let projection = goc3_static_data(&tables).expect("static data");
        let blocks = goc3_price_blocks(&projection.cost_vector_pr, &projection.cost_vector_cs);
        assert_eq!(blocks.producer.len(), 2); // 2 periods x 1 block
        assert_eq!(
            blocks.producer.iter().map(|r| r.flat_k).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            blocks.producer.iter().map(|r| r.t).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(blocks.producer[0].c_en, 10.0);
        assert_eq!(blocks.producer[0].p_max, 5.0);
        assert_eq!(blocks.producer[1].c_en, 11.0);
        assert_eq!(blocks.producer[1].p_max, 6.0);
        assert_eq!(blocks.producer[0].uid, "sd_00");
        assert_eq!(blocks.consumer.len(), 2);
        assert_eq!(blocks.consumer[0].uid, "sd_01");
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn ac_contingency_survivors_match_hand_checked_small_fixture() {
        let tables = small_tables();
        let surv = goc3_ac_contingency_survivors(&tables).expect("ac survivors");
        assert_eq!(surv.ln.len(), 3); // one group per contingency
        assert_eq!(surv.ln[0].len(), 1); // ctg_00 outages acl_00 -> acl_01 survives
        assert_eq!(surv.ln[0][0].uid, "acl_01");
        assert_eq!(surv.ln[0][0].ctg, 1);
        assert_eq!(surv.ln[0][0].j_ln, 2);
        assert_eq!(surv.ln[0][0].b_sr, -1.0); // -x/(x^2+r^2) = -1/(1+0)
        assert_eq!(surv.ln[0][0].s_max_ctg, 8.0); // mva_ub_em
        assert_eq!(surv.ln[1].len(), 2); // ctg_01 outages dc_00 -> both lines survive
        assert_eq!(surv.xf[0][0].j_xf, 1); // transformer survives ctg_00
        assert_eq!(surv.ln[2].len(), 1);
        assert_eq!(surv.ln[2][0].uid, "acl_01");
        assert!(surv.xf[2].is_empty()); // ctg_02 also outages xf_00
    }

    #[test]
    fn dc_contingency_flows_match_hand_checked_small_fixture() {
        let tables = small_tables();
        let jtk_dc = goc3_dc_contingency_flows(&tables).expect("dc flows");
        // ctg_00 and ctg_02: dc_00 survives x 2 periods; ctg_01 outages dc_00.
        assert_eq!(jtk_dc.len(), 4);
        assert_eq!(
            jtk_dc.iter().map(|r| r.ctg).collect::<Vec<_>>(),
            vec![1, 1, 3, 3]
        );
        assert_eq!(
            jtk_dc.iter().map(|r| r.t).collect::<Vec<_>>(),
            vec![1, 2, 1, 2]
        );
        assert_eq!(jtk_dc[0].ctg, 1);
        assert_eq!(jtk_dc[0].j_dc, 1);
    }

    #[test]
    fn scopf_data_matches_the_individual_projections() {
        let tables = small_tables();
        let projection = goc3_static_data(&tables).expect("static data");
        let ew = goc3_energy_windows(&tables).expect("energy windows");
        let blocks = goc3_price_blocks(&projection.cost_vector_pr, &projection.cost_vector_cs);
        let surv = goc3_ac_contingency_survivors(&tables).expect("ac survivors");
        let dc = goc3_dc_contingency_flows(&tables).expect("dc flows");

        let scd = goc3_scopf_data(&tables).expect("scopf data");
        assert_eq!(scd.static_data, projection.static_data);
        assert_eq!(scd.lengths, projection.lengths);
        assert_eq!(scd.energy_windows, ew);
        assert_eq!(scd.price_blocks, blocks);
        assert_eq!(scd.ac_contingency_survivors, surv);
        assert_eq!(scd.dc_contingency_flows, dc);

        let via_str = goc3_scopf_data_from_str(SMALL_FIXTURE).expect("scopf data from text");
        assert_eq!(via_str, scd);
    }

    #[test]
    fn scopf_data_serializes_with_julia_field_names() {
        let scd = goc3_scopf_data_from_str(SMALL_FIXTURE).expect("scopf data");
        let json = serde_json::to_value(&scd).expect("serialize");
        let obj = json.as_object().expect("object");
        assert!(obj.contains_key("static"));
        assert!(!obj.contains_key("static_data"));
        let lengths = &obj["lengths"];
        assert!(lengths.get("L_J_ln").is_some());
        assert!(lengths.get("I").is_some());
        let reserve = &obj["static"]["active_reserve"][0];
        assert!(reserve.get("σ_rgu").is_some());
    }

    /// GOC3 SCOPF projections have no writer of their own: they are a
    /// derived, format-neutral instance read fresh from parsed GOC3 tables
    /// (like `powerio-matrix`'s `OpfInstance`), not a stored representation.
    /// The round trip that matters is at the GOC3 document level:
    /// `BalancedNetwork`'s GOC3 reader retains the source text and echoes it
    /// byte for byte on write (`goc3_write_without_retained_source_is_write_unsupported`
    /// and `parses_goc3_json_static_network` in `powerio/tests/convert.rs`
    /// cover that). This test checks the complementary property this module
    /// owns: reparsing that echoed text reproduces an equal
    /// [`Goc3ScopfData`], so the projection is a deterministic, lossless
    /// function of the document text alone.
    #[test]
    fn scopf_data_is_stable_across_a_reparse_of_the_same_text() {
        let first = goc3_scopf_data_from_str(SMALL_FIXTURE).expect("first parse");
        let second = goc3_scopf_data_from_str(SMALL_FIXTURE).expect("second parse");
        assert_eq!(first, second);

        // Round trip through the module's own JSON encoding too, standing in
        // for the C ABI's `pio_goc3_scopf_data_json`: serialize, deserialize,
        // and confirm the structure survives (this is not GOC3 JSON, since
        // `Goc3ScopfData` has no GOC3 writer; see the module doc comment).
        let json = serde_json::to_string(&first).expect("serialize");
        let back: Goc3ScopfData = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, first);
    }

    /// Pinned upstream copy of `14bus_20220707.json` from GOCompetition's
    /// C3DataUtilities `test_data/`, the real ARPA-E GO Challenge 3 case
    /// PowerIO.jl's own `test/test_goc3_static.jl` cross-checks against.
    const GOC3_14BUS_URL: &str = "https://raw.githubusercontent.com/GOCompetition/C3DataUtilities/bb5df337553b21ab8be89ae5f9106958541730d4/test_data/14bus_20220707.json";

    /// Fetch the real 14-bus case for [`scopf_data_matches_real_14bus_case_scale`].
    /// The ~340 KB file is not vendored; set `POWERIO_GOC3_14BUS_JSON` to a
    /// local path to run offline, else it is downloaded from `GOC3_14BUS_URL`
    /// (overridable via `POWERIO_GOC3_14BUS_URL`) with `curl`.
    fn fetch_14bus_case() -> String {
        if let Ok(path) = std::env::var("POWERIO_GOC3_14BUS_JSON") {
            return std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read POWERIO_GOC3_14BUS_JSON ({path}): {e}"));
        }
        let url =
            std::env::var("POWERIO_GOC3_14BUS_URL").unwrap_or_else(|_| GOC3_14BUS_URL.to_string());
        let output = std::process::Command::new("curl")
            .args(["--fail", "--silent", "--show-error", "--location", &url])
            .output()
            .unwrap_or_else(|e| panic!("run curl for {url}: {e}"));
        assert!(
            output.status.success(),
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("fixture is valid UTF-8")
    }

    /// The real 14-bus case exercised at scale. Counts are pinned to this
    /// scenario, matching the Julia side's own pinned assertions. Ignored by
    /// default because it fetches the case over the network; run with
    /// `cargo test -p powerio-pkg -- --ignored` (or point
    /// `POWERIO_GOC3_14BUS_JSON` at a local copy).
    #[test]
    #[ignore = "fetches a ~340 KB GOC3 case over the network; run with --ignored"]
    fn scopf_data_matches_real_14bus_case_scale() {
        let text = fetch_14bus_case();
        let tables = Goc3Tables::parse(&text).expect("parse real GOC3 case");
        let projection = goc3_static_data(&tables).expect("static data");
        let lengths = projection.lengths;
        assert_eq!(lengths.i, 14);
        assert_eq!((lengths.l_j_ln, lengths.l_j_xf, lengths.l_j_dc), (17, 3, 0));
        assert_eq!((lengths.l_j_pr, lengths.l_j_cs, lengths.l_t), (6, 11, 24));
        assert_eq!(projection.static_data.prod.len(), 6);
        assert_eq!(projection.static_data.cons.len(), 11);
        assert_eq!(
            projection
                .static_data
                .bus
                .iter()
                .map(|b| b.i)
                .collect::<Vec<_>>(),
            (1..=14).map(BusId).collect::<Vec<_>>()
        );

        let blocks = goc3_price_blocks(&projection.cost_vector_pr, &projection.cost_vector_cs);
        assert_eq!(blocks.producer.len(), 720);
        assert_eq!(blocks.consumer.len(), 1056);

        let surv = goc3_ac_contingency_survivors(&tables).expect("ac survivors");
        assert_eq!(surv.ln.len(), 19);
        assert_eq!(surv.xf.len(), 19);

        let dc = goc3_dc_contingency_flows(&tables).expect("dc flows");
        assert!(dc.is_empty()); // no DC lines in this case

        // The combined entry point matches the individually built instance.
        let scd = goc3_scopf_data(&tables).expect("scopf data");
        assert_eq!(scd.lengths, lengths);
        assert_eq!(scd.dc_contingency_flows, dc);
    }
}

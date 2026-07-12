//! Read ARPA-E GO Challenge 3 JSON input data into the transmission `Network`.
//!
//! GO Challenge 3 is a unit commitment data model. `Network` is a static power
//! flow model, so this reader maps the first time interval into static generator
//! and load bounds, retains the original JSON source, and reports the scheduling
//! data it leaves in the source document.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::network::{
    Branch, BranchCharging, BranchRatingSet, Bus, BusId, BusType, Extras, GenCost, Generator, Hvdc,
    Load, Network, Shunt, SourceFormat, TransformerControl, TransformerControlMode,
};
use crate::normalize;
use crate::{Error, Result};

const FMT: &str = "GO Challenge 3 JSON";

/// GOC3 source document: the file parsed once and shared by the format's
/// adapters (the balanced network reader here, the operating point extractor
/// in `powerio-pkg`, and the SCOPF instance builder in `powerio-prob`), so
/// section order, uid, bus ID, and device row rules have one owner.
#[derive(Clone, Debug)]
pub struct Goc3Document {
    root: Map<String, Value>,
}

impl Goc3Document {
    /// Parse one GOC3 JSON document.
    pub fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text).map_err(|error| bad(error.to_string()))?;
        let root = value
            .as_object()
            .cloned()
            .ok_or_else(|| bad("top level is not a JSON object"))?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Map<String, Value> {
        &self.root
    }

    pub fn network(&self) -> Result<&Map<String, Value>> {
        self.root
            .get("network")
            .and_then(Value::as_object)
            .ok_or_else(|| bad("missing object `network`"))
    }

    pub fn time_series_input(&self) -> Result<&Map<String, Value>> {
        self.root
            .get("time_series_input")
            .and_then(Value::as_object)
            .ok_or_else(|| bad("missing object `time_series_input`"))
    }

    #[must_use]
    pub fn time_series_output(&self) -> Option<&Map<String, Value>> {
        self.root
            .get("time_series_output")
            .and_then(Value::as_object)
    }

    #[must_use]
    pub fn reliability(&self) -> Option<&Map<String, Value>> {
        self.root.get("reliability").and_then(Value::as_object)
    }

    /// Read a network section in source document order.
    pub fn network_records(&self, name: &'static str) -> Result<Vec<Goc3Record<'_>>> {
        records(self.network()?, name)
    }

    /// Read a time series input section in source document order.
    pub fn time_series_input_records(&self, name: &'static str) -> Result<Vec<Goc3Record<'_>>> {
        records(self.time_series_input()?, name)
    }

    /// Read a time series output section in source document order.
    pub fn time_series_output_records(&self, name: &'static str) -> Result<Vec<Goc3Record<'_>>> {
        self.time_series_output()
            .map_or_else(|| Ok(Vec::new()), |output| records(output, name))
    }

    /// Enumerate dispatchable devices with the balanced model row assignment.
    pub fn dispatchable_devices(&self) -> Result<Vec<Goc3DeviceRecord<'_>>> {
        device_rows(self.network()?)
    }

    /// Map bus UIDs to the external bus IDs assigned by the balanced reader.
    pub fn bus_ids(&self) -> Result<HashMap<String, BusId>> {
        bus_id_by_uid(&section(self.network()?, "bus")?)
    }

    /// Build the period cost curve used by the balanced reader.
    #[must_use]
    pub fn dispatchable_device_cost_at(
        &self,
        device: &Map<String, Value>,
        time_series: Option<&Value>,
        period: usize,
        base_mva: f64,
    ) -> Option<GenCost> {
        cost_at(device, time_series, period, base_mva)
    }
}

/// One GOC3 section record in source document order.
#[derive(Clone, Debug)]
pub struct Goc3Record<'a> {
    pub uid: Option<String>,
    pub value: &'a Value,
}

#[derive(Debug)]
struct Goc3BusMap {
    by_uid: HashMap<String, BusId>,
}

impl Goc3BusMap {
    fn get(&self, uid: &str) -> Result<BusId> {
        self.by_uid
            .get(uid)
            .copied()
            .ok_or_else(|| bad(format!("unknown bus uid `{uid}`")))
    }
}

/// Parse a GO Challenge 3 JSON input file.
pub fn parse_goc3_json(content: &str) -> Result<super::Parsed> {
    let mut warnings = Vec::new();
    let network = parse_goc3_source(Arc::new(content.to_owned()), None, &mut warnings)?;
    Ok(super::Parsed { network, warnings })
}

#[allow(clippy::too_many_lines)]
pub(crate) fn parse_goc3_source(
    source: Arc<String>,
    name_hint: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<Network> {
    let document = Goc3Document::parse(&source)?;
    let root = document.root();
    let network = document.network()?;

    let base_mva = network
        .get("general")
        .and_then(Value::as_object)
        .and_then(|general| number(general, "base_norm_mva"))
        .unwrap_or_else(|| {
            push_once(
                warnings,
                "missing `network.general.base_norm_mva`; using 100.0 MVA",
            );
            100.0
        });
    if !base_mva.is_finite() || base_mva <= 0.0 {
        return Err(Error::InvalidBaseMva { base: base_mva });
    }

    let name = root
        .get("uid")
        .and_then(Value::as_str)
        .or_else(|| {
            network
                .get("general")
                .and_then(Value::as_object)
                .and_then(|general| general.get("uid"))
                .and_then(Value::as_str)
        })
        .or(name_hint)
        .unwrap_or("goc3")
        .to_owned();

    warn_static_reduction(root, network, warnings);

    let (mut buses, bus_map) = read_buses(network)?;
    let bus_pos: HashMap<BusId, usize> = buses
        .iter()
        .enumerate()
        .map(|(index, bus)| (bus.id, index))
        .collect();
    let time_series = root.get("time_series_input").and_then(Value::as_object);
    let device_ts = device_time_series(time_series)?;

    let mut branches = Vec::new();
    branches.extend(read_branches(network, "ac_line", false, &bus_map)?);
    branches.extend(read_branches(
        network,
        "two_winding_transformer",
        true,
        &bus_map,
    )?);

    let shunts = read_shunts(network, base_mva, &bus_map)?;
    let mut loads = Vec::new();
    let mut generators = Vec::new();
    let mut generator_buses = HashSet::new();
    let mut reference_candidate: Option<(BusId, f64)> = None;

    for device in device_rows(network)? {
        let obj = device.obj;
        let bus = bus_ref(obj, "bus", &bus_map)?;
        let ts = device
            .uid
            .as_deref()
            .and_then(|key| device_ts.get(key).copied());

        match device.kind {
            Goc3DeviceKind::Generators => {
                let generator = read_producer(obj, ts, bus, base_mva, device.uid.clone());
                generator_buses.insert(bus);
                if reference_candidate
                    .as_ref()
                    .is_none_or(|(_, pmax)| generator.pmax > *pmax)
                {
                    reference_candidate = Some((bus, generator.pmax));
                }
                generators.push(generator);
            }
            Goc3DeviceKind::Loads => {
                loads.push(read_consumer(obj, ts, bus, base_mva, device.uid.clone()));
            }
        }
    }

    assign_bus_types(
        &mut buses,
        &bus_pos,
        &generator_buses,
        reference_candidate,
        warnings,
    );

    let hvdc = read_hvdc(network, base_mva, &bus_map)?;

    let net = Network {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        geo: None,
        buses,
        loads,
        shunts,
        branches,
        switches: Vec::new(),
        generators,
        storage: Vec::new(),
        hvdc,
        transformers_3w: Vec::new(),
        areas: Vec::new(),
        solver: None,
        source_format: SourceFormat::Goc3Json,
        source: Some(source),
    };
    net.check_references(FMT)?;
    Ok(net)
}

fn read_buses(network: &Map<String, Value>) -> Result<(Vec<Bus>, Goc3BusMap)> {
    let items = section(network, "bus")?;
    if items.is_empty() {
        return Err(bad("missing non-empty `network.bus` section"));
    }
    let mut records = Vec::with_capacity(items.len());
    let mut seen_uids = HashSet::new();
    for item in &items {
        let obj = item_object(*item, "bus")?;
        let uid = item_uid(*item, obj).ok_or_else(|| bad("bus record missing `uid`"))?;
        if !seen_uids.insert(uid.clone()) {
            return Err(bad(format!("duplicate bus uid `{uid}`")));
        }
        records.push((uid, obj));
    }

    let ids = bus_id_by_uid(&items)?;

    let mut by_uid = HashMap::with_capacity(records.len());
    let mut buses = Vec::with_capacity(records.len());
    for (uid, obj) in records {
        let id = ids[&uid];
        by_uid.insert(uid.clone(), id);
        let initial = initial_status(obj);
        buses.push(Bus {
            id,
            kind: BusType::Pq,
            vm: initial.and_then(|s| number(s, "vm")).unwrap_or(1.0),
            va: initial.and_then(|s| number(s, "va")).unwrap_or(0.0) * normalize::RAD_TO_DEG,
            base_kv: number(obj, "base_nom_volt").unwrap_or(0.0),
            vmax: number(obj, "vm_ub").unwrap_or(1.1),
            vmin: number(obj, "vm_lb").unwrap_or(0.9),
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: Some(uid.clone()),
            uid: Some(uid),
            location: None,
            extras: extras(
                obj,
                &["uid", "base_nom_volt", "vm_ub", "vm_lb", "initial_status"],
            ),
        });
    }
    Ok((buses, Goc3BusMap { by_uid }))
}

fn read_branches(
    network: &Map<String, Value>,
    section_name: &'static str,
    transformer: bool,
    buses: &Goc3BusMap,
) -> Result<Vec<Branch>> {
    section(network, section_name)?
        .into_iter()
        .map(|item| {
            let obj = item_object(item, section_name)?;
            let from = bus_ref(obj, "fr_bus", buses)?;
            let to = bus_ref(obj, "to_bus", buses)?;
            let initial = initial_status(obj);
            let b = number(obj, "b").unwrap_or(0.0);
            let rate_a = number(obj, "mva_ub_nom").unwrap_or(0.0);
            let rate_b = number(obj, "mva_ub_em").unwrap_or(rate_a);
            // GO Challenge 3 puts b/2 per terminal in addition to the extra
            // g_fr/b_fr/g_to/b_to shunts (PNNL-35792 eq. 149/151).
            let charging = if number(obj, "additional_shunt").unwrap_or(0.0) == 0.0 {
                BranchCharging::from_total_b(b)
            } else {
                BranchCharging {
                    g_fr: number(obj, "g_fr").unwrap_or(0.0),
                    b_fr: b / 2.0 + number(obj, "b_fr").unwrap_or(0.0),
                    g_to: number(obj, "g_to").unwrap_or(0.0),
                    b_to: b / 2.0 + number(obj, "b_to").unwrap_or(0.0),
                }
            };
            let tap = if transformer {
                initial
                    .and_then(|s| number(s, "tm"))
                    .or_else(|| equal_bounds(obj, "tm_lb", "tm_ub"))
                    .unwrap_or(1.0)
            } else {
                0.0
            };
            let shift = if transformer {
                initial.and_then(|s| number(s, "ta")).unwrap_or(0.0) * normalize::RAD_TO_DEG
            } else {
                0.0
            };
            Ok(Branch {
                from,
                to,
                r: number(obj, "r").unwrap_or(0.0),
                x: number(obj, "x").unwrap_or(0.0),
                b,
                charging: Some(charging),
                rate_a,
                rate_b,
                rate_c: rate_b,
                rating_sets: (rate_b != 0.0 && (rate_b - rate_a).abs() > f64::EPSILON)
                    .then(|| BranchRatingSet::new("mva_ub_em", rate_b))
                    .into_iter()
                    .collect(),
                current_ratings: None,
                tap,
                shift,
                in_service: initial_status_flag(obj, true),
                angmin: -360.0,
                angmax: 360.0,
                control: shifter_control(obj, transformer),
                solution: None,
                uid: item_uid(item, obj),
                extras: extras(
                    obj,
                    &[
                        "uid",
                        "fr_bus",
                        "to_bus",
                        "r",
                        "x",
                        "b",
                        "mva_ub_nom",
                        "mva_ub_em",
                        "initial_status",
                        "additional_shunt",
                        "g_fr",
                        "g_to",
                        "b_fr",
                        "b_to",
                        "tm_lb",
                        "tm_ub",
                        "ta_lb",
                        "ta_ub",
                    ],
                ),
            })
        })
        .collect()
}

/// GOC3 `ta_lb`/`ta_ub` bound the phase shift decision variable: a device
/// control range, not a bus angle difference limit, so they map to an
/// `ActiveFlow` control block (whose tap limits carry the phase angle in
/// degrees), never to `angmin`/`angmax`.
fn shifter_control(obj: &Map<String, Value>, transformer: bool) -> Option<TransformerControl> {
    if !transformer {
        return None;
    }
    let lb = number(obj, "ta_lb");
    let ub = number(obj, "ta_ub");
    if lb.is_none() && ub.is_none() {
        return None;
    }
    let mut control = TransformerControl::new(TransformerControlMode::ActiveFlow);
    control.tap_min = lb.unwrap_or(-std::f64::consts::TAU) * normalize::RAD_TO_DEG;
    control.tap_max = ub.unwrap_or(std::f64::consts::TAU) * normalize::RAD_TO_DEG;
    Some(control)
}

fn read_shunts(
    network: &Map<String, Value>,
    base_mva: f64,
    buses: &Goc3BusMap,
) -> Result<Vec<Shunt>> {
    section(network, "shunt")?
        .into_iter()
        .map(|item| {
            let obj = item_object(item, "shunt")?;
            let step = initial_status(obj)
                .and_then(|s| number(s, "step"))
                .unwrap_or(1.0);
            Ok(Shunt {
                bus: bus_ref(obj, "bus", buses)?,
                g: number(obj, "gs").unwrap_or(0.0) * step * base_mva,
                b: number(obj, "bs").unwrap_or(0.0) * step * base_mva,
                in_service: step != 0.0,
                control: None,
                uid: item_uid(item, obj),
                extras: extras(
                    obj,
                    &[
                        "uid",
                        "bus",
                        "gs",
                        "bs",
                        "step_lb",
                        "step_ub",
                        "initial_status",
                    ],
                ),
            })
        })
        .collect()
}

fn read_producer(
    obj: &Map<String, Value>,
    ts: Option<&Value>,
    bus: BusId,
    base_mva: f64,
    uid: Option<String>,
) -> Generator {
    let initial = initial_status(obj);
    Generator {
        bus,
        pg: initial.and_then(|s| number(s, "p")).unwrap_or(0.0) * base_mva,
        qg: initial.and_then(|s| number(s, "q")).unwrap_or(0.0) * base_mva,
        pmax: first_number(ts, "p_ub").unwrap_or(0.0) * base_mva,
        pmin: first_number(ts, "p_lb").unwrap_or(0.0) * base_mva,
        qmax: first_number(ts, "q_ub").unwrap_or(0.0) * base_mva,
        qmin: first_number(ts, "q_lb").unwrap_or(0.0) * base_mva,
        vg: 1.0,
        mbase: base_mva,
        in_service: initial_status_flag(obj, true),
        cost: cost_at(obj, ts, 0, base_mva),
        caps: [None; crate::network::GEN_EXTRA_KEYS.len()],
        regulated_bus: None,
        uid,
    }
}

fn read_consumer(
    obj: &Map<String, Value>,
    ts: Option<&Value>,
    bus: BusId,
    base_mva: f64,
    uid: Option<String>,
) -> Load {
    let initial = initial_status(obj);
    let p = initial
        .and_then(|s| number(s, "p"))
        .or_else(|| first_number(ts, "p_ub"))
        .unwrap_or(0.0)
        .abs()
        * base_mva;
    let q = initial
        .and_then(|s| number(s, "q"))
        .or_else(|| first_number(ts, "q_ub"))
        .unwrap_or(0.0)
        .abs()
        * base_mva;
    Load {
        bus,
        p,
        q,
        voltage_model: None,
        in_service: initial_status_flag(obj, true),
        uid,
        extras: extras(
            obj,
            &[
                "uid",
                "bus",
                "device_type",
                "initial_status",
                "startup_cost",
                "shutdown_cost",
            ],
        ),
    }
}

fn read_hvdc(network: &Map<String, Value>, base_mva: f64, buses: &Goc3BusMap) -> Result<Vec<Hvdc>> {
    section(network, "dc_line")?
        .into_iter()
        .map(|item| {
            let obj = item_object(item, "dc_line")?;
            let initial = initial_status(obj);
            let pdc = initial.and_then(|s| number(s, "pdc_fr")).unwrap_or(0.0) * base_mva;
            Ok(Hvdc {
                from: bus_ref(obj, "fr_bus", buses)?,
                to: bus_ref(obj, "to_bus", buses)?,
                in_service: initial_status_flag(obj, true),
                pf: pdc,
                pt: -pdc,
                qf: initial.and_then(|s| number(s, "qdc_fr")).unwrap_or(0.0) * base_mva,
                qt: initial.and_then(|s| number(s, "qdc_to")).unwrap_or(0.0) * base_mva,
                vf: 1.0,
                vt: 1.0,
                pmin: -number(obj, "pdc_ub").unwrap_or(0.0) * base_mva,
                pmax: number(obj, "pdc_ub").unwrap_or(0.0) * base_mva,
                qminf: number(obj, "qdc_fr_lb").unwrap_or(0.0) * base_mva,
                qmaxf: number(obj, "qdc_fr_ub").unwrap_or(0.0) * base_mva,
                qmint: number(obj, "qdc_to_lb").unwrap_or(0.0) * base_mva,
                qmaxt: number(obj, "qdc_to_ub").unwrap_or(0.0) * base_mva,
                loss0: 0.0,
                loss1: 0.0,
                cost: None,
                uid: item_uid(item, obj),
                extras: extras(
                    obj,
                    &[
                        "uid",
                        "fr_bus",
                        "to_bus",
                        "pdc_ub",
                        "qdc_fr_lb",
                        "qdc_fr_ub",
                        "qdc_to_lb",
                        "qdc_to_ub",
                        "initial_status",
                    ],
                ),
            })
        })
        .collect()
}

fn assign_bus_types(
    buses: &mut [Bus],
    bus_pos: &HashMap<BusId, usize>,
    generator_buses: &HashSet<BusId>,
    reference_candidate: Option<(BusId, f64)>,
    warnings: &mut Vec<String>,
) {
    for bus in generator_buses {
        super::set_bus_kind(buses, bus_pos, *bus, BusType::Pv);
    }
    if let Some((bus, _)) = reference_candidate
        && bus_pos.contains_key(&bus)
    {
        super::set_bus_kind(buses, bus_pos, bus, BusType::Ref);
        warnings.push(format!(
            "GO Challenge 3 has no explicit reference bus; selected bus {} from the largest producer pmax",
            bus.0
        ));
    }
}

/// Which payload table a simple dispatchable device row lands in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goc3DeviceKind {
    Generators,
    Loads,
}

/// One simple dispatchable device with the payload row index the parser
/// assigns it.
pub struct Goc3DeviceRecord<'a> {
    pub kind: Goc3DeviceKind,
    pub row: usize,
    pub uid: Option<String>,
    pub obj: &'a Map<String, Value>,
}

/// Enumerate simple dispatchable devices with their generator/load row
/// indices. Row assignment lives here and nowhere else: a consumer that
/// addresses payload rows by index (the operating point extractor in
/// `powerio-pkg`) must enumerate devices through this function so its indices
/// match the parsed network, uid or no uid.
fn device_rows(network: &Map<String, Value>) -> Result<Vec<Goc3DeviceRecord<'_>>> {
    let mut rows = Vec::new();
    let mut generators = 0usize;
    let mut loads = 0usize;
    for item in section(network, "simple_dispatchable_device")? {
        let obj = item_object(item, "simple_dispatchable_device")?;
        let uid = item_uid(item, obj);
        let (table, row) = match string(obj, "device_type").unwrap_or("producer") {
            "producer" => {
                generators += 1;
                (Goc3DeviceKind::Generators, generators - 1)
            }
            "consumer" => {
                loads += 1;
                (Goc3DeviceKind::Loads, loads - 1)
            }
            other => {
                return Err(bad(format!(
                    "simple_dispatchable_device `{}` has unsupported `device_type` `{other}`",
                    uid.unwrap_or_else(|| "?".into())
                )));
            }
        };
        rows.push(Goc3DeviceRecord {
            kind: table,
            row,
            uid,
            obj,
        });
    }
    Ok(rows)
}

/// Piecewise marginal cost blocks for period `index`, integrated into a
/// cumulative MATPOWER piecewise linear curve. Shared with the operating point
/// extractor so a materialized period matches what this parser builds for the
/// static payload.
fn cost_at(
    obj: &Map<String, Value>,
    ts: Option<&Value>,
    index: usize,
    base_mva: f64,
) -> Option<GenCost> {
    let periods = ts?.get("cost")?.as_array()?;
    let curve = periods.get(index)?.as_array()?;
    let mut coeffs = vec![0.0, 0.0];
    let mut p = 0.0;
    let mut y = 0.0;
    for segment in curve {
        let values = segment.as_array()?;
        let marginal = values.first()?.as_f64()?;
        let width = values.get(1)?.as_f64()?;
        if !marginal.is_finite() || !width.is_finite() || width <= 0.0 {
            continue;
        }
        p += width * base_mva;
        y += marginal * width;
        coeffs.push(p);
        coeffs.push(y);
    }
    (coeffs.len() >= 4).then_some(GenCost {
        model: 1,
        startup: number(obj, "startup_cost").unwrap_or(0.0),
        shutdown: number(obj, "shutdown_cost").unwrap_or(0.0),
        ncost: coeffs.len() / 2,
        coeffs,
    })
}

fn device_time_series(time_series: Option<&Map<String, Value>>) -> Result<HashMap<String, &Value>> {
    let Some(time_series) = time_series else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::new();
    for item in section(time_series, "simple_dispatchable_device")? {
        if let Some(key) = item.key {
            out.insert(key.to_owned(), item.value);
        }
        if let Some(obj) = item.value.as_object() {
            if let Some(uid) = string(obj, "uid") {
                out.insert(uid.to_owned(), item.value);
            }
        }
    }
    Ok(out)
}

fn warn_static_reduction(
    root: &Map<String, Value>,
    network: &Map<String, Value>,
    warnings: &mut Vec<String>,
) {
    if root.get("time_series_input").is_some() {
        warnings.push(
            "time_series_input reduced to the first interval for static Network dispatch and limits"
                .into(),
        );
    }
    if root.get("reliability").is_some() {
        warnings.push("reliability contingencies retained in source only".into());
    }
    for section in [
        "active_zonal_reserve",
        "reactive_zonal_reserve",
        "violation_cost",
    ] {
        if network.get(section).is_some() {
            warnings.push(format!("network.{section} retained in source only"));
        }
    }
    if !section(network, "simple_dispatchable_device")
        .unwrap_or_default()
        .is_empty()
    {
        warnings.push(
            "simple dispatchable device commitment, ramp, reserve, and multi-interval cost data retained in source only"
                .into(),
        );
    }
}

#[derive(Clone, Copy)]
struct SectionItem<'a> {
    pub key: Option<&'a str>,
    pub value: &'a Value,
}

fn section<'a>(parent: &'a Map<String, Value>, name: &'static str) -> Result<Vec<SectionItem<'a>>> {
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
        other => Err(bad(format!(
            "`network.{name}` is not an array or object, got {}",
            kind(other)
        ))),
    }
}

fn item_object<'a>(
    item: SectionItem<'a>,
    section_name: &'static str,
) -> Result<&'a Map<String, Value>> {
    item.value.as_object().ok_or_else(|| {
        bad(format!(
            "`network.{section_name}` record is not an object, got {}",
            kind(item.value)
        ))
    })
}

fn item_uid(item: SectionItem<'_>, obj: &Map<String, Value>) -> Option<String> {
    string(obj, "uid")
        .map(str::to_owned)
        .or_else(|| item.key.map(str::to_owned))
        .filter(|uid| !uid.is_empty())
}

// Numeric keys sort by value ahead of non-numeric keys, which sort
// lexicographically. The tiers keep this a total order on mixed key sets
// ("2" < "10" numerically while "10" < "1x" < "2" lexically is a cycle, and
// sort_by panics on a comparator that is not a strict weak ordering).
fn compare_keys(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(a_num), Ok(b_num)) => a_num.cmp(&b_num).then_with(|| a.cmp(b)),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

fn bus_ref(obj: &Map<String, Value>, key: &'static str, buses: &Goc3BusMap) -> Result<BusId> {
    let uid = string(obj, key).ok_or_else(|| bad(format!("missing string `{key}`")))?;
    buses.get(uid)
}

fn official_bus_suffix(uid: &str) -> Option<usize> {
    let rest = uid.strip_prefix("bus_")?;
    (!rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        .then(|| rest.parse::<usize>().ok())
        .flatten()
}

/// Map GOC3 bus uids to the same 1-based row positions `read_buses` assigns
/// as `BusId`: the numeric `bus_<n>` suffix + 1 when every bus in `items` has
/// one and they are unique, else the 1-based position in document order.
/// Shared with `powerio-prob`'s GOC3 SCOPF adapter so its bus identities
/// agree with `BalancedNetwork`'s `BusId` for the same document.
fn bus_id_by_uid(items: &[SectionItem<'_>]) -> Result<HashMap<String, BusId>> {
    let mut uids = Vec::with_capacity(items.len());
    for item in items {
        let obj = item
            .value
            .as_object()
            .ok_or_else(|| bad("bus section item is not an object"))?;
        let uid = item_uid(*item, obj).ok_or_else(|| bad("bus section item missing `uid`"))?;
        uids.push(uid);
    }
    let suffixes: Vec<Option<usize>> = uids.iter().map(|uid| official_bus_suffix(uid)).collect();
    let suffixes_unique = suffixes.iter().all(Option::is_some)
        && suffixes
            .iter()
            .flatten()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            == suffixes.len();
    Ok(uids
        .into_iter()
        .zip(suffixes)
        .enumerate()
        .map(|(index, (uid, suffix))| {
            let id = match suffix {
                Some(suffix) if suffixes_unique => suffix + 1,
                _ => index + 1,
            };
            (uid, BusId(id))
        })
        .collect())
}

fn string<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

fn number(obj: &Map<String, Value>, key: &str) -> Option<f64> {
    obj.get(key).and_then(Value::as_f64)
}

fn first_number(value: Option<&Value>, key: &str) -> Option<f64> {
    value?.get(key)?.as_array()?.first().and_then(Value::as_f64)
}

fn initial_status(obj: &Map<String, Value>) -> Option<&Map<String, Value>> {
    obj.get("initial_status").and_then(Value::as_object)
}

fn initial_status_flag(obj: &Map<String, Value>, default: bool) -> bool {
    initial_status(obj)
        .and_then(|status| number(status, "on_status"))
        .map_or(default, |v| v != 0.0)
}

fn equal_bounds(obj: &Map<String, Value>, low: &str, high: &str) -> Option<f64> {
    let lo = number(obj, low)?;
    let hi = number(obj, high)?;
    ((lo - hi).abs() <= f64::EPSILON).then_some(lo)
}

fn extras(obj: &Map<String, Value>, known: &[&str]) -> Extras {
    obj.iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn push_once(warnings: &mut Vec<String>, warning: &str) {
    if !warnings.iter().any(|w| w == warning) {
        warnings.push(warning.to_owned());
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

fn bad(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FMT,
        message: message.into(),
    }
}

fn records<'a>(parent: &'a Map<String, Value>, name: &'static str) -> Result<Vec<Goc3Record<'a>>> {
    section(parent, name).map(|items| {
        items
            .into_iter()
            .map(|item| Goc3Record {
                uid: item
                    .value
                    .as_object()
                    .and_then(|object| item_uid(item, object))
                    .or_else(|| item.key.map(str::to_owned)),
                value: item.value,
            })
            .collect()
    })
}

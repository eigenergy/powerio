//! Decode DOE GO Challenge 3 problem data for the typed calculation parser.
//!
//! The GO Challenge 3 data format covers static electrical data, time varying
//! data, contingencies, and solution data for Challenge 3 and later uses.
//! The top level parser combines the decoded electrical network with the
//! scheduling and contingency data. This module is not a public network
//! parser.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::diagnostics::{Diagnostics, codes};
use crate::network::{
    BalancedNetwork, BalancedNetworkTables, Branch, BranchCharging, BranchRatingSet, Bus, BusId,
    BusType, Extras, GenCost, Generator, GeneratorEnergySource, Hvdc, Load, Shunt, SourceFormat,
};
use crate::normalize;
use crate::{CoordinateSpace, CoordsKind, GeoMeta, Location};
use crate::{Error, Result};

const FMT: &str = "GO Challenge 3 JSON";

/// GOC3 source document shared by the balanced network reader and the AC SCUC
/// instance builder, so section order, uid, bus ID, and device row rules have
/// one owner.
#[derive(Clone, Debug)]
pub struct Goc3Document {
    root: Map<String, Value>,
}

impl Goc3Document {
    /// Parse one GOC3 JSON document.
    pub fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text).map_err(|error| bad(error.to_string()))?;
        let Value::Object(root) = value else {
            return Err(bad("top level is not a JSON object"));
        };
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

/// Parse the balanced network used by a complete Challenge 3 calculation
/// instance. Scheduling and contingency data are consumed by the caller, so
/// this path does not claim that those sections survive only in source bytes.
#[doc(hidden)]
pub fn parse_goc3_instance_network(
    content: &str,
) -> Result<(BalancedNetwork, Vec<super::Diagnostic>, Arc<Goc3Document>)> {
    parse_goc3_json_with_reduction_diagnostics(content, false)
}

fn parse_goc3_json_with_reduction_diagnostics(
    content: &str,
    report_static_reduction: bool,
) -> Result<(BalancedNetwork, Vec<super::Diagnostic>, Arc<Goc3Document>)> {
    let mut warnings = Diagnostics::new();
    let (network, document) =
        parse_goc3_source(content, None, &mut warnings, report_static_reduction)?;
    Ok((network, warnings.into_records(), document))
}

/// Parse GOC3 source text into the balanced network plus the document the
/// parse went through, so the caller hands the document forward instead of
/// letting downstream adapters reparse the retained text.
#[allow(clippy::too_many_lines)]
pub(crate) fn parse_goc3_source(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
    report_static_reduction: bool,
) -> Result<(BalancedNetwork, Arc<Goc3Document>)> {
    let document = Arc::new(Goc3Document::parse(source)?);
    let root = document.root();
    let network = document.network()?;

    let base_mva = network
        .get("general")
        .and_then(Value::as_object)
        .and_then(|general| number(general, "base_norm_mva"))
        .ok_or_else(|| bad("missing required number `network.general.base_norm_mva`"))?;
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

    if report_static_reduction {
        warn_static_reduction(root, network, warnings);
    }

    let (mut buses, bus_map) = read_buses(network)?;
    let bus_pos: HashMap<BusId, usize> = buses
        .iter()
        .enumerate()
        .map(|(index, bus)| (bus.id, index))
        .collect();
    let time_series = root.get("time_series_input").and_then(Value::as_object);
    let device_ts = device_time_series(time_series)?;

    let mut branches = Vec::new();
    branches.extend(read_branches(
        network, "ac_line", false, base_mva, &bus_map,
    )?);
    branches.extend(read_branches(
        network,
        "two_winding_transformer",
        true,
        base_mva,
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

    let net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        geo: buses
            .iter()
            .any(|bus| bus.location.is_some())
            .then_some(GeoMeta {
                space: CoordinateSpace::Geographic { crs: None },
                kind: Some(CoordsKind::Source),
            }),
        case_metadata: crate::network::CaseMetadata::default(),
        detailed_connectivity: None,
        generated_uids: std::sync::Arc::default(),
        buses: buses.into(),
        loads: loads.into(),
        shunts: shunts.into(),
        static_var_compensators: Vec::new().into(),
        branches: branches.into(),
        switches: Vec::new().into(),
        generators: generators.into(),
        storage: Vec::new().into(),
        hvdc: hvdc.into(),
        transformers_3w: Vec::new().into(),
        areas: Vec::new().into(),
        solver: None,
        source_format: SourceFormat::Goc3Json,
    });
    net.check_references(FMT)?;
    Ok((net, document))
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
        let kind = match string(obj, "type") {
            Some("PV") => BusType::Pv,
            Some("Slack") => BusType::Ref,
            Some("Not_used") => BusType::Isolated,
            _ => BusType::Pq,
        };
        let location = number(obj, "longitude")
            .zip(number(obj, "latitude"))
            .map(|(x, y)| Location {
                x,
                y,
                kind: Some(CoordsKind::Source),
            });
        let mut bus_extras = extras(
            obj,
            &[
                "uid",
                "base_nom_volt",
                "vm_ub",
                "vm_lb",
                "initial_status",
                // This field remains in the pinned 2022 GO-3 data model and
                // its D1/D2/D3 files, but data format 1.1.1 removed it. The
                // AC SCUC reader reports it instead of treating it as an
                // electrical network attribute.
                "con_loss_factor",
            ],
        );
        if location.is_some() {
            bus_extras.remove("longitude");
            bus_extras.remove("latitude");
        }
        if matches!(
            string(obj, "type"),
            Some("PQ" | "PV" | "Slack" | "Not_used")
        ) {
            bus_extras.remove("type");
        }
        buses.push(Bus {
            id,
            kind,
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
            location,
            extras: bus_extras,
        });
    }
    Ok((buses, Goc3BusMap { by_uid }))
}

fn read_branches(
    network: &Map<String, Value>,
    section_name: &'static str,
    transformer: bool,
    base_mva: f64,
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
            let rate_a_per_unit = number(obj, "mva_ub_nom").unwrap_or(0.0);
            let rate_b_per_unit = number(obj, "mva_ub_sht").unwrap_or(rate_a_per_unit);
            let rate_c_per_unit = number(obj, "mva_ub_em").unwrap_or(rate_b_per_unit);
            let rate_a = rate_a_per_unit * base_mva;
            let rate_b = rate_b_per_unit * base_mva;
            let rate_c = rate_c_per_unit * base_mva;
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
                name: None,
                from,
                to,
                r: number(obj, "r").unwrap_or(0.0),
                x: number(obj, "x").unwrap_or(0.0),
                b,
                charging: Some(charging),
                rate_a,
                rate_b,
                rate_c,
                rating_sets: [
                    number(obj, "mva_ub_sht")
                        .map(|rating| BranchRatingSet::new("mva_ub_sht", rating * base_mva)),
                    number(obj, "mva_ub_em")
                        .map(|rating| BranchRatingSet::new("mva_ub_em", rating * base_mva)),
                ]
                .into_iter()
                .flatten()
                .collect(),
                current_ratings: None,
                tap,
                shift,
                in_service: initial_status_flag(obj, true),
                angmin: -360.0,
                angmax: 360.0,
                // GOC3 `ta_lb`/`ta_ub` bound the SCUC phase shift decision.
                // They are not automatic active power flow control settings
                // on the reusable electrical network.
                control: None,
                solution: None,
                uid: item_uid(item, obj),
                route: None,
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
                        "mva_ub_sht",
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
                section_count: None,
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
        energy_source: GeneratorEnergySource::default(),
        pg: initial.and_then(|s| number(s, "p")).unwrap_or(0.0) * base_mva,
        qg: initial.and_then(|s| number(s, "q")).unwrap_or(0.0) * base_mva,
        pmax: first_number(ts, "p_ub").unwrap_or(0.0) * base_mva,
        pmin: first_number(ts, "p_lb").unwrap_or(0.0) * base_mva,
        qmax: first_number(ts, "q_ub").unwrap_or(0.0) * base_mva,
        qmin: first_number(ts, "q_lb").unwrap_or(0.0) * base_mva,
        vg: number(obj, "vm_setpoint").unwrap_or(1.0),
        mbase: number(obj, "nameplate_capacity").unwrap_or(1.0) * base_mva,
        in_service: initial_status_flag(obj, true),
        cost: cost_at(obj, ts, 0, base_mva),
        caps: [None; crate::network::GEN_EXTRA_KEYS.len()],
        voltage_regulation_on: true,
        regulating_terminal: None,
        regulated_bus: None,
        active_power_control: None,
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
                resistance_ohm: None,
                nominal_voltage_kv: None,
                converters_mode: None,
                converter1: None,
                converter2: None,
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
    warnings: &mut Diagnostics,
) {
    for bus in generator_buses {
        if let Some(&index) = bus_pos.get(bus)
            && buses[index].kind == BusType::Pq
        {
            buses[index].kind = BusType::Pv;
        }
    }
    if buses.iter().any(|bus| bus.kind == BusType::Ref) {
        return;
    }
    if let Some((bus, _)) = reference_candidate
        && bus_pos.contains_key(&bus)
    {
        super::set_bus_kind(buses, bus_pos, bus, BusType::Ref);
        warnings.push(&codes::READ_GOC3_VALUE_INFERRED, format!(
            "GO Challenge 3 has no explicit reference bus; selected bus {} from the largest producer pmax",
            bus.0
        ));
    }
}

/// Which payload table a simple dispatchable device row lands in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Goc3DeviceKind {
    Generators,
    Loads,
}

/// One simple dispatchable device with the payload row index the parser
/// assigns it.
struct Goc3DeviceRecord<'a> {
    kind: Goc3DeviceKind,
    uid: Option<String>,
    obj: &'a Map<String, Value>,
}

/// Enumerate simple dispatchable devices with their generator/load row
/// indices. Row assignment lives here and nowhere else so the balanced network
/// reader and SCUC instance builder agree, uid or no uid.
fn device_rows(network: &Map<String, Value>) -> Result<Vec<Goc3DeviceRecord<'_>>> {
    let mut rows = Vec::new();
    // Time-series bounds and costs are joined back onto devices by uid
    // (`device_time_series` is last-write-wins on its map), so a repeated uid
    // would silently hand one device the other's series; reject it the way
    // `read_buses` rejects a repeated bus uid.
    let mut seen_uids = std::collections::HashSet::new();
    for item in section(network, "simple_dispatchable_device")? {
        let obj = item_object(item, "simple_dispatchable_device")?;
        let uid = item_uid(item, obj);
        if let Some(uid) = &uid
            && !seen_uids.insert(uid.clone())
        {
            return Err(bad(format!(
                "duplicate simple_dispatchable_device uid `{uid}`"
            )));
        }
        let kind = match string(obj, "device_type").unwrap_or("producer") {
            "producer" => Goc3DeviceKind::Generators,
            "consumer" => Goc3DeviceKind::Loads,
            other => {
                return Err(bad(format!(
                    "simple_dispatchable_device `{}` has unsupported `device_type` `{other}`",
                    uid.unwrap_or_else(|| "?".into())
                )));
            }
        };
        rows.push(Goc3DeviceRecord { kind, uid, obj });
    }
    Ok(rows)
}

/// Piecewise marginal cost blocks for period `index`, integrated into a
/// cumulative MATPOWER piecewise linear curve for the balanced network's
/// source assignment.
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
        if let Some(obj) = item.value.as_object()
            && let Some(uid) = string(obj, "uid")
        {
            out.insert(uid.to_owned(), item.value);
        }
    }
    Ok(out)
}

fn warn_static_reduction(
    root: &Map<String, Value>,
    network: &Map<String, Value>,
    warnings: &mut Diagnostics,
) {
    if root.get("time_series_input").is_some() {
        warnings.push(&codes::READ_GOC3_RETAINED_SOURCE_ONLY,
            "time_series_input reduced to the first interval for static BalancedNetwork dispatch and limits"
                ,
        );
    }
    if root.get("reliability").is_some() {
        warnings.push(
            &codes::READ_GOC3_RETAINED_SOURCE_ONLY,
            "reliability contingencies retained in source only",
        );
    }
    for section in [
        "active_zonal_reserve",
        "reactive_zonal_reserve",
        "violation_cost",
    ] {
        if network.get(section).is_some() {
            warnings.push(
                &codes::READ_GOC3_RETAINED_SOURCE_ONLY,
                format!("network.{section} retained in source only"),
            );
        }
    }
    if !section(network, "simple_dispatchable_device")
        .unwrap_or_default()
        .is_empty()
    {
        warnings.push(&codes::READ_GOC3_RETAINED_SOURCE_ONLY,
            "simple dispatchable device commitment, ramp, reserve, and multi-interval cost data retained in source only"
                ,
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
/// Shared with `powerio-prob`'s GOC3 reader so its bus identities
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
            // `suffix + 1` could overflow when a uid carries `usize::MAX` as
            // its suffix (a single such bus is trivially "unique"); fall back
            // to the 1-based position rather than panicking.
            let id = match suffix.and_then(|s| s.checked_add(1)) {
                Some(id) if suffixes_unique => id,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_device_uid_is_rejected() {
        // Time-series bounds and costs join back onto devices by uid; a
        // repeated uid would silently hand one device the other's series.
        let network: Map<String, Value> = serde_json::from_str(
            r#"{"simple_dispatchable_device":[
                {"uid":"sd1","device_type":"producer"},
                {"uid":"sd1","device_type":"consumer"}]}"#,
        )
        .unwrap();
        let Err(err) = device_rows(&network) else {
            panic!("duplicate uid must be rejected")
        };
        assert!(err.to_string().contains("duplicate"), "got: {err}");
    }

    #[test]
    fn extreme_bus_suffix_does_not_overflow() {
        // A `bus_<usize::MAX>` uid parses to usize::MAX and is trivially the
        // unique suffix; `suffix + 1` would overflow (panic under overflow
        // checks). The bus falls back to its 1-based position instead.
        let content = format!(
            r#"{{"network":{{"general":{{"base_norm_mva":100}},"bus":[{{"uid":"bus_{}","base_nom_volt":100}}]}}}}"#,
            usize::MAX
        );
        let (network, _diagnostics, _document) =
            parse_goc3_json_with_reduction_diagnostics(&content, true).unwrap();
        assert_eq!(network.buses().len(), 1);
        assert_eq!(network.buses()[0].id, BusId(1));
    }

    #[test]
    fn producer_nameplate_capacity_sets_machine_base() {
        let object: Map<String, Value> = serde_json::from_str(
            r#"{
                "uid":"producer-1",
                "device_type":"producer",
                "nameplate_capacity":2.5,
                "vm_setpoint":1.03,
                "initial_status":{"on_status":1,"p":0.5,"q":0.1}
            }"#,
        )
        .unwrap();
        let generator = read_producer(&object, None, BusId(1), 100.0, Some("producer-1".into()));
        assert!((generator.mbase - 250.0).abs() < f64::EPSILON);
        assert!((generator.vg - 1.03).abs() < f64::EPSILON);
    }
}

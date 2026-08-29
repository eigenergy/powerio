//! Map a parsed [`AuxFile`] to the typed [`BalancedNetwork`], and write a `BalancedNetwork`
//! back out as aux text.
//!
//! Only the power flow core object types (Bus, Load, Shunt, Gen, Branch) feed
//! the typed model; every other `DATA` section stays reachable through the
//! generic layer (see [`super::aux`]) and survives the same format round trip
//! via the retained source.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use super::auxiliary::{AuxFile, AuxObject, parse_aux};
use crate::diagnostics::codes::EMIT_POWERWORLD as F;
use crate::diagnostics::{Diagnostics, codes};
use crate::format::{Conversion, sanitize_quoted, warn_extra_branch_rating_sets};
use crate::network::{
    BalancedNetwork, BalancedNetworkTables, Branch, Bus, BusId, BusType, Extras, Generator, Load,
    LoadVoltageModel, Shunt, SourceFormat,
};
use crate::{Error, Result};

const FMT: &str = "PowerWorld .aux";

/// The double quote would close a PowerWorld quoted value early on re-read (the
/// tokenizer toggles on `"` with no un-escaping), shifting every later column.
const NAME_FORBIDDEN: &[char] = &['"'];

/// Branch identity extras keys, shared with the `.pwb` reader. They double as
/// the aux field names (extras keep PowerWorld fields verbatim), so every
/// PowerWorld reader produces the same extras.
pub(super) const LINE_CIRCUIT: &str = "LineCircuit";
pub(super) const BRANCH_DEVICE_TYPE: &str = "BranchDeviceType";

// ---- Reader -----------------------------------------------------------------

/// Owned-source entry used by the format hub: parse by borrowing `source`, then
/// move the buffer into the retained source (no copy). `name_hint` (e.g. a file
/// stem) names the network when the `.aux` carries no export marker.
#[expect(clippy::too_many_lines)]
pub(in crate::format) fn parse_powerworld_source(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let content: &str = source;
    // PowerWorld `.aux` does not carry the system base in the case data, so we
    // default to 100 MVA (the de-facto standard, and what our own writer records
    // in the `// baseMVA` marker below). Reading a real base from PowerWorld's
    // project files is tracked separately; defaulting here is deliberate, not a
    // silent guess — erroring would reject every base-less third-party `.aux`.
    let mut base_mva = 100.0;
    let mut name = name_hint.unwrap_or("case").to_string();
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("// baseMVA ") {
            if let Ok(v) = rest.trim().parse::<f64>() {
                base_mva = v;
            }
        } else if let Some((_, n)) = t.split_once("powerio export: ") {
            name = n.trim().to_string();
        }
    }

    let aux = parse_aux(content)?;
    if aux.data().next().is_none() {
        return Err(Error::FormatRead {
            format: FMT,
            message: "no DATA blocks found".into(),
        });
    }

    // A complete case export spreads one object type over several DATA
    // sections, each declaring a different field group for the same objects
    // (Simulator 19 era exports write Bus twice, Gen three times, and put the
    // transformer regulation fields in a separate `Transformer` object).
    // Merge sections by the type's key fields before mapping; a later section
    // updates the fields it declares, exactly like loading the aux into
    // Simulator would.
    let mut merged_buses = Merge::new(&[&["BusNum", "Number"]]);
    let mut merged_loads = Merge::new(&[&["BusNum", "BusName_NomVolt"], &["LoadID", "ID"]]);
    let mut merged_shunts = Merge::new(&[&["BusNum", "BusName_NomVolt"], &["ShuntID", "ID"]]);
    let mut merged_gens = Merge::new(&[&["BusNum", "BusName_NomVolt"], &["GenID", "ID"]]);
    let mut merged_branches = Merge::new(&[
        &["BusNum", "BusNumFrom", "BusName_NomVolt"],
        &["BusNum:1", "BusNumTo", "BusName_NomVolt:1"],
        &[LINE_CIRCUIT, "Circuit"],
    ]);
    let mut unmodeled: BTreeMap<&str, usize> = BTreeMap::new();
    for blk in aux.data() {
        // Simulator resolves object names case insensitively, and real
        // exports use that: the UIUC 150 bus case spells every section
        // `BUS`/`GEN`/`LOAD`. An exact match read such a file as a case with
        // no buses.
        match blk.object_type.to_ascii_lowercase().as_str() {
            "bus" => merged_buses.absorb(
                blk,
                blk.field_index("BusNum").is_some() || blk.field_index("Number").is_some(),
            ),
            "load" => merged_loads.absorb(blk, true),
            "shunt" => merged_shunts.absorb(blk, true),
            "gen" => merged_gens.absorb(blk, true),
            "branch" => merged_branches.absorb(blk, true),
            // Transformer sections augment existing branches with regulation
            // fields; a transformer with no Branch record carries no impedance
            // and cannot stand alone, so unmatched rows are not created.
            "transformer" => merged_branches.absorb(blk, false),
            _ => {
                if !blk.rows.is_empty() {
                    *unmodeled.entry(&blk.object_type).or_default() += blk.rows.len();
                }
            }
        }
    }
    for (object, rows) in unmodeled {
        warnings.push(
            &codes::READ_POWERWORLD_RETAINED_SOURCE_ONLY,
            format!(
                "PowerWorld .aux DATA {object} has {rows} row(s) not modeled in BalancedNetwork; \
                 retained only in source text for same-format writeback"
            ),
        );
    }

    let mut buses = Vec::new();
    let mut bus_labels = HashMap::new();
    for r in merged_buses.rows() {
        let bus = read_bus(r)?;
        if let Some(label) = first(r, &["BusName_NomVolt"]) {
            bus_labels.insert(label, bus.id);
        }
        buses.push(bus);
    }
    let mut loads = Vec::new();
    for r in merged_loads.rows() {
        loads.push(read_load(r, &bus_labels, loads.len())?);
    }
    let mut shunts = Vec::new();
    for r in merged_shunts.rows() {
        shunts.push(read_shunt(r, &bus_labels, shunts.len())?);
    }
    let mut generators = Vec::new();
    for r in merged_gens.rows() {
        generators.push(read_gen(r, &bus_labels)?);
    }
    let mut branches = Vec::new();
    let mut parallel: HashMap<(BusId, BusId), u32> = HashMap::new();
    for r in merged_branches.rows() {
        branches.push(read_branch(r, &bus_labels, &mut parallel)?);
    }
    derive_bus_kinds(&mut buses, &generators);

    let net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        geo: super::super::geographic_meta(&buses),
        buses,
        loads,
        shunts,
        branches,
        switches: Vec::new(),
        generators,
        storage: Vec::new(),
        hvdc: Vec::new(),
        transformers_3w: Vec::new(),
        areas: Vec::new(),
        solver: None,
        source_format: SourceFormat::PowerWorld,
    });
    net.check_references(FMT)?;
    Ok(net)
}

/// Parse the auxiliary sections of a PowerWorld-sourced [`BalancedNetwork`]'s retained
/// source. The typed model carries the power flow core; everything else in the
/// original file (contingencies, limit sets, substations, ...) is reachable
/// here.
///
/// Returns `None` when the network was not read from a `.aux` source.
///
/// # Errors
/// As [`parse_aux`], on retained source text that no longer parses.
pub fn aux_sections(source_text: &str) -> Result<AuxFile> {
    parse_aux(source_text)
}

type Row<'a> = HashMap<&'a str, &'a str>;

/// Merges the rows of one object type across its DATA sections, keyed by the
/// type's key fields. Insertion order is kept, so the first section fixes the
/// element order and later sections update fields in place.
#[derive(PartialEq, Eq, Hash)]
enum MergeKey<'a> {
    Fields(Vec<&'a str>),
    /// A section with none of the type's key columns identifies its rows by
    /// position (our own writer's output identifies devices by order).
    Ordinal(usize),
}

struct Merge<'a> {
    /// Key columns as alias groups: each group lists the same key under its
    /// naming generations (`BusNum`/`Number`, `LineCircuit`/`Circuit`, ...);
    /// a section keys on whichever name it declares.
    key_fields: &'static [&'static [&'static str]],
    index: HashMap<MergeKey<'a>, usize>,
    merged: Vec<Row<'a>>,
}

impl<'a> Merge<'a> {
    fn new(key_fields: &'static [&'static [&'static str]]) -> Self {
        Merge {
            key_fields,
            index: HashMap::new(),
            merged: Vec::new(),
        }
    }

    /// Fold a DATA section in. With `create`, rows whose key is unseen become
    /// new elements; otherwise they are dropped (augmentation only sections,
    /// like `Transformer`).
    fn absorb(&mut self, blk: &'a AuxObject, create: bool) {
        let positions: Vec<Vec<usize>> = self
            .key_fields
            .iter()
            .map(|group| group.iter().filter_map(|k| blk.field_index(k)).collect())
            .collect();
        let keyless = positions.iter().all(Vec::is_empty);
        for (at, row) in blk.rows.iter().enumerate() {
            let key = if keyless {
                MergeKey::Ordinal(at)
            } else {
                MergeKey::Fields(
                    positions
                        .iter()
                        .map(|aliases| {
                            aliases
                                .iter()
                                .filter_map(|i| row.values.get(*i).map(|v| v.as_str().trim()))
                                .find(|v| !v.is_empty())
                                .unwrap_or("")
                        })
                        .collect(),
                )
            };
            let slot = match self.index.get(&key) {
                Some(&i) => i,
                None if create => {
                    self.index.insert(key, self.merged.len());
                    self.merged.push(HashMap::with_capacity(blk.fields.len()));
                    self.merged.len() - 1
                }
                None => continue,
            };
            let entry = &mut self.merged[slot];
            for (f, v) in blk.fields.iter().zip(&row.values) {
                entry.insert(f.as_str(), v.as_str());
            }
        }
    }

    fn rows(&self) -> impl Iterator<Item = &Row<'a>> {
        self.merged.iter()
    }
}

fn bad_field(key: &str, tok: &str) -> Error {
    Error::FormatRead {
        format: FMT,
        message: format!("field {key} {tok:?} is not a number"),
    }
}

/// Field `key` as f64, defaulting to 0.0 when absent. Present but unparseable is
/// a hard error: a malformed number must not silently become a plausible default
/// and corrupt the matrices downstream.
fn f(r: &Row, key: &str) -> Result<f64> {
    f_or(r, key, 0.0)
}
/// Field `key` as f64, absent → `default`, present-but-unparseable → error.
fn f_or(r: &Row, key: &str, default: f64) -> Result<f64> {
    match r.get(key).copied() {
        None | Some("") => Ok(default),
        Some(s) => s.trim().parse().map_err(|_| bad_field(key, s)),
    }
}
/// Field `key` as a bus id (parsed as f64 then truncated). Absent → 0;
/// present-but-unparseable → error.
fn uid(r: &Row, key: &str) -> Result<usize> {
    match r.get(key).copied() {
        None | Some("") => Ok(0),
        Some(s) => bus_id_from_field(key, s.trim()),
    }
}

/// A bus id string validated the way [`uid`] validates every bus reference:
/// parse through f64 (some exports print ids with a decimal point) but reject
/// anything a float to integer cast would silently bend — NaN and negatives
/// would saturate to 0 and rewire the network, huge values to usize::MAX, and
/// fractions would truncate.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn bus_id_from_field(key: &str, s: &str) -> Result<usize> {
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() && v.fract() == 0.0 && (0.0..=4_294_967_295.0).contains(&v) => {
            Ok(v as usize)
        }
        // A fractional, non-finite, or out-of-range value is a number, so
        // `bad_field`'s "is not a number" would misdescribe it: name the bus
        // id constraint instead.
        Ok(_) => Err(Error::FormatRead {
            format: FMT,
            message: format!(
                "field {key} {s:?} is not a valid bus id \
                 (a whole number in 0..=4294967295)"
            ),
        }),
        Err(_) => Err(bad_field(key, s)),
    }
}
fn on(r: &Row, key: &str) -> Result<bool> {
    // A closed vocabulary: an unrecognized status token must not silently
    // mean energized (the same rule applies to numbers in f_or). Absent
    // or empty keeps the documented in service default.
    match r.get(key).copied().map(str::trim) {
        None | Some("") => Ok(true),
        Some(tok) if tok.eq_ignore_ascii_case("Closed") || tok == "1" => Ok(true),
        Some(tok) if tok.eq_ignore_ascii_case("Open") || tok == "0" => Ok(false),
        Some(tok) => Err(bad_field(key, tok)),
    }
}
/// [`on`] over the first present field among `keys` (naming generations).
fn on_alias(r: &Row, keys: &[&str]) -> Result<bool> {
    match keys.iter().find(|k| r.contains_key(*k)) {
        Some(k) => on(r, k),
        None => Ok(true),
    }
}
/// [`uid`] over the first present, non-empty field among `keys`.
fn uid_alias(r: &Row, keys: &[&str]) -> Result<usize> {
    match keys
        .iter()
        .find(|k| matches!(r.get(*k), Some(v) if !v.trim().is_empty()))
    {
        Some(k) => uid(r, k),
        None => Ok(0),
    }
}

fn bus_ref(
    r: &Row,
    num_keys: &[&str],
    label_keys: &[&str],
    bus_labels: &HashMap<&str, BusId>,
) -> Result<BusId> {
    let id = uid_alias(r, num_keys)?;
    if id != 0 {
        return Ok(BusId(id));
    }
    if let Some(label) = first(r, label_keys) {
        return bus_labels
            .get(label)
            .copied()
            .ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: format!("unknown BusName_NomVolt label {label:?}"),
            });
    }
    Err(Error::FormatRead {
        format: FMT,
        message: format!(
            "row missing a bus key (expected one of {} or {})",
            num_keys.join("/"),
            label_keys.join("/")
        ),
    })
}

/// First present, non-empty field among `keys`, trimmed.
fn first<'a>(r: &Row<'a>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| r.get(k).copied())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// First present, non-empty field among `keys` as f64; absent → `default`.
fn f_alias(r: &Row, keys: &[&str], default: f64) -> Result<f64> {
    match keys
        .iter()
        .find(|k| matches!(r.get(*k), Some(v) if !v.trim().is_empty()))
    {
        Some(k) => f_or(r, k, default),
        None => Ok(default),
    }
}

/// Copy `keys` into `extras` verbatim (trimmed of the padding PowerWorld pads
/// quoted values with), skipping absent or empty fields. The PowerWorld field
/// name is the extras key, so the provenance is self describing and the writer
/// can put the value back in the same field.
fn keep_extras(r: &Row, keys: &[&str], extras: &mut Extras) {
    for k in keys {
        if let Some(v) = r.get(k) {
            let v = v.trim();
            if !v.is_empty() {
                extras.insert((*k).to_string(), serde_json::Value::String(v.to_string()));
            }
        }
    }
}

/// `BusCat` (our writer's vocabulary) when present; real exports carry
/// `BusSlack` instead and the PV/PQ split is derived from the generators in
/// [`derive_bus_kinds`].
fn bus_kind(r: &Row) -> BusType {
    match r.get("BusCat").copied().map(str::trim) {
        Some("PV") => BusType::Pv,
        Some("Slack") => BusType::Ref,
        Some("Disconnected") => BusType::Isolated,
        _ => {
            if first(r, &["BusSlack", "Slack"]).is_some_and(|v| v.eq_ignore_ascii_case("YES")) {
                BusType::Ref
            } else {
                BusType::Pq
            }
        }
    }
}

/// PowerWorld stores no PV/PQ bus type; it follows from the machines. A bus
/// with an in-service generator regulates voltage (PV) unless it is the slack.
/// Only buses left at the PQ default are promoted, so an explicit `BusCat`
/// from our own writer is never overridden.
pub(super) fn derive_bus_kinds(buses: &mut [Bus], generators: &[Generator]) {
    use std::collections::HashSet;
    let gen_buses: HashSet<BusId> = generators
        .iter()
        .filter(|g| g.in_service)
        .map(|g| g.bus)
        .collect();
    for b in buses {
        if b.kind == BusType::Pq && gen_buses.contains(&b.id) {
            b.kind = BusType::Pv;
        }
    }
}

fn read_bus(r: &Row) -> Result<Bus> {
    // The bus's own identity goes through the same validation as every bus
    // reference (`uid`): a fractional or out-of-range id is a read error, not
    // a silently truncated or saturated id. Report whichever key carried the
    // value so an error names the column the file actually used.
    let (id_key, id_field) = ["BusNum", "Number"]
        .into_iter()
        .find_map(|k| first(r, &[k]).map(|v| (k, v)))
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "Bus block row missing a numeric BusNum/Number".into(),
        })?;
    let id = bus_id_from_field(id_key, id_field)?;
    let name = first(r, &["BusName", "Name"]).map(ToString::to_string);
    let mut extras = Extras::new();
    // `SubNum` stays in extras: it is identity rather than geometry, and the
    // substation join reads it back.
    let promoted = bus_location(r);
    let location = promoted.map(|(location, _)| location);
    keep_extras(
        r,
        &["SubNum", "SubNumber", "OwnerNum", "OwnerNumber", "BANumber"],
        &mut extras,
    );
    // Only the pair that promoted leaves extras. A complete case export
    // carries both pairs, and the one the location did not come from is
    // unmodeled data like any other column.
    let used = promoted.map_or([""; 2], |(_, pair)| pair);
    for key in ["Latitude:1", "Longitude:1", "Latitude", "Longitude"] {
        if !used.contains(&key) {
            keep_extras(r, &[key], &mut extras);
        }
    }
    Ok(Bus {
        id: BusId(id),
        kind: bus_kind(r),
        vm: f_alias(r, &["BusPUVolt", "Vpu"], 1.0)?,
        va: f_alias(r, &["BusAngle", "Vangle"], 0.0)?,
        base_kv: f_alias(r, &["BusNomVolt", "NomkV"], 0.0)?,
        // Real exports carry per rating set voltage limits; set 1 (set A in
        // the 2022 vocabulary) is the default set. Our writer's
        // BusVMax/BusVMin are the fallback aliases.
        vmax: f_alias(r, &["BusVoltLimHigh:1", "LimitHighA", "BusVMax"], 1.1)?,
        vmin: f_alias(r, &["BusVoltLimLow:1", "LimitLowA", "BusVMin"], 0.9)?,
        evhi: None,
        evlo: None,
        area: uid_alias(r, &["AreaNum", "AreaNumber"])?,
        zone: uid_alias(r, &["ZoneNum", "ZoneNumber"])?,
        name,
        uid: None,
        location,
        extras,
    })
}

/// The promoted geographic location and the column pair it came from: the
/// substation `Latitude:1`/`Longitude:1` pair, else the bus's own bare
/// `Latitude`/`Longitude` pair (older complete case exports). A pair promotes
/// only when both of its own columns parse finite, so a half filled pair never
/// pairs a latitude with the other pair's longitude.
fn bus_location(r: &Row) -> Option<(crate::geo::Location, [&'static str; 2])> {
    for pair in [["Latitude:1", "Longitude:1"], ["Latitude", "Longitude"]] {
        let (Some(lat), Some(lon)) = (
            first(r, &pair[..1]).and_then(|v| v.parse::<f64>().ok()),
            first(r, &pair[1..]).and_then(|v| v.parse::<f64>().ok()),
        ) else {
            continue;
        };
        if lat.is_finite() && lon.is_finite() {
            return Some((
                crate::geo::Location {
                    x: lon,
                    y: lat,
                    kind: None,
                },
                pair,
            ));
        }
    }
    None
}

fn read_load(r: &Row, bus_labels: &HashMap<&str, BusId>, index: usize) -> Result<Load> {
    // Complete case exports write ZIP components (constant power S, constant
    // current I, constant impedance Z, each MW/MVAr at nominal voltage); the
    // simple LoadMW/LoadMVR pair is our own writer's form. The typed model
    // carries the total at nominal voltage; nonzero I/Z components are kept in
    // extras so nothing about the voltage dependence is lost.
    let (p, q);
    let mut extras = Extras::new();
    if r.contains_key("LoadMW") || r.contains_key("LoadMVR") {
        p = f(r, "LoadMW")?;
        q = f(r, "LoadMVR")?;
    } else {
        let smw = f_alias(r, &["LoadSMW", "SMW"], 0.0)?;
        let imw = f_alias(r, &["LoadIMW", "IMW"], 0.0)?;
        let zmw = f_alias(r, &["LoadZMW", "ZMW"], 0.0)?;
        let smvr = f_alias(r, &["LoadSMVR", "SMvar"], 0.0)?;
        let imvr = f_alias(r, &["LoadIMVR", "IMvar"], 0.0)?;
        let zmvr = f_alias(r, &["LoadZMVR", "ZMvar"], 0.0)?;
        p = smw + imw + zmw;
        q = smvr + imvr + zmvr;
        if imw != 0.0 || zmw != 0.0 || imvr != 0.0 || zmvr != 0.0 {
            keep_extras(
                r,
                &[
                    "LoadSMW", "LoadSMVR", "LoadIMW", "LoadIMVR", "LoadZMW", "LoadZMVR",
                ],
                &mut extras,
            );
        }
    }
    keep_extras(r, &["LoadID", "ID"], &mut extras);
    super::drop_positional_id(&mut extras, &["LoadID", "ID"], index);
    Ok(Load {
        bus: bus_ref(r, &["BusNum"], &["BusName_NomVolt"], bus_labels)?,
        p,
        q,
        voltage_model: None,
        in_service: on_alias(r, &["LoadStatus", "Status"])?,
        uid: None,
        extras,
    })
}

fn read_shunt(r: &Row, bus_labels: &HashMap<&str, BusId>, index: usize) -> Result<Shunt> {
    let mut extras = Extras::new();
    keep_extras(r, &["ShuntID", "ID", "SSCMode", "ShuntMode"], &mut extras);
    super::drop_positional_id(&mut extras, &["ShuntID", "ID"], index);
    Ok(Shunt {
        bus: bus_ref(r, &["BusNum"], &["BusName_NomVolt"], bus_labels)?,
        // Switched shunt nominal MW/MVAr in real exports (MWNom/MvarNom in
        // the 2022 vocabulary); ShuntMW/ShuntMVR from our writer.
        g: f_alias(r, &["ShuntMW", "SSNMW", "MWNom"], 0.0)?,
        b: f_alias(r, &["ShuntMVR", "SSNMVR", "MvarNom"], 0.0)?,
        in_service: on_alias(r, &["ShuntStatus", "SSStatus", "Status"])?,
        control: None,
        uid: None,
        extras,
    })
}

// `Generator` has no extras map (a deliberate parse-performance decision; see
// the `GenCaps` doc), so GenID and the regulation fields are not retained on
// the typed model. They stay reachable through the generic layer and survive
// aux → aux via the retained source.
fn read_gen(r: &Row, bus_labels: &HashMap<&str, BusId>) -> Result<Generator> {
    Ok(Generator {
        bus: bus_ref(r, &["BusNum"], &["BusName_NomVolt"], bus_labels)?,
        // GenMW is the solved output; complete case exports write the
        // dispatch setpoint instead.
        pg: f_alias(r, &["GenMW", "GenMWSetPoint", "MWSetPoint"], 0.0)?,
        qg: f_alias(r, &["GenMVR", "GenMvrSetPoint", "MvarSetPoint"], 0.0)?,
        pmax: f_alias(r, &["GenMWMax", "MWMax"], 0.0)?,
        pmin: f_alias(r, &["GenMWMin", "MWMin"], 0.0)?,
        qmax: f_alias(r, &["GenMVRMax", "MvarMax"], 0.0)?,
        qmin: f_alias(r, &["GenMVRMin", "MvarMin"], 0.0)?,
        vg: f_alias(r, &["GenVoltSet", "VoltSet"], 1.0)?,
        mbase: f_alias(r, &["GenMVABase", "MVABase"], 100.0)?,
        in_service: on_alias(r, &["GenStatus", "Status"])?,
        cost: None,
        caps: Default::default(),
        regulated_bus: None,
        uid: None,
    })
}

fn read_branch(
    r: &Row,
    bus_labels: &HashMap<&str, BusId>,
    parallel: &mut HashMap<(BusId, BusId), u32>,
) -> Result<Branch> {
    let is_xf = first(r, &[BRANCH_DEVICE_TYPE]).is_some_and(|v| v == "Transformer");
    let from = bus_ref(
        r,
        &["BusNum", "BusNumFrom"],
        &["BusName_NomVolt"],
        bus_labels,
    )?;
    let to = bus_ref(
        r,
        &["BusNum:1", "BusNumTo"],
        &["BusName_NomVolt:1"],
        bus_labels,
    )?;
    let nth = parallel.entry((from, to)).or_insert(0);
    *nth += 1;
    let mut extras = Extras::new();
    // Branch identity beyond the bus pair: circuit ID and device type. Kept
    // verbatim (PowerWorld pads circuit IDs) so aux → aux through the typed
    // model reproduces them exactly — except when a value states exactly what
    // the writer derives on its own: the positional circuit for this bus pair,
    // or the Line/Transformer kind the tap already encodes. Exotic device
    // types (Breaker, Series Cap, ...) are always kept.
    if let Some(v) = r.get(LINE_CIRCUIT).or_else(|| r.get("Circuit")) {
        extras.insert(
            LINE_CIRCUIT.to_string(),
            serde_json::Value::String((*v).to_string()),
        );
    }
    keep_extras(r, &[BRANCH_DEVICE_TYPE, "LineLength"], &mut extras);
    // The circuit counter is per bus pair, so the second parallel branch's "2"
    // is as much a positional default as the first's "1". Dropped through the
    // shared rule rather than by a filter on the insert, so the aux and pwb
    // readers cannot disagree about what a default is.
    super::drop_positional_id(&mut extras, &[LINE_CIRCUIT], *nth as usize - 1);
    // Transformer records in complete case exports carry their impedance and
    // tap under `:1` locations (values on the system base after correction);
    // line records use the bare names. Our writer's LineXFRatio is the tap
    // fallback.
    // 2016 era exports use the bare name here like everywhere else.
    let tap = f_alias(
        r,
        &["LineTap:1", "Tapxfbase", "LineXFRatio", "LineTap"],
        1.0,
    )?;
    let stored_tap = if is_xf { tap } else { 0.0 };
    let shift = f_alias(r, &["LinePhase", "Phase"], 0.0)?;
    // Drop the device type only when the writer would derive this exact value
    // back. It derives from `tap != 0 || shift != 0`, so a record that states
    // `Line` while stating a phase shift does not round trip: dropping it here
    // rewrites the branch as a `Transformer`. Comparing against the rule
    // rather than against the pair of names is what keeps the two in step.
    let derived = if stored_tap != 0.0 || shift != 0.0 {
        "Transformer"
    } else {
        "Line"
    };
    if extras.get(BRANCH_DEVICE_TYPE).and_then(|v| v.as_str()) == Some(derived) {
        extras.remove(BRANCH_DEVICE_TYPE);
    }
    Ok(Branch {
        from,
        to,
        r: f_alias(r, &["LineR", "LineR:1", "R", "Rxfbase"], 0.0)?,
        x: f_alias(r, &["LineX", "LineX:1", "X", "Xxfbase"], 0.0)?,
        b: f_alias(r, &["LineC", "LineC:1", "B", "Bxfbase"], 0.0)?,
        charging: None,
        rate_a: f_alias(r, &["LineAMVA", "LimitMVAA"], 0.0)?,
        rate_b: f_alias(r, &["LineAMVA:1", "LineBMVA", "LimitMVAB"], 0.0)?,
        rate_c: f_alias(r, &["LineAMVA:2", "LineCMVA", "LimitMVAC"], 0.0)?,
        rating_sets: Vec::new(),
        current_ratings: None,
        tap: stored_tap,
        shift,
        in_service: on_alias(r, &["LineStatus", "Status"])?,
        angmin: -360.0,
        angmax: 360.0,
        control: None,
        solution: None,
        uid: None,
        route: None,
        extras,
    })
}

// ---- Writer -----------------------------------------------------------------

#[must_use]
// A flat serializer: one section per PowerWorld object type; splitting it would
// add indirection without clarity.
#[expect(clippy::too_many_lines)]
pub fn write_powerworld(net: &BalancedNetwork) -> Conversion {
    let mut warnings = Diagnostics::new();
    let mut nonfinite = false;
    let mut sanitized_names = 0usize;
    let mut n = |x: f64| -> String {
        if x.is_finite() {
            format!("{x}")
        } else {
            nonfinite = true;
            format!(
                "{}",
                if x > 0.0 {
                    1.0e10
                } else if x < 0.0 {
                    -1.0e10
                } else {
                    0.0
                }
            )
        }
    };
    let mut s = String::new();
    // A `//` comment ends at the line break, so a terminator in the name
    // would leave the rest of it uncommented as an aux DATA block.
    let _ = writeln!(
        s,
        "// PowerWorld auxiliary file — powerio export: {}",
        sanitize_quoted(net.name(), NAME_FORBIDDEN, ' ')
    );
    let _ = writeln!(s, "// baseMVA {}", net.base_mva());
    let _ = writeln!(s);

    // The coordinate columns appear only when the case carries locations, so
    // a case without geometry writes exactly as before. A located case still
    // writes `""` for the odd bus without a point; the reader leaves those
    // unpromoted.
    let write_locations = net.buses().iter().any(|b| b.location.is_some());
    block(
        &mut s,
        "Bus",
        if write_locations {
            "[BusNum, BusName, BusNomVolt, BusPUVolt, BusAngle, AreaNum, ZoneNum, BusVMax, BusVMin, BusCat, Latitude:1, Longitude:1]"
        } else {
            "[BusNum, BusName, BusNomVolt, BusPUVolt, BusAngle, AreaNum, ZoneNum, BusVMax, BusVMin, BusCat]"
        },
        |rows| {
            for b in net.buses() {
                let raw_name = b.name.as_deref().unwrap_or("");
                let name = sanitize_quoted(raw_name, NAME_FORBIDDEN, ' ');
                if matches!(name, std::borrow::Cow::Owned(_)) {
                    sanitized_names += 1;
                }
                let mut row = format!(
                    "{} \"{}\" {} {} {} {} {} {} {} \"{}\"",
                    b.id,
                    name,
                    n(b.base_kv),
                    n(b.vm),
                    n(b.va),
                    b.area,
                    b.zone,
                    n(b.vmax),
                    n(b.vmin),
                    bus_cat(b.kind)
                );
                if write_locations {
                    match b.location {
                        Some(location) => {
                            let _ = write!(row, " {} {}", n(location.y), n(location.x));
                        }
                        None => row.push_str(" \"\" \"\""),
                    }
                }
                rows.push(row);
            }
        },
    );

    block(
        &mut s,
        "Load",
        "[BusNum, LoadID, LoadMW, LoadMVR, LoadStatus]",
        |rows| {
            for (i, l) in net.loads().iter().enumerate() {
                rows.push(format!(
                    "{} \"{}\" {} {} \"{}\"",
                    l.bus,
                    id_of(&l.extras, "LoadID", i),
                    n(l.p),
                    n(l.q),
                    status(l.in_service)
                ));
            }
        },
    );

    block(
        &mut s,
        "Shunt",
        "[BusNum, ShuntID, ShuntMW, ShuntMVR, ShuntStatus]",
        |rows| {
            for (i, sh) in net.shunts().iter().enumerate() {
                rows.push(format!(
                    "{} \"{}\" {} {} \"{}\"",
                    sh.bus,
                    id_of(&sh.extras, "ShuntID", i),
                    n(sh.g),
                    n(sh.b),
                    status(sh.in_service)
                ));
            }
        },
    );

    block(
        &mut s,
        "Gen",
        "[BusNum, GenID, GenMW, GenMVR, GenMWMax, GenMWMin, GenMVRMax, GenMVRMin, GenVoltSet, GenMVABase, GenStatus]",
        |rows| {
            for (i, g) in net.generators().iter().enumerate() {
                rows.push(format!(
                    "{} \"{}\" {} {} {} {} {} {} {} {} \"{}\"",
                    g.bus,
                    i + 1,
                    n(g.pg),
                    n(g.qg),
                    n(g.pmax),
                    n(g.pmin),
                    n(g.qmax),
                    n(g.qmin),
                    n(g.vg),
                    n(g.mbase),
                    status(g.in_service)
                ));
            }
        },
    );

    block(
        &mut s,
        "Branch",
        "[BusNum, BusNum:1, LineCircuit, LineR, LineX, LineC, LineAMVA, LineBMVA, LineCMVA, LineXFRatio, LinePhase, LineStatus, BranchDeviceType]",
        |rows| {
            // Parallel branches need distinct circuit IDs: the bus pair plus
            // circuit is the PowerWorld branch identity, and a reader (ours
            // included) treats equal identities as one device.
            let mut parallel: HashMap<(BusId, BusId), u32> = HashMap::new();
            for br in net.branches() {
                let kind = match br.extras.get(BRANCH_DEVICE_TYPE).and_then(|v| v.as_str()) {
                    Some(v) => v,
                    None if br.is_transformer() => "Transformer",
                    None => "Line",
                };
                let nth = parallel.entry((br.from, br.to)).or_insert(0);
                *nth += 1;
                let fallback = nth.to_string();
                let circuit = br
                    .extras
                    .get(LINE_CIRCUIT)
                    .and_then(|v| v.as_str())
                    .unwrap_or(&fallback);
                rows.push(format!(
                    "{} {} \"{}\" {} {} {} {} {} {} {} {} \"{}\" \"{}\"",
                    br.from,
                    br.to,
                    circuit,
                    n(br.r),
                    n(br.x),
                    n(br.total_charging_b()),
                    n(br.rate_a),
                    n(br.rate_b),
                    n(br.rate_c),
                    n(br.effective_tap()),
                    n(br.shift),
                    status(br.in_service),
                    kind
                ));
            }
        },
    );

    if net.generators().iter().any(|g| g.cost.is_some()) {
        warnings.push(
            &F.field_dropped,
            "generator cost curves dropped: not written to PowerWorld .aux",
        );
    }
    if !net.hvdc().is_empty() {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} dcline(s) dropped: PowerWorld HVDC not modeled",
                net.hvdc().len()
            ),
        );
    }
    if !net.transformers_3w().is_empty() {
        warnings.push(&F.record_dropped, format!(
            "{} 3-winding transformer(s) dropped: the PowerWorld .aux writer emits no 3-winding record",
            net.transformers_3w().len()
        ));
    }
    if net
        .buses()
        .iter()
        .any(|b| b.evhi.is_some() || b.evlo.is_some())
    {
        warnings.push(
            &F.field_dropped,
            "emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band",
        );
    }
    if !net.storage().is_empty() {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} storage unit(s) dropped: PowerWorld storage not modeled",
                net.storage().len()
            ),
        );
    }
    let voltage_loads = net
        .loads()
        .iter()
        .filter(|l| {
            l.voltage_model
                .as_ref()
                .is_some_and(LoadVoltageModel::has_non_matpower_fields)
        })
        .count();
    if voltage_loads > 0 {
        warnings.push(&F.field_dropped, format!(
            "{voltage_loads} voltage dependent load model(s) dropped: PowerWorld Load records carry static MW/MVR only"
        ));
    }
    let terminal_charging = net
        .branches()
        .iter()
        .filter(|b| b.has_non_matpower_charging())
        .count();
    if terminal_charging > 0 {
        warnings.push(&F.value_collapsed, format!(
            "{terminal_charging} branch terminal admittance record(s) collapsed to total susceptance: PowerWorld aux branch rows written here cannot carry conductance or asymmetric terminal charging"
        ));
    }
    let current_ratings = net
        .branches()
        .iter()
        .filter(|b| b.current_ratings.is_some())
        .count();
    if current_ratings > 0 {
        warnings.push(&F.field_dropped, format!(
            "{current_ratings} branch current rating record(s) dropped: PowerWorld aux branch rows written here carry MVA ratings only"
        ));
    }
    warn_extra_branch_rating_sets(&F, "PowerWorld .aux", net, &mut warnings);
    // The keys this writer replays: the two device ids, the branch circuit,
    // and the device type. Anything else a reader retained — a foreign
    // format's ids, ZIP components, LineLength — is dropped, and says so
    // (#330).
    super::super::warn_dropped_extras(
        &F,
        "PowerWorld .aux",
        net,
        |key| matches!(key, "LoadID" | "ShuntID" | "BranchDeviceType") || key == LINE_CIRCUIT,
        &mut warnings,
    );
    super::super::warn_dropped_areas(&F, "PowerWorld .aux", net, &mut warnings);
    let branch_solutions = net
        .branches()
        .iter()
        .filter(|b| b.solution.is_some())
        .count();
    if branch_solutions > 0 {
        warnings.push(&F.field_dropped, format!(
            "{branch_solutions} branch solution value set(s) dropped: PowerWorld aux result fields are not written"
        ));
    }
    if net.branches().iter().any(Branch::has_angle_limits) {
        warnings.push(
            &F.field_dropped,
            "branch angle limits (angmin/angmax) dropped: not written to PowerWorld .aux",
        );
    }
    if net.generators().iter().any(Generator::has_caps) {
        warnings.push(
            &F.field_dropped,
            "generator ramp/capability columns dropped: not written to PowerWorld .aux",
        );
    }
    if nonfinite {
        warnings.push(
            &F.not_a_number,
            "non-finite values written as ±1e10 sentinels",
        );
    }
    if sanitized_names > 0 {
        warnings.push(
            &F.value_substituted,
            format!(
                "{sanitized_names} bus name(s) contained a double quote that would corrupt a \
             PowerWorld value; replaced with spaces"
            ),
        );
    }

    Conversion::new(s, warnings)
}

/// Device ID for the writer: the retained PowerWorld ID from `extras` when the
/// element came from an aux read, else the 1-based position.
fn id_of(extras: &Extras, key: &str, index: usize) -> String {
    match extras.get(key).and_then(serde_json::Value::as_str) {
        Some(v) => v.to_string(),
        None => (index + 1).to_string(),
    }
}

fn block(s: &mut String, object: &str, fields: &str, fill: impl FnOnce(&mut Vec<String>)) {
    let mut rows = Vec::new();
    fill(&mut rows);
    let _ = writeln!(s, "DATA ({object}, {fields})");
    let _ = writeln!(s, "{{");
    for r in &rows {
        let _ = writeln!(s, "  {r}");
    }
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
}

fn status(on: bool) -> &'static str {
    if on { "Closed" } else { "Open" }
}

fn bus_cat(kind: BusType) -> &'static str {
    match kind {
        BusType::Pq => "PQ",
        BusType::Pv => "PV",
        BusType::Ref => "Slack",
        BusType::Isolated => "Disconnected",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_bus_rejects_non_integral_and_out_of_range_ids() {
        // The bus's own identity goes through the same guard as every bus
        // reference: a fractional or oversized BusNum is a read error, not a
        // silently truncated or saturated id.
        for bad in ["5.7", "1e20", "-3", "NaN", "inf"] {
            let mut row: Row = HashMap::new();
            row.insert("BusNum", bad);
            assert!(
                read_bus(&row).is_err(),
                "BusNum={bad} should be refused, not bent to an id"
            );
        }
        let mut ok: Row = HashMap::new();
        ok.insert("BusNum", "42");
        assert_eq!(read_bus(&ok).unwrap().id, BusId(42));
    }
}

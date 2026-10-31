//! Distribution CIM into a [`MulticonductorNetwork`].
//!
//! `ConnectivityNode` is the bus. Each conducting equipment's `Terminal`s tie
//! it to nodes, and its per phase child objects (`ACLineSegmentPhase`, …)
//! give the conductors. Impedance comes from `PerLengthPhaseImpedance` (a
//! matrix in ohm/m, a direct fit for [`DistLineCode`]) or from sequence
//! values (`PerLengthSequenceImpedance`, or totals on the segment) expanded
//! to a phase matrix.
//!
//! Nothing drops silently. Objects of a class the mapping does not read are
//! counted per class, properties of mapped objects the mapping does not read
//! are counted per class and property, and every default, substitution,
//! dropped record, and unresolved reference has its own finding.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::f64::consts::PI;
use std::rc::Rc;

use super::xml::{CimDocument, CimObject, PropValue, parse_cimxml};
use super::{
    LEAKAGE_READING, no_load_test_pct, sequence_to_phase, short_circuit_test_pct, tank_phase_clock,
    wrap_angle, z_base,
};
use crate::diagnostics::{Diagnostic, DiagnosticInfo, codes as C};
use crate::geo::{CoordinateSpace, DistCoordsKind, DistGeoMeta, DistLocation};
use crate::model::{
    ConductorMatrix, Configuration, DistBus, DistIbr, DistLine, DistLineCode, DistLoad, DistShunt,
    DistSourceFormat, DistSwitch, DistTransformer, DistWinding, DistWindingConn, IbrPrimeMover,
    IbrTopology, MulticonductorNetwork, MulticonductorNetworkTables, VoltageSource,
};
use crate::readiness::UNRESOLVED_IMPEDANCE_KEY;
use crate::{Error, Result};

mod sedms;

const FMT: &str = "distribution CIM";

/// The most conductors a phase impedance matrix may state. The count sizes
/// four square matrices, so a larger one is dropped rather than allocated.
const MAX_CONDUCTORS: usize = 64;

/// The switch classes the reader maps to [`DistSwitch`].
pub(super) const SWITCH_CLASSES: [&str; 8] = [
    "Switch",
    "LoadBreakSwitch",
    "Breaker",
    "Recloser",
    "Sectionaliser",
    "Fuse",
    "Disconnector",
    "Jumper",
];

/// Classes the mapping reads. An object of any other class is reported as
/// unmapped, per class.
const MAPPED_CLASSES: [&str; 23] = [
    "ConnectivityNode",
    "Terminal",
    "BaseFrequency",
    "ACLineSegment",
    "ACLineSegmentPhase",
    "PerLengthPhaseImpedance",
    "PhaseImpedanceData",
    "PerLengthSequenceImpedance",
    "EnergyConsumer",
    "EnergyConsumerPhase",
    "LinearShuntCompensator",
    "LinearShuntCompensatorPhase",
    "ShuntCompensatorPhase",
    "SwitchPhase",
    "EnergySource",
    "EnergySourcePhase",
    "PowerTransformer",
    "PowerTransformerEnd",
    "TransformerMeshImpedance",
    "TransformerCoreAdmittance",
    "TransformerTank",
    "TransformerTankEnd",
    "RatioTapChanger",
];

/// Association roles that may hold several values: a repeated statement
/// adds a value, within one document or across documents, rather than
/// conflicting with the first.
const MULTI_VALUED: [&str; 2] = [
    "Asset.PowerSystemResources",
    "ShortCircuitTest.GroundedEnds",
];

/// Containment, naming, version, and base voltage classes: context the
/// multiconductor model has no table for. They are reported at `Remark`
/// severity, since they carry no electrical data the network loses. A
/// `CompositeSwitch` only groups switches the mapping reads one by one.
const CONTEXT_CLASSES: [&str; 21] = [
    "BaseVoltage",
    "GeographicalRegion",
    "SubGeographicalRegion",
    "Substation",
    "VoltageLevel",
    "Bay",
    "Feeder",
    "Circuit",
    "Line",
    "CompositeSwitch",
    "PSRType",
    "Name",
    "NameType",
    "NameTypeAuthority",
    "IEC61970CIMVersion",
    "IEC61968CIMVersion",
    "CoordinateSystem",
    "Analog",
    "Discrete",
    "AnalogValue",
    "DiscreteValue",
];

/// Properties of mapped objects that only carry context (descriptions,
/// containment, grouping, catalog and location links). Reported at `Remark`
/// severity when present, like [`CONTEXT_CLASSES`].
const CONTEXT_PROPERTIES: [&str; 9] = [
    "IdentifiedObject.description",
    "IdentifiedObject.aliasName",
    "IdentifiedObject.Names",
    "ConductingEquipment.BaseVoltage",
    "Equipment.EquipmentContainer",
    "ConnectivityNode.ConnectivityNodeContainer",
    "PowerSystemResource.Location",
    "PowerSystemResource.PSRType",
    "Switch.CompositeSwitch",
];

/// The wire and cable catalog classes whose `WireInfo.ratedCurrent` a
/// segment's `PowerSystemResource.AssetDatasheet` can name.
const WIRE_INFO_CLASSES: [&str; 5] = [
    "WireInfo",
    "OverheadWireInfo",
    "CableInfo",
    "ConcentricNeutralCableInfo",
    "TapeShieldCableInfo",
];

// ---------------------------------------------------------------------------
// Source acquisition
// ---------------------------------------------------------------------------

/// Read a source into the multiconductor model: one CIMXML document, or every
/// `.xml` file at a directory's top level read as one case, in name order.
/// Files in subdirectories are not part of the case.
pub(crate) fn parse_cim_collecting(
    source: &powerio_core::Source,
    diags: &mut crate::collect::Diagnostics,
) -> Result<MulticonductorNetwork> {
    let mut documents = Vec::new();
    let name_hint;
    if source.is_directory() {
        name_hint = std::path::Path::new(source.name())
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_owned);
        for name in source.entry_names().map_err(|e| source_error(&e))? {
            if !super::is_case_document(name.as_str()) {
                continue;
            }
            let buffer = source
                .root_buffer(name.as_str())
                .map_err(|e| source_error(&e))?;
            documents.push((name.as_str().to_owned(), buffer));
        }
        if documents.is_empty() {
            return Err(format_error(format!(
                "the directory {} holds no .xml instance files at its top level",
                source.name()
            )));
        }
    } else {
        name_hint = std::path::Path::new(source.name())
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_owned);
        let buffer = source.primary_buffer().map_err(|e| source_error(&e))?;
        documents.push((source.name().to_owned(), buffer));
    }
    let mut parsed = Vec::with_capacity(documents.len());
    for (name, buffer) in &documents {
        let text = std::str::from_utf8(buffer.content_bytes())
            .map_err(|e| format_error(format!("{name} is not valid UTF-8: {e}")))?;
        parsed.push((name.clone(), parse_cimxml(text)?));
    }
    read_documents(parsed, name_hint.as_deref(), diags)
}

fn format_error(message: String) -> Error {
    Error::FormatRead {
        format: FMT,
        message,
    }
}

fn source_error(error: &powerio_core::Error) -> Error {
    format_error(error.to_string())
}

// ---------------------------------------------------------------------------
// Merged object store
// ---------------------------------------------------------------------------

struct Merged {
    id: String,
    class: String,
    defined: bool,
    props: Vec<(String, PropValue)>,
    /// Properties stated with nested elements instead of a value.
    nested: Vec<String>,
}

/// Every object of the document set, merged by id, with a record of which
/// properties the mapping read.
struct Store {
    objects: Vec<Merged>,
    by_id: HashMap<String, usize>,
    by_class: HashMap<String, Vec<usize>>,
    read: RefCell<Vec<Vec<bool>>>,
}

impl Store {
    fn new() -> Self {
        Store {
            objects: Vec::new(),
            by_id: HashMap::new(),
            by_class: HashMap::new(),
            read: RefCell::new(Vec::new()),
        }
    }

    /// Merge one document. An `rdf:about` extension adds properties to the
    /// object its id names. A property a second statement assigns a
    /// different value keeps its first value, with a finding, except a
    /// [`MULTI_VALUED`] role, which collects every distinct value.
    fn merge(
        &mut self,
        document: &str,
        doc: CimDocument,
        diags: &mut crate::collect::Diagnostics,
    ) -> Result<()> {
        for CimObject {
            class,
            id,
            definition,
            props,
            nested,
        } in doc.objects
        {
            if id.is_empty() {
                return Err(format_error(format!(
                    "{document}: a {class} object has neither rdf:ID nor rdf:about"
                )));
            }
            let Some(&at) = self.by_id.get(&id) else {
                self.by_id.insert(id.clone(), self.objects.len());
                self.objects.push(Merged {
                    id,
                    class,
                    defined: definition,
                    props,
                    nested,
                });
                continue;
            };
            let existing = &mut self.objects[at];
            if definition && existing.defined {
                return Err(format_error(format!(
                    "{document}: rdf:ID `{id}` is defined twice"
                )));
            }
            if existing.class != class {
                return Err(format_error(format!(
                    "{document}: `{id}` is a {} and a {class}",
                    existing.class
                )));
            }
            existing.defined |= definition;
            existing.nested.extend(nested);
            for (key, value) in props {
                if MULTI_VALUED.contains(&key.as_str()) {
                    if !existing.props.iter().any(|(k, v)| *k == key && *v == value) {
                        existing.props.push((key, value));
                    }
                    continue;
                }
                match existing.props.iter().find(|(k, _)| *k == key) {
                    Some((_, first)) if *first == value => {}
                    Some((_, first)) => {
                        let message = format!(
                            "{class} {id}: {document} states {key} = `{}`, but an earlier \
                             statement gave `{}`; the first is kept",
                            value.as_str(),
                            first.as_str()
                        );
                        let mut diagnostic =
                            Diagnostic::of(&C::READ_CIM_DIST_VALUE_CONFLICT, message);
                        let _ = diagnostic.insert_detail("class", serde_json::Value::from(&*class));
                        let _ = diagnostic.insert_detail("mrid", serde_json::Value::from(&*id));
                        diags.record(diagnostic);
                    }
                    None => existing.props.push((key, value)),
                }
            }
        }
        Ok(())
    }

    /// Index by class and size the read record once merging is complete.
    fn seal(&mut self) {
        for (at, object) in self.objects.iter().enumerate() {
            self.by_class
                .entry(object.class.clone())
                .or_default()
                .push(at);
        }
        *self.read.get_mut() = self
            .objects
            .iter()
            .map(|o| vec![false; o.props.len()])
            .collect();
    }

    fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    fn class_of(&self, id: &str) -> Option<&str> {
        self.by_id
            .get(id)
            .map(|&at| self.objects[at].class.as_str())
    }

    /// The ids of every object of `class`, in document order.
    fn ids_of(&self, class: &str) -> Vec<String> {
        self.by_class.get(class).map_or_else(Vec::new, |at| {
            at.iter().map(|&i| self.objects[i].id.clone()).collect()
        })
    }

    fn count_of(&self, class: &str) -> usize {
        self.by_class.get(class).map_or(0, Vec::len)
    }

    /// The first value of `key` on object `id`, recording the read.
    fn prop(&self, id: &str, key: &str) -> Option<&PropValue> {
        let &at = self.by_id.get(id)?;
        let object = &self.objects[at];
        let mut found = None;
        let mut read = self.read.borrow_mut();
        for (i, (k, v)) in object.props.iter().enumerate() {
            if k == key {
                read[at][i] = true;
                found.get_or_insert(v);
            }
        }
        found
    }

    /// Every value of `key` on object `id`, in statement order, recording
    /// the reads.
    fn props_all(&self, id: &str, key: &str) -> Vec<&PropValue> {
        let Some(&at) = self.by_id.get(id) else {
            return Vec::new();
        };
        let object = &self.objects[at];
        let mut read = self.read.borrow_mut();
        let mut found = Vec::new();
        for (i, (k, v)) in object.props.iter().enumerate() {
            if k == key {
                read[at][i] = true;
                found.push(v);
            }
        }
        found
    }

    /// The target ids of every value of reference `key` on `id`.
    fn refs_all(&self, id: &str, key: &str) -> Vec<&str> {
        self.props_all(id, key)
            .into_iter()
            .filter_map(|v| match v {
                PropValue::Ref(target) => Some(target.as_str()),
                PropValue::Text(_) => None,
            })
            .collect()
    }

    /// Record `key` on `id` as consumed without using its value.
    fn consume(&self, id: &str, key: &str) {
        let _ = self.prop(id, key);
    }

    fn text(&self, id: &str, key: &str) -> Option<&str> {
        self.prop(id, key).map(PropValue::as_str)
    }

    fn f(&self, id: &str, key: &str) -> Option<f64> {
        self.text(id, key)?
            .trim()
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite())
    }

    fn boolean(&self, id: &str, key: &str) -> Option<bool> {
        match self.text(id, key)?.trim() {
            "true" | "TRUE" | "True" | "1" => Some(true),
            "false" | "FALSE" | "False" | "0" => Some(false),
            _ => None,
        }
    }

    /// The target id of a reference property.
    fn refv(&self, id: &str, key: &str) -> Option<&str> {
        match self.prop(id, key)? {
            PropValue::Ref(target) => Some(target),
            PropValue::Text(_) => None,
        }
    }

    /// The value of an enumeration property: the `value` tail of an
    /// `EnumClass.value` reference, or the literal an export writes as text
    /// instead (`<cim:Terminal.phases>ABC</cim:Terminal.phases>`), as some
    /// CIM16 exports do.
    fn enum_value(&self, id: &str, key: &str) -> Option<&str> {
        match self.prop(id, key)? {
            PropValue::Ref(target) => target.rsplit('.').next(),
            PropValue::Text(text) => text.trim().rsplit('.').next().filter(|v| !v.is_empty()),
        }
    }

    /// The object's name, when it states a nonempty one.
    fn name_of(&self, id: &str) -> Option<String> {
        self.text(id, "IdentifiedObject.name")
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
    }

    /// The object ids of `class` grouped by the target of reference `key`.
    fn index_by(&self, class: &str, key: &str) -> HashMap<String, Vec<String>> {
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for id in self.ids_of(class) {
            if let Some(target) = self.refv(&id, key) {
                out.entry(target.to_owned()).or_default().push(id.clone());
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Phase mapping
// ---------------------------------------------------------------------------

/// A `SinglePhaseKind` value to its wire coordinate terminal name.
fn phase_terminal(phase: &str) -> Option<&'static str> {
    match phase {
        "A" | "s1" => Some("1"),
        "B" | "s2" => Some("2"),
        "C" => Some("3"),
        "N" => Some("4"),
        _ => None,
    }
}

/// Expand a `PhaseCode` value (`A`, `ABC`, `ABCN`, `AB`, `s1`, `s12`,
/// `s1N`, `s12N`) into terminal names in canonical order. `None` for a value
/// outside that alphabet.
fn expand_phase_code(code: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |t: &str| {
        if !out.iter().any(|o| o == t) {
            out.push(t.to_owned());
        }
    };
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            'A' => push("1"),
            'B' => push("2"),
            'C' => push("3"),
            'N' => push("4"),
            's' => match chars.next() {
                Some('1') => {
                    push("1");
                    if chars.peek() == Some(&'2') {
                        chars.next();
                        push("2");
                    }
                }
                Some('2') => push("2"),
                _ => return None,
            },
            _ => return None,
        }
    }
    Some(out)
}

/// A wye or single phase element's terminal map connects its phases plus the
/// grounded neutral (terminal 4). Delta maps have no neutral.
fn with_neutral(mut map: Vec<String>, config: Configuration) -> Vec<String> {
    if matches!(config, Configuration::Wye | Configuration::SinglePhase)
        && !map.iter().any(|t| t == "4")
    {
        map.push("4".to_owned());
    }
    map
}

fn three_phases() -> Vec<String> {
    vec!["1".to_owned(), "2".to_owned(), "3".to_owned()]
}

/// A wye source on `phases` with a grounded neutral (terminal 4): the line
/// to neutral magnitude of `n` phases spaced 2π/n apart from the line to
/// line `v_ll` (the dss convention), the first phase at `angle`.
fn wye_source(
    name: String,
    bus: String,
    phases: Vec<String>,
    v_ll: f64,
    angle: f64,
) -> VoltageSource {
    let n = phases.len();
    let v_ln = if n >= 2 {
        v_ll / (2.0 * (PI / n as f64).sin())
    } else {
        v_ll
    };
    let map = with_neutral(phases, Configuration::Wye);
    let v_magnitude = map
        .iter()
        .map(|t| if t == "4" { 0.0 } else { v_ln })
        .collect();
    let v_angle = map
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if t == "4" {
                0.0
            } else {
                wrap_angle(angle - (i as f64) * 2.0 * PI / n as f64)
            }
        })
        .collect();
    VoltageSource::new(name, bus, map, v_magnitude, v_angle)
}

/// The shape each diagram object draws, by the mRID of the element it
/// draws: its points in order.
type Shapes = HashMap<String, Vec<(f64, f64)>>;

/// What the Diagram Layout places: the network's coordinate metadata and a
/// location for each connectivity node it reaches, by node mRID.
struct Layout {
    meta: DistGeoMeta,
    at_node: HashMap<String, DistLocation>,
}

/// The `EPSG:<code>` token in a text (`EPSG:4326`, or the `EPSG::4326` of a
/// URN), as `EPSG:<code>`.
fn epsg_code(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    let at = upper.find("EPSG:")?;
    let digits: String = upper[at + 5..]
        .trim_start_matches(':')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (!digits.is_empty()).then(|| format!("EPSG:{digits}"))
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

fn read_documents(
    docs: Vec<(String, CimDocument)>,
    name_hint: Option<&str>,
    diags: &mut crate::collect::Diagnostics,
) -> Result<MulticonductorNetwork> {
    let mut store = Store::new();
    let mut namespaces = BTreeSet::new();
    let mut description = None;
    let mut sedms = false;
    for (name, doc) in docs {
        namespaces.extend(doc.cim_namespaces.iter().cloned());
        sedms |= sedms::uses_extension(&doc);
        if description.is_none() {
            description = doc
                .header
                .as_ref()
                .and_then(|h| h.description.clone())
                .filter(|d| !d.trim().is_empty());
        }
        store.merge(&name, doc, diags)?;
    }
    if namespaces.is_empty() {
        return Err(format_error(
            "no CIM namespace is declared; not a CIM document".into(),
        ));
    }
    if namespaces.iter().any(|n| n.contains("CIM-schema-cim16"))
        && namespaces.iter().any(|n| n.contains("CIM100"))
    {
        return Err(format_error(format!(
            "the documents mix CIM16 and CIM100 namespaces: {}",
            namespaces.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    let cim16 = namespaces.iter().all(|n| n.contains("CIM-schema-cim16"));
    store.seal();
    if store.count_of("ConnectivityNode") == 0 {
        let message = if store.count_of("TopologicalNode") > 0 {
            "the documents connect equipment through TopologicalNode only; the reader needs \
             ConnectivityNode connectivity"
        } else {
            "no ConnectivityNode records; not a node based CIM feeder"
        };
        return Err(format_error(message.into()));
    }

    let mut mapper = Mapper::new(&store, diags);
    mapper.sedms = sedms;
    mapper.validate_electrical_state()?;
    let (line_codes, mut lines) = mapper.lines();
    let mut loads = mapper.loads();
    let shunts = mapper.shunts()?;
    let switches = mapper.switches();
    let transformers = mapper.transformers()?;
    let mut sources = mapper.sources();
    let extension = if sedms {
        mapper.sedms_extension()
    } else {
        sedms::Extension::default()
    };
    loads.extend(extension.loads);
    let (ibrs, supply) = (extension.ibrs, extension.supply);
    // The supply switches are the generic `Switch` class, the first class the
    // switch mapping reads, so they lead and a CIM rewrite reads back in the
    // same order.
    let switches: Vec<DistSwitch> = supply.switches.into_iter().chain(switches).collect();
    sources.extend(supply.sources);
    let layout = mapper.layout(&mut lines);

    let used = terminal_usage(
        &lines,
        &switches,
        &loads,
        &ibrs,
        &shunts,
        &transformers,
        &sources,
    );
    let mut buses = derive_buses(&mapper.nodes, &supply.buses, &used);
    let geo = layout.map(|layout| {
        locate_buses(&mut buses, &layout);
        sedms::locate_busbars(&mut buses, &switches, &supply.buses);
        layout.meta
    });
    let base_frequency = mapper.base_frequency(cim16);
    let name = description
        .or_else(|| mapper.feeder_name())
        .or_else(|| name_hint.map(str::to_owned));
    mapper.report_unread();

    let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
        name,
        source_format: Some(DistSourceFormat::CimDist),
        base_frequency,
        geo,
        buses,
        line_codes,
        lines,
        switches,
        transformers,
        loads,
        ibrs,
        shunts,
        sources,
        ..MulticonductorNetworkTables::default()
    });
    crate::model::warn_unresolved_references(&net, diags);
    Ok(net)
}

/// Terminal wiring: equipment to its terminals (in sequence order), and
/// terminal to connectivity node.
struct Wiring {
    of_equipment: HashMap<String, Vec<String>>,
    node_of: HashMap<String, String>,
}

/// The lookups the tank form reads through, built once per read.
struct TankIndex {
    /// Tank mRID to its `TransformerTankEnd`s.
    ends_of: HashMap<String, Vec<String>>,
    /// Power system resource mRID to the `(Asset, TransformerTankInfo)`
    /// pairs whose `Asset.PowerSystemResources` lists it.
    info_of: HashMap<String, Vec<(String, String)>>,
    end_infos_of: HashMap<String, Vec<String>>,
    /// End info mRID to the tests that energise it.
    short_circuit_of: HashMap<String, Vec<String>>,
    no_load_of: HashMap<String, Vec<String>>,
    tap_changers: HashMap<String, Vec<String>>,
}

/// One `TransformerTankInfo` with its end infos and tests resolved.
struct Catalog {
    /// In end number order.
    ends: Vec<CatalogEnd>,
    /// Pair reactances, percent on the first end's rating, keyed by end
    /// numbers.
    pairs: HashMap<(i64, i64), f64>,
    /// `(%noloadloss, %imag)` on the first end's rating.
    no_load: Option<(f64, f64)>,
}

struct CatalogEnd {
    id: String,
    number: f64,
    delta: bool,
    clock: Option<f64>,
    v_ref: f64,
    s_rating: f64,
    /// `TransformerEndInfo.r` as percent of the end's base, when stated and
    /// nonzero.
    stated_r: Option<f64>,
    /// The winding resistance read, percent of the end's base.
    r_pct: f64,
}

/// An end number as an integer key.
#[allow(clippy::cast_possible_truncation)] // end numbers are small integers
fn end_key(number: f64) -> i64 {
    number as i64
}

/// A winding pair key from two end numbers, lower first.
fn pair_key(a: f64, b: f64) -> (i64, i64) {
    let (a, b) = (end_key(a), end_key(b));
    (a.min(b), a.max(b))
}

/// A tank end's terminal map and connection from its
/// `TransformerTankEnd.phases`, in the conventions the dss reader produces:
/// a line to neutral end `A` is `1, 4` (`bus.1.0`), a line to line end `AB`
/// is a delta `1, 2`, a three phase end is `1, 2, 3` in delta or `1, 2, 3, 4`
/// in wye, and the center tap halves `s1` and `s2` are `1, 4` and `4, 2`
/// (`bus.1.0` and `bus.0.2`), so they stack to the full secondary voltage.
fn tank_terminal_map(code: &str, delta: bool) -> Option<(Vec<String>, DistWindingConn)> {
    let names = |terminals: &[&str]| terminals.iter().map(|t| (*t).to_owned()).collect();
    Some(match code {
        "s1" | "s1N" => (names(&["1", "4"]), DistWindingConn::Wye),
        "s2" | "s2N" => (names(&["4", "2"]), DistWindingConn::Wye),
        "A" | "AN" | "B" | "BN" | "C" | "CN" => (
            names(&[phase_terminal(&code[..1])?, "4"]),
            DistWindingConn::Wye,
        ),
        "AB" | "BC" | "AC" => (expand_phase_code(code)?, DistWindingConn::Delta),
        "ABC" | "ABCN" if delta => (names(&["1", "2", "3"]), DistWindingConn::Delta),
        "ABC" | "ABCN" => (names(&["1", "2", "3", "4"]), DistWindingConn::Wye),
        _ => return None,
    })
}

struct Mapper<'a> {
    store: &'a Store,
    diags: &'a mut crate::collect::Diagnostics,
    wiring: Wiring,
    /// Connectivity node mRID to bus id, and the nodes in document order.
    bus_of_node: HashMap<String, String>,
    nodes: Vec<(String, String)>,
    /// Lower case names already given, per model table.
    taken: HashMap<&'static str, HashSet<String>>,
    /// Objects whose record was dropped; their properties are not reported
    /// again as unmapped.
    dropped: HashSet<String>,
    /// Objects of an otherwise unmapped class the mapping read (the feeder
    /// that names the network, the catalog entries tanks and segment ratings
    /// come from, the diagram the layout reads, the usage points and service
    /// locations loads come from).
    consumed: HashSet<String>,
    /// Transformer catalog entries read so far, by `TransformerTankInfo`
    /// mRID; `None` for one without end infos.
    catalogs: HashMap<String, Option<Rc<Catalog>>>,
    /// Core admittances a transformer read.
    core_read: HashSet<String>,
    /// Whether the documents use the Schneider Electric ADMS extension
    /// ([`sedms`]).
    sedms: bool,
    /// Service locations already resolved, so the usage points and
    /// resources that share one report its findings once.
    service_points: HashMap<String, Option<sedms::ServicePoint>>,
}

impl<'a> Mapper<'a> {
    fn new(store: &'a Store, diags: &'a mut crate::collect::Diagnostics) -> Self {
        let mut mapper = Mapper {
            store,
            diags,
            wiring: Wiring {
                of_equipment: HashMap::new(),
                node_of: HashMap::new(),
            },
            bus_of_node: HashMap::new(),
            nodes: Vec::new(),
            taken: HashMap::new(),
            dropped: HashSet::new(),
            consumed: HashSet::new(),
            catalogs: HashMap::new(),
            core_read: HashSet::new(),
            sedms: false,
            service_points: HashMap::new(),
        };
        mapper.build_wiring();
        mapper.name_nodes();
        mapper
    }

    // -- findings ----------------------------------------------------------

    fn note(&mut self, info: &'static DiagnosticInfo, class: &str, id: &str, message: String) {
        let mut diagnostic = Diagnostic::of(info, message);
        let _ = diagnostic.insert_detail("class", serde_json::Value::from(class));
        let _ = diagnostic.insert_detail("mrid", serde_json::Value::from(id));
        self.diags.record(diagnostic);
    }

    /// Drop a record with a finding naming why.
    fn drop_record(&mut self, class: &str, id: &str, why: &str) {
        let label = self.label(id);
        self.dropped.insert(id.to_owned());
        self.note(
            &C::READ_CIM_DIST_RECORD_DROPPED,
            class,
            id,
            format!("{class} {label}: {why}; dropped"),
        );
    }

    /// `name (mrid)` when the object states a name, else its mRID.
    fn label(&self, id: &str) -> String {
        match self.store.name_of(id) {
            Some(name) if name != id => format!("{name} ({id})"),
            _ => id.to_owned(),
        }
    }

    /// The target of reference `key` on `id` when the document set defines
    /// it; a reference to an undefined object is reported and reads as
    /// absent.
    fn target(&mut self, class: &str, id: &str, key: &str) -> Option<String> {
        let target = self.store.refv(id, key)?.to_owned();
        if self.store.contains(&target) {
            return Some(target);
        }
        let label = self.label(id);
        self.note(
            &C::READ_CIM_DIST_REFERENCE_DROPPED,
            class,
            id,
            format!("{class} {label}: {key} names `{target}`, which no document defines"),
        );
        None
    }

    /// A unique model name for an object in table `table`: its CIM name, or
    /// its mRID when the name is empty or already taken.
    fn unique_name(&mut self, table: &'static str, class: &str, id: &str) -> String {
        let stated = self.store.name_of(id);
        let taken = self.taken.entry(table).or_default();
        if let Some(name) = &stated
            && taken.insert(name.to_ascii_lowercase())
        {
            return name.clone();
        }
        let mut candidate = id.to_owned();
        let mut n = 1;
        while !taken.insert(candidate.to_ascii_lowercase()) {
            n += 1;
            candidate = format!("{id}_{n}");
        }
        let why = match &stated {
            Some(name) => format!("its name `{name}` repeats another {table} name"),
            None => "it states no name".to_owned(),
        };
        self.note(
            &C::READ_CIM_DIST_IDENTIFIER_SUBSTITUTED,
            class,
            id,
            format!("{class} {id}: {why}; the model names it `{candidate}`"),
        );
        candidate
    }

    // -- wiring and buses --------------------------------------------------

    fn build_wiring(&mut self) {
        let mut of_equipment: HashMap<String, Vec<(f64, String)>> = HashMap::new();
        for id in self.store.ids_of("Terminal") {
            let seq = self
                .store
                .f(&id, "ACDCTerminal.sequenceNumber")
                .or_else(|| self.store.f(&id, "Terminal.sequenceNumber"))
                .unwrap_or(1.0);
            if let Some(eq) = self.target("Terminal", &id, "Terminal.ConductingEquipment") {
                of_equipment.entry(eq).or_default().push((seq, id.clone()));
            }
            if let Some(cn) = self.target("Terminal", &id, "Terminal.ConnectivityNode") {
                self.wiring.node_of.insert(id, cn);
            }
        }
        self.wiring.of_equipment = of_equipment
            .into_iter()
            .map(|(eq, mut terms)| {
                terms.sort_by(|a, b| a.0.total_cmp(&b.0));
                (eq, terms.into_iter().map(|(_, t)| t).collect())
            })
            .collect();
    }

    fn name_nodes(&mut self) {
        for cn in self.store.ids_of("ConnectivityNode") {
            let bus = self.unique_name("bus", "ConnectivityNode", &cn);
            self.bus_of_node.insert(cn.clone(), bus.clone());
            self.nodes.push((bus, cn));
        }
    }

    fn terminals(&self, equipment: &str) -> Vec<String> {
        self.wiring
            .of_equipment
            .get(equipment)
            .cloned()
            .unwrap_or_default()
    }

    /// The bus at the equipment's terminal of the given sequence index
    /// (0 based).
    fn bus_at(&self, equipment: &str, index: usize) -> Option<String> {
        let terminal = self.wiring.of_equipment.get(equipment)?.get(index)?;
        self.bus_of_terminal(terminal)
    }

    fn bus_of_terminal(&self, terminal: &str) -> Option<String> {
        let node = self.wiring.node_of.get(terminal)?;
        self.bus_of_node.get(node).cloned()
    }

    /// The terminal names a terminal's `Terminal.phases` states.
    fn terminal_phases(&mut self, terminal: &str) -> Option<Vec<String>> {
        let code = self
            .store
            .enum_value(terminal, "Terminal.phases")?
            .to_owned();
        let expanded = expand_phase_code(&code).filter(|e| !e.is_empty());
        if expanded.is_none() {
            let label = self.label(terminal);
            self.note(
                &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                "Terminal",
                terminal,
                format!("Terminal {label}: phase code `{code}` is not read; phases ABC are used"),
            );
        }
        expanded
    }

    /// The terminal map of an equipment without per phase children: its
    /// first terminal's `Terminal.phases`, or ABC with a finding. The other
    /// terminals' codes are read too, so they are not reported as unmapped.
    fn fallback_map(&mut self, class: &str, id: &str) -> Vec<String> {
        let terminals = self.terminals(id);
        let mut first = None;
        for (i, terminal) in terminals.iter().enumerate() {
            let phases = self.terminal_phases(terminal);
            if i == 0 {
                first = phases;
            }
        }
        first.unwrap_or_else(|| {
            let label = self.label(id);
            self.note(
                &C::READ_CIM_DIST_VALUE_DEFAULTED,
                class,
                id,
                format!("{class} {label}: no phases are stated; three phases ABC are assumed"),
            );
            three_phases()
        })
    }

    /// Per phase child objects of `parent` as `(terminal, child id)` pairs,
    /// ordered by `sequence_key` when every child states it, else by
    /// terminal name. `phase_key` names the child's `SinglePhaseKind`.
    fn phase_children(
        &mut self,
        children: &[String],
        class_of_child: &str,
        phase_key: &str,
        sequence_key: Option<&str>,
    ) -> Vec<(String, String)> {
        let mut out: Vec<(Option<f64>, String, String)> = Vec::new();
        for child in children {
            let Some(phase) = self.store.enum_value(child, phase_key).map(str::to_owned) else {
                self.drop_record(class_of_child, child, &format!("{phase_key} is not stated"));
                continue;
            };
            let Some(terminal) = phase_terminal(&phase) else {
                self.drop_record(
                    class_of_child,
                    child,
                    &format!("phase `{phase}` has no conductor in the model"),
                );
                continue;
            };
            if out.iter().any(|(_, t, _)| t == terminal) {
                self.drop_record(
                    class_of_child,
                    child,
                    &format!("phase `{phase}` repeats another child of the same equipment"),
                );
                continue;
            }
            let sequence = sequence_key.and_then(|key| self.store.f(child, key));
            out.push((sequence, terminal.to_owned(), child.clone()));
        }
        if out.iter().all(|(s, _, _)| s.is_some()) {
            out.sort_by(|a, b| a.0.unwrap_or(0.0).total_cmp(&b.0.unwrap_or(0.0)));
        } else {
            out.sort_by(|a, b| a.1.cmp(&b.1));
        }
        out.into_iter().map(|(_, t, c)| (t, c)).collect()
    }

    /// The ids of `class` objects grouped by the parent their reference `key`
    /// names. A child whose parent no document defines is reported and
    /// dropped.
    fn children_of(&mut self, class: &str, key: &str) -> HashMap<String, Vec<String>> {
        let mut out = self.store.index_by(class, key);
        let orphans: Vec<String> = out
            .keys()
            .filter(|parent| !self.store.contains(parent))
            .cloned()
            .collect();
        for parent in orphans {
            for child in out.remove(&parent).unwrap_or_default() {
                let label = self.label(&child);
                self.dropped.insert(child.clone());
                self.note(
                    &C::READ_CIM_DIST_REFERENCE_DROPPED,
                    class,
                    &child,
                    format!("{class} {label}: {key} names `{parent}`, which no document defines"),
                );
            }
        }
        out
    }

    /// The stated base frequency. Without one, a CIM16 document set takes
    /// 50 Hz: CIM16 is the CGMES 2.4.15 release, and the CGMES reader
    /// assumes the same ENTSO-E default. CIM100, the GridAPPS-D and CIMHub release, takes 60 Hz.
    fn base_frequency(&mut self, cim16: bool) -> f64 {
        let frequencies = self.store.ids_of("BaseFrequency");
        let stated = frequencies
            .iter()
            .find_map(|f| self.store.f(f, "BaseFrequency.frequency"))
            .filter(|f| *f > 0.0);
        stated.unwrap_or_else(|| {
            let (frequency, message) = if cim16 {
                (
                    50.0,
                    "no BaseFrequency states a frequency; 50 Hz is assumed for the CIM16 \
                     namespace, the ENTSO-E default",
                )
            } else {
                (
                    60.0,
                    "no BaseFrequency states a frequency; 60 Hz is assumed",
                )
            };
            self.diags
                .record(Diagnostic::of(&C::READ_CIM_DIST_VALUE_DEFAULTED, message));
            frequency
        })
    }

    /// The first feeder's name, which names the network when no header
    /// description does.
    fn feeder_name(&mut self) -> Option<String> {
        let feeder = self.store.ids_of("Feeder").into_iter().next()?;
        let name = self.store.name_of(&feeder)?;
        self.consumed.insert(feeder);
        Some(name)
    }

    // -- lines -------------------------------------------------------------

    fn lines(&mut self) -> (Vec<DistLineCode>, Vec<DistLine>) {
        let phase_children =
            self.children_of("ACLineSegmentPhase", "ACLineSegmentPhase.ACLineSegment");
        let impedance_data =
            self.children_of("PhaseImpedanceData", "PhaseImpedanceData.PhaseImpedance");
        let mut line_codes: Vec<DistLineCode> = Vec::new();
        // (impedance object, its matrix rows in segment phase order) to line
        // code name.
        let mut seen: HashMap<(String, Vec<usize>), String> = HashMap::new();
        self.phase_impedance_codes(&impedance_data, &mut line_codes, &mut seen);
        let mut lines = Vec::new();
        for id in self.store.ids_of("ACLineSegment") {
            let (Some(from), Some(to)) = (self.bus_at(&id, 0), self.bus_at(&id, 1)) else {
                self.drop_record(
                    "ACLineSegment",
                    &id,
                    "it needs two terminals on connectivity nodes",
                );
                continue;
            };
            let children = phase_children.get(&id).cloned().unwrap_or_default();
            let (map, rows): (Vec<String>, Vec<usize>) = if children.is_empty() {
                let map = self.fallback_map("ACLineSegment", &id);
                let rows = (1..=map.len()).collect();
                (map, rows)
            } else {
                for terminal in self.terminals(&id) {
                    self.store.consume(&terminal, "Terminal.phases");
                }
                let phases = self.phase_children(
                    &children,
                    "ACLineSegmentPhase",
                    "ACLineSegmentPhase.phase",
                    Some("ACLineSegmentPhase.sequenceNumber"),
                );
                let rows = self.impedance_rows(&id, &phases);
                (phases.into_iter().map(|(t, _)| t).collect(), rows)
            };
            if map.is_empty() {
                self.drop_record("ACLineSegment", &id, "no conductor phase is readable");
                continue;
            }
            let stated = self.store.f(&id, "Conductor.length").filter(|l| *l > 0.0);
            let length_stated = stated.is_some();
            let length = if let Some(length) = stated {
                length
            } else {
                let label = self.label(&id);
                self.note(
                    &C::READ_CIM_DIST_VALUE_DEFAULTED,
                    "ACLineSegment",
                    &id,
                    format!("ACLineSegment {label}: no positive Conductor.length; 1 m is used"),
                );
                1.0
            };

            let name = self.unique_name("line", "ACLineSegment", &id);
            let (code, resolved) = self.line_code_for(
                &id,
                &name,
                &rows,
                length,
                length_stated,
                &impedance_data,
                &mut line_codes,
                &mut seen,
            );
            let conductors = map.len();
            let mut line = DistLine::new(name, from, to, map.clone(), map, code, length);
            if let Some(rated) = self.wire_rating(&id) {
                line.i_max = Some(vec![rated; conductors]);
            }
            line.extras.insert("cim_mrid".into(), id.clone().into());
            if !resolved {
                // Conversion refuses the segment: its zero placeholder
                // impedance would be a short circuit.
                line.extras.insert(
                    UNRESOLVED_IMPEDANCE_KEY.into(),
                    serde_json::Value::Bool(true),
                );
            }
            lines.push(line);
        }
        (line_codes, lines)
    }

    /// A per-length impedance needs the segment's own length: the 1 m
    /// placeholder would turn it into a near short circuit, so the segment
    /// stays unresolved and conversion refuses it.
    fn per_length_is_scaled(&mut self, line: &str, pli: &str, length_stated: bool) -> bool {
        if !length_stated {
            let label = self.label(line);
            let pli = self.label(pli);
            self.note(
                &C::READ_CIM_DIST_IMPEDANCE_UNRESOLVED,
                "ACLineSegment",
                line,
                format!(
                    "ACLineSegment {label}: its PerLengthImpedance {pli} needs a positive \
                     Conductor.length; the segment has no series impedance"
                ),
            );
        }
        length_stated
    }

    /// Every phase impedance object is a line code at its own conductor
    /// count, whether or not a segment uses it; a segment naming another set
    /// of rows takes its own projection in [`Self::line_code_for`].
    fn phase_impedance_codes(
        &mut self,
        impedance_data: &HashMap<String, Vec<String>>,
        line_codes: &mut Vec<DistLineCode>,
        seen: &mut HashMap<(String, Vec<usize>), String>,
    ) {
        for pli in self.store.ids_of("PerLengthPhaseImpedance") {
            if impedance_data.get(&pli).is_none_or(Vec::is_empty) {
                self.drop_record(
                    "PerLengthPhaseImpedance",
                    &pli,
                    "it states no PhaseImpedanceData",
                );
                continue;
            }
            let count = self.conductor_count(&pli, impedance_data);
            if count < 1.0 {
                self.drop_record(
                    "PerLengthPhaseImpedance",
                    &pli,
                    "it states neither a conductor count nor a PhaseImpedanceData row",
                );
                continue;
            }
            if count > MAX_CONDUCTORS as f64 {
                self.drop_record(
                    "PerLengthPhaseImpedance",
                    &pli,
                    &format!("{count} conductors exceed the supported maximum of {MAX_CONDUCTORS}"),
                );
                continue;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 1..=64
            let rows: Vec<usize> = (1..=count as usize).collect();
            if let Some((r, x, g, b)) = self.per_length_matrices(&pli, &rows, impedance_data) {
                let base = self.store.name_of(&pli).unwrap_or_else(|| pli.clone());
                let name = self.unique_line_code_name(&base);
                let mut code = DistLineCode::new(name.clone(), r, x);
                set_shunt_halves(&mut code, &g, &b);
                code.extras.insert("cim_mrid".into(), pli.clone().into());
                line_codes.push(code);
                seen.insert((pli, rows), name);
            } else {
                self.drop_record(
                    "PerLengthPhaseImpedance",
                    &pli,
                    "its PhaseImpedanceData states no self impedance",
                );
            }
        }
    }

    /// The line code for a segment: shared per impedance object and matrix
    /// rows, or the segment's own from its series totals. Only the first code
    /// an impedance object yields carries its mRID, so a projection onto
    /// other rows writes as an object of its own. The flag is false when the
    /// segment states no impedance and the code is a zero placeholder.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn line_code_for(
        &mut self,
        line: &str,
        line_name: &str,
        rows: &[usize],
        length: f64,
        length_stated: bool,
        impedance_data: &HashMap<String, Vec<String>>,
        line_codes: &mut Vec<DistLineCode>,
        seen: &mut HashMap<(String, Vec<usize>), String>,
    ) -> (String, bool) {
        let n = rows.len();
        let mut unusable = None;
        if let Some(pli) = self.target("ACLineSegment", line, "ACLineSegment.PerLengthImpedance") {
            // Sequence values expand the same way for any phases, so only a
            // phase matrix's code depends on which rows the segment takes.
            let key_rows = if self.store.class_of(&pli) == Some("PerLengthPhaseImpedance") {
                rows.to_vec()
            } else {
                (1..=n).collect()
            };
            let key = (pli.clone(), key_rows);
            if let Some(name) = seen.get(&key) {
                let name = name.clone();
                return (name, self.per_length_is_scaled(line, &pli, length_stated));
            }
            let matrices = if self.dropped.contains(&pli) {
                None
            } else {
                self.per_length_matrices(&pli, rows, impedance_data)
            };
            if let Some((r, x, g, b)) = matrices {
                let first = !seen.keys().any(|(p, _)| *p == pli);
                let base = self.store.name_of(&pli).unwrap_or_else(|| pli.clone());
                let name = if first { base } else { format!("{base}_{n}") };
                let name = self.unique_line_code_name(&name);
                let mut code = DistLineCode::new(name.clone(), r, x);
                set_shunt_halves(&mut code, &g, &b);
                if first {
                    code.extras.insert("cim_mrid".into(), pli.clone().into());
                }
                line_codes.push(code);
                seen.insert(key, name.clone());
                return (name, self.per_length_is_scaled(line, &pli, length_stated));
            }
            unusable = Some(pli);
        }
        // Series totals on the segment, per length through the length.
        let total = |key: &str| self.store.f(line, key);
        let (r1, x1) = (total("ACLineSegment.r"), total("ACLineSegment.x"));
        let name = self.unique_line_code_name(&format!("{line_name}_lc"));
        let resolved = r1.is_some() || x1.is_some();
        if !resolved {
            let label = self.label(line);
            let why = match &unusable {
                Some(pli) => format!(
                    "its PerLengthImpedance {} gives no usable impedance and it states no series \
                     r and x",
                    self.label(pli)
                ),
                None => "states neither a PerLengthImpedance nor series r and x (wire geometry \
                         is not calculated)"
                    .to_owned(),
            };
            self.note(
                &C::READ_CIM_DIST_IMPEDANCE_UNRESOLVED,
                "ACLineSegment",
                line,
                format!("ACLineSegment {label}: {why}; the segment has no series impedance"),
            );
        }
        let r1 = r1.unwrap_or(0.0);
        let x1 = x1.unwrap_or(0.0);
        let b1 = total("ACLineSegment.bch").unwrap_or(0.0);
        let g1 = total("ACLineSegment.gch").unwrap_or(0.0);
        let r0 = total("ACLineSegment.r0").unwrap_or(r1);
        let x0 = total("ACLineSegment.x0").unwrap_or(x1);
        let b0 = total("ACLineSegment.b0ch").unwrap_or(b1);
        let g0 = total("ACLineSegment.g0ch").unwrap_or(g1);
        let per = |v: f64| v / length;
        let mut code = DistLineCode::new(
            name.clone(),
            sequence_to_phase(per(r0), per(r1), n),
            sequence_to_phase(per(x0), per(x1), n),
        );
        set_shunt_halves(
            &mut code,
            &sequence_to_phase(per(g0), per(g1), n),
            &sequence_to_phase(per(b0), per(b1), n),
        );
        line_codes.push(code);
        (name, resolved)
    }

    /// The ampacity of a segment's conductors, amps: the `ratedCurrent` of
    /// the wire or cable catalog entry its `PowerSystemResource.AssetDatasheet`
    /// names.
    fn wire_rating(&mut self, line: &str) -> Option<f64> {
        let info = self.target("ACLineSegment", line, "PowerSystemResource.AssetDatasheet")?;
        let class = self.store.class_of(&info)?.to_owned();
        if !WIRE_INFO_CLASSES.contains(&class.as_str()) {
            let label = self.label(line);
            let datasheet = self.label(&info);
            self.note(
                &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                "ACLineSegment",
                line,
                format!(
                    "ACLineSegment {label}: its AssetDatasheet {datasheet} is a {class}, not a \
                     wire or cable catalog entry; no ampacity is read"
                ),
            );
            return None;
        }
        self.consumed.insert(info.clone());
        self.store
            .f(&info, "WireInfo.ratedCurrent")
            .filter(|rated| *rated > 0.0)
    }

    /// A line code name no earlier line code holds.
    fn unique_line_code_name(&mut self, base: &str) -> String {
        let taken = self.taken.entry("linecode").or_default();
        let mut candidate = base.to_owned();
        let mut n = 1;
        while !taken.insert(candidate.to_ascii_lowercase()) {
            n += 1;
            candidate = format!("{base}_{n}");
        }
        candidate
    }

    /// A phase impedance object's conductor count: `conductorCount`, else the
    /// largest row or column its data states. The caller bounds it before
    /// sizing matrices.
    fn conductor_count(&self, pli: &str, impedance_data: &HashMap<String, Vec<String>>) -> f64 {
        if let Some(count) = self
            .store
            .f(pli, "PerLengthPhaseImpedance.conductorCount")
            .filter(|c| *c >= 1.0)
        {
            return count;
        }
        impedance_data
            .get(pli)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .flat_map(|d| {
                [
                    self.store.f(d, "PhaseImpedanceData.row"),
                    self.store.f(d, "PhaseImpedanceData.column"),
                ]
            })
            .flatten()
            .fold(0.0_f64, f64::max)
    }

    /// The phase impedance matrix row of each segment phase, in the
    /// segment's phase order. `ACLineSegmentPhase.sequenceNumber` numbers the
    /// rows and columns of `PhaseImpedanceData`, so a phase C conductor
    /// numbered 3 takes row 3 even on a two phase segment. Phases that state
    /// no numbers take rows in phase order; numbers that cannot each name a
    /// distinct row do too, with a finding.
    fn impedance_rows(&mut self, line: &str, phases: &[(String, String)]) -> Vec<usize> {
        let positional: Vec<usize> = (1..=phases.len()).collect();
        let stated: Vec<Option<f64>> = phases
            .iter()
            .map(|(_, child)| self.store.f(child, "ACLineSegmentPhase.sequenceNumber"))
            .collect();
        if stated.iter().all(Option::is_none) {
            return positional;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 1..=64
        let rows: Option<Vec<usize>> = stated
            .iter()
            .map(|s| {
                s.filter(|v| v.fract() == 0.0 && (1.0..=MAX_CONDUCTORS as f64).contains(v))
                    .map(|v| v as usize)
            })
            .collect();
        if let Some(rows) = rows
            && rows.iter().collect::<HashSet<_>>().len() == rows.len()
        {
            return rows;
        }
        let label = self.label(line);
        self.note(
            &C::READ_CIM_DIST_VALUE_DEFAULTED,
            "ACLineSegment",
            line,
            format!(
                "ACLineSegment {label}: its ACLineSegmentPhase sequence numbers do not each name \
                 a distinct impedance matrix row; the rows follow phase order"
            ),
        );
        positional
    }

    /// The (r, x, g, b) matrices of a `PerLengthImpedance` for a segment
    /// whose phases take matrix `rows`: from the phase matrix data, or
    /// expanded from sequence values. ohm/m and S/m. `None` when the object
    /// gives the segment no impedance.
    #[allow(clippy::many_single_char_names)] // r, x, g, b are the matrix names
    fn per_length_matrices(
        &mut self,
        pli: &str,
        rows: &[usize],
        impedance_data: &HashMap<String, Vec<String>>,
    ) -> Option<(
        ConductorMatrix,
        ConductorMatrix,
        ConductorMatrix,
        ConductorMatrix,
    )> {
        let n = rows.len();
        let class = self.store.class_of(pli)?.to_owned();
        let label = self.label(pli);
        match class.as_str() {
            "PerLengthPhaseImpedance" => {
                self.phase_impedance_matrices(pli, &label, rows, impedance_data)
            }
            "PerLengthSequenceImpedance" => {
                let store = self.store;
                let value = |key: &str| store.f(pli, key);
                let (r1, x1) = (
                    value("PerLengthSequenceImpedance.r"),
                    value("PerLengthSequenceImpedance.x"),
                );
                if r1.is_none() && x1.is_none() {
                    self.drop_record(
                        "PerLengthSequenceImpedance",
                        pli,
                        "it states neither r nor x",
                    );
                    return None;
                }
                let r1 = r1.unwrap_or(0.0);
                let x1 = x1.unwrap_or(0.0);
                let b1 = value("PerLengthSequenceImpedance.bch").unwrap_or(0.0);
                let g1 = value("PerLengthSequenceImpedance.gch").unwrap_or(0.0);
                let r0 = value("PerLengthSequenceImpedance.r0");
                let x0 = value("PerLengthSequenceImpedance.x0");
                let b0 = value("PerLengthSequenceImpedance.b0ch").unwrap_or(b1);
                let g0 = value("PerLengthSequenceImpedance.g0ch").unwrap_or(g1);
                if r0.is_none() || x0.is_none() {
                    self.note(
                        &C::READ_CIM_DIST_VALUE_DEFAULTED,
                        "PerLengthSequenceImpedance",
                        pli,
                        format!(
                            "PerLengthSequenceImpedance {label}: no zero sequence r0 and x0; the \
                             positive sequence values are used"
                        ),
                    );
                }
                let (r0, x0) = (r0.unwrap_or(r1), x0.unwrap_or(x1));
                Some((
                    sequence_to_phase(r0, r1, n),
                    sequence_to_phase(x0, x1, n),
                    sequence_to_phase(g0, g1, n),
                    sequence_to_phase(b0, b1, n),
                ))
            }
            other => {
                self.note(
                    &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                    other,
                    pli,
                    format!(
                        "{other} {label}: is not a PerLengthPhaseImpedance or \
                         PerLengthSequenceImpedance; the segment's own series values are used"
                    ),
                );
                None
            }
        }
    }

    /// The (r, x, g, b) matrices of a `PerLengthPhaseImpedance` for a
    /// segment whose phases take matrix `rows`; `None` when its data states
    /// no self impedance for any of them.
    #[allow(clippy::many_single_char_names)] // r, x, g, b are the matrix names
    fn phase_impedance_matrices(
        &mut self,
        pli: &str,
        label: &str,
        rows: &[usize],
        impedance_data: &HashMap<String, Vec<String>>,
    ) -> Option<(
        ConductorMatrix,
        ConductorMatrix,
        ConductorMatrix,
        ConductorMatrix,
    )> {
        let n = rows.len();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let count = self
            .store
            .f(pli, "PerLengthPhaseImpedance.conductorCount")
            .map(|c| c as usize);
        let position: HashMap<usize, usize> =
            rows.iter().enumerate().map(|(k, &row)| (row, k)).collect();
        let mut r = vec![vec![0.0; n]; n];
        let mut x = vec![vec![0.0; n]; n];
        let mut g = vec![vec![0.0; n]; n];
        let mut b = vec![vec![0.0; n]; n];
        let mut outside = 0usize;
        let mut unstated = 0usize;
        let mut diagonal = vec![false; n];
        for d in impedance_data.get(pli).map_or(&[][..], Vec::as_slice) {
            let row = self.store.f(d, "PhaseImpedanceData.row");
            let col = self.store.f(d, "PhaseImpedanceData.column");
            let (Some(row), Some(col)) = (row, col) else {
                self.drop_record("PhaseImpedanceData", d, "row or column is not stated");
                continue;
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let at = |v: f64| (v >= 1.0).then(|| position.get(&(v as usize)).copied());
            let (at_row, at_col) = (at(row).flatten(), at(col).flatten());
            let (Some(i), Some(j)) = (at_row, at_col) else {
                outside += 1;
                self.store.consume(d, "PhaseImpedanceData.r");
                self.store.consume(d, "PhaseImpedanceData.x");
                self.store.consume(d, "PhaseImpedanceData.g");
                self.store.consume(d, "PhaseImpedanceData.b");
                continue;
            };
            diagonal[i] |= i == j;
            unstated += usize::from(self.store.f(d, "PhaseImpedanceData.r").is_none())
                + usize::from(self.store.f(d, "PhaseImpedanceData.x").is_none());
            for (matrix, key) in [
                (&mut r, "PhaseImpedanceData.r"),
                (&mut x, "PhaseImpedanceData.x"),
                (&mut g, "PhaseImpedanceData.g"),
                (&mut b, "PhaseImpedanceData.b"),
            ] {
                let v = self.store.f(d, key).unwrap_or(0.0);
                matrix[i][j] = v;
                matrix[j][i] = v;
            }
        }
        if !diagonal.iter().any(|d| *d) {
            return None;
        }
        if count.is_some_and(|c| c != n) || outside > 0 {
            self.note(
                &C::READ_CIM_DIST_VALUE_COLLAPSED,
                "PerLengthPhaseImpedance",
                pli,
                format!(
                    "PerLengthPhaseImpedance {label}: states {} conductors for a segment \
                     with {n} phases; {outside} data rows outside the {n}×{n} matrix were \
                     not carried",
                    count.map_or_else(|| "no count of".to_owned(), |c| c.to_string()),
                ),
            );
        }
        if unstated > 0 {
            self.note(
                &C::READ_CIM_DIST_VALUE_DEFAULTED,
                "PerLengthPhaseImpedance",
                pli,
                format!(
                    "PerLengthPhaseImpedance {label}: {unstated} r or x values of its \
                     PhaseImpedanceData are not stated; 0 is used"
                ),
            );
        }
        let missing: Vec<String> = rows
            .iter()
            .zip(&diagonal)
            .filter(|(_, stated)| !**stated)
            .map(|(row, _)| row.to_string())
            .collect();
        if !missing.is_empty() {
            self.note(
                &C::READ_CIM_DIST_VALUE_DEFAULTED,
                "PerLengthPhaseImpedance",
                pli,
                format!(
                    "PerLengthPhaseImpedance {label}: no PhaseImpedanceData on the \
                     diagonal for row {}; that conductor's self impedance is 0",
                    missing.join(", ")
                ),
            );
        }
        Some((r, x, g, b))
    }

    /// Refuse states the fixed 0.11 model cannot represent before constructing
    /// any electrically active records. Switches have an explicit open state.
    fn validate_electrical_state(&self) -> Result<()> {
        for class in [
            "ACLineSegment",
            "EnergyConsumer",
            "LinearShuntCompensator",
            "PowerTransformer",
            "TransformerTank",
            "EnergySource",
        ] {
            for id in self.store.ids_of(class) {
                self.ensure_in_service(class, &id)?;
                for terminal in self.terminals(&id) {
                    self.ensure_connected_terminal(&terminal)?;
                }
            }
        }
        // End-to-terminal associations also wire transformers directly; they
        // need not be found through Terminal.ConductingEquipment.
        for end in self
            .store
            .ids_of("PowerTransformerEnd")
            .into_iter()
            .chain(self.store.ids_of("TransformerTankEnd"))
        {
            if let Some(terminal) = self.store.refv(&end, "TransformerEnd.Terminal") {
                self.ensure_connected_terminal(terminal)?;
            }
        }
        Ok(())
    }

    fn out_of_service(&self, id: &str) -> bool {
        let actual = self.store.boolean(id, "Equipment.inService");
        let normal = self.store.boolean(id, "Equipment.normallyInService");
        actual == Some(false) || (actual.is_none() && normal == Some(false))
    }

    fn ensure_in_service(&self, class: &str, id: &str) -> Result<()> {
        if self.out_of_service(id) {
            return Err(format_error(format!(
                "{class} {}: out-of-service equipment cannot be represented; electrical conversion refused",
                self.label(id)
            )));
        }
        Ok(())
    }

    fn ensure_connected_terminal(&self, id: &str) -> Result<()> {
        if self.store.boolean(id, "ACDCTerminal.connected") == Some(false) {
            return Err(format_error(format!(
                "Terminal {}: ACDCTerminal.connected is false; electrical conversion refused",
                self.label(id)
            )));
        }
        Ok(())
    }

    // -- loads -------------------------------------------------------------

    fn loads(&mut self) -> Vec<DistLoad> {
        let phase_children =
            self.children_of("EnergyConsumerPhase", "EnergyConsumerPhase.EnergyConsumer");
        let mut loads = Vec::new();
        for id in self.store.ids_of("EnergyConsumer") {
            let Some(bus) = self.bus_at(&id, 0) else {
                self.drop_record("EnergyConsumer", &id, "no terminal on a connectivity node");
                continue;
            };
            let configuration =
                self.shunt_configuration("EnergyConsumer", &id, "EnergyConsumer.phaseConnection");
            let children = phase_children.get(&id).cloned().unwrap_or_default();
            // Powers are per phase conductor: the neutral carries none.
            let (phases, p_nom, q_nom) = if children.is_empty() {
                let phases: Vec<String> = self
                    .fallback_map("EnergyConsumer", &id)
                    .into_iter()
                    .filter(|t| t != "4")
                    .collect();
                let unstated = vec![None; phases.len()];
                let p_nom = self.phase_powers(&id, "p", &unstated, false);
                let q_nom = self.phase_powers(&id, "q", &unstated, false);
                (phases, p_nom, q_nom)
            } else {
                for terminal in self.terminals(&id) {
                    self.store.consume(&terminal, "Terminal.phases");
                }
                let mut phases = Vec::new();
                let mut p_stated = Vec::new();
                let mut q_stated = Vec::new();
                for (terminal, child) in self.phase_children(
                    &children,
                    "EnergyConsumerPhase",
                    "EnergyConsumerPhase.phase",
                    None,
                ) {
                    if terminal == "4" {
                        self.drop_record(
                            "EnergyConsumerPhase",
                            &child,
                            "a load phase on the neutral has no model conductor",
                        );
                        continue;
                    }
                    phases.push(terminal);
                    p_stated.push(self.store.f(&child, "EnergyConsumerPhase.p"));
                    q_stated.push(self.store.f(&child, "EnergyConsumerPhase.q"));
                }
                let p_nom = self.phase_powers(&id, "p", &p_stated, true);
                let q_nom = self.phase_powers(&id, "q", &q_stated, true);
                (phases, p_nom, q_nom)
            };
            if phases.is_empty() {
                self.drop_record("EnergyConsumer", &id, "no readable phase");
                continue;
            }
            if configuration != Configuration::Delta
                && self.store.boolean(&id, "EnergyConsumer.grounded") == Some(false)
            {
                let label = self.label(&id);
                self.note(
                    &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                    "EnergyConsumer",
                    &id,
                    format!(
                        "EnergyConsumer {label}: an ungrounded wye load is read with a grounded \
                         neutral"
                    ),
                );
            }

            // The dss reader's shapes: one phase to neutral, or one phase to
            // phase, is single phase; two or more phases to a star point is
            // wye.
            let (configuration, map, p_nom, q_nom) = match (configuration, phases.len()) {
                (Configuration::Delta, 2) => (
                    Configuration::SinglePhase,
                    phases,
                    vec![p_nom.iter().sum()],
                    vec![q_nom.iter().sum()],
                ),
                (Configuration::Delta, _) => (Configuration::Delta, phases, p_nom, q_nom),
                (_, 1) => (
                    Configuration::SinglePhase,
                    with_neutral(phases, Configuration::SinglePhase),
                    p_nom,
                    q_nom,
                ),
                (_, _) => (
                    Configuration::Wye,
                    with_neutral(phases, Configuration::Wye),
                    p_nom,
                    q_nom,
                ),
            };
            let name = self.unique_name("load", "EnergyConsumer", &id);
            let mut load = DistLoad::new(name, bus, map, configuration, p_nom, q_nom);
            load.extras.insert("cim_mrid".into(), id.clone().into());
            loads.push(load);
        }
        loads
    }

    /// A consumer's per phase powers for `quantity` (`p` or `q`). Without
    /// `EnergyConsumerPhase` records (`phase_records` false) the total is
    /// shared evenly. With them, phases that state none share the total
    /// evenly when no phase states one, and are 0 otherwise; either way with
    /// a finding. A stated total that disagrees with the phases' sum is
    /// reported, and the phase values are kept.
    fn phase_powers(
        &mut self,
        id: &str,
        quantity: &str,
        stated: &[Option<f64>],
        phase_records: bool,
    ) -> Vec<f64> {
        let total_key = format!("EnergyConsumer.{quantity}");
        let total = self.store.f(id, &total_key);
        let label = self.label(id);
        let unstated = stated.iter().filter(|v| v.is_none()).count();
        if stated.is_empty() {
            return Vec::new();
        }
        if unstated == stated.len() {
            let Some(total) = total else {
                self.note(
                    &C::READ_CIM_DIST_VALUE_DEFAULTED,
                    "EnergyConsumer",
                    id,
                    format!(
                        "EnergyConsumer {label}: neither the consumer nor its phases state \
                         {quantity}; 0 is used"
                    ),
                );
                return vec![0.0; stated.len()];
            };
            #[allow(clippy::cast_precision_loss)] // a handful of phases
            let share = total / stated.len() as f64;
            if phase_records {
                self.note(
                    &C::READ_CIM_DIST_VALUE_DEFAULTED,
                    "EnergyConsumer",
                    id,
                    format!(
                        "EnergyConsumer {label}: its phases state no {quantity}; the total \
                         {total_key} = {total} is split evenly over {} phases",
                        stated.len()
                    ),
                );
            }
            return vec![share; stated.len()];
        }
        let values: Vec<f64> = stated.iter().map(|v| v.unwrap_or(0.0)).collect();
        if unstated > 0 {
            self.note(
                &C::READ_CIM_DIST_VALUE_DEFAULTED,
                "EnergyConsumer",
                id,
                format!(
                    "EnergyConsumer {label}: {unstated} of its phases state no {quantity}; 0 is \
                     used for them"
                ),
            );
        }
        let sum: f64 = values.iter().sum();
        if let Some(total) = total
            && (sum - total).abs() > 1e-6 * sum.abs().max(total.abs()).max(1.0)
        {
            self.note(
                &C::READ_CIM_DIST_VALUE_CONFLICT,
                "EnergyConsumer",
                id,
                format!(
                    "EnergyConsumer {label}: {total_key} = {total}, but its phases sum to \
                     {sum}; the phase values are kept"
                ),
            );
        }
        values
    }

    /// The connection of a load or shunt from its `PhaseShuntConnectionKind`:
    /// delta, or a star point (wye, or `I` for an independent single phase
    /// connection).
    fn shunt_configuration(&mut self, class: &str, id: &str, key: &str) -> Configuration {
        let label = self.label(id);
        match self.store.enum_value(id, key) {
            Some("D") => Configuration::Delta,
            Some("I") => Configuration::SinglePhase,
            Some("Y" | "Yn") => Configuration::Wye,
            Some(other) => {
                let other = other.to_owned();
                self.note(
                    &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                    class,
                    id,
                    format!("{class} {label}: connection `{other}` is read as wye"),
                );
                Configuration::Wye
            }
            None => {
                self.note(
                    &C::READ_CIM_DIST_VALUE_DEFAULTED,
                    class,
                    id,
                    format!("{class} {label}: no {key}; wye is assumed"),
                );
                Configuration::Wye
            }
        }
    }

    // -- shunts ------------------------------------------------------------

    #[allow(clippy::too_many_lines)] // one compensator shape per branch
    fn shunts(&mut self) -> Result<Vec<DistShunt>> {
        let mut phase_children = self.children_of(
            "LinearShuntCompensatorPhase",
            "ShuntCompensatorPhase.ShuntCompensator",
        );
        for (parent, mut children) in self.children_of(
            "ShuntCompensatorPhase",
            "ShuntCompensatorPhase.ShuntCompensator",
        ) {
            phase_children
                .entry(parent)
                .or_default()
                .append(&mut children);
        }
        let mut shunts = Vec::new();
        for id in self.store.ids_of("LinearShuntCompensator") {
            let Some(bus) = self.bus_at(&id, 0) else {
                self.drop_record(
                    "LinearShuntCompensator",
                    &id,
                    "no terminal on a connectivity node",
                );
                continue;
            };
            let configuration = self.shunt_configuration(
                "LinearShuntCompensator",
                &id,
                "ShuntCompensator.phaseConnection",
            );
            let parent_sections = self.sections(&id, "ShuntCompensator", None);
            let children = phase_children.get(&id).cloned().unwrap_or_default();
            if configuration == Configuration::Delta && !children.is_empty() {
                return Err(format_error(format!(
                    "LinearShuntCompensator {}: per-phase delta compensators are not supported; electrical conversion refused",
                    self.label(&id)
                )));
            }
            let (map, g_diag, b_diag) = if children.is_empty() {
                let map: Vec<String> = self
                    .fallback_map("LinearShuntCompensator", &id)
                    .into_iter()
                    .filter(|t| t != "4")
                    .collect();
                let g = self
                    .store
                    .f(&id, "LinearShuntCompensator.gPerSection")
                    .unwrap_or(0.0);
                let b = self
                    .store
                    .f(&id, "LinearShuntCompensator.bPerSection")
                    .unwrap_or(0.0);
                let sections = parent_sections
                    .unwrap_or_else(|| self.default_sections("LinearShuntCompensator", &id));
                if configuration == Configuration::Delta && map.len() == 3 {
                    // A balanced delta bank: bPerSection is the positive
                    // sequence (star equivalent) susceptance, three times
                    // each branch's, so each branch carries a third.

                    let name = self.unique_name("shunt", "LinearShuntCompensator", &id);
                    let mut shunt = DistShunt::new(
                        name,
                        bus,
                        map,
                        delta_matrix(g * sections / 3.0),
                        delta_matrix(b * sections / 3.0),
                    );
                    shunt.extras.insert("cim_mrid".into(), id.clone().into());
                    shunts.push(shunt);
                    continue;
                }
                if configuration == Configuration::Delta {
                    return Err(format_error(format!(
                        "LinearShuntCompensator {}: delta compensator on {} phases is not supported; electrical conversion refused",
                        self.label(&id),
                        map.len()
                    )));
                }
                let n = map.len();
                (map, vec![g * sections; n], vec![b * sections; n])
            } else {
                for terminal in self.terminals(&id) {
                    self.store.consume(&terminal, "Terminal.phases");
                }
                let mut map = Vec::new();
                let mut g_diag = Vec::new();
                let mut b_diag = Vec::new();
                let mut defaulted = None;
                for (terminal, child) in self.phase_children(
                    &children,
                    "ShuntCompensatorPhase",
                    "ShuntCompensatorPhase.phase",
                    None,
                ) {
                    let class = self
                        .store
                        .class_of(&child)
                        .unwrap_or("ShuntCompensatorPhase")
                        .to_owned();
                    let per_section = |key: &str| {
                        self.store
                            .f(&child, &format!("{class}.{key}"))
                            .or_else(|| {
                                self.store
                                    .f(&child, &format!("ShuntCompensatorPhase.{key}"))
                            })
                            .unwrap_or(0.0)
                    };
                    let (g, b) = (per_section("gPerSection"), per_section("bPerSection"));
                    let sections =
                        match self.sections(&child, "ShuntCompensatorPhase", parent_sections) {
                            Some(sections) => sections,
                            None => *defaulted.get_or_insert_with(|| {
                                self.default_sections("LinearShuntCompensator", &id)
                            }),
                        };
                    map.push(terminal);
                    g_diag.push(g * sections);
                    b_diag.push(b * sections);
                }
                (map, g_diag, b_diag)
            };
            if map.is_empty() {
                self.drop_record("LinearShuntCompensator", &id, "no readable phase");
                continue;
            }

            let name = self.unique_name("shunt", "LinearShuntCompensator", &id);
            let mut shunt = DistShunt::new(name, bus, map, diagonal(&g_diag), diagonal(&b_diag));
            shunt.extras.insert("cim_mrid".into(), id.clone().into());
            shunts.push(shunt);
        }
        Ok(shunts)
    }

    /// The energized section count: `sections`, else `normalSections`, else
    /// the fallback.
    fn sections(&self, id: &str, class: &str, fallback: Option<f64>) -> Option<f64> {
        self.store
            .f(id, &format!("{class}.sections"))
            .or_else(|| self.store.f(id, &format!("{class}.normalSections")))
            .or(fallback)
    }

    fn default_sections(&mut self, class: &str, id: &str) -> f64 {
        let label = self.label(id);
        self.note(
            &C::READ_CIM_DIST_VALUE_DEFAULTED,
            class,
            id,
            format!("{class} {label}: no sections or normalSections; one section is assumed"),
        );
        1.0
    }

    // -- switches ----------------------------------------------------------

    fn switches(&mut self) -> Vec<DistSwitch> {
        let phase_children = self.children_of("SwitchPhase", "SwitchPhase.Switch");
        let mut switches = Vec::new();
        for class in SWITCH_CLASSES {
            for id in self.store.ids_of(class) {
                let (Some(from), Some(to)) = (self.bus_at(&id, 0), self.bus_at(&id, 1)) else {
                    self.drop_record(class, &id, "it needs two terminals on connectivity nodes");
                    continue;
                };
                let out_of_service = self.out_of_service(&id);
                let disconnected = self.terminals(&id).iter().any(|terminal| {
                    self.store.boolean(terminal, "ACDCTerminal.connected") == Some(false)
                });
                let open_now = self.store.boolean(&id, "Switch.open");
                let normal = self.store.boolean(&id, "Switch.normalOpen");
                let stated_open = match (open_now, normal) {
                    (Some(open), Some(normal)) => {
                        if open != normal {
                            let label = self.label(&id);
                            self.note(
                                &C::READ_CIM_DIST_VALUE_COLLAPSED,
                                class,
                                &id,
                                format!(
                                    "{class} {label}: Switch.open ({open}) differs from \
                                     Switch.normalOpen ({normal}); the model keeps the operating \
                                     state"
                                ),
                            );
                        }
                        open
                    }
                    (Some(open), None) | (None, Some(open)) => open,
                    (None, None) => {
                        let label = self.label(&id);
                        self.note(
                            &C::READ_CIM_DIST_VALUE_DEFAULTED,
                            class,
                            &id,
                            format!("{class} {label}: no Switch.open or Switch.normalOpen; closed is assumed"),
                        );
                        false
                    }
                };
                // CIM has an equipment service state but the distribution model
                // has only a switch's open/closed state. Preserve the source
                // topology by representing an out-of-service switch as open.
                let open = stated_open || out_of_service || disconnected;
                let children = phase_children.get(&id).cloned().unwrap_or_default();
                let (map_from, map_to) = if children.is_empty() {
                    let map = self.fallback_map(class, &id);
                    (map.clone(), map)
                } else {
                    for terminal in self.terminals(&id) {
                        self.store.consume(&terminal, "Terminal.phases");
                    }
                    self.switch_phases(class, &id, &children, open)
                };
                if map_from.is_empty() {
                    self.drop_record(class, &id, "no readable phase");
                    continue;
                }
                if out_of_service || disconnected {
                    let reason = if out_of_service {
                        "is out of service"
                    } else {
                        "has a disconnected terminal"
                    };
                    let label = self.label(&id);
                    self.note(
                        &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                        class,
                        &id,
                        format!(
                            "{class} {label}: {reason}; represented as an open switch to preserve topology"
                        ),
                    );
                }
                let name = self.unique_name("switch", class, &id);
                let mut switch = DistSwitch::new(name, from, to, map_from.clone(), map_to, open);
                if let Some(rated) = self
                    .store
                    .f(&id, "Switch.ratedCurrent")
                    .filter(|r| *r > 0.0)
                {
                    switch.i_max = Some(vec![rated; map_from.len()]);
                }
                switch.extras.insert("cim_mrid".into(), id.clone().into());
                switch.extras.insert("cim_class".into(), class.into());
                switches.push(switch);
            }
        }
        switches
    }

    /// The side 1 and side 2 terminal maps of a switch's `SwitchPhase`
    /// children (`phaseSide1`, `phaseSide2`), with a finding when a phase's
    /// own state differs from the switch's.
    fn switch_phases(
        &mut self,
        class: &str,
        id: &str,
        children: &[String],
        open: bool,
    ) -> (Vec<String>, Vec<String>) {
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut disagree = false;
        for child in children {
            let side1 = self
                .store
                .enum_value(child, "SwitchPhase.phaseSide1")
                .map(str::to_owned);
            let side2 = self
                .store
                .enum_value(child, "SwitchPhase.phaseSide2")
                .map(str::to_owned);
            let (Some(side1), Some(side2)) =
                (side1.clone().or_else(|| side2.clone()), side2.or(side1))
            else {
                self.drop_record(
                    "SwitchPhase",
                    child,
                    "neither phaseSide1 nor phaseSide2 is stated",
                );
                continue;
            };
            let (Some(t1), Some(t2)) = (phase_terminal(&side1), phase_terminal(&side2)) else {
                self.drop_record(
                    "SwitchPhase",
                    child,
                    &format!("phases `{side1}`/`{side2}` have no conductor in the model"),
                );
                continue;
            };
            if pairs.iter().any(|(a, _)| a == t1) {
                self.drop_record(
                    "SwitchPhase",
                    child,
                    "its phase repeats another child of the same switch",
                );
                continue;
            }
            let closed = self.store.boolean(child, "SwitchPhase.closed");
            let normal_open = self.store.boolean(child, "SwitchPhase.normalOpen");
            let phase_open = closed.map(|c| !c).or(normal_open);
            if phase_open.is_some_and(|p| p != open) {
                disagree = true;
            }
            pairs.push((t1.to_owned(), t2.to_owned()));
        }
        if disagree {
            let label = self.label(id);
            self.note(
                &C::READ_CIM_DIST_VALUE_COLLAPSED,
                class,
                id,
                format!(
                    "{class} {label}: a SwitchPhase state differs from the switch's; the model \
                     carries one state per switch"
                ),
            );
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs.into_iter().unzip()
    }

    // -- transformers ------------------------------------------------------

    // One pass per transformer form, then the records that hang off neither.
    #[allow(clippy::too_many_lines)]
    fn transformers(&mut self) -> Result<Vec<DistTransformer>> {
        let ends_of = self.children_of(
            "PowerTransformerEnd",
            "PowerTransformerEnd.PowerTransformer",
        );
        let tanks_of = self.children_of("TransformerTank", "TransformerTank.PowerTransformer");
        let meshes_of = self.children_of(
            "TransformerMeshImpedance",
            "TransformerMeshImpedance.FromTransformerEnd",
        );
        let cores_of = self.children_of(
            "TransformerCoreAdmittance",
            "TransformerCoreAdmittance.TransformerEnd",
        );
        let taps_of = self.children_of("RatioTapChanger", "RatioTapChanger.TransformerEnd");
        let index = self.tank_index(taps_of);
        let mut handled: HashSet<String> = HashSet::new();
        let mut transformers = Vec::new();
        for id in self.store.ids_of("PowerTransformer") {
            let tanks = tanks_of.get(&id).cloned().unwrap_or_default();
            handled.extend(tanks.iter().cloned());
            let Some(ends) = ends_of.get(&id).cloned().filter(|e| !e.is_empty()) else {
                if tanks.is_empty() {
                    self.drop_record(
                        "PowerTransformer",
                        &id,
                        "it has no PowerTransformerEnd or TransformerTank records",
                    );
                    continue;
                }
                for tank in &tanks {
                    if let Some(transformer) = self.tank(&id, tank, &index)? {
                        transformers.push(transformer);
                    }
                }
                continue;
            };
            for tank in &tanks {
                let label = self.label(tank);
                let parent = self.label(&id);
                self.note(
                    &C::READ_CIM_DIST_VALUE_CONFLICT,
                    "TransformerTank",
                    tank,
                    format!(
                        "TransformerTank {label}: its PowerTransformer {parent} also has \
                         PowerTransformerEnd records; the end form is read and the tank is not"
                    ),
                );
                self.drop_tank_ends(tank, &index);
            }
            let mut ends: Vec<(f64, String)> = ends
                .into_iter()
                .map(|e| {
                    (
                        self.store
                            .f(&e, "TransformerEnd.endNumber")
                            .unwrap_or(f64::MAX),
                        e,
                    )
                })
                .collect();
            ends.sort_by(|a, b| a.0.total_cmp(&b.0));
            let ends: Vec<String> = ends.into_iter().map(|(_, e)| e).collect();
            if ends.len() < 2 {
                self.drop_record("PowerTransformer", &id, "it has fewer than two ends");
                continue;
            }
            let phases = self.transformer_phase_count(&ends);
            let Some(mut windings) = self.build_windings(&ends, phases) else {
                self.drop_record(
                    "PowerTransformer",
                    &id,
                    "an end has no terminal on a connectivity node",
                );
                continue;
            };
            for (winding, end) in windings.iter_mut().zip(&ends) {
                if let Some(tap) = self.end_tap(end, &index.tap_changers) {
                    winding.tap = tap;
                }
            }
            // The same clocks the tank form refuses; an absent clock reads as
            // it always has.
            let mut clocks = Vec::with_capacity(ends.len());
            for end in &ends {
                let clock = self.store.f(end, "PowerTransformerEnd.phaseAngleClock");
                self.check_clock_value(
                    "PowerTransformerEnd",
                    end,
                    "PowerTransformerEnd.phaseAngleClock",
                    clock,
                )?;
                clocks.push(clock);
            }
            self.check_clocks("PowerTransformer", &id, &windings, &clocks, false)?;
            let xsc = self.short_circuit_pct(&id, &ends, &windings, &meshes_of);
            let no_load = self.core_admittance(&ends, &windings, &cores_of);
            let name = self.unique_name("transformer", "PowerTransformer", &id);
            let mut transformer = DistTransformer::new(name, windings, xsc, phases);
            transformer
                .extras
                .insert("cim_mrid".into(), id.clone().into());
            transformer.extras.insert(
                "cim_end_mrids".into(),
                serde_json::Value::from(ends.clone()),
            );
            if let Some((loss, imag)) = no_load {
                transformer.extras.insert("%noloadloss".into(), loss.into());
                transformer.extras.insert("%imag".into(), imag.into());
            }
            transformers.push(transformer);
        }
        // Records that hang off no transformer the loop reads.
        for tank in self.store.ids_of("TransformerTank") {
            if !handled.contains(&tank) && !self.dropped.contains(&tank) {
                self.drop_tank(&tank, &index, "it names no PowerTransformer");
            }
        }
        let attached: HashSet<&String> = index.ends_of.values().flatten().collect();
        for end in self.store.ids_of("TransformerTankEnd") {
            if !attached.contains(&end) && !self.dropped.contains(&end) {
                self.drop_record("TransformerTankEnd", &end, "it names no TransformerTank");
            }
        }
        for core in self.store.ids_of("TransformerCoreAdmittance") {
            if !self.core_read.contains(&core) && !self.dropped.contains(&core) {
                self.drop_record(
                    "TransformerCoreAdmittance",
                    &core,
                    "its TransformerEnd is not an end of a transformer the reader maps",
                );
            }
        }
        Ok(transformers)
    }

    /// The no-load percentages `(%noloadloss, %imag)` on winding 1's rating
    /// from the `TransformerCoreAdmittance` on the lowest numbered end that
    /// has one: `g` and `b` (siemens) times that end's impedance base.
    fn core_admittance(
        &mut self,
        ends: &[String],
        windings: &[DistWinding],
        cores_of: &HashMap<String, Vec<String>>,
    ) -> Option<(f64, f64)> {
        let s_1 = windings.first().map_or(0.0, |w| w.s_rating);
        let mut found = None;
        for (end, winding) in ends.iter().zip(windings) {
            for core in cores_of.get(end).map_or(&[][..], Vec::as_slice) {
                if found.is_some() {
                    self.drop_record(
                        "TransformerCoreAdmittance",
                        core,
                        "another end of the transformer already states the core admittance",
                    );
                    continue;
                }
                self.core_read.insert(core.clone());
                let g = self
                    .store
                    .f(core, "TransformerCoreAdmittance.g")
                    .unwrap_or(0.0);
                let b = self
                    .store
                    .f(core, "TransformerCoreAdmittance.b")
                    .unwrap_or(0.0);
                let Some(base) = z_base(winding.v_ref, s_1) else {
                    let label = self.label(core);
                    self.note(
                        &C::READ_CIM_DIST_VALUE_DEFAULTED,
                        "TransformerCoreAdmittance",
                        core,
                        format!(
                            "TransformerCoreAdmittance {label}: its end or winding 1 states no \
                             rating; the no-load data is not carried"
                        ),
                    );
                    found = Some(None);
                    continue;
                };
                found = Some(Some((100.0 * g * base, 100.0 * b * base)));
            }
        }
        found.flatten()
    }

    // -- transformer tanks -------------------------------------------------

    /// The lookups the tank form reads through: each tank's ends, the
    /// catalog entry each tank's `Asset` links, and the catalog's end infos
    /// and tests.
    fn tank_index(&mut self, taps_of: HashMap<String, Vec<String>>) -> TankIndex {
        let ends_of = self.children_of("TransformerTankEnd", "TransformerTankEnd.TransformerTank");
        let store = self.store;
        let mut info_of: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for asset in store.ids_of("Asset") {
            let Some(info) = store.refv(&asset, "Asset.AssetInfo") else {
                continue;
            };
            if store.class_of(info) != Some("TransformerTankInfo") {
                continue;
            }
            for psr in store.refs_all(&asset, "Asset.PowerSystemResources") {
                info_of
                    .entry(psr.to_owned())
                    .or_default()
                    .push((asset.clone(), info.to_owned()));
            }
        }
        TankIndex {
            ends_of,
            info_of,
            end_infos_of: store.index_by(
                "TransformerEndInfo",
                "TransformerEndInfo.TransformerTankInfo",
            ),
            short_circuit_of: store.index_by("ShortCircuitTest", "ShortCircuitTest.EnergisedEnd"),
            no_load_of: store.index_by("NoLoadTest", "NoLoadTest.EnergisedEnd"),
            tap_changers: taps_of,
        }
    }

    /// Mark a tank and its ends dropped, so they are not reported again
    /// property by property.
    fn drop_tank_ends(&mut self, tank: &str, index: &TankIndex) {
        self.dropped.insert(tank.to_owned());
        for end in index.ends_of.get(tank).map_or(&[][..], Vec::as_slice) {
            self.dropped.insert(end.clone());
        }
    }

    fn drop_tank(&mut self, tank: &str, index: &TankIndex, why: &str) {
        self.drop_record("TransformerTank", tank, why);
        self.drop_tank_ends(tank, index);
    }

    /// One `TransformerTank` as a [`DistTransformer`]: windings from its
    /// `TransformerTankEnd`s, ratings, resistances and reactances from its
    /// catalog entry, and taps from the tap changers on its ends.
    #[allow(clippy::too_many_lines, clippy::float_cmp)] // clock numbers are discrete
    fn tank(&mut self, pt: &str, tank: &str, index: &TankIndex) -> Result<Option<DistTransformer>> {
        let Some(links) = index.info_of.get(tank) else {
            self.drop_tank(tank, index, "it has no TransformerTankInfo");
            return Ok(None);
        };
        let (asset, info) = links[0].clone();
        self.consumed.insert(asset);
        if let Some((_, other)) = links.iter().find(|(_, i)| *i != info) {
            let label = self.label(tank);
            self.note(
                &C::READ_CIM_DIST_VALUE_CONFLICT,
                "TransformerTank",
                tank,
                format!(
                    "TransformerTank {label}: Assets link it to TransformerTankInfo `{info}` and \
                     `{other}`; the first is read"
                ),
            );
        }
        let Some(catalog) = self.catalog(&info, index) else {
            self.drop_tank(
                tank,
                index,
                &format!("its TransformerTankInfo `{info}` has no TransformerEndInfo"),
            );
            return Ok(None);
        };
        let mut ends: Vec<(f64, String)> = index
            .ends_of
            .get(tank)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|e| {
                (
                    self.store
                        .f(e, "TransformerEnd.endNumber")
                        .unwrap_or(f64::MAX),
                    e.clone(),
                )
            })
            .collect();
        ends.sort_by(|a, b| a.0.total_cmp(&b.0));
        if ends.len() < 2 {
            self.drop_tank(tank, index, "it has fewer than two ends");
            return Ok(None);
        }
        let mut windings = Vec::with_capacity(ends.len());
        let mut numbers = Vec::with_capacity(ends.len());
        let mut clocks = Vec::with_capacity(ends.len());
        for (number, end) in &ends {
            let Some(info_end) = catalog
                .ends
                .iter()
                .find(|e| end_key(e.number) == end_key(*number))
            else {
                let why = if *number >= f64::MAX {
                    "an end states no endNumber".to_owned()
                } else {
                    format!("end {number} has no TransformerEndInfo of the same endNumber")
                };
                self.drop_tank(tank, index, &why);
                return Ok(None);
            };
            let terminal = self.target("TransformerTankEnd", end, "TransformerEnd.Terminal");
            let Some(bus) = terminal.as_deref().and_then(|t| self.bus_of_terminal(t)) else {
                self.drop_tank(tank, index, "an end has no terminal on a connectivity node");
                return Ok(None);
            };
            if let Some(terminal) = &terminal {
                self.store.consume(terminal, "Terminal.phases");
            }
            let Some(code) = self
                .store
                .enum_value(end, "TransformerTankEnd.phases")
                .map(str::to_owned)
            else {
                self.drop_tank(
                    tank,
                    index,
                    &format!("end {number} states no TransformerTankEnd.phases"),
                );
                return Ok(None);
            };
            let Some((map, conn)) = tank_terminal_map(&code, info_end.delta) else {
                self.drop_tank(
                    tank,
                    index,
                    &format!("end {number} states phases `{code}`, which the reader does not map"),
                );
                return Ok(None);
            };
            self.check_clock_value(
                "TransformerEndInfo",
                &info_end.id,
                "TransformerEndInfo.phaseAngleClock",
                info_end.clock,
            )?;
            let mut winding = DistWinding::new(bus, map, conn, info_end.v_ref, info_end.s_rating);
            winding.r_pct = info_end.r_pct;
            self.grounding(end, &mut winding);
            if let Some(tap) = self.end_tap(end, &index.tap_changers) {
                winding.tap = tap;
            }
            windings.push(winding);
            numbers.push(*number);
            clocks.push(
                info_end
                    .clock
                    .map_or(serde_json::Value::Null, serde_json::Value::from),
            );
        }
        let stated: Vec<Option<f64>> = clocks.iter().map(serde_json::Value::as_f64).collect();
        self.check_clocks("TransformerTank", tank, &windings, &stated, true)?;
        let mut xsc = Vec::new();
        let mut missing = Vec::new();
        for i in 0..numbers.len() {
            for j in i + 1..numbers.len() {
                let x = catalog.pairs.get(&pair_key(numbers[i], numbers[j]));
                if x.is_none() {
                    missing.push(format!("{}-{}", i + 1, j + 1));
                }
                xsc.push(x.copied().unwrap_or(0.0));
            }
        }
        if !missing.is_empty() {
            let label = self.label(tank);
            self.note(
                &C::READ_CIM_DIST_VALUE_DEFAULTED,
                "TransformerTank",
                tank,
                format!(
                    "TransformerTank {label}: no ShortCircuitTest states winding pair(s) {}; \
                     their reactance is 0",
                    missing.join(", ")
                ),
            );
        }
        let phases = match windings[0]
            .terminal_map
            .iter()
            .filter(|t| *t != "4")
            .count()
        {
            3 => 3,
            _ => 1,
        };
        let name = self.unique_name("transformer", "TransformerTank", tank);
        let pt_name = self.store.name_of(pt);
        let mut transformer = DistTransformer::new(name, windings, xsc, phases);
        let extras = &mut transformer.extras;
        extras.insert("cim_mrid".into(), tank.to_owned().into());
        extras.insert(
            "cim_end_mrids".into(),
            serde_json::Value::from(ends.into_iter().map(|(_, e)| e).collect::<Vec<_>>()),
        );
        extras.insert("cim_form".into(), "tank".into());
        extras.insert("cim_power_transformer".into(), pt.to_owned().into());
        if let Some(pt_name) = pt_name {
            extras.insert("cim_power_transformer_name".into(), pt_name.into());
        }
        extras.insert("cim_tank_info".into(), info.into());
        extras.insert("cim_phase_angle_clock".into(), clocks.into());
        if let Some((loss, imag)) = catalog.no_load {
            extras.insert("%noloadloss".into(), loss.into());
            extras.insert("%imag".into(), imag.into());
        }
        Ok(Some(transformer))
    }

    /// Refuse a stated phase-angle clock that is not an integer on a clock
    /// face. Both transformer forms read their clocks through this.
    #[allow(clippy::float_cmp)] // clock numbers are discrete
    fn check_clock_value(
        &mut self,
        class: &str,
        id: &str,
        property: &str,
        clock: Option<f64>,
    ) -> Result<()> {
        if self.store.text(id, property).is_some()
            && clock.is_none_or(|c| !c.is_finite() || c.fract() != 0.0 || !(0.0..12.0).contains(&c))
        {
            return Err(format_error(format!(
                "{class} {}: invalid phase-angle clock; electrical conversion refused",
                self.label(id)
            )));
        }
        Ok(())
    }

    /// Refuse a transformer whose stated winding clocks differ from the
    /// clocks its winding layout represents. An end without a clock takes
    /// the represented one, noted when `note_absent` (the tank form).
    #[allow(clippy::float_cmp)] // clock numbers are discrete
    fn check_clocks(
        &mut self,
        class: &str,
        id: &str,
        windings: &[DistWinding],
        clocks: &[Option<f64>],
        note_absent: bool,
    ) -> Result<()> {
        let three_phase = windings[0]
            .terminal_map
            .iter()
            .filter(|t| *t != "4")
            .count()
            == 3;
        for (i, winding) in windings.iter().enumerate() {
            let represented = tank_phase_clock(&windings[0], winding, three_phase);
            match clocks[i] {
                Some(clock) if clock != represented => {
                    return Err(format_error(format!(
                        "{class} {}: winding {} phase-angle clock {clock} is not represented by its winding layout (clock {represented}); electrical conversion refused",
                        self.label(id),
                        i + 1,
                    )));
                }
                None if note_absent => {
                    self.note(&C::READ_CIM_DIST_VALUE_DEFAULTED, class, id,
                        format!("{class} {}: winding {} has no phase-angle clock; represented clock {represented} is assumed", self.label(id), i + 1));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A `TransformerTankInfo` and everything under it, read once however
    /// many tanks share it.
    fn catalog(&mut self, info: &str, index: &TankIndex) -> Option<Rc<Catalog>> {
        if let Some(catalog) = self.catalogs.get(info) {
            return catalog.clone();
        }
        let catalog = self.read_catalog(info, index).map(Rc::new);
        self.catalogs.insert(info.to_owned(), catalog.clone());
        catalog
    }

    /// The end infos of a catalog entry, in end number order, with their
    /// ratings and connections.
    fn catalog_ends(&mut self, info: &str, index: &TankIndex) -> Vec<CatalogEnd> {
        let mut ends: Vec<CatalogEnd> = Vec::new();
        for id in index.end_infos_of.get(info).map_or(&[][..], Vec::as_slice) {
            let Some(number) = self.store.f(id, "TransformerEndInfo.endNumber") else {
                self.drop_record("TransformerEndInfo", id, "it states no endNumber");
                continue;
            };
            if ends.iter().any(|e| end_key(e.number) == end_key(number)) {
                self.drop_record(
                    "TransformerEndInfo",
                    id,
                    &format!("another end info of its catalog entry is end {number}"),
                );
                continue;
            }
            self.consumed.insert(id.clone());
            let label = self.label(id);
            let delta = match self
                .store
                .enum_value(id, "TransformerEndInfo.connectionKind")
            {
                Some("D") => true,
                Some("Y" | "Yn" | "I") => false,
                Some(other) => {
                    let other = other.to_owned();
                    self.note(
                        &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                        "TransformerEndInfo",
                        id,
                        format!(
                            "TransformerEndInfo {label}: winding connection `{other}` is read as \
                             wye"
                        ),
                    );
                    false
                }
                None => {
                    self.note(
                        &C::READ_CIM_DIST_VALUE_DEFAULTED,
                        "TransformerEndInfo",
                        id,
                        format!("TransformerEndInfo {label}: no connectionKind; wye is assumed"),
                    );
                    false
                }
            };
            let v_ref = self.store.f(id, "TransformerEndInfo.ratedU");
            let s_rating = self.store.f(id, "TransformerEndInfo.ratedS");
            if v_ref.is_none() || s_rating.is_none() {
                self.note(
                    &C::READ_CIM_DIST_VALUE_DEFAULTED,
                    "TransformerEndInfo",
                    id,
                    format!(
                        "TransformerEndInfo {label}: ratedU or ratedS is not stated; 0 is used \
                         and the end's impedance cannot be carried"
                    ),
                );
            }
            let (v_ref, s_rating) = (v_ref.unwrap_or(0.0), s_rating.unwrap_or(0.0));
            let stated_r = self
                .store
                .f(id, "TransformerEndInfo.r")
                .filter(|r| *r != 0.0)
                .and_then(|r| z_base(v_ref, s_rating).map(|base| r / base * 100.0));
            ends.push(CatalogEnd {
                id: id.clone(),
                number,
                delta,
                clock: self.store.f(id, "TransformerEndInfo.phaseAngleClock"),
                v_ref,
                s_rating,
                stated_r,
                r_pct: 0.0,
            });
        }
        ends.sort_by(|a, b| a.number.total_cmp(&b.number));
        ends
    }

    fn read_catalog(&mut self, info: &str, index: &TankIndex) -> Option<Catalog> {
        self.consumed.insert(info.to_owned());
        if let Some(container) = self.target(
            "TransformerTankInfo",
            info,
            "TransformerTankInfo.PowerTransformerInfo",
        ) {
            self.consumed.insert(container);
        }
        let mut ends = self.catalog_ends(info, index);
        if ends.is_empty() {
            return None;
        }
        let (mut pairs, losses, no_reactance) = self.short_circuit_tests(&ends, index);
        self.winding_resistances(info, &mut ends, &losses);
        // Resolve the winding resistances first: a zero/absent test loss
        // does not erase resistance stated by the end infos or other tests.
        for (&(i, j), magnitude) in &mut pairs {
            // A pair whose test states no reactance is already 0 and noted.
            if no_reactance.contains(&(i, j)) {
                continue;
            }
            let on_base = |number| {
                ends.iter()
                    .find(|e| end_key(e.number) == number)
                    .filter(|e| e.s_rating > 0.0)
                    .map_or(0.0, |e| e.r_pct * ends[0].s_rating / e.s_rating)
            };
            let r = on_base(i) + on_base(j);
            let square = *magnitude * *magnitude - r * r;
            if square < 0.0 {
                self.note(&C::READ_CIM_DIST_VALUE_UNSUPPORTED, "TransformerTankInfo", info,
                    format!("TransformerTankInfo {}: pair {i}-{j} leakage magnitude is smaller than the resolved winding resistance; pair reactance is 0", self.label(info)));
            }
            *magnitude = square.max(0.0).sqrt();
        }
        let no_load = self.no_load_test(&ends, index);
        Some(Catalog {
            ends,
            pairs,
            no_load,
        })
    }

    /// The pair reactances of a catalog entry's short circuit tests, and the
    /// pair resistances the tests with a nonzero load loss give, with the
    /// test stating each. Both are percent on the first end's rating. The
    /// third value holds the pairs whose test states no reactance, which the
    /// first map carries as 0.
    #[allow(clippy::type_complexity)]
    fn short_circuit_tests(
        &mut self,
        ends: &[CatalogEnd],
        index: &TankIndex,
    ) -> (
        HashMap<(i64, i64), f64>,
        Vec<((i64, i64), f64, String)>,
        HashSet<(i64, i64)>,
    ) {
        let s_1 = ends[0].s_rating;
        let mut pairs: HashMap<(i64, i64), f64> = HashMap::new();
        let mut losses = Vec::new();
        let mut no_reactance = HashSet::new();
        for (i, energised) in ends.iter().enumerate() {
            for test in index
                .short_circuit_of
                .get(&energised.id)
                .map_or(&[][..], Vec::as_slice)
            {
                let grounded: Vec<String> = self
                    .store
                    .refs_all(test, "ShortCircuitTest.GroundedEnds")
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                if grounded.len() != 1 {
                    self.drop_record(
                        "ShortCircuitTest",
                        test,
                        &format!(
                            "it grounds {} ends; the reader reads tests between two ends",
                            grounded.len()
                        ),
                    );
                    continue;
                }
                let Some(j) = ends.iter().position(|e| e.id == grounded[0]) else {
                    self.drop_record(
                        "ShortCircuitTest",
                        test,
                        "its ends belong to different catalog entries",
                    );
                    continue;
                };
                if j == i {
                    self.drop_record("ShortCircuitTest", test, "it grounds its energised end");
                    continue;
                }
                let key = pair_key(energised.number, ends[j].number);
                if pairs.contains_key(&key) {
                    self.drop_record(
                        "ShortCircuitTest",
                        test,
                        "another test already states the same winding pair",
                    );
                    continue;
                }
                self.consumed.insert(test.clone());
                let label = self.label(test);
                let leakage = self.store.f(test, "ShortCircuitTest.leakageImpedance");
                let loss = self.store.f(test, "ShortCircuitTest.loss").unwrap_or(0.0);
                let base = self.store.f(test, "TransformerTest.basePower");
                let base = base.unwrap_or_else(|| {
                    self.note(
                        &C::READ_CIM_DIST_VALUE_DEFAULTED,
                        "ShortCircuitTest",
                        test,
                        format!(
                            "ShortCircuitTest {label}: no basePower; the energised end's ratedS \
                             is used"
                        ),
                    );
                    energised.s_rating
                });
                let pct = leakage.and_then(|z| {
                    short_circuit_test_pct(z, 0.0, energised.v_ref, base, s_1, LEAKAGE_READING)
                });
                let Some(pct) = pct else {
                    self.note(
                        &C::READ_CIM_DIST_VALUE_DEFAULTED,
                        "ShortCircuitTest",
                        test,
                        format!(
                            "ShortCircuitTest {label}: needs leakageImpedance and positive \
                             ratings; the pair reactance is 0"
                        ),
                    );
                    pairs.insert(key, 0.0);
                    no_reactance.insert(key);
                    continue;
                };
                pairs.insert(key, pct.x);
                // A loss of 0 states nothing: some producers leave it 0 on
                // every pair but one.
                if loss != 0.0 {
                    let r = short_circuit_test_pct(
                        0.0,
                        loss,
                        energised.v_ref,
                        base,
                        s_1,
                        LEAKAGE_READING,
                    )
                    .map_or(0.0, |p| p.r);
                    losses.push((key, r, test.clone()));
                }
            }
        }
        (pairs, losses, no_reactance)
    }

    /// Winding resistances: the end infos' own when any states one (a load
    /// loss that disagrees is reported), else the split of the pair
    /// resistances the load losses give, as CIMHub's
    /// `SetWindingResistances` does.
    fn winding_resistances(
        &mut self,
        info: &str,
        ends: &mut [CatalogEnd],
        losses: &[((i64, i64), f64, String)],
    ) {
        let s_1 = ends[0].s_rating;
        // A percentage on the end's own rating to one on the first end's,
        // and back.
        let to_base = |pct: f64, s: f64| if s > 0.0 { pct * s_1 / s } else { 0.0 };
        let from_base = |pct: f64, s: f64| if s_1 > 0.0 { pct * s / s_1 } else { 0.0 };
        if ends.iter().any(|e| e.stated_r.is_some()) {
            for end in ends.iter_mut() {
                end.r_pct = end.stated_r.unwrap_or(0.0);
            }
            for (key, pair_r, test) in losses {
                let on_base = |number: i64| {
                    ends.iter()
                        .find(|e| end_key(e.number) == number)
                        .map_or(0.0, |e| to_base(e.r_pct, e.s_rating))
                };
                let sum = on_base(key.0) + on_base(key.1);
                if (pair_r - sum).abs() > 1e-6 * pair_r.abs().max(sum.abs()) {
                    let label = self.label(test);
                    self.note(
                        &C::READ_CIM_DIST_VALUE_CONFLICT,
                        "ShortCircuitTest",
                        test,
                        format!(
                            "ShortCircuitTest {label}: its loss gives a pair resistance of \
                             {pair_r}%, but the end resistances sum to {sum}%; the end \
                             resistances are kept"
                        ),
                    );
                }
            }
            return;
        }
        if losses.is_empty() {
            return;
        }
        let pair_r = |a: f64, b: f64| {
            losses
                .iter()
                .find(|(k, _, _)| *k == pair_key(a, b))
                .map_or(0.0, |(_, r, _)| *r)
        };
        let split = match ends {
            [a, b] => vec![pair_r(a.number, b.number) / 2.0; 2],
            [a, b, c] => {
                // A pair whose test states no loss is unknown, not 0, and
                // the delta to star split needs all three.
                let (na, nb, nc) = (a.number, b.number, c.number);
                let missing: Vec<String> = [(na, nb), (na, nc), (nb, nc)]
                    .iter()
                    .filter(|(x, y)| !losses.iter().any(|(k, _, _)| *k == pair_key(*x, *y)))
                    .map(|(x, y)| format!("{x}-{y}"))
                    .collect();
                let (rab, rac, rbc) = (pair_r(na, nb), pair_r(na, nc), pair_r(nb, nc));
                let split = vec![
                    (rab + rac - rbc) / 2.0,
                    (rab + rbc - rac) / 2.0,
                    (rac + rbc - rab) / 2.0,
                ];
                let tolerance = 1e-9 * (rab + rac + rbc);
                let reason = if !missing.is_empty() {
                    format!(
                        "winding pair(s) {} state no load loss, so the split is not determined",
                        missing.join(", ")
                    )
                } else if split.iter().any(|r| *r < -tolerance) {
                    "the split gives a negative winding resistance".to_owned()
                } else {
                    String::new()
                };
                if !reason.is_empty() {
                    let label = self.label(info);
                    self.note(
                        &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                        "TransformerTankInfo",
                        info,
                        format!(
                            "TransformerTankInfo {label}: {reason}; the winding resistances are 0"
                        ),
                    );
                    return;
                }
                split.into_iter().map(|r| r.max(0.0)).collect()
            }
            _ => {
                let label = self.label(info);
                self.note(
                    &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                    "TransformerTankInfo",
                    info,
                    format!(
                        "TransformerTankInfo {label}: {} windings and no end resistance; load \
                         losses split between two or three windings only, so the resistances \
                         are 0",
                        ends.len()
                    ),
                );
                return;
            }
        };
        for (end, r) in ends.iter_mut().zip(split) {
            end.r_pct = from_base(r, end.s_rating);
        }
    }

    /// The no-load percentages from the `NoLoadTest` that energises the
    /// first end.
    fn no_load_test(&mut self, ends: &[CatalogEnd], index: &TankIndex) -> Option<(f64, f64)> {
        let s_1 = ends[0].s_rating;
        let mut no_load = None;
        for (i, end) in ends.iter().enumerate() {
            for test in index.no_load_of.get(&end.id).map_or(&[][..], Vec::as_slice) {
                if i > 0 {
                    self.drop_record(
                        "NoLoadTest",
                        test,
                        "it energises an end other than end 1, whose test the reader reads",
                    );
                    continue;
                }
                if no_load.is_some() {
                    self.drop_record(
                        "NoLoadTest",
                        test,
                        "another NoLoadTest already states end 1's no-load data",
                    );
                    continue;
                }
                self.consumed.insert(test.clone());
                // The energised end's ratedU states the same voltage.
                self.store.consume(test, "NoLoadTest.energisedEndVoltage");
                let loss = self.store.f(test, "NoLoadTest.loss").unwrap_or(0.0);
                let exciting = self
                    .store
                    .f(test, "NoLoadTest.excitingCurrent")
                    .unwrap_or(0.0);
                let base = self
                    .store
                    .f(test, "TransformerTest.basePower")
                    .unwrap_or(s_1);
                no_load = Some(no_load_test_pct(loss, exciting, base, s_1));
            }
        }
        no_load.flatten()
    }

    fn transformer_phase_count(&mut self, ends: &[String]) -> usize {
        for end in ends {
            if let Some(term) = self
                .store
                .refv(end, "TransformerEnd.Terminal")
                .map(str::to_owned)
                && let Some(map) = self.terminal_phases(&term)
            {
                let n = map.iter().filter(|t| *t != "4").count();
                if n > 0 {
                    return n;
                }
            }
        }
        3
    }

    fn build_windings(&mut self, ends: &[String], phases: usize) -> Option<Vec<DistWinding>> {
        let default_map: Vec<String> = (1..=phases).map(|i| i.to_string()).collect();
        let mut windings = Vec::new();
        for end in ends {
            let term = self.target("PowerTransformerEnd", end, "TransformerEnd.Terminal")?;
            let bus = self.bus_of_terminal(&term)?;
            let label = self.label(end);
            // The connection, and whether the winding brings its star point out
            // as a neutral terminal (`Yn`, `Zn`), which the model grounds.
            let (conn, neutral) = match self
                .store
                .enum_value(end, "PowerTransformerEnd.connectionKind")
            {
                Some("D") => (DistWindingConn::Delta, false),
                Some("Y") => (DistWindingConn::Wye, false),
                Some("Yn") => (DistWindingConn::Wye, true),
                Some(other) => {
                    let other = other.to_owned();
                    self.note(
                        &C::READ_CIM_DIST_VALUE_UNSUPPORTED,
                        "PowerTransformerEnd",
                        end,
                        format!(
                            "PowerTransformerEnd {label}: winding connection `{other}` is read as \
                             wye"
                        ),
                    );
                    (DistWindingConn::Wye, other.ends_with('n'))
                }
                None => {
                    self.note(
                        &C::READ_CIM_DIST_VALUE_DEFAULTED,
                        "PowerTransformerEnd",
                        end,
                        format!(
                            "PowerTransformerEnd {label}: no connectionKind; wye with a neutral \
                             is assumed"
                        ),
                    );
                    (DistWindingConn::Wye, true)
                }
            };
            let stated = self.terminal_phases(&term).filter(|m| !m.is_empty());
            let map = match (conn, stated) {
                // A delta winding has no neutral terminal.
                (DistWindingConn::Delta, Some(map)) => {
                    map.into_iter().filter(|t| t != "4").collect::<Vec<_>>()
                }
                (_, Some(map)) => map,
                (_, None) => default_map.clone(),
            };
            let map = if neutral {
                with_neutral(map, Configuration::Wye)
            } else {
                map
            };
            let v_ref = self.store.f(end, "PowerTransformerEnd.ratedU");
            let s_rating = self.store.f(end, "PowerTransformerEnd.ratedS");
            if v_ref.is_none() || s_rating.is_none() {
                self.note(
                    &C::READ_CIM_DIST_VALUE_DEFAULTED,
                    "PowerTransformerEnd",
                    end,
                    format!(
                        "PowerTransformerEnd {label}: ratedU or ratedS is not stated; 0 is used \
                         and the end's impedance cannot be carried"
                    ),
                );
            }
            let (v_ref, s_rating) = (v_ref.unwrap_or(0.0), s_rating.unwrap_or(0.0));
            let mut winding = DistWinding::new(bus, map, conn, v_ref, s_rating);
            if let Some(r) = self.store.f(end, "PowerTransformerEnd.r")
                && let Some(base) = z_base(v_ref, s_rating)
            {
                winding.r_pct = r / base * 100.0;
            }
            self.grounding(end, &mut winding);
            windings.push(winding);
        }
        Some(windings)
    }

    /// A grounded end's neutral impedance.
    fn grounding(&mut self, end: &str, winding: &mut DistWinding) {
        if self.store.boolean(end, "TransformerEnd.grounded") == Some(true) {
            winding.r_neutral = Some(self.store.f(end, "TransformerEnd.rground").unwrap_or(0.0));
            winding.x_neutral = Some(self.store.f(end, "TransformerEnd.xground").unwrap_or(0.0));
        } else {
            self.store.consume(end, "TransformerEnd.rground");
            self.store.consume(end, "TransformerEnd.xground");
        }
    }

    /// The tap of an end from its `RatioTapChanger`: the position (`step`,
    /// else `normalStep`) above `neutralStep`, times `stepVoltageIncrement`
    /// percent. Tap changer controls are not carried.
    fn end_tap(&mut self, end: &str, taps_of: &HashMap<String, Vec<String>>) -> Option<f64> {
        let changers = taps_of.get(end)?;
        let changer = changers.first()?.clone();
        for extra in changers.iter().skip(1) {
            self.drop_record(
                "RatioTapChanger",
                extra,
                "its end already has a tap changer",
            );
        }
        let position = self
            .store
            .f(&changer, "TapChanger.step")
            .or_else(|| self.store.f(&changer, "TapChanger.normalStep"));
        let neutral = self.store.f(&changer, "TapChanger.neutralStep");
        let increment = self
            .store
            .f(&changer, "RatioTapChanger.stepVoltageIncrement");
        let label = self.label(&changer);
        if let (Some(position), Some(neutral), Some(increment)) = (position, neutral, increment) {
            Some(1.0 + (position - neutral) * increment / 100.0)
        } else {
            self.note(
                &C::READ_CIM_DIST_VALUE_DEFAULTED,
                "RatioTapChanger",
                &changer,
                format!(
                    "RatioTapChanger {label}: needs step or normalStep, neutralStep, and \
                     stepVoltageIncrement; the nominal tap is used"
                ),
            );
            None
        }
    }

    /// Short circuit reactances between winding pairs, percent on winding 1's
    /// power base, in the model's `12, 13, …, 23, …` order. A
    /// `TransformerMeshImpedance` between the two ends wins; otherwise the
    /// pair is the sum of the ends' own `x` (a star equivalent).
    fn short_circuit_pct(
        &mut self,
        transformer: &str,
        ends: &[String],
        windings: &[DistWinding],
        meshes_of: &HashMap<String, Vec<String>>,
    ) -> Vec<f64> {
        let s_base = windings.first().map_or(0.0, |w| w.s_rating);
        let per_unit = |x: f64, v_ref: f64| z_base(v_ref, s_base).map(|base| x / base);
        let mut star = Vec::with_capacity(ends.len());
        for (end, winding) in ends.iter().zip(windings) {
            let x = self.store.f(end, "PowerTransformerEnd.x").unwrap_or(0.0);
            star.push(per_unit(x, winding.v_ref).unwrap_or(0.0));
        }
        let mut mesh: HashMap<(usize, usize), f64> = HashMap::new();
        for (i, end) in ends.iter().enumerate() {
            for m in meshes_of.get(end).map_or(&[][..], Vec::as_slice) {
                let Some(to) = self.target(
                    "TransformerMeshImpedance",
                    m,
                    "TransformerMeshImpedance.ToTransformerEnd",
                ) else {
                    continue;
                };
                let Some(j) = ends.iter().position(|e| *e == to) else {
                    self.drop_record(
                        "TransformerMeshImpedance",
                        m,
                        "its ends belong to different transformers",
                    );
                    continue;
                };
                if self
                    .store
                    .f(m, "TransformerMeshImpedance.r")
                    .is_some_and(|r| r != 0.0)
                {
                    let label = self.label(m);
                    self.note(
                        &C::READ_CIM_DIST_FIELD_UNMAPPED,
                        "TransformerMeshImpedance",
                        m,
                        format!(
                            "TransformerMeshImpedance {label}: the pair resistance is not carried; \
                             winding resistances come from the ends"
                        ),
                    );
                }
                let x = self.store.f(m, "TransformerMeshImpedance.x").unwrap_or(0.0);
                if let Some(pu) = per_unit(x, windings[i].v_ref) {
                    mesh.insert((i.min(j), i.max(j)), pu);
                }
            }
        }
        if s_base <= 0.0 {
            let label = self.label(transformer);
            self.note(
                &C::READ_CIM_DIST_VALUE_DEFAULTED,
                "PowerTransformer",
                transformer,
                format!("PowerTransformer {label}: winding 1 states no ratedS; short circuit reactances are 0"),
            );
        }
        let mut xsc = Vec::new();
        for i in 0..ends.len() {
            for j in i + 1..ends.len() {
                let pu = mesh.get(&(i, j)).copied().unwrap_or(star[i] + star[j]);
                xsc.push(pu * 100.0);
            }
        }
        xsc
    }

    // -- sources -----------------------------------------------------------

    fn sources(&mut self) -> Vec<VoltageSource> {
        let phase_children =
            self.children_of("EnergySourcePhase", "EnergySourcePhase.EnergySource");
        let mut sources = Vec::new();
        for id in self.store.ids_of("EnergySource") {
            let Some(bus) = self.bus_at(&id, 0) else {
                self.drop_record("EnergySource", &id, "no terminal on a connectivity node");
                continue;
            };
            let children = phase_children.get(&id).cloned().unwrap_or_default();
            let phases: Vec<String> = if children.is_empty() {
                self.fallback_map("EnergySource", &id)
            } else {
                for terminal in self.terminals(&id) {
                    self.store.consume(&terminal, "Terminal.phases");
                }
                self.phase_children(
                    &children,
                    "EnergySourcePhase",
                    "EnergySourcePhase.phase",
                    None,
                )
                .into_iter()
                .map(|(t, _)| t)
                .collect()
            };
            let phases: Vec<String> = phases.into_iter().filter(|t| t != "4").collect();
            if phases.is_empty() {
                self.drop_record("EnergySource", &id, "no readable phase");
                continue;
            }
            let label = self.label(&id);
            let v_ll = self
                .store
                .f(&id, "EnergySource.voltageMagnitude")
                .or_else(|| self.store.f(&id, "EnergySource.nominalVoltage"));
            self.store.consume(&id, "EnergySource.nominalVoltage");
            let v_ll = v_ll.unwrap_or_else(|| {
                self.note(
                    &C::READ_CIM_DIST_VALUE_DEFAULTED,
                    "EnergySource",
                    &id,
                    format!(
                        "EnergySource {label}: no voltageMagnitude or nominalVoltage; 0 V is used"
                    ),
                );
                0.0
            });
            let base_angle = self
                .store
                .f(&id, "EnergySource.voltageAngle")
                .unwrap_or(0.0);
            let name = self.unique_name("source", "EnergySource", &id);
            let mut source = wye_source(name, bus, phases, v_ll, base_angle);
            let map = source.terminal_map.clone();
            let impedance = |key: &str| self.store.f(&id, key);
            let (r1, x1) = (impedance("EnergySource.r"), impedance("EnergySource.x"));
            let (r0, x0) = (impedance("EnergySource.r0"), impedance("EnergySource.x0"));
            if r1.is_some() || x1.is_some() || r0.is_some() || x0.is_some() {
                // The PMD reader's convention: full matrices over every
                // terminal, which the dss and PMD writers both read.
                let (r1, x1) = (r1.unwrap_or(0.0), x1.unwrap_or(0.0));
                let rs = sequence_to_phase(r0.unwrap_or(r1), r1, map.len());
                let xs = sequence_to_phase(x0.unwrap_or(x1), x1, map.len());
                source.extras.insert("rs".into(), matrix_value(&rs));
                source.extras.insert("xs".into(), matrix_value(&xs));
                // The stated values too: rebuilding them from the matrices
                // can differ in the last bit, which would break a byte
                // stable rewrite.
                source.extras.insert(
                    "cim_sequence_impedance".into(),
                    serde_json::json!({"r": r1, "x": x1, "r0": r0.unwrap_or(r1), "x0": x0.unwrap_or(x1)}),
                );
            }

            source.extras.insert("cim_mrid".into(), id.clone().into());
            sources.push(source);
        }
        sources
    }

    // -- layout ------------------------------------------------------------

    /// The coordinate reference system a diagram names: an `EPSG:<code>`
    /// token in its name or description, as a GIS export states it.
    fn diagram_crs(&self, diagram: &str) -> Option<String> {
        ["IdentifiedObject.name", "IdentifiedObject.description"]
            .iter()
            .filter_map(|key| self.store.text(diagram, key))
            .find_map(epsg_code)
    }

    /// Coordinates from the Diagram Layout. A segment's points are its
    /// route, the first at terminal 1. A connectivity node has no diagram
    /// object of its own, so it takes the end of the first segment there
    /// (segment ends meet at the nodes they share), else the point of other
    /// equipment at it or of the composite switch that holds a switch, else,
    /// with the [`sedms`] extension, the point of a service location served
    /// from it; these node locations are derived. `None` when the layout places nothing.
    fn layout(&mut self, lines: &mut [DistLine]) -> Option<Layout> {
        let (space, shape_of) = self.diagram_shapes()?;
        let mut placed = false;
        for line in lines.iter_mut() {
            let path = line
                .extras
                .get("cim_mrid")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| shape_of.get(id))
                .filter(|path| path.len() >= 2);
            if let Some(path) = path {
                line.route = Some(
                    path.iter()
                        .map(|&(x, y)| DistLocation { x, y, kind: None })
                        .collect(),
                );
                placed = true;
            }
        }

        let mut at_node = HashMap::new();
        for segment in self.store.ids_of("ACLineSegment") {
            if let Some(shape) = shape_of.get(&segment) {
                self.place_terminals(&segment, shape, &mut at_node);
            }
        }
        let mut equipment = Vec::new();
        let mut seen = HashSet::new();
        for terminal in self.store.ids_of("Terminal") {
            if let Some(owner) = self.store.refv(&terminal, "Terminal.ConductingEquipment")
                && self.store.class_of(owner) != Some("ACLineSegment")
                && seen.insert(owner.to_owned())
            {
                equipment.push(owner.to_owned());
            }
        }
        for owner in equipment {
            let shape = shape_of.get(&owner).or_else(|| {
                self.store
                    .refv(&owner, "Switch.CompositeSwitch")
                    .and_then(|composite| shape_of.get(composite))
            });
            if let Some(shape) = shape {
                self.place_terminals(&owner, shape, &mut at_node);
            }
        }
        if self.sedms {
            self.place_service_locations(&shape_of, &mut at_node);
        }
        placed |= !at_node.is_empty();
        placed.then_some(Layout {
            meta: DistGeoMeta {
                space,
                kind: Some(DistCoordsKind::Source),
            },
            at_node,
        })
    }

    /// The coordinate space of one `Diagram` (the first that names an
    /// `EPSG:` coordinate reference system, else the first) and the shape of
    /// each element its `DiagramObject`s draw: their `DiagramObjectPoint`s in
    /// `sequenceNumber` order. A named CRS makes the coordinates projected
    /// (geographic for EPSG:4326), a [`sedms`] GIS diagram projected with no
    /// CRS, and anything else a drawing.
    fn diagram_shapes(&mut self) -> Option<(CoordinateSpace, Shapes)> {
        let diagrams = self.store.ids_of("Diagram");
        let named: Vec<Option<String>> = diagrams.iter().map(|d| self.diagram_crs(d)).collect();
        let at = named.iter().position(Option::is_some).unwrap_or(0);
        let diagram = diagrams.get(at)?.clone();
        let space = match named[at].clone() {
            Some(crs) if crs == "EPSG:4326" => CoordinateSpace::Geographic { crs: Some(crs) },
            Some(crs) => CoordinateSpace::Projected { crs: Some(crs) },
            None if self.sedms && self.sedms_gis_diagram(&diagram) => {
                CoordinateSpace::Projected { crs: None }
            }
            None => CoordinateSpace::Diagram { canvas: None },
        };
        self.consumed.insert(diagram.clone());

        // The diagram's objects by the element they draw, the first per
        // element. An object that names no diagram belongs to the only one.
        let only = diagrams.len() == 1;
        let mut drawn: HashMap<String, String> = HashMap::new();
        for object in self.store.ids_of("DiagramObject") {
            let ours = self
                .store
                .refv(&object, "DiagramObject.Diagram")
                .map_or(only, |d| d == diagram);
            if !ours {
                continue;
            }
            self.consumed.insert(object.clone());
            if let Some(element) =
                self.target("DiagramObject", &object, "DiagramObject.IdentifiedObject")
            {
                drawn.entry(element).or_insert(object);
            }
        }
        let objects: HashSet<String> = drawn.values().cloned().collect();
        let mut points: HashMap<String, Vec<(f64, usize, f64, f64)>> = HashMap::new();
        for (order, point) in self
            .store
            .ids_of("DiagramObjectPoint")
            .into_iter()
            .enumerate()
        {
            let Some(object) = self
                .store
                .refv(&point, "DiagramObjectPoint.DiagramObject")
                .filter(|object| objects.contains(*object))
                .map(str::to_owned)
            else {
                continue;
            };
            let x = self.store.f(&point, "DiagramObjectPoint.xPosition");
            let y = self.store.f(&point, "DiagramObjectPoint.yPosition");
            let sequence = self
                .store
                .f(&point, "DiagramObjectPoint.sequenceNumber")
                .unwrap_or(f64::MAX);
            self.consumed.insert(point.clone());
            let (Some(x), Some(y)) = (x, y) else {
                self.drop_record(
                    "DiagramObjectPoint",
                    &point,
                    "it states no finite xPosition and yPosition",
                );
                continue;
            };
            points
                .entry(object)
                .or_default()
                .push((sequence, order, x, y));
        }
        let mut shape_of = Shapes::new();
        for (element, object) in drawn {
            if let Some(mut path) = points.remove(&object) {
                path.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                shape_of.insert(
                    element,
                    path.into_iter().map(|(_, _, x, y)| (x, y)).collect(),
                );
            }
        }
        Some((space, shape_of))
    }

    /// Place the nodes at an equipment's terminals from its drawn shape: the
    /// last terminal of several at the last point, every other terminal at
    /// the first. A node keeps the first location it gets.
    fn place_terminals(
        &self,
        equipment: &str,
        shape: &[(f64, f64)],
        at_node: &mut HashMap<String, DistLocation>,
    ) {
        let (Some(&first), Some(&last)) = (shape.first(), shape.last()) else {
            return;
        };
        let terminals = self.terminals(equipment);
        let end = terminals.len().saturating_sub(1);
        for (k, terminal) in terminals.iter().enumerate() {
            let Some(node) = self.wiring.node_of.get(terminal) else {
                continue;
            };
            let (x, y) = if k == end && k > 0 { last } else { first };
            at_node.entry(node.clone()).or_insert(DistLocation {
                x,
                y,
                kind: Some(DistCoordsKind::Derived),
            });
        }
    }

    // -- unmapped content --------------------------------------------------

    /// Report every object of a class the mapping does not read, per class,
    /// and every property of a mapped object the mapping did not read, per
    /// class and property.
    fn report_unread(&mut self) {
        let store = self.store;
        let mapped =
            |class: &str| MAPPED_CLASSES.contains(&class) || SWITCH_CLASSES.contains(&class);
        // A terminal of equipment the mapping does not carry is reported
        // with that equipment, not property by property.
        let mut uncovered_terminals: HashSet<&str> = HashSet::new();
        for object in store.objects.iter().filter(|o| o.class == "Terminal") {
            let equipment = object.props.iter().find_map(|(k, v)| match v {
                PropValue::Ref(target) if k == "Terminal.ConductingEquipment" => Some(target),
                _ => None,
            });
            let covered = equipment.is_some_and(|eq| {
                !self.dropped.contains(eq) && store.class_of(eq).is_some_and(mapped)
            });
            if !covered {
                uncovered_terminals.insert(object.id.as_str());
            }
        }
        let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
        let mut fields: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        let mut nested: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        let read = store.read.borrow();
        for (at, object) in store.objects.iter().enumerate() {
            let class = object.class.as_str();
            if !mapped(class) && !self.consumed.contains(&object.id) {
                *classes.entry(class).or_default() += 1;
                continue;
            }
            if self.dropped.contains(&object.id) || uncovered_terminals.contains(object.id.as_str())
            {
                continue;
            }
            for (i, (key, _)) in object.props.iter().enumerate() {
                if read[at][i] || key == "IdentifiedObject.mRID" || key == "IdentifiedObject.name" {
                    continue;
                }
                *fields.entry((class, key.as_str())).or_default() += 1;
            }
            for key in &object.nested {
                *nested.entry((class, key.as_str())).or_default() += 1;
            }
        }
        drop(read);
        for (class, count) in classes {
            let (info, what) = if CONTEXT_CLASSES.contains(&class) {
                (&C::READ_CIM_DIST_CONTEXT_UNMAPPED, "context")
            } else {
                (&C::READ_CIM_DIST_CLASS_UNMAPPED, "data")
            };
            let mut diagnostic = Diagnostic::of(
                info,
                format!(
                    "{count} {class} object(s) have no multiconductor mapping ({what}) and are not carried"
                ),
            );
            let _ = diagnostic.insert_detail("class", serde_json::Value::from(class));
            let _ = diagnostic.insert_detail("count", serde_json::Value::from(count));
            self.diags.record(diagnostic);
        }
        for ((class, key), count) in fields {
            let info = if CONTEXT_PROPERTIES.contains(&key) || CONTEXT_CLASSES.contains(&class) {
                &C::READ_CIM_DIST_CONTEXT_UNMAPPED
            } else {
                &C::READ_CIM_DIST_FIELD_UNMAPPED
            };
            let mut diagnostic = Diagnostic::of(
                info,
                format!("{key} on {count} {class} object(s) is not carried"),
            );
            let _ = diagnostic.insert_detail("class", serde_json::Value::from(class));
            let _ = diagnostic.insert_detail("property", serde_json::Value::from(key));
            let _ = diagnostic.insert_detail("count", serde_json::Value::from(count));
            self.diags.record(diagnostic);
        }
        for ((class, key), count) in nested {
            let mut diagnostic = Diagnostic::of(
                &C::READ_CIM_DIST_FIELD_UNMAPPED,
                format!(
                    "{key} on {count} {class} object(s) holds nested RDF/XML elements instead of \
                     a value and is not carried"
                ),
            );
            let _ = diagnostic.insert_detail("class", serde_json::Value::from(class));
            let _ = diagnostic.insert_detail("property", serde_json::Value::from(key));
            let _ = diagnostic.insert_detail("count", serde_json::Value::from(count));
            self.diags.record(diagnostic);
        }
    }
}

/// Each bus at the location the layout derives for its connectivity node.
fn locate_buses(buses: &mut [DistBus], layout: &Layout) {
    for bus in buses.iter_mut() {
        let node = bus
            .extras
            .get("cim_mrid")
            .and_then(serde_json::Value::as_str);
        if let Some(location) = node.and_then(|node| layout.at_node.get(node)) {
            bus.location = Some(*location);
        }
    }
}

/// The terminals each bus presents: the ones its elements connect, by lower
/// case bus id.
fn terminal_usage(
    lines: &[DistLine],
    switches: &[DistSwitch],
    loads: &[DistLoad],
    ibrs: &[DistIbr],
    shunts: &[DistShunt],
    transformers: &[DistTransformer],
    sources: &[VoltageSource],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut used: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut connect = |bus: &str, terminals: &[String]| {
        used.entry(bus.to_ascii_lowercase())
            .or_default()
            .extend(terminals.iter().cloned());
    };
    for line in lines {
        connect(&line.bus_from, &line.terminal_map_from);
        connect(&line.bus_to, &line.terminal_map_to);
    }
    for switch in switches {
        connect(&switch.bus_from, &switch.terminal_map_from);
        connect(&switch.bus_to, &switch.terminal_map_to);
    }
    for load in loads {
        connect(&load.bus, &load.terminal_map);
    }
    for ibr in ibrs {
        connect(&ibr.bus, &ibr.terminal_map);
    }
    for shunt in shunts {
        connect(&shunt.bus, &shunt.terminal_map);
    }
    for winding in transformers.iter().flat_map(|t| &t.windings) {
        connect(&winding.bus, &winding.terminal_map);
    }
    for source in sources {
        connect(&source.bus, &source.terminal_map);
    }
    used
}

/// Buses from the connectivity nodes, then the supply busbars the reader
/// placed, with each bus's terminals taken from the terminals the elements
/// present there (so a wye element's grounded neutral appears on its bus). A
/// neutral terminal reads as perfectly grounded, the wire coordinate
/// convention shared with the dss reader.
fn derive_buses(
    nodes: &[(String, String)],
    busbars: &[String],
    used: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<DistBus> {
    let bus = |bus_id: &str| {
        let terminals: Vec<String> = used
            .get(&bus_id.to_ascii_lowercase())
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default();
        let grounded = if terminals.iter().any(|t| t == "4") {
            vec!["4".to_owned()]
        } else {
            Vec::new()
        };
        let mut bus = DistBus::new(bus_id, terminals);
        bus.grounded = grounded;
        bus
    };
    nodes
        .iter()
        .map(|(bus_id, mrid)| {
            let mut bus = bus(bus_id);
            bus.extras.insert("cim_mrid".into(), mrid.clone().into());
            bus
        })
        .chain(busbars.iter().map(|bus_id| bus(bus_id)))
        .collect()
}

/// The nodal matrix of three equal delta branches of admittance `y`: `2y`
/// on the diagonal and `−y` off it.
fn delta_matrix(y: f64) -> ConductorMatrix {
    let mut m = vec![vec![-y; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 2.0 * y;
    }
    m
}

/// A diagonal siemens matrix from per conductor values.
fn diagonal(values: &[f64]) -> ConductorMatrix {
    let n = values.len();
    let mut m = vec![vec![0.0; n]; n];
    for (i, &v) in values.iter().enumerate() {
        m[i][i] = v;
    }
    m
}

/// Split a total per length shunt admittance evenly between the two ends.
fn set_shunt_halves(code: &mut DistLineCode, g: &ConductorMatrix, b: &ConductorMatrix) {
    let half = |m: &ConductorMatrix| -> ConductorMatrix {
        m.iter()
            .map(|row| row.iter().map(|v| v / 2.0).collect())
            .collect()
    };
    code.g_from = half(g);
    code.g_to = half(g);
    code.b_from = half(b);
    code.b_to = half(b);
}

fn matrix_value(m: &ConductorMatrix) -> serde_json::Value {
    serde_json::Value::from(
        m.iter()
            .map(|row| serde_json::Value::from(row.clone()))
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_codes_expand_to_terminal_names() {
        let expand = |code: &str| expand_phase_code(code).unwrap();
        assert_eq!(expand("ABC"), ["1", "2", "3"]);
        assert_eq!(expand("ABCN"), ["1", "2", "3", "4"]);
        assert_eq!(expand("AB"), ["1", "2"]);
        assert_eq!(expand("s1s2"), ["1", "2"]);
        assert_eq!(expand("s12N"), ["1", "2", "4"]);
        assert_eq!(expand("C"), ["3"]);
        assert_eq!(expand_phase_code("XY"), None);
    }
}

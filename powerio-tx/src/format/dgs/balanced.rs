//! The balanced network a sequence data DGS export describes.
//!
//! The mapping follows the PowSybl PowerFactory converter: one calculated bus
//! per group of `ElmTerm` terminals joined by closed `ElmCoup` switches,
//! lines and transformers from their `TypLne`/`TypTr2`/`TypTr3` types, loads
//! from the `mode_inp` pair they state, machines and external grids as
//! generators, and a two converter DC island as one HVDC record. DGS carries
//! no system base, so per unit values use 100 MVA and each bus's `uknom`.

use std::collections::{BTreeMap, HashMap, HashSet};

use powerio_core::{Diagnostic, DiagnosticInfo, SourceId, SourceSpan};

use super::tokens::{DgsDocument, DgsObject};
use crate::diagnostics::{Diagnostics, codes};
use crate::network::{
    BalancedNetwork, BalancedNetworkTables, Branch, BranchCharging, Bus, BusId, BusType, Extras,
    Generator, GeneratorEnergySource, Hvdc, HvdcConvertersMode, Impedance, Load, Shunt,
    ShuntBlock, SourceFormat, Switch, SwitchedShuntControl, SwitchedShuntMode, Transformer3W,
    TransformerControl, TransformerControlMode, Winding,
};
use crate::{Error, Result};

const FMT: &str = super::tokens::FMT;

/// DGS carries no system base; per unit values use this base.
pub(crate) const DEFAULT_BASE_MVA: f64 = 100.0;

/// PowerFactory's own default nominal frequency.
const DEFAULT_FREQUENCY: f64 = 50.0;

/// Where the decoded text sits in the retained source, so a finding can name
/// the row it comes from.
#[derive(Clone, Debug)]
pub struct SpanContext {
    pub source: SourceId,
    /// Byte offset of the decoded text within the retained buffer.
    pub offset: u64,
}

/// Classes the balanced mapping reads, references, or deliberately ignores.
const HANDLED_CLASSES: [&str; 33] = [
    "ElmNet",
    "ElmTerm",
    "ElmLne",
    "ElmTow",
    "ElmZpu",
    "ElmTr2",
    "ElmTr3",
    "ElmLod",
    "ElmLodmv",
    "ElmLodlv",
    "ElmLodlvp",
    "ElmSym",
    "ElmGenstat",
    "ElmPvsys",
    "ElmAsm",
    "ElmXnet",
    "ElmShnt",
    "ElmCoup",
    "ElmVsc",
    "ElmTapctrl",
    "ElmZone",
    "ElmArea",
    "ElmSite",
    "ElmSubstat",
    "ElmTrfstat",
    "StaCubic",
    "StaSwitch",
    "TypLne",
    "TypTow",
    "TypTr2",
    "TypTr3",
    "TypSym",
    "TypLod",
];

/// Element classes whose rows the mapping never reads: their presence is
/// reported once per class because they can carry electrical data.
fn reports_when_unmapped(class: &str) -> bool {
    class.starts_with("Elm") || class.starts_with("Cha")
}

#[derive(Clone, Copy, Debug)]
struct Connection {
    terminal: i64,
    side: i64,
    closed: bool,
}

struct Context<'a> {
    doc: &'a DgsDocument,
    frequency: f64,
    /// Element id to its connections, sorted by side.
    connections: HashMap<i64, Vec<Connection>>,
    /// AC terminal id to the calculated bus it belongs to.
    terminal_bus: HashMap<i64, BusId>,
    bus_kv: HashMap<BusId, f64>,
    dc_terminals: HashSet<i64>,
    dc_elements: HashSet<i64>,
    spans: Option<SpanContext>,
}

impl Context<'_> {
    fn diagnostic(
        &self,
        code: &'static DiagnosticInfo,
        message: String,
        object: Option<&DgsObject>,
    ) -> Diagnostic {
        let record = Diagnostic::of(code, message);
        match (object, &self.spans) {
            (Some(object), Some(spans)) => SourceSpan::new(
                spans.source.clone(),
                spans.offset + object.byte_start as u64,
                spans.offset + object.byte_end as u64,
            )
            .ok()
            .and_then(|span| record.clone().with_span(span).ok())
            .unwrap_or(record),
            _ => record,
        }
    }

    fn warn(
        &self,
        warnings: &mut Diagnostics,
        code: &'static DiagnosticInfo,
        message: String,
        object: Option<&DgsObject>,
    ) {
        warnings.record(self.diagnostic(code, message, object));
    }

    fn kv(&self, bus: BusId) -> f64 {
        self.bus_kv.get(&bus).copied().unwrap_or(0.0)
    }

    /// The connections of `element`, in side order.
    fn connections(&self, element: &DgsObject) -> &[Connection] {
        self.connections
            .get(&element.id)
            .map_or(&[], Vec::as_slice)
    }

    /// The AC bus at each connection of `element`, when every connection
    /// lands on an AC terminal.
    fn buses(&self, element: &DgsObject) -> Option<Vec<(BusId, bool)>> {
        self.connections(element)
            .iter()
            .map(|connection| {
                self.terminal_bus
                    .get(&connection.terminal)
                    .map(|bus| (*bus, connection.closed))
            })
            .collect()
    }

    fn out_of_service(element: &DgsObject) -> bool {
        element.int("outserv") == Some(1)
    }
}

fn label(object: &DgsObject) -> String {
    format!("{} `{}`", object.class(), object.name())
}

/// The balanced network of a decoded document.
///
/// # Errors
/// [`Error::FormatRead`] when the export states no usable terminal or an
/// element references a bus the mapping did not build.
#[allow(clippy::too_many_lines)]
pub(crate) fn build_balanced(
    doc: &DgsDocument,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
    spans: Option<SpanContext>,
) -> Result<BalancedNetwork> {
    let net = doc.of_class("ElmNet").next();
    let name = net
        .and_then(|net| net.str("loc_name"))
        .filter(|name| !name.is_empty())
        .or(name_hint)
        .unwrap_or("dgs")
        .to_owned();
    let mut context = Context {
        doc,
        frequency: DEFAULT_FREQUENCY,
        connections: HashMap::new(),
        terminal_bus: HashMap::new(),
        bus_kv: HashMap::new(),
        dc_terminals: HashSet::new(),
        dc_elements: HashSet::new(),
        spans,
    };
    match net.and_then(|net| net.real("frnom")) {
        Some(frequency) if frequency > 0.0 => context.frequency = frequency,
        _ => context.warn(
            warnings,
            &codes::READ_DGS_VALUE_DEFAULTED,
            format!("no ElmNet states a nominal frequency; {DEFAULT_FREQUENCY} Hz assumed"),
            net,
        ),
    }
    collect_connections(&mut context, warnings);
    let hvdc_islands = classify_dc(&mut context, warnings);

    let mut buses = build_buses(&mut context, warnings)?;
    let switches = build_couplers(&context, warnings);
    let mut loads = Vec::new();
    let mut generators = Vec::new();
    for class in ["ElmLod", "ElmLodmv", "ElmLodlv", "ElmLodlvp"] {
        for load in doc.of_class(class) {
            if let Some((record, generator)) = read_load(&context, load, warnings) {
                loads.push(record);
                generators.extend(generator);
            }
        }
    }
    let mut shunts = Vec::new();
    for shunt in doc.of_class("ElmShnt") {
        shunts.extend(read_shunt(&context, shunt, warnings));
    }
    let mut reference = Vec::new();
    for class in ["ElmSym", "ElmGenstat", "ElmPvsys", "ElmAsm"] {
        for machine in doc.of_class(class) {
            if let Some((generator, slack)) = read_machine(&context, machine, warnings) {
                if slack {
                    reference.push(generator.bus);
                }
                generators.push(generator);
            }
        }
    }
    for grid in doc.of_class("ElmXnet") {
        if let Some((generator, slack)) = read_external_grid(&context, grid, warnings) {
            if slack {
                reference.push(generator.bus);
            }
            generators.push(generator);
        }
    }
    let mut branches = Vec::new();
    let mut tower_lines = HashSet::new();
    for tower in doc.of_class("ElmTow") {
        branches.extend(read_tower(&context, tower, &mut tower_lines, warnings));
    }
    for line in doc.of_class("ElmLne") {
        if context.dc_elements.contains(&line.id) || tower_lines.contains(&line.id) {
            continue;
        }
        branches.extend(read_line(&context, line, warnings));
    }
    for impedance in doc.of_class("ElmZpu") {
        branches.extend(read_common_impedance(&context, impedance, warnings));
    }
    for transformer in doc.of_class("ElmTr2") {
        branches.extend(read_two_winding(&context, transformer, warnings));
    }
    let mut transformers_3w = Vec::new();
    for transformer in doc.of_class("ElmTr3") {
        transformers_3w.extend(read_three_winding(&context, transformer, warnings));
    }
    let hvdc = hvdc_islands
        .into_iter()
        .filter_map(|island| read_hvdc(&context, &island, warnings))
        .collect::<Vec<_>>();

    assign_bus_kinds(&context, &mut buses, &generators, &reference, warnings);
    report_unmapped_classes(&context, warnings);

    let geo = super::super::geographic_meta(&buses);
    let net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name,
        base_mva: DEFAULT_BASE_MVA,
        base_frequency: context.frequency,
        geo,
        case_metadata: Default::default(),
        detailed_connectivity: None,
        buses: buses.into(),
        loads: loads.into(),
        shunts: shunts.into(),
        static_var_compensators: Vec::new().into(),
        branches: branches.into(),
        switches: switches.into(),
        generators: generators.into(),
        storage: Vec::new().into(),
        hvdc: hvdc.into(),
        transformers_3w: transformers_3w.into(),
        areas: Vec::new().into(),
        solver: None,
        source_format: SourceFormat::Dgs,
    });
    net.check_references(FMT)?;
    Ok(net)
}

/// Every cubicle: which element it connects to which terminal, on which
/// side, and whether its switch is closed. A cubicle without a switch is a
/// closed connection.
fn collect_connections(context: &mut Context<'_>, warnings: &mut Diagnostics) {
    let doc = context.doc;
    let mut dangling = 0usize;
    for cubicle in doc.of_class("StaCubic") {
        let Some(terminal) = doc.parent(cubicle).filter(|parent| parent.class() == "ElmTerm")
        else {
            context.warn(
                warnings,
                &codes::READ_DGS_REFERENCE_DROPPED,
                format!(
                    "{} names no ElmTerm terminal in `fold_id`; the connection was dropped",
                    label(cubicle)
                ),
                Some(cubicle),
            );
            continue;
        };
        let Some(element) = doc.referenced(cubicle, "obj_id") else {
            dangling += 1;
            continue;
        };
        let mut closed = true;
        for switch in doc.children_of_class(cubicle.id, "StaSwitch") {
            // PowSybl reads an absent `on_off` as open.
            if switch.int("on_off").unwrap_or(0) == 0 {
                closed = false;
            }
        }
        context
            .connections
            .entry(element.id)
            .or_default()
            .push(Connection {
                terminal: terminal.id,
                side: cubicle.int("obj_bus").unwrap_or(0),
                closed,
            });
    }
    if dangling > 0 {
        context.warn(
            warnings,
            &codes::READ_DGS_REFERENCE_DROPPED,
            format!("{dangling} StaCubic cubicle(s) name no element in `obj_id`"),
            None,
        );
    }
    for connections in context.connections.values_mut() {
        connections.sort_by_key(|connection| connection.side);
    }
}

/// One DC island: the terminals and elements it contains and the converters
/// at its edge, in document order.
struct DcIsland {
    terminals: Vec<i64>,
    lines: Vec<i64>,
    converters: Vec<i64>,
}

/// Mark the DC side of every `ElmVsc` and walk the DC network from there.
/// The converter's side 0 cubicle is its AC terminal; the other sides are DC.
fn classify_dc(context: &mut Context<'_>, warnings: &mut Diagnostics) -> Vec<DcIsland> {
    let doc = context.doc;
    let mut seeds = Vec::new();
    for converter in doc.of_class("ElmVsc") {
        for connection in context.connections(converter) {
            if connection.side >= 1 {
                seeds.push(connection.terminal);
            }
        }
    }
    if seeds.is_empty() {
        return Vec::new();
    }
    // Two terminal elements by terminal, for the walk.
    let mut adjacency: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
    for class in ["ElmLne", "ElmCoup", "ElmZpu"] {
        for element in doc.of_class(class) {
            let connections = context.connections(element);
            if let [a, b] = connections {
                adjacency
                    .entry(a.terminal)
                    .or_default()
                    .push((b.terminal, element.id));
                adjacency
                    .entry(b.terminal)
                    .or_default()
                    .push((a.terminal, element.id));
            }
        }
    }
    let mut islands = Vec::new();
    let mut seen = HashSet::new();
    for seed in seeds {
        if !seen.insert(seed) {
            continue;
        }
        let mut terminals = vec![seed];
        let mut lines = HashSet::new();
        let mut stack = vec![seed];
        while let Some(terminal) = stack.pop() {
            for (other, element) in adjacency.get(&terminal).into_iter().flatten() {
                lines.insert(*element);
                if seen.insert(*other) {
                    terminals.push(*other);
                    stack.push(*other);
                }
            }
        }
        terminals.sort_unstable();
        let mut lines = lines.into_iter().collect::<Vec<_>>();
        lines.sort_unstable();
        let converters = doc
            .of_class("ElmVsc")
            .filter(|converter| {
                context
                    .connections(converter)
                    .iter()
                    .any(|connection| connection.side >= 1 && terminals.contains(&connection.terminal))
            })
            .map(|converter| converter.id)
            .collect();
        context.dc_terminals.extend(terminals.iter().copied());
        context.dc_elements.extend(lines.iter().copied());
        islands.push(DcIsland {
            terminals,
            lines,
            converters,
        });
    }
    // The positive and negative poles of one link are separate DC networks
    // between the same converters; they form one HVDC record.
    let mut merged: Vec<DcIsland> = Vec::new();
    for island in islands {
        match merged
            .iter_mut()
            .find(|other| other.converters == island.converters)
        {
            Some(other) => {
                other.terminals.extend(island.terminals);
                other.lines.extend(island.lines);
                other.terminals.sort_unstable();
                other.lines.sort_unstable();
            }
            None => merged.push(island),
        }
    }
    let islands = merged;
    if !islands.is_empty() {
        let terminals = context.dc_terminals.len();
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_COLLAPSED,
            format!(
                "{terminals} DC terminal(s) behind ElmVsc converters are represented by HVDC \
                 records rather than as buses"
            ),
            None,
        );
    }
    islands
}

/// One bus per group of AC terminals joined by a closed, in service `ElmCoup`.
fn build_buses(context: &mut Context<'_>, warnings: &mut Diagnostics) -> Result<Vec<Bus>> {
    let doc = context.doc;
    let terminals = doc
        .of_class("ElmTerm")
        .filter(|terminal| !context.dc_terminals.contains(&terminal.id))
        .collect::<Vec<_>>();
    if terminals.is_empty() {
        return Err(Error::FormatRead {
            format: FMT,
            message: "the export declares no AC ElmTerm terminal".into(),
        });
    }
    let mut parent: HashMap<i64, i64> = terminals.iter().map(|t| (t.id, t.id)).collect();
    fn find(parent: &mut HashMap<i64, i64>, id: i64) -> i64 {
        let mut root = id;
        while parent[&root] != root {
            root = parent[&root];
        }
        let mut cursor = id;
        while parent[&cursor] != root {
            let next = parent[&cursor];
            parent.insert(cursor, root);
            cursor = next;
        }
        root
    }
    for coupler in doc.of_class("ElmCoup") {
        if context.dc_elements.contains(&coupler.id) || !coupler_closed(context, coupler, warnings) {
            continue;
        }
        let ends = context.connections(coupler);
        let [a, b] = ends else {
            continue;
        };
        if !parent.contains_key(&a.terminal) || !parent.contains_key(&b.terminal) {
            continue;
        }
        let (ra, rb) = (find(&mut parent, a.terminal), find(&mut parent, b.terminal));
        if ra != rb {
            let (keep, drop) = if ra < rb { (ra, rb) } else { (rb, ra) };
            parent.insert(drop, keep);
        }
    }
    let mut groups: BTreeMap<i64, Vec<&DgsObject>> = BTreeMap::new();
    for terminal in &terminals {
        let root = find(&mut parent, terminal.id);
        groups.entry(root).or_default().push(terminal);
    }
    let mut buses = Vec::with_capacity(groups.len());
    for (root, members) in groups {
        let lead = members
            .iter()
            .copied()
            .find(|terminal| terminal.id == root)
            .unwrap_or(members[0]);
        let id = BusId::new(usize::try_from(root).map_err(|_| Error::FormatRead {
            format: FMT,
            message: format!("ElmTerm id {root} is negative"),
        })?);
        let kv = lead.real("uknom").unwrap_or(0.0);
        if kv <= 0.0 {
            context.warn(
                warnings,
                &codes::READ_DGS_VALUE_DEFAULTED,
                format!(
                    "{} states no positive nominal voltage `uknom`; per unit conversion \
                     for its equipment uses a zero base",
                    label(lead)
                ),
                Some(lead),
            );
        }
        for member in members.iter().skip(1) {
            if member.real("uknom").is_some_and(|other| (other - kv).abs() > 1e-9) {
                context.warn(
                    warnings,
                    &codes::READ_DGS_VALUE_COLLAPSED,
                    format!(
                        "{} is joined to {} by a closed switch but states a different `uknom`; \
                         the calculated bus keeps {kv} kV",
                        label(member),
                        label(lead)
                    ),
                    Some(member),
                );
            }
        }
        let mut bus = Bus::new(id, BusType::Pq, kv);
        bus.name = Some(lead.name());
        if let (Some(u), Some(phi)) = (lead.real("m:u"), lead.real("m:phiu"))
            && u >= 0.0
        {
            bus.vm = u;
            bus.va = phi;
        }
        if let Some(vmax) = lead.real("vmax").filter(|v| *v > 0.0) {
            bus.vmax = vmax;
        }
        if let Some(vmin) = lead.real("vmin").filter(|v| *v > 0.0) {
            bus.vmin = vmin;
        }
        if let (Some(lat), Some(lon)) = (lead.real("GPSlat"), lead.real("GPSlon"))
            && lat.is_finite()
            && lon.is_finite()
            && (lat != 0.0 || lon != 0.0)
        {
            bus.location = Some(crate::geo::Location {
                x: lon,
                y: lat,
                kind: None,
            });
        }
        bus.extras
            .insert("dgs.class".into(), "ElmTerm".into());
        if members.len() > 1 {
            bus.extras.insert(
                "dgs.terminals".into(),
                members.iter().map(|m| m.id).collect::<Vec<_>>().into(),
            );
        }
        for member in &members {
            context.terminal_bus.insert(member.id, id);
        }
        context.bus_kv.insert(id, kv);
        buses.push(bus);
    }
    Ok(buses)
}

/// Whether an `ElmCoup` joins its terminals: in service, closed itself, and
/// closed at both cubicles.
fn coupler_closed(context: &Context<'_>, coupler: &DgsObject, warnings: &mut Diagnostics) -> bool {
    if Context::out_of_service(coupler) {
        return false;
    }
    let own = match coupler.int("on_off") {
        Some(state) => state != 0,
        None => {
            context.warn(
                warnings,
                &codes::READ_DGS_VALUE_DEFAULTED,
                format!(
                    "{} states no `on_off`; the switch is read as closed",
                    label(coupler)
                ),
                Some(coupler),
            );
            true
        }
    };
    own && context
        .connections(coupler)
        .iter()
        .all(|connection| connection.closed)
}

/// Open AC couplers between two calculated buses become switch records.
fn build_couplers(context: &Context<'_>, warnings: &mut Diagnostics) -> Vec<Switch> {
    let mut switches = Vec::new();
    for coupler in context.doc.of_class("ElmCoup") {
        if context.dc_elements.contains(&coupler.id) {
            continue;
        }
        let Some(ends) = context.buses(coupler) else {
            continue;
        };
        let [(from, _), (to, _)] = ends[..] else {
            context.warn(
                warnings,
                &codes::READ_DGS_RECORD_UNMAPPED,
                format!(
                    "{} connects {} terminal(s) rather than two; the switch was dropped",
                    label(coupler),
                    ends.len()
                ),
                Some(coupler),
            );
            continue;
        };
        if from == to {
            continue;
        }
        let mut switch = Switch::new(from, to, false);
        switch.uid = Some(uid(context, coupler));
        switch.extras.insert("dgs.class".into(), "ElmCoup".into());
        switch.extras.insert("dgs.id".into(), coupler.id.into());
        if let Some(usage) = coupler.str("aUsage") {
            switch.extras.insert("dgs.aUsage".into(), usage.into());
        }
        switches.push(switch);
    }
    switches
}

/// A row identity: the `loc_name`, or `loc_name#id` when the name repeats
/// within its class.
fn uid(context: &Context<'_>, object: &DgsObject) -> String {
    let name = object.name();
    let repeated = context
        .doc
        .of_class(object.class())
        .filter(|other| other.name() == name)
        .nth(1)
        .is_some();
    if repeated {
        format!("{name}#{}", object.id)
    } else {
        name
    }
}

fn element_extras(object: &DgsObject) -> Extras {
    let mut extras = Extras::new();
    extras.insert("dgs.class".into(), object.class().into());
    extras.insert("dgs.id".into(), object.id.into());
    extras.insert("dgs.name".into(), object.name().into());
    extras
}

/// The single AC bus of a one terminal element and whether its cubicle
/// switch is closed.
fn single_bus(
    context: &Context<'_>,
    element: &DgsObject,
    warnings: &mut Diagnostics,
) -> Option<(BusId, bool)> {
    let connections = context.connections(element);
    let [connection] = connections else {
        context.warn(
            warnings,
            &codes::READ_DGS_RECORD_UNMAPPED,
            format!(
                "{} connects to {} terminal(s) rather than one; the element was dropped",
                label(element),
                connections.len()
            ),
            Some(element),
        );
        return None;
    };
    match context.terminal_bus.get(&connection.terminal) {
        Some(bus) => Some((*bus, connection.closed)),
        None => {
            context.warn(
                warnings,
                &codes::READ_DGS_RECORD_UNMAPPED,
                format!(
                    "{} connects to a DC terminal; the element was dropped",
                    label(element)
                ),
                Some(element),
            );
            None
        }
    }
}

/// The two AC buses of a two terminal element in side order, and whether
/// both cubicle switches are closed.
fn branch_buses(
    context: &Context<'_>,
    element: &DgsObject,
    warnings: &mut Diagnostics,
) -> Option<(BusId, BusId, bool)> {
    let Some(ends) = context.buses(element) else {
        context.warn(
            warnings,
            &codes::READ_DGS_RECORD_UNMAPPED,
            format!(
                "{} connects to a DC terminal; the element was dropped",
                label(element)
            ),
            Some(element),
        );
        return None;
    };
    let [(from, closed_from), (to, closed_to)] = ends[..] else {
        context.warn(
            warnings,
            &codes::READ_DGS_RECORD_UNMAPPED,
            format!(
                "{} connects to {} terminal(s) rather than two; the element was dropped",
                label(element),
                ends.len()
            ),
            Some(element),
        );
        return None;
    };
    let closed = closed_from && closed_to;
    if !closed && (closed_from || closed_to) {
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_COLLAPSED,
            format!(
                "{} is open at one end only; the balanced branch is out of service at both",
                label(element)
            ),
            Some(element),
        );
    }
    Some((from, to, closed))
}

/// PowerFactory's `mode_inp` pair: the two stated quantities that define
/// active and reactive power. Returns `None` when no pair is stated.
fn power_from_mode(
    mode: Option<&str>,
    p: Option<f64>,
    q: Option<f64>,
    s: Option<f64>,
    cos_phi: Option<f64>,
) -> Option<(f64, f64)> {
    let from_p_s = |p: f64, s: f64| (s * s - p * p >= 0.0).then(|| (p, (s * s - p * p).sqrt()));
    let from_q_s = |q: f64, s: f64| (s * s - q * q >= 0.0).then(|| ((s * s - q * q).sqrt(), q));
    let from_p_cos = |p: f64, cos_phi: f64| {
        let disc = 1.0 - cos_phi * cos_phi;
        (disc >= 0.0 && cos_phi != 0.0).then(|| (p, p * disc.sqrt() / cos_phi))
    };
    let from_q_cos = |q: f64, cos_phi: f64| {
        let disc = 1.0 - cos_phi * cos_phi;
        (disc > 0.0).then(|| (q * cos_phi / disc.sqrt(), q))
    };
    let from_s_cos = |s: f64, cos_phi: f64| {
        let disc = 1.0 - cos_phi * cos_phi;
        (disc >= 0.0).then(|| (s * cos_phi, s * disc.sqrt()))
    };
    let stated = match mode.map(str::trim) {
        Some("PQ") => p.zip(q),
        Some("SP") => p.zip(s).and_then(|(p, s)| from_p_s(p, s)),
        Some("SQ") => q.zip(s).and_then(|(q, s)| from_q_s(q, s)),
        Some("PC") => p.zip(cos_phi).and_then(|(p, c)| from_p_cos(p, c)),
        Some("QC") => q.zip(cos_phi).and_then(|(q, c)| from_q_cos(q, c)),
        Some("SC") => s.zip(cos_phi).and_then(|(s, c)| from_s_cos(s, c)),
        _ => None,
    };
    if stated.is_some() {
        return stated;
    }
    p.zip(q)
        .or_else(|| p.zip(s).and_then(|(p, s)| from_p_s(p, s)))
        .or_else(|| q.zip(s).and_then(|(q, s)| from_q_s(q, s)))
        .or_else(|| p.zip(cos_phi).and_then(|(p, c)| from_p_cos(p, c)))
        .or_else(|| q.zip(cos_phi).and_then(|(q, c)| from_q_cos(q, c)))
        .or_else(|| s.zip(cos_phi).and_then(|(s, c)| from_s_cos(s, c)))
}

/// A derived active power takes the sign of the stated reactive power, as
/// the PowSybl converter does.
fn signed_power(p_stated: Option<f64>, q_stated: Option<f64>, (p, q): (f64, f64)) -> (f64, f64) {
    let sign = match (p_stated, q_stated) {
        (None, Some(q)) => q.signum(),
        _ => 1.0,
    };
    (p * sign, q)
}

fn read_load(
    context: &Context<'_>,
    load: &DgsObject,
    warnings: &mut Diagnostics,
) -> Option<(Load, Option<Generator>)> {
    let (bus, closed) = single_bus(context, load, warnings)?;
    let p = load.real("plini");
    let q = load.real("qlini");
    let (mut p_mw, mut q_mvar) = match power_from_mode(
        load.str("mode_inp"),
        p,
        q,
        load.real("slini"),
        load.real("coslini"),
    ) {
        Some(pair) => signed_power(p, q, pair),
        None => {
            context.warn(
                warnings,
                &codes::READ_DGS_VALUE_DEFAULTED,
                format!(
                    "{} states no pair of quantities that determines its demand; zero demand assumed",
                    label(load)
                ),
                Some(load),
            );
            (0.0, 0.0)
        }
    };
    if let Some(scale) = load.real("scale0").filter(|scale| *scale != 1.0) {
        p_mw *= scale;
        q_mvar *= scale;
    }
    let mut record = Load::new(bus, p_mw, q_mvar);
    record.in_service = closed && !Context::out_of_service(load);
    record.uid = Some(uid(context, load));
    record.extras = element_extras(load);
    report_load_voltage_dependence(context, load, warnings);
    let generator = if load.class() == "ElmLodmv" {
        read_load_generation(context, load, bus, record.in_service)
    } else {
        None
    };
    Some((record, generator))
}

/// The generation an `ElmLodmv` medium voltage load states beside its demand.
fn read_load_generation(
    context: &Context<'_>,
    load: &DgsObject,
    bus: BusId,
    in_service: bool,
) -> Option<Generator> {
    let p = load.real("pgini");
    let q = load.real("qgini");
    let (p_mw, q_mvar) = signed_power(
        p,
        q,
        power_from_mode(None, p, q, load.real("sgini"), load.real("cosgini"))?,
    );
    let mut generator = Generator::new(bus);
    generator.pg = p_mw;
    generator.qg = q_mvar;
    generator.pmin = p_mw;
    generator.pmax = p_mw;
    generator.qmin = q_mvar;
    generator.qmax = q_mvar;
    generator.mbase = DEFAULT_BASE_MVA;
    generator.voltage_regulation_on = false;
    generator.in_service = in_service;
    generator.uid = Some(format!("{}-G", uid(context, load)));
    Some(generator)
}

/// PowerFactory's load type voltage dependence `aP v^kpu0 + bP v^kpu1 +
/// (1 - aP - bP) v^kpu` has no balanced spelling; report a type that is not
/// constant power.
fn report_load_voltage_dependence(
    context: &Context<'_>,
    load: &DgsObject,
    warnings: &mut Diagnostics,
) {
    let Some(typ) = context.doc.referenced(load, "typ_id") else {
        return;
    };
    let constant = |a: &str, exponent: &str| {
        typ.real(a).unwrap_or(1.0) == 1.0 && typ.real(exponent).unwrap_or(0.0) == 0.0
    };
    if !constant("aP", "kpu0") || !constant("aQ", "kqu0") {
        context.warn(
            warnings,
            &codes::READ_DGS_FIELD_UNMAPPED,
            format!(
                "{} uses {} whose voltage dependence is not constant power; the balanced load \
                 is constant power",
                label(load),
                label(typ)
            ),
            Some(load),
        );
    }
}

fn read_shunt(
    context: &Context<'_>,
    shunt: &DgsObject,
    warnings: &mut Diagnostics,
) -> Option<Shunt> {
    let (bus, closed) = single_bus(context, shunt, warnings)?;
    let kv = context.kv(bus);
    // Siemens per section from the technology the export states.
    let (g_section, b_section) = match shunt.int("shtype") {
        Some(1) => {
            let r = shunt.real("rrea").unwrap_or(0.0);
            let x = shunt.real("xrea").unwrap_or(0.0);
            if r != 0.0 || x != 0.0 {
                let denominator = r * r + x * x;
                (r / denominator, -x / denominator)
            } else {
                match (shunt.real("ushnm"), shunt.real("qcapn")) {
                    (Some(u), Some(q)) if u > 0.0 => (0.0, -q / (u * u)),
                    _ => {
                        context.warn(
                            warnings,
                            &codes::READ_DGS_VALUE_UNSUPPORTED,
                            format!(
                                "{} is an R-L shunt without `rrea`/`xrea` or `ushnm`/`qcapn`; \
                                 the shunt was dropped",
                                label(shunt)
                            ),
                            Some(shunt),
                        );
                        return None;
                    }
                }
            }
        }
        Some(2) => (
            shunt.real("gparac").unwrap_or(0.0) * 1e-6,
            shunt.real("bcap").unwrap_or(0.0) * 1e-6,
        ),
        other => {
            context.warn(
                warnings,
                &codes::READ_DGS_VALUE_UNSUPPORTED,
                format!(
                    "{} states shunt type `shtype={}`; only R-L (1) and C (2) are read and the \
                     shunt was dropped",
                    label(shunt),
                    other.map_or_else(|| "absent".to_owned(), |code| code.to_string())
                ),
                Some(shunt),
            );
            return None;
        }
    };
    let sections = shunt.int("ncapa").unwrap_or(1).max(0);
    let max_sections = shunt.int("ncapx").unwrap_or(sections).max(sections);
    #[allow(clippy::cast_precision_loss)]
    let active = sections as f64;
    // Siemens times kV squared is MVAr at one per unit.
    let to_mva = kv * kv;
    let mut record = Shunt::new(bus, g_section * to_mva * active, b_section * to_mva * active);
    record.in_service = closed && !Context::out_of_service(shunt);
    record.section_count = Some(u32::try_from(sections).unwrap_or(u32::MAX));
    record.uid = Some(uid(context, shunt));
    record.extras = element_extras(shunt);
    record
        .extras
        .insert("dgs.ncapx".into(), max_sections.into());
    if shunt.int("iswitch") == Some(1) {
        let voltage = shunt.str("imldc").is_some_and(|mode| mode.trim() == "V");
        let (vhigh, vlow) = (
            shunt.real("usetp_mx").unwrap_or(1.1),
            shunt.real("usetp_mn").unwrap_or(0.9),
        );
        let steps = u32::try_from(max_sections).unwrap_or(u32::MAX);
        record.control = Some(SwitchedShuntControl::new(
            if voltage {
                SwitchedShuntMode::Discrete
            } else {
                SwitchedShuntMode::Locked
            },
            vhigh,
            vlow,
            vec![ShuntBlock::with_admittance(
                steps,
                g_section * to_mva,
                b_section * to_mva,
            )],
        ));
    }
    Some(record)
}

fn energy_source(category: Option<&str>) -> GeneratorEnergySource {
    let category = category.unwrap_or("").trim().to_ascii_lowercase();
    if category.contains("hydro") {
        GeneratorEnergySource::Hydro
    } else if category.contains("nuclear") {
        GeneratorEnergySource::Nuclear
    } else if category.contains("wind") {
        GeneratorEnergySource::Wind
    } else if category.contains("solar") || category.contains("photovoltaic") || category == "pv" {
        GeneratorEnergySource::Solar
    } else if ["coal", "gas", "oil", "diesel", "lignite", "peat", "biogas", "biomass", "waste"]
        .iter()
        .any(|fuel| category.contains(fuel))
    {
        GeneratorEnergySource::Thermal
    } else {
        GeneratorEnergySource::Other
    }
}

/// Reactive limits in MVAr: a capability curve collapses to its extreme
/// values; otherwise the element or type limits the `iqtype` selects.
fn reactive_limits(
    context: &Context<'_>,
    machine: &DgsObject,
    rated_mva: Option<f64>,
    warnings: &mut Diagnostics,
) -> Option<(f64, f64)> {
    if let Some(curve) = context.doc.referenced(machine, "pQlimType")
        && let (Some(q_min), Some(q_max)) = (curve.real_vec("cap_Qmn"), curve.real_vec("cap_Qmx"))
        && !q_min.is_empty()
        && q_min.len() == q_max.len()
    {
        let low = q_min.iter().copied().fold(f64::INFINITY, f64::min);
        let high = q_max.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_COLLAPSED,
            format!(
                "{} states the capability curve {}; the balanced generator keeps its \
                 widest reactive range {low} to {high} MVAr",
                label(machine),
                label(curve)
            ),
            Some(machine),
        );
        return Some((low, high));
    }
    let typ = context.doc.referenced(machine, "typ_id");
    let scale = rated_mva?;
    let from_element = machine
        .real("q_min")
        .zip(machine.real("q_max"))
        .map(|(low, high)| (low * scale, high * scale));
    let from_type = typ.and_then(|typ| {
        typ.real("Q_min")
            .zip(typ.real("Q_max"))
            .or_else(|| {
                typ.real("q_min")
                    .zip(typ.real("q_max"))
                    .map(|(low, high)| (low * scale, high * scale))
            })
    });
    match machine.int("iqtype") {
        Some(0) => from_element.or(from_type),
        Some(_) => from_type.or(from_element),
        None => from_element.or(from_type),
    }
}

/// A synchronous or static machine as a generator, and whether it is the
/// declared slack.
fn read_machine(
    context: &Context<'_>,
    machine: &DgsObject,
    warnings: &mut Diagnostics,
) -> Option<(Generator, bool)> {
    let (bus, closed) = single_bus(context, machine, warnings)?;
    let typ = context.doc.referenced(machine, "typ_id");
    #[allow(clippy::cast_precision_loss)]
    let units = machine.int("ngnum").filter(|n| *n > 0).unwrap_or(1) as f64;
    let rated = machine
        .real("sgn")
        .or_else(|| typ.and_then(|typ| typ.real("sgn")))
        .filter(|s| *s > 0.0)
        .map(|s| s * units);
    let mut generator = Generator::new(bus);
    generator.energy_source = energy_source(machine.str("cCategory"));
    generator.pg = machine
        .real("pgini_a")
        .or_else(|| machine.real("pgini"))
        .unwrap_or(0.0);
    generator.qg = machine
        .real("qgini_a")
        .or_else(|| machine.real("qgini"))
        .unwrap_or(0.0);
    generator.vg = machine.real("usetp").unwrap_or(1.0);
    generator.voltage_regulation_on = match machine.int("iv_mode") {
        Some(mode) => mode == 1,
        None => machine.str("av_mode").is_some_and(|mode| mode.trim() == "constv"),
    };
    generator.pmin = machine.real("Pmin_uc").unwrap_or(generator.pg.min(0.0));
    generator.pmax = machine.real("Pmax_uc").unwrap_or(generator.pg.max(0.0));
    match reactive_limits(context, machine, rated, warnings) {
        Some((low, high)) => {
            generator.qmin = low;
            generator.qmax = high;
        }
        None => {
            generator.qmin = generator.qg.min(0.0);
            generator.qmax = generator.qg.max(0.0);
            context.warn(
                warnings,
                &codes::READ_DGS_VALUE_DEFAULTED,
                format!(
                    "{} states no reactive limits; its reactive range is fixed at its set point",
                    label(machine)
                ),
                Some(machine),
            );
        }
    }
    generator.mbase = match rated {
        Some(rated) => rated,
        None => {
            context.warn(
                warnings,
                &codes::READ_DGS_VALUE_DEFAULTED,
                format!(
                    "{} states no rated apparent power `sgn`; {DEFAULT_BASE_MVA} MVA assumed",
                    label(machine)
                ),
                Some(machine),
            );
            DEFAULT_BASE_MVA
        }
    };
    generator.in_service = closed && !Context::out_of_service(machine);
    generator.uid = Some(uid(context, machine));
    let slack = machine.int("ip_ctrl") == Some(1)
        || machine.str("bustp").is_some_and(|kind| kind.trim() == "SL");
    Some((generator, slack))
}

/// An external grid as a generator: PQ, PV, or the slack by `bustp`.
fn read_external_grid(
    context: &Context<'_>,
    grid: &DgsObject,
    warnings: &mut Diagnostics,
) -> Option<(Generator, bool)> {
    let (bus, closed) = single_bus(context, grid, warnings)?;
    let kind = grid.str("bustp").map(str::trim).unwrap_or("SL");
    let p = grid.real("pgini");
    let q = grid.real("qgini");
    let (p_mw, q_mvar) = power_from_mode(
        grid.str("mode_inp"),
        p,
        q,
        grid.real("sgini"),
        grid.real("cosgini"),
    )
    .map_or((0.0, 0.0), |pair| signed_power(p, q, pair));
    let mut generator = Generator::new(bus);
    generator.pg = p_mw;
    generator.qg = q_mvar;
    generator.vg = grid.real("usetp").unwrap_or(1.0);
    generator.voltage_regulation_on = matches!(kind, "PV" | "SL");
    let mut limit = |name: &str, fallback: f64, what: &str| match grid.real(name) {
        Some(value) => value,
        None => {
            context.warn(
                warnings,
                &codes::READ_DGS_VALUE_DEFAULTED,
                format!("{} states no {what} `{name}`; {fallback} assumed", label(grid)),
                Some(grid),
            );
            fallback
        }
    };
    let pmax = limit("MaxS", DEFAULT_BASE_MVA * 100.0, "active power ceiling");
    generator.pmax = pmax;
    generator.pmin = limit("Pmin_uc", -pmax, "active power floor");
    generator.qmax = limit("cQ_max", DEFAULT_BASE_MVA * 100.0, "reactive power ceiling");
    generator.qmin = limit("cQ_min", -generator.qmax, "reactive power floor");
    generator.mbase = grid.real("snss").filter(|s| *s > 0.0).unwrap_or(DEFAULT_BASE_MVA);
    generator.in_service = closed && !Context::out_of_service(grid);
    generator.uid = Some(uid(context, grid));
    if !matches!(kind, "PQ" | "PV" | "SL") {
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_UNSUPPORTED,
            format!(
                "{} states bus type `bustp={kind}`; PQ, PV, and SL are read and the grid was \
                 read as a slack",
                label(grid)
            ),
            Some(grid),
        );
    }
    Some((generator, kind == "SL" || !matches!(kind, "PQ" | "PV")))
}

/// Series ohms and total shunt siemens of a line, to a balanced branch on
/// the PowerModels line convention for unequal terminal voltages.
fn line_branch(
    context: &Context<'_>,
    from: BusId,
    to: BusId,
    r_ohm: f64,
    x_ohm: f64,
    g_siemens: f64,
    b_siemens: f64,
) -> Branch {
    let (kv_from, kv_to) = (context.kv(from), context.kv(to));
    let scale = DEFAULT_BASE_MVA / (kv_from * kv_to);
    let mut branch = Branch::new(from, to, r_ohm * scale, x_ohm * scale);
    let denominator = r_ohm * r_ohm + x_ohm * x_ohm;
    let (y_real, y_imag) = if denominator == 0.0 {
        (0.0, 0.0)
    } else {
        (r_ohm / denominator, -x_ohm / denominator)
    };
    let convert = |shunt: f64, at: f64, other: f64, transmission: f64| {
        (shunt * at * at + (at - other) * at * transmission) / DEFAULT_BASE_MVA
    };
    let charging = BranchCharging::new(
        convert(g_siemens / 2.0, kv_from, kv_to, y_real),
        convert(b_siemens / 2.0, kv_from, kv_to, y_imag),
        convert(g_siemens / 2.0, kv_to, kv_from, y_real),
        convert(b_siemens / 2.0, kv_to, kv_from, y_imag),
    );
    branch.b = charging.calc_total_b();
    branch.charging = Some(charging);
    branch
}

/// Rated line current to an MVA rating at the from bus voltage.
fn line_rating(kv: f64, current_ka: Option<f64>, parallel: f64) -> f64 {
    current_ka
        .filter(|i| *i > 0.0 && *i < 99_999.0)
        .map_or(0.0, |i| 3f64.sqrt() * kv * i * parallel)
}

fn read_line(context: &Context<'_>, line: &DgsObject, warnings: &mut Diagnostics) -> Option<Branch> {
    let Some(typ) = context.doc.referenced(line, "typ_id") else {
        context.warn(
            warnings,
            &codes::READ_DGS_REFERENCE_DROPPED,
            format!(
                "{} names no TypLne line type in `typ_id` and belongs to no ElmTow tower; \
                 the line was dropped",
                label(line)
            ),
            Some(line),
        );
        return None;
    };
    let (from, to, closed) = branch_buses(context, line, warnings)?;
    let length = line.real("dline").unwrap_or(1.0);
    #[allow(clippy::cast_precision_loss)]
    let parallel = line.int("nlnum").filter(|n| *n >= 1).unwrap_or(1) as f64;
    let r = typ.real("rline").unwrap_or(0.0) * length / parallel;
    let x = typ.real("xline").unwrap_or(0.0) * length / parallel;
    let b_per_km = typ.real("bline").map_or_else(
        || {
            typ.real("cline").map_or(0.0, |c| {
                2.0 * std::f64::consts::PI
                    * typ.real("frnom").unwrap_or(context.frequency)
                    * c
                    * 1e-6
            })
        },
        |b| b * 1e-6,
    );
    let g_per_km = typ.real("gline").map_or_else(
        || {
            typ.real("tline")
                .zip(typ.real("bline"))
                .map_or(0.0, |(t, b)| b * t * 1e-6)
        },
        |g| g * 1e-6,
    );
    let mut branch = line_branch(
        context,
        from,
        to,
        r,
        x,
        g_per_km * length * parallel,
        b_per_km * length * parallel,
    );
    branch.rate_a = line_rating(context.kv(from), typ.real("sline"), parallel);
    branch.in_service = closed && !Context::out_of_service(line);
    branch.name = Some(line.name());
    branch.uid = Some(uid(context, line));
    branch.extras = element_extras(line);
    if let Some((rows, cols, data)) = line.matrix("GPScoords")
        && cols >= 2
        && rows >= 2
    {
        branch.route = Some(
            data.chunks(cols)
                .map(|point| crate::geo::Location {
                    x: point[1],
                    y: point[0],
                    kind: None,
                })
                .collect(),
        );
    }
    Some(branch)
}

/// The lines an `ElmTow` tower carries: each circuit takes the diagonal of
/// the tower's positive sequence circuit matrices.
fn read_tower(
    context: &Context<'_>,
    tower: &DgsObject,
    tower_lines: &mut HashSet<i64>,
    warnings: &mut Diagnostics,
) -> Vec<Branch> {
    let mut branches = Vec::new();
    let Some(typ) = tower
        .ref_vec("pGeo")
        .and_then(|refs| refs.first())
        .and_then(|key| context.doc.resolve(key))
    else {
        context.warn(
            warnings,
            &codes::READ_DGS_REFERENCE_DROPPED,
            format!(
                "{} names no TypTow tower type in `pGeo`; its lines were dropped",
                label(tower)
            ),
            Some(tower),
        );
        return branches;
    };
    let matrix = |name: &str| typ.matrix(name);
    let (Some(r), Some(x)) = (matrix("R_c1"), matrix("X_c1")) else {
        context.warn(
            warnings,
            &codes::READ_DGS_FIELD_UNMAPPED,
            format!(
                "{} states no positive sequence circuit matrices `R_c1`/`X_c1`; its lines \
                 were dropped",
                label(typ)
            ),
            Some(typ),
        );
        return branches;
    };
    let lines = tower.ref_vec("plines").unwrap_or(&[]);
    let diagonal = |(rows, cols, data): (usize, usize, &[f64]), circuit: usize| {
        (circuit < rows && circuit < cols)
            .then(|| data[circuit * cols + circuit])
            .unwrap_or(0.0)
    };
    let coupled = r.0 > 1 || x.0 > 1;
    for (circuit, key) in lines.iter().enumerate() {
        let Some(line) = context.doc.resolve(key) else {
            context.warn(
                warnings,
                &codes::READ_DGS_REFERENCE_DROPPED,
                format!("{} names a line in `plines` that the export lacks", label(tower)),
                Some(tower),
            );
            continue;
        };
        tower_lines.insert(line.id);
        let Some((from, to, closed)) = branch_buses(context, line, warnings) else {
            continue;
        };
        let length = line.real("dline").unwrap_or(1.0);
        let g = matrix("G_c1").map_or(0.0, |m| diagonal(m, circuit)) * 1e-6;
        let b = matrix("B_c1").map_or(0.0, |m| diagonal(m, circuit)) * 1e-6;
        let mut branch = line_branch(
            context,
            from,
            to,
            diagonal(r, circuit) * length,
            diagonal(x, circuit) * length,
            g * length,
            b * length,
        );
        branch.in_service = closed && !Context::out_of_service(line);
        branch.name = Some(line.name());
        branch.uid = Some(uid(context, line));
        branch.extras = element_extras(line);
        branch.extras.insert("dgs.tower".into(), tower.id.into());
        branches.push(branch);
    }
    if coupled && lines.len() > 1 {
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_COLLAPSED,
            format!(
                "{} couples {} circuits through the off diagonal entries of `R_c1`/`X_c1`; \
                 the balanced branches keep the diagonal self impedances only",
                label(tower),
                lines.len()
            ),
            Some(tower),
        );
    }
    branches
}

/// An `ElmZpu` common impedance: per unit on `Sn` at each terminal's own
/// nominal voltage, so the branch is a nominal ratio transformer with tap 1.
fn read_common_impedance(
    context: &Context<'_>,
    impedance: &DgsObject,
    warnings: &mut Diagnostics,
) -> Option<Branch> {
    let (from, to, closed) = branch_buses(context, impedance, warnings)?;
    let Some(sn) = impedance.real("Sn").filter(|s| *s > 0.0) else {
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_DEFAULTED,
            format!(
                "{} states no positive rated power `Sn`; the impedance was dropped",
                label(impedance)
            ),
            Some(impedance),
        );
        return None;
    };
    let scale = DEFAULT_BASE_MVA / sn;
    let r12 = impedance.real("r_pu").unwrap_or(0.0);
    let x12 = impedance.real("x_pu").unwrap_or(0.0);
    let r21 = impedance.real("r_pu_ji").unwrap_or(r12);
    let x21 = impedance.real("x_pu_ji").unwrap_or(x12);
    if (r12 - r21).abs() > 1e-12 || (x12 - x21).abs() > 1e-12 {
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_COLLAPSED,
            format!(
                "{} states different impedances in each direction; the balanced branch keeps \
                 their mean",
                label(impedance)
            ),
            Some(impedance),
        );
    }
    let mut branch = Branch::new(
        from,
        to,
        (r12 + r21) / 2.0 * scale,
        (x12 + x21) / 2.0 * scale,
    );
    let charging = BranchCharging::new(
        impedance.real("gi_pu").unwrap_or(0.0) / scale,
        impedance.real("bi_pu").unwrap_or(0.0) / scale,
        impedance.real("gj_pu").unwrap_or(0.0) / scale,
        impedance.real("bj_pu").unwrap_or(0.0) / scale,
    );
    branch.b = charging.calc_total_b();
    branch.charging = Some(charging);
    branch.rate_a = sn;
    if impedance.int("nphshift").is_some_and(|shift| shift != 0) {
        branch.shift = impedance.real("ag").unwrap_or(0.0);
    }
    branch.in_service = closed && !Context::out_of_service(impedance);
    branch.name = Some(impedance.name());
    branch.uid = Some(uid(context, impedance));
    branch.extras = element_extras(impedance);
    Some(branch)
}

/// Leakage impedance from the short circuit test, per unit on the
/// transformer rating: `uktr` percent and `pcutr` kW.
fn short_circuit_impedance(uk_percent: f64, copper_kw: f64, rated_mva: f64) -> (f64, f64) {
    let z = uk_percent / 100.0;
    let r = copper_kw / (1000.0 * rated_mva);
    let x = (z * z - r * r).max(0.0).sqrt() * z.signum();
    (r, x)
}

/// Magnetizing admittance from the open circuit test, per unit on the
/// rating: `curmg` percent and `pfe` kW. Inductive, so `b` is negative.
fn magnetizing_admittance(current_percent: f64, iron_kw: f64, rated_mva: f64) -> (f64, f64) {
    if current_percent == 0.0 {
        return (0.0, 0.0);
    }
    let y = current_percent / 100.0;
    let g = iron_kw / (1000.0 * rated_mva);
    let b = -(y * y - g * g).max(0.0).sqrt();
    (g, b)
}

/// One tap changer's ratio and angle at `position`, from the type's step
/// definition or from an explicit `mTaps` table on the element.
struct TapState {
    ratio: f64,
    angle: f64,
    position: i64,
    min: i64,
    max: i64,
    neutral: i64,
    step_percent: f64,
    step_angle: f64,
}

#[allow(clippy::too_many_arguments)]
fn tap_state(
    context: &Context<'_>,
    element: &DgsObject,
    typ: &DgsObject,
    position_attr: &str,
    neutral_attr: &str,
    min_attr: &str,
    max_attr: &str,
    step_attr: &str,
    angle_attr: &str,
    taps_table: Option<(usize, usize, &[f64])>,
    warnings: &mut Diagnostics,
) -> TapState {
    let neutral = typ.int(neutral_attr).unwrap_or(0);
    let min = typ.int(min_attr).unwrap_or(neutral);
    let max = typ.int(max_attr).unwrap_or(neutral);
    let stated = element.int(position_attr).unwrap_or(neutral);
    let position = stated.clamp(min.min(max), max.max(min));
    if position != stated {
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_SUBSTITUTED,
            format!(
                "{} states tap position {stated} outside {min}..{max}; position {position} used",
                label(element)
            ),
            Some(element),
        );
    }
    let step_percent = typ.real(step_attr).unwrap_or(0.0);
    let step_angle = typ.real(angle_attr).unwrap_or(0.0);
    #[allow(clippy::cast_precision_loss)]
    let offset = (position - neutral) as f64;
    let mut state = TapState {
        ratio: 1.0 + offset * step_percent / 100.0,
        angle: offset * step_angle,
        position,
        min,
        max,
        neutral,
        step_percent,
        step_angle,
    };
    if let Some((rows, cols, data)) = taps_table {
        let row = usize::try_from(position - min).ok();
        match row {
            Some(row) if row < rows && cols >= 2 => {
                let entry = &data[row * cols..(row + 1) * cols];
                state.angle = entry[1];
                if cols == 5 {
                    state.ratio = entry[4];
                } else {
                    state.ratio = 1.0;
                }
            }
            _ => context.warn(
                warnings,
                &codes::READ_DGS_VALUE_SUBSTITUTED,
                format!(
                    "{} states an `mTaps` table without a row for position {position}; the \
                     type's step definition was used",
                    label(element)
                ),
                Some(element),
            ),
        }
    }
    state
}

/// Automatic control of a tap changer: `ntrcn` enables it and `imldc`
/// selects the controlled quantity.
fn transformer_control(
    context: &Context<'_>,
    element: &DgsObject,
    tap: &TapState,
    rated_mva: f64,
    local_bus: BusId,
    warnings: &mut Diagnostics,
) -> Option<TransformerControl> {
    let automatic = element.int("ntrcn") == Some(1);
    let mode_text = element
        .str("imldc")
        .map(|mode| {
            mode.chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
        })
        .unwrap_or_default();
    let mode = match mode_text.as_str() {
        "V" | "" => TransformerControlMode::Voltage,
        "P" => TransformerControlMode::ActiveFlow,
        "Q" => TransformerControlMode::ReactiveFlow,
        other => {
            context.warn(
                warnings,
                &codes::READ_DGS_VALUE_UNSUPPORTED,
                format!(
                    "{} states tap control mode `imldc={other}`; V, P, and Q are read and the \
                     tap changer is fixed",
                    label(element)
                ),
                Some(element),
            );
            return None;
        }
    };
    if !automatic && tap.step_percent == 0.0 && tap.step_angle == 0.0 {
        return None;
    }
    let mut control = TransformerControl::new(if automatic {
        mode
    } else {
        TransformerControlMode::Fixed
    });
    control.enabled = automatic;
    control.mva_base = rated_mva;
    #[allow(clippy::cast_precision_loss)]
    let ratio_at = |position: i64| 1.0 + (position - tap.neutral) as f64 * tap.step_percent / 100.0;
    #[allow(clippy::cast_precision_loss)]
    let angle_at = |position: i64| (position - tap.neutral) as f64 * tap.step_angle;
    if mode == TransformerControlMode::ActiveFlow {
        control.tap_min = angle_at(tap.min).min(angle_at(tap.max));
        control.tap_max = angle_at(tap.min).max(angle_at(tap.max));
    } else {
        control.tap_min = ratio_at(tap.min).min(ratio_at(tap.max));
        control.tap_max = ratio_at(tap.min).max(ratio_at(tap.max));
    }
    control.ntp = u32::try_from(tap.max - tap.min + 1).unwrap_or(1).max(1);
    let (low, high) = match mode {
        TransformerControlMode::ActiveFlow => (element.real("psp_low"), element.real("psp_up")),
        TransformerControlMode::ReactiveFlow => (element.real("qsp_low"), element.real("qsp_up")),
        _ => (element.real("usp_low"), element.real("usp_up")),
    };
    if let (Some(low), Some(high)) = (low, high) {
        control.band_min = low;
        control.band_max = high;
    } else if let Some(target) = element.real(match mode {
        TransformerControlMode::ActiveFlow => "psetp",
        TransformerControlMode::ReactiveFlow => "qsetp",
        _ => "usetp",
    }) {
        control.band_min = target;
        control.band_max = target;
    }
    if let Some(tapctrl) = context.doc.referenced(element, "tapctrl") {
        if let (Some(low), Some(high)) = (tapctrl.real("usetp_mn"), tapctrl.real("usetp_mx")) {
            control.band_min = low;
            control.band_max = high;
        }
        control.enabled = tapctrl.int("isAutoTap") == Some(1);
        control.controlled_bus = context
            .doc
            .referenced(tapctrl, "rembar")
            .and_then(|terminal| context.terminal_bus.get(&terminal.id).copied());
    } else if element.int("i_rem") == Some(1) {
        control.controlled_bus = context
            .doc
            .referenced(element, "p_rem")
            .and_then(|terminal| context.terminal_bus.get(&terminal.id).copied());
    }
    if control.controlled_bus == Some(local_bus) {
        control.controlled_bus = None;
    }
    Some(control)
}

/// A two winding transformer: from the HV side (cubicle side 0) to the LV
/// side, impedance referred to the LV bus base, tap on the HV side.
#[allow(clippy::too_many_lines)]
fn read_two_winding(
    context: &Context<'_>,
    transformer: &DgsObject,
    warnings: &mut Diagnostics,
) -> Option<Branch> {
    let Some(typ) = context.doc.referenced(transformer, "typ_id") else {
        context.warn(
            warnings,
            &codes::READ_DGS_REFERENCE_DROPPED,
            format!(
                "{} names no TypTr2 transformer type in `typ_id`; the transformer was dropped",
                label(transformer)
            ),
            Some(transformer),
        );
        return None;
    };
    let (from, to, closed) = branch_buses(context, transformer, warnings)?;
    let Some(rated_mva) = typ.real("strn").filter(|s| *s > 0.0) else {
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_DEFAULTED,
            format!(
                "{} states no positive rated power `strn`; the transformer was dropped",
                label(typ)
            ),
            Some(typ),
        );
        return None;
    };
    let (kv_h, kv_l) = (context.kv(from), context.kv(to));
    let rated_h = typ.real("utrn_h").filter(|u| *u > 0.0).unwrap_or(kv_h);
    let rated_l = typ.real("utrn_l").filter(|u| *u > 0.0).unwrap_or(kv_l);
    // Per unit on the LV bus base: the rating base scaled by the LV winding's
    // rated to nominal voltage ratio.
    let base_scale = DEFAULT_BASE_MVA / rated_mva
        * if kv_l > 0.0 {
            (rated_l / kv_l).powi(2)
        } else {
            1.0
        };
    let (r, x) = short_circuit_impedance(
        typ.real("uktr").unwrap_or(0.0),
        typ.real("pcutr").unwrap_or(0.0),
        rated_mva,
    );
    let (g, b) = magnetizing_admittance(
        typ.real("curmg").unwrap_or(0.0),
        typ.real("pfe").unwrap_or(0.0),
        rated_mva,
    );
    let tap = tap_state(
        context,
        transformer,
        typ,
        "nntap",
        "nntap0",
        "ntpmn",
        "ntpmx",
        "dutap",
        "phitr",
        transformer.matrix("mTaps"),
        warnings,
    );
    // `tap_side` 0 puts the tap changer on the HV winding, 1 on the LV.
    let tap_on_hv = typ.int("tap_side").unwrap_or(0) == 0;
    let (turns_h, turns_l) = if tap_on_hv {
        (rated_h * tap.ratio, rated_l)
    } else {
        (rated_h, rated_l * tap.ratio)
    };
    let ratio = if kv_h > 0.0 && kv_l > 0.0 {
        (turns_h / kv_h) / (turns_l / kv_l)
    } else {
        1.0
    };
    let mut branch = Branch::new(from, to, r * base_scale, x * base_scale);
    branch.tap = if ratio.is_finite() && ratio > 0.0 {
        ratio
    } else {
        1.0
    };
    branch.shift = if tap_on_hv { tap.angle } else { -tap.angle };
    let charging = BranchCharging::new(0.0, 0.0, g / base_scale, b / base_scale);
    branch.b = charging.calc_total_b();
    branch.charging = Some(charging);
    branch.rate_a = rated_mva;
    branch.in_service = closed && !Context::out_of_service(transformer);
    branch.name = Some(transformer.name());
    branch.uid = Some(uid(context, transformer));
    branch.extras = element_extras(transformer);
    branch
        .extras
        .insert("dgs.tap_position".into(), tap.position.into());
    let local = match transformer.int("t2ldc") {
        Some(1) => to,
        _ => from,
    };
    branch.control = transformer_control(context, transformer, &tap, rated_mva, local, warnings);
    if let Some(clock) = typ.real("nt2ag").filter(|clock| *clock != 0.0) {
        branch.extras.insert("dgs.nt2ag".into(), clock.into());
        context.warn(
            warnings,
            &codes::READ_DGS_FIELD_UNMAPPED,
            format!(
                "{} states vector group clock `nt2ag={clock}`; the balanced branch keeps it \
                 in `extras` and applies no vector group phase shift",
                label(typ)
            ),
            Some(typ),
        );
    }
    Some(branch)
}

/// A three winding transformer: HV, MV, and LV windings from cubicle sides
/// 0, 1, and 2; pairwise impedances on the smaller rating of each pair.
#[allow(clippy::too_many_lines)]
fn read_three_winding(
    context: &Context<'_>,
    transformer: &DgsObject,
    warnings: &mut Diagnostics,
) -> Option<Transformer3W> {
    let Some(typ) = context.doc.referenced(transformer, "typ_id") else {
        context.warn(
            warnings,
            &codes::READ_DGS_REFERENCE_DROPPED,
            format!(
                "{} names no TypTr3 transformer type in `typ_id`; the transformer was dropped",
                label(transformer)
            ),
            Some(transformer),
        );
        return None;
    };
    let Some(ends) = context.buses(transformer) else {
        context.warn(
            warnings,
            &codes::READ_DGS_RECORD_UNMAPPED,
            format!(
                "{} connects to a DC terminal; the transformer was dropped",
                label(transformer)
            ),
            Some(transformer),
        );
        return None;
    };
    let [(bus_h, closed_h), (bus_m, closed_m), (bus_l, closed_l)] = ends[..] else {
        context.warn(
            warnings,
            &codes::READ_DGS_RECORD_UNMAPPED,
            format!(
                "{} connects to {} terminal(s) rather than three; the transformer was dropped",
                label(transformer),
                ends.len()
            ),
            Some(transformer),
        );
        return None;
    };
    let closed = closed_h && closed_m && closed_l;
    if !closed && (closed_h || closed_m || closed_l) {
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_COLLAPSED,
            format!(
                "{} is open at some windings only; the transformer is out of service",
                label(transformer)
            ),
            Some(transformer),
        );
    }
    let rating = |name: &str| typ.real(name).filter(|s| *s > 0.0);
    let (Some(s_h), Some(s_m), Some(s_l)) =
        (rating("strn3_h"), rating("strn3_m"), rating("strn3_l"))
    else {
        context.warn(
            warnings,
            &codes::READ_DGS_VALUE_DEFAULTED,
            format!(
                "{} states no positive winding ratings `strn3_h/m/l`; the transformer was dropped",
                label(typ)
            ),
            Some(typ),
        );
        return None;
    };
    let pair = |uk: &str, pcu: &str, base: f64| {
        let (r, x) = short_circuit_impedance(
            typ.real(uk).unwrap_or(0.0),
            typ.real(pcu).unwrap_or(0.0),
            base,
        );
        Impedance::new(r * DEFAULT_BASE_MVA / base, x * DEFAULT_BASE_MVA / base, base)
    };
    let z = [
        pair("uktr3_h", "pcut3_h", s_h.min(s_m)),
        pair("uktr3_m", "pcut3_m", s_m.min(s_l)),
        pair("uktr3_l", "pcut3_l", s_l.min(s_h)),
    ];
    let taps_table = transformer.matrix("mTaps");
    let measured = transformer.int("iMeasTap").unwrap_or(0);
    let mut windings = Vec::with_capacity(3);
    for (index, (bus, rated_attr, rating, names)) in [
        (
            bus_h,
            "utrn3_h",
            s_h,
            ("n3tap_h", "n3tp0_h", "n3tmn_h", "n3tmx_h", "du3tp_h", "ph3tr_h"),
        ),
        (
            bus_m,
            "utrn3_m",
            s_m,
            ("n3tap_m", "n3tp0_m", "n3tmn_m", "n3tmx_m", "du3tp_m", "ph3tr_m"),
        ),
        (
            bus_l,
            "utrn3_l",
            s_l,
            ("n3tap_l", "n3tp0_l", "n3tmn_l", "n3tmx_l", "du3tp_l", "ph3tr_l"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let table = (i64::try_from(index).ok() == Some(measured)).then_some(taps_table).flatten();
        let tap = tap_state(
            context,
            transformer,
            typ,
            names.0,
            names.1,
            names.2,
            names.3,
            names.4,
            names.5,
            table,
            warnings,
        );
        let kv = context.kv(bus);
        let rated = typ.real(rated_attr).filter(|u| *u > 0.0).unwrap_or(kv);
        let mut winding = Winding::new(bus);
        winding.nominal_kv = rated;
        winding.tap = if kv > 0.0 { rated * tap.ratio / kv } else { tap.ratio };
        winding.shift = tap.angle;
        winding.rate_a = rating;
        if transformer.int("ictrlside") == Some(i64::try_from(index).unwrap_or(-1)) {
            winding.control =
                transformer_control(context, transformer, &tap, rating, bus, warnings);
        }
        windings.push(winding);
    }
    let Ok(windings): std::result::Result<[Winding; 3], _> = windings.try_into() else {
        return None;
    };
    let (g, b) = magnetizing_admittance(
        typ.real("curm3").unwrap_or(0.0),
        typ.real("pfe").unwrap_or(0.0),
        s_h,
    );
    let mut record = Transformer3W::new(windings, z);
    record.mag_g = g * s_h / DEFAULT_BASE_MVA;
    record.mag_b = b * s_h / DEFAULT_BASE_MVA;
    record.in_service = closed && !Context::out_of_service(transformer);
    record.name = Some(transformer.name());
    record.uid = Some(uid(context, transformer));
    record.extras = element_extras(transformer);
    for (name, attr) in [
        ("dgs.nt3ag_h", "nt3ag_h"),
        ("dgs.nt3ag_m", "nt3ag_m"),
        ("dgs.nt3ag_l", "nt3ag_l"),
    ] {
        if let Some(clock) = typ.real(attr).filter(|clock| *clock != 0.0) {
            record.extras.insert(name.into(), clock.into());
        }
    }
    Some(record)
}

/// A two converter DC island as one HVDC record between the converters' AC
/// buses; the first converter in document order sends.
fn read_hvdc(context: &Context<'_>, island: &DcIsland, warnings: &mut Diagnostics) -> Option<Hvdc> {
    let doc = context.doc;
    let converters = island
        .converters
        .iter()
        .filter_map(|id| doc.by_id(*id))
        .collect::<Vec<_>>();
    let ac_bus = |converter: &DgsObject| {
        let ac = context
            .connections(converter)
            .iter()
            .filter(|connection| connection.side == 0)
            .collect::<Vec<_>>();
        match ac[..] {
            [connection] => context
                .terminal_bus
                .get(&connection.terminal)
                .map(|bus| (*bus, connection.closed)),
            _ => None,
        }
    };
    let [sending, receiving] = converters[..] else {
        context.warn(
            warnings,
            &codes::READ_DGS_RECORD_UNMAPPED,
            format!(
                "a DC island of {} terminal(s) and {} line(s) touches {} ElmVsc converter(s); \
                 only a two converter island maps to an HVDC record and the island was dropped",
                island.terminals.len(),
                island.lines.len(),
                converters.len()
            ),
            None,
        );
        return None;
    };
    let (Some((from, closed_from)), Some((to, closed_to))) = (ac_bus(sending), ac_bus(receiving))
    else {
        context.warn(
            warnings,
            &codes::READ_DGS_RECORD_UNMAPPED,
            format!(
                "{} or {} connects to other than one AC terminal; the HVDC link was dropped",
                label(sending),
                label(receiving)
            ),
            Some(sending),
        );
        return None;
    };
    let mut record = Hvdc::new(from, to);
    let p_send = sending.real("psetp").unwrap_or(0.0);
    let losses = sending.real("Pnold").unwrap_or(0.0) / 1000.0
        + receiving.real("Pnold").unwrap_or(0.0) / 1000.0;
    record.pf = p_send;
    record.loss0 = losses;
    record.pt = Hvdc::calc_delivered_power(p_send, losses, 0.0);
    record.qf = sending.real("qsetp").unwrap_or(0.0);
    record.qt = receiving.real("qsetp").unwrap_or(0.0);
    record.vf = sending.real("usetp").unwrap_or(1.0);
    record.vt = receiving.real("usetp").unwrap_or(1.0);
    let pmax = sending
        .real("P_max")
        .or_else(|| receiving.real("P_max"))
        .unwrap_or(p_send.abs());
    record.pmax = pmax;
    record.pmin = -pmax;
    record.in_service = closed_from
        && closed_to
        && !Context::out_of_service(sending)
        && !Context::out_of_service(receiving);
    // DC lines in parallel between the converters: their conductances add.
    let mut conductance = 0.0;
    let mut nominal_kv = None;
    for line in island.lines.iter().filter_map(|id| doc.by_id(*id)) {
        if line.class() != "ElmLne" {
            continue;
        }
        let typ = doc.referenced(line, "typ_id");
        let r = typ.and_then(|typ| typ.real("rline")).unwrap_or(0.0) * line.real("dline").unwrap_or(1.0);
        if r > 0.0 {
            conductance += 1.0 / r;
        }
        nominal_kv = nominal_kv
            .or_else(|| line.real("Unom"))
            .or_else(|| typ.and_then(|typ| typ.real("uline")));
    }
    record.resistance_ohm = Some(if conductance > 0.0 { 1.0 / conductance } else { 0.0 });
    record.nominal_voltage_kv = nominal_kv.or_else(|| sending.real("Unomdc"));
    record.converters_mode = Some(HvdcConvertersMode::Side1RectifierSide2Inverter);
    record.uid = Some(uid(context, sending));
    record.extras = element_extras(sending);
    record
        .extras
        .insert("dgs.converter2".into(), receiving.id.into());
    record.extras.insert(
        "dgs.dc_terminals".into(),
        island.terminals.clone().into(),
    );
    Some(record)
}

/// The declared slack is the reference bus; every other bus with an in
/// service voltage regulating generator is PV. Without a declared slack the
/// largest in service regulating generator's bus becomes the reference.
fn assign_bus_kinds(
    context: &Context<'_>,
    buses: &mut [Bus],
    generators: &[Generator],
    reference: &[BusId],
    warnings: &mut Diagnostics,
) {
    let mut regulating: HashSet<BusId> = HashSet::new();
    for generator in generators {
        if generator.in_service && generator.voltage_regulation_on {
            regulating.insert(generator.bus);
        }
    }
    let mut reference: HashSet<BusId> = reference.iter().copied().collect();
    if reference.is_empty() {
        if let Some(largest) = generators
            .iter()
            .filter(|generator| generator.in_service && generator.voltage_regulation_on)
            .max_by(|a, b| a.mbase.total_cmp(&b.mbase))
        {
            reference.insert(largest.bus);
            context.warn(
                warnings,
                &codes::READ_DGS_VALUE_DEFAULTED,
                format!(
                    "no ElmSym, ElmGenstat, or ElmXnet declares the slack (`ip_ctrl=1` or \
                     `bustp=SL`); bus {} of the largest voltage regulating generator is the \
                     reference bus",
                    largest.bus
                ),
                None,
            );
        }
    }
    for bus in buses.iter_mut() {
        bus.kind = if reference.contains(&bus.id) {
            BusType::Ref
        } else if regulating.contains(&bus.id) {
            BusType::Pv
        } else {
            BusType::Pq
        };
    }
}

fn report_unmapped_classes(context: &Context<'_>, warnings: &mut Diagnostics) {
    for (class, rows) in context.doc.class_counts() {
        if HANDLED_CLASSES.contains(&class) || !reports_when_unmapped(class) {
            continue;
        }
        if class == "ElmVsc" {
            continue;
        }
        context.warn(
            warnings,
            &codes::READ_DGS_CLASS_UNMAPPED,
            format!(
                "{rows} `{class}` row(s) have no balanced network spelling and remain only in \
                 the retained source"
            ),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_pairs_follow_the_stated_input_mode() {
        assert_eq!(
            power_from_mode(Some("PQ"), Some(50.0), Some(25.0), Some(55.9), Some(0.89)),
            Some((50.0, 25.0))
        );
        let (p, q) = power_from_mode(Some("SC"), None, None, Some(100.0), Some(0.8)).unwrap();
        assert!((p - 80.0).abs() < 1e-9 && (q - 60.0).abs() < 1e-9);
        let (p, q) = power_from_mode(Some("SP"), Some(30.0), None, Some(50.0), None).unwrap();
        assert!((p - 30.0).abs() < 1e-9 && (q - 40.0).abs() < 1e-9);
        // An unsupported mode falls back to whichever pair is stated.
        assert_eq!(
            power_from_mode(Some("EC"), Some(1.0), Some(2.0), None, None),
            Some((1.0, 2.0))
        );
        assert_eq!(power_from_mode(Some("DEF"), None, None, Some(15.0), None), None);
    }

    #[test]
    fn transformer_tests_convert_to_per_unit_on_the_rating() {
        let (r, x) = short_circuit_impedance(10.0, 500.0, 100.0);
        assert!((r - 0.005).abs() < 1e-12);
        assert!((x - (0.01 - 0.000_025).sqrt()).abs() < 1e-12);
        let (g, b) = magnetizing_admittance(1.0, 100.0, 100.0);
        assert!((g - 0.001).abs() < 1e-12);
        assert!((b + (0.0001 - 0.000_001).sqrt()).abs() < 1e-12);
        assert_eq!(magnetizing_admittance(0.0, 100.0, 100.0), (0.0, 0.0));
    }

    #[test]
    fn energy_sources_follow_the_category_text() {
        assert_eq!(energy_source(Some("Hydro")), GeneratorEnergySource::Hydro);
        assert_eq!(energy_source(Some("Gas")), GeneratorEnergySource::Thermal);
        assert_eq!(energy_source(Some("Others")), GeneratorEnergySource::Other);
        assert_eq!(energy_source(None), GeneratorEnergySource::Other);
    }
}

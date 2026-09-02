//! The multiconductor network a conductor level DGS export describes.
//!
//! Terminals become buses whose conductor set follows the `ElmTerm` phase
//! technology, line types become line codes whose phase domain matrices come
//! from the sequence parameters and the neutral conductor parameters, and
//! loads keep the per phase demand they state. Values are SI: volts, watts,
//! ohm per meter, siemens per meter.
//!
//! PowerFactory phase technology codes, from the element references:
//!
//! - `ElmTerm.phtech`: 0 ABC, 1 ABC-N, 2 BI, 3 BI-N, 4 2PH, 5 2PH-N, 6 1PH,
//!   7 1PH-N, 8 N.
//! - `ElmLod.phtech`: 0 3PH delta, 1 3PH phase to earth, 2 3PH grounded
//!   wye, 3 1PH phase to earth, 4 1PH phase to neutral, 5 1PH phase to
//!   phase, 6 2PH phase to earth, 7 2PH grounded wye, 8 2PH phase to phase.

use std::collections::{BTreeMap, HashMap};

use powerio_core::{Diagnostic, DiagnosticInfo, SourceSpan};
use powerio_dist::{
    Configuration, CoordinateSpace, DistBus, DistGenerator, DistGeoMeta, DistLine, DistLineCode,
    DistLoad, DistLoadVoltageModel, DistLocation, DistShunt, DistSourceFormat, DistSwitch,
    DistTransformer, DistWinding, DistWindingConn, MulticonductorNetwork, UntypedObject,
    VoltageSource,
};
use powerio_tx::diagnostics::codes;
use powerio_tx::format::dgs::{DgsDocument, DgsObject, DgsValue, SpanContext};

/// The neutral conductor's terminal name, as PowerModelsDistribution and the
/// OpenDSS reader spell a materialized neutral.
const NEUTRAL: &str = "4";

/// PowerFactory's own default nominal frequency.
const DEFAULT_FREQUENCY: f64 = 50.0;

/// Classes the mapping reads, references, or deliberately ignores.
const HANDLED_CLASSES: [&str; 28] = [
    "ElmNet",
    "ElmTerm",
    "ElmLne",
    "ElmTr2",
    "ElmTr3",
    "ElmLod",
    "ElmLodmv",
    "ElmLodlv",
    "ElmLodlvp",
    "ElmSym",
    "ElmGenstat",
    "ElmPvsys",
    "ElmXnet",
    "ElmShnt",
    "ElmCoup",
    "ElmZone",
    "ElmArea",
    "ElmSite",
    "ElmSubstat",
    "ElmTrfstat",
    "StaCubic",
    "StaSwitch",
    "TypLne",
    "TypTr2",
    "TypTr3",
    "TypSym",
    "TypLod",
    "TypSwitch",
];

#[derive(Clone, Debug)]
struct Connection {
    terminal: i64,
    side: i64,
    closed: bool,
    /// Terminal phase conductor for each element phase conductor.
    phases: Vec<String>,
}

struct Context<'a> {
    doc: &'a DgsDocument,
    spans: &'a SpanContext,
    diagnostics: Vec<Diagnostic>,
    connections: HashMap<i64, Vec<Connection>>,
    /// Terminal id to bus id.
    bus_ids: HashMap<i64, String>,
    /// Terminal id to nominal line to line kilovolts.
    bus_kv: HashMap<i64, f64>,
}

impl Context<'_> {
    fn warn(&mut self, code: &'static DiagnosticInfo, message: String, object: Option<&DgsObject>) {
        let record = Diagnostic::of(code, message);
        let record = match object {
            Some(object) => SourceSpan::new(
                self.spans.source.clone(),
                self.spans.offset + object.byte_start as u64,
                self.spans.offset + object.byte_end as u64,
            )
            .ok()
            .and_then(|span| record.clone().with_span(span).ok())
            .unwrap_or(record),
            None => record,
        };
        self.diagnostics.push(record);
    }

    fn connections(&self, element: &DgsObject) -> &[Connection] {
        self.connections
            .get(&element.id)
            .map_or(&[], Vec::as_slice)
    }

    fn bus_of(&self, connection: &Connection) -> Option<&String> {
        self.bus_ids.get(&connection.terminal)
    }

    /// The bus and conductor map of a one terminal element.
    fn single(&mut self, element: &DgsObject) -> Option<(String, Vec<String>, bool)> {
        let connections = self.connections(element);
        let [connection] = connections else {
            let count = connections.len();
            self.warn(
                &codes::READ_DGS_RECORD_UNMAPPED,
                format!(
                    "{} connects to {count} terminal(s) rather than one; the element was dropped",
                    label(element)
                ),
                Some(element),
            );
            return None;
        };
        let connection = connection.clone();
        let bus = self.bus_of(&connection)?.clone();
        Some((bus, connection.phases, connection.closed))
    }

    /// The buses and conductor maps of a two terminal element.
    #[allow(clippy::type_complexity)]
    fn pair(&mut self, element: &DgsObject) -> Option<((String, Vec<String>), (String, Vec<String>), bool)> {
        let connections = self.connections(element);
        let [a, b] = connections else {
            let count = connections.len();
            self.warn(
                &codes::READ_DGS_RECORD_UNMAPPED,
                format!(
                    "{} connects to {count} terminal(s) rather than two; the element was dropped",
                    label(element)
                ),
                Some(element),
            );
            return None;
        };
        let (a, b) = (a.clone(), b.clone());
        let bus_a = self.bus_of(&a)?.clone();
        let bus_b = self.bus_of(&b)?.clone();
        Some(((bus_a, a.phases), (bus_b, b.phases), a.closed && b.closed))
    }
}

fn label(object: &DgsObject) -> String {
    format!("{} `{}`", object.class(), object.name())
}

fn out_of_service(element: &DgsObject) -> bool {
    element.int("outserv") == Some(1)
}

/// The phase conductors an `ElmTerm` phase technology declares, and whether
/// a neutral is among them.
fn terminal_conductors(phtech: i64) -> (Vec<&'static str>, bool) {
    match phtech {
        1 => (vec!["1", "2", "3"], true),
        2 | 4 => (vec!["1", "2"], false),
        3 | 5 => (vec!["1", "2"], true),
        6 => (vec!["1"], false),
        7 => (vec!["1"], true),
        8 => (vec![], true),
        _ => (vec!["1", "2", "3"], false),
    }
}

/// A row identity: the `loc_name`, or `loc_name#id` when the name repeats
/// within its class.
fn uid(doc: &DgsDocument, object: &DgsObject) -> String {
    let name = object.name();
    let repeated = doc
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

/// Build the multiconductor network of a decoded document.
pub(super) fn build(
    doc: &DgsDocument,
    name_hint: Option<&str>,
    spans: &SpanContext,
) -> (MulticonductorNetwork, Vec<Diagnostic>) {
    let net_object = doc.of_class("ElmNet").next();
    let name = net_object
        .and_then(|net| net.str("loc_name"))
        .filter(|name| !name.is_empty())
        .or(name_hint)
        .map(str::to_owned);
    let mut net = MulticonductorNetwork::new();
    *net.name_mut() = name;
    let mut context = Context {
        doc,
        spans,
        diagnostics: Vec::new(),
        connections: HashMap::new(),
        bus_ids: HashMap::new(),
        bus_kv: HashMap::new(),
    };
    match net_object.and_then(|net| net.real("frnom")) {
        Some(frequency) if frequency > 0.0 => *net.base_frequency_mut() = frequency,
        _ => {
            *net.base_frequency_mut() = DEFAULT_FREQUENCY;
            context.warn(
                &codes::READ_DGS_VALUE_DEFAULTED,
                format!("no ElmNet states a nominal frequency; {DEFAULT_FREQUENCY} Hz assumed"),
                net_object,
            );
        }
    }
    let frequency = net.base_frequency();
    build_buses(&mut context, &mut net);
    collect_connections(&mut context);
    for typ in doc.of_class("TypLne") {
        if let Some(code) = read_linecode(&mut context, typ, frequency) {
            net.linecodes_mut().push(code);
        }
    }
    for line in doc.of_class("ElmLne") {
        if let Some(record) = read_line(&mut context, line) {
            net.lines_mut().push(record);
        }
    }
    for coupler in doc.of_class("ElmCoup") {
        if let Some(record) = read_switch(&mut context, coupler) {
            net.switches_mut().push(record);
        }
    }
    for class in ["ElmLod", "ElmLodmv", "ElmLodlv", "ElmLodlvp"] {
        for load in doc.of_class(class) {
            if let Some(record) = read_load(&mut context, load) {
                net.loads_mut().push(record);
            }
        }
    }
    for class in ["ElmSym", "ElmGenstat", "ElmPvsys"] {
        for machine in doc.of_class(class) {
            if let Some(record) = read_generator(&mut context, machine) {
                net.generators_mut().push(record);
            }
        }
    }
    for grid in doc.of_class("ElmXnet") {
        if let Some(record) = read_source(&mut context, grid) {
            net.sources_mut().push(record);
        }
    }
    for shunt in doc.of_class("ElmShnt") {
        if let Some(record) = read_shunt(&mut context, shunt) {
            net.shunts_mut().push(record);
        }
    }
    for transformer in doc.of_class("ElmTr2") {
        if let Some(record) = read_two_winding(&mut context, transformer) {
            net.transformers_mut().push(record);
        }
    }
    for transformer in doc.of_class("ElmTr3") {
        if let Some(record) = read_three_winding(&mut context, transformer) {
            net.transformers_mut().push(record);
        }
    }
    report_unmapped_classes(&mut context, &mut net);
    if net.buses().iter().any(|bus| bus.location.is_some()) {
        *net.geo_mut() = Some(DistGeoMeta {
            space: CoordinateSpace::Geographic { crs: None },
            kind: None,
        });
    }
    *net.source_format_mut() = Some(DistSourceFormat::Dgs);
    (net, context.diagnostics)
}

fn build_buses(context: &mut Context<'_>, net: &mut MulticonductorNetwork) {
    let doc = context.doc;
    for terminal in doc.of_class("ElmTerm") {
        let (phases, neutral) = terminal_conductors(terminal.int("phtech").unwrap_or(0));
        let mut terminals: Vec<String> = phases.iter().map(|p| (*p).to_owned()).collect();
        if neutral {
            terminals.push(NEUTRAL.to_owned());
        }
        let id = uid(doc, terminal);
        let mut bus = DistBus::new(id.clone(), terminals);
        if neutral && terminal.int("ciEarthed") == Some(1) {
            bus.grounded.push(NEUTRAL.to_owned());
        }
        let kv = terminal.real("uknom").unwrap_or(0.0);
        if kv <= 0.0 {
            context.warn(
                &codes::READ_DGS_VALUE_DEFAULTED,
                format!(
                    "{} states no positive nominal voltage `uknom`; its equipment reads \
                     against a zero voltage",
                    label(terminal)
                ),
                Some(terminal),
            );
        }
        if let (Some(lat), Some(lon)) = (terminal.real("GPSlat"), terminal.real("GPSlon"))
            && lat.is_finite()
            && lon.is_finite()
            && (lat != 0.0 || lon != 0.0)
        {
            bus.location = Some(DistLocation {
                x: lon,
                y: lat,
                kind: None,
            });
        }
        bus.extras.insert("dgs.id".into(), terminal.id.into());
        bus.extras.insert("dgs.uknom_kv".into(), kv.into());
        if let Some(phtech) = terminal.int("phtech") {
            bus.extras.insert("dgs.phtech".into(), phtech.into());
        }
        context.bus_ids.insert(terminal.id, id);
        context.bus_kv.insert(terminal.id, kv);
        net.buses_mut().push(bus);
    }
}

/// Every cubicle: the element, its side, its switch state, and the terminal
/// conductor each element phase lands on (`it2p1..3`, identity by default).
fn collect_connections(context: &mut Context<'_>) {
    let doc = context.doc;
    for cubicle in doc.of_class("StaCubic") {
        let Some(terminal) = doc.parent(cubicle).filter(|parent| parent.class() == "ElmTerm")
        else {
            context.warn(
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
            continue;
        };
        let mut closed = true;
        for switch in doc.children_of_class(cubicle.id, "StaSwitch") {
            if switch.int("on_off").unwrap_or(0) == 0 {
                closed = false;
            }
        }
        let count = usize::try_from(cubicle.int("nphase").unwrap_or(3)).unwrap_or(3);
        let mapping = [
            cubicle.int("it2p1"),
            cubicle.int("it2p2"),
            cubicle.int("it2p3"),
        ];
        let phases = (0..count.min(3))
            .map(|k| {
                let target = mapping[k].unwrap_or(i64::try_from(k).unwrap_or(0));
                (target + 1).to_string()
            })
            .collect();
        context
            .connections
            .entry(element.id)
            .or_default()
            .push(Connection {
                terminal: terminal.id,
                side: cubicle.int("obj_bus").unwrap_or(0),
                closed,
                phases,
            });
    }
    for connections in context.connections.values_mut() {
        connections.sort_by_key(|connection| connection.side);
    }
}

/// Symmetric `n` by `n` matrix with `diagonal` on the diagonal and `mutual`
/// elsewhere.
fn symmetric(n: usize, diagonal: f64, mutual: f64) -> Vec<Vec<f64>> {
    (0..n)
        .map(|i| {
            (0..n)
                .map(|j| if i == j { diagonal } else { mutual })
                .collect()
        })
        .collect()
}

/// Phase domain self and mutual values from zero and positive sequence
/// values: `(z0 + 2 z1) / 3` and `(z0 - z1) / 3`.
fn phase_from_sequence(zero: f64, positive: f64) -> (f64, f64) {
    ((zero + 2.0 * positive) / 3.0, (zero - positive) / 3.0)
}

/// A line type as a line code in ohm per meter and siemens per meter, with
/// the phase conductors first and the neutral last.
fn read_linecode(context: &mut Context<'_>, typ: &DgsObject, frequency: f64) -> Option<DistLineCode> {
    let phases = usize::try_from(typ.int("nlnph").unwrap_or(3)).unwrap_or(3).clamp(1, 3);
    let neutral = typ.int("nneutral").unwrap_or(0) > 0;
    let n = phases + usize::from(neutral);
    let per_km = 1e-3;
    let r1 = typ.real("rline").unwrap_or(0.0);
    let x1 = typ.real("xline").unwrap_or(0.0);
    let r0 = typ.real("rline0").unwrap_or(r1);
    let x0 = typ.real("xline0").unwrap_or(x1);
    let susceptance = |positive: &str, capacitance: &str| {
        typ.real(positive).map_or_else(
            || {
                typ.real(capacitance).map_or(0.0, |c| {
                    2.0 * std::f64::consts::PI * typ.real("frnom").unwrap_or(frequency) * c
                })
            },
            |b| b,
        ) * 1e-6
    };
    let b1 = susceptance("bline", "cline");
    let b0 = typ
        .real("bline0")
        .map_or_else(|| typ.real("cline0").map_or(b1, |_| susceptance("bline0", "cline0")), |b| b * 1e-6);
    let g1 = typ.real("gline").unwrap_or(0.0) * 1e-6;
    let g0 = typ.real("gline0").unwrap_or(g1) * 1e-6;
    let (r_self, r_mutual) = if phases == 1 { (r1, 0.0) } else { phase_from_sequence(r0, r1) };
    let (x_self, x_mutual) = if phases == 1 { (x1, 0.0) } else { phase_from_sequence(x0, x1) };
    let (b_self, b_mutual) = if phases == 1 { (b1, 0.0) } else { phase_from_sequence(b0, b1) };
    let (g_self, g_mutual) = if phases == 1 { (g1, 0.0) } else { phase_from_sequence(g0, g1) };
    let mut r = symmetric(n, r_self, r_mutual);
    let mut x = symmetric(n, x_self, x_mutual);
    let mut b = symmetric(n, b_self, b_mutual);
    let mut g = symmetric(n, g_self, g_mutual);
    if neutral {
        let last = n - 1;
        let stated = |name: &str| typ.real(name);
        let (rn, xn) = match (stated("rnline"), stated("xnline")) {
            (Some(rn), Some(xn)) => (rn, xn),
            _ => {
                context.warn(
                    &codes::READ_DGS_VALUE_DEFAULTED,
                    format!(
                        "{} declares a neutral conductor without `rnline`/`xnline`; the neutral \
                         takes the phase conductor self impedance",
                        label(typ)
                    ),
                    Some(typ),
                );
                (r_self, x_self)
            }
        };
        let rpn = stated("rpnline").unwrap_or(r_mutual);
        let xpn = stated("xpnline").unwrap_or(x_mutual);
        let bn = stated("bnline").map_or(0.0, |v| v * 1e-6);
        let bpn = stated("bpnline").map_or(0.0, |v| v * 1e-6);
        let gn = stated("gnline").map_or(0.0, |v| v * 1e-6);
        for k in 0..last {
            r[k][last] = rpn;
            r[last][k] = rpn;
            x[k][last] = xpn;
            x[last][k] = xpn;
            b[k][last] = bpn;
            b[last][k] = bpn;
            g[k][last] = 0.0;
            g[last][k] = 0.0;
        }
        r[last][last] = rn;
        x[last][last] = xn;
        b[last][last] = bn;
        g[last][last] = gn;
    }
    let scale = |m: &Vec<Vec<f64>>, k: f64| -> Vec<Vec<f64>> {
        m.iter()
            .map(|row| row.iter().map(|v| v * k).collect())
            .collect()
    };
    let mut code = DistLineCode::new(uid(context.doc, typ), scale(&r, per_km), scale(&x, per_km));
    code.n_conductors = n;
    code.g_from = scale(&g, per_km / 2.0);
    code.g_to = scale(&g, per_km / 2.0);
    code.b_from = scale(&b, per_km / 2.0);
    code.b_to = scale(&b, per_km / 2.0);
    if let Some(rating) = typ.real("sline").filter(|i| *i > 0.0 && *i < 99_999.0) {
        code.i_max = Some(vec![rating * 1e3; n]);
    }
    code.source = Some("dgs".to_owned());
    code.extras.insert("dgs.id".into(), typ.id.into());
    code.extras.insert("dgs.nlnph".into(), i64::try_from(phases).unwrap_or(3).into());
    code.extras.insert("dgs.nneutral".into(), i64::from(neutral).into());
    Some(code)
}

fn read_line(context: &mut Context<'_>, line: &DgsObject) -> Option<DistLine> {
    let doc = context.doc;
    let Some(typ) = doc.referenced(line, "typ_id") else {
        context.warn(
            &codes::READ_DGS_REFERENCE_DROPPED,
            format!(
                "{} names no TypLne line type in `typ_id`; the line was dropped",
                label(line)
            ),
            Some(line),
        );
        return None;
    };
    let ((bus_from, mut map_from), (bus_to, mut map_to), closed) = context.pair(line)?;
    let phases = usize::try_from(typ.int("nlnph").unwrap_or(3)).unwrap_or(3).clamp(1, 3);
    let neutral = typ.int("nneutral").unwrap_or(0) > 0;
    map_from.truncate(phases);
    map_to.truncate(phases);
    if neutral {
        map_from.push(NEUTRAL.to_owned());
        map_to.push(NEUTRAL.to_owned());
    }
    let length = line.real("dline").unwrap_or(1.0) * 1e3;
    let mut record = DistLine::new(
        uid(doc, line),
        bus_from,
        bus_to,
        map_from,
        map_to,
        uid(doc, typ),
        length,
    );
    if let Some(parallel) = line.int("nlnum").filter(|n| *n > 1) {
        context.warn(
            &codes::READ_DGS_FIELD_UNMAPPED,
            format!(
                "{} states `nlnum={parallel}` parallel circuits; the multiconductor line \
                 carries one circuit of its line code",
                label(line)
            ),
            Some(line),
        );
        record.extras.insert("dgs.nlnum".into(), parallel.into());
    }
    if !closed || out_of_service(line) {
        record.extras.insert("dgs.outserv".into(), true.into());
        context.warn(
            &codes::READ_DGS_FIELD_UNMAPPED,
            format!(
                "{} is out of service or open at a cubicle; the multiconductor line has no \
                 service flag and the state is kept in `extras`",
                label(line)
            ),
            Some(line),
        );
    }
    if let Some((rows, cols, data)) = line.matrix("GPScoords")
        && cols >= 2
        && rows >= 2
    {
        record.route = Some(
            data.chunks(cols)
                .map(|point| DistLocation {
                    x: point[1],
                    y: point[0],
                    kind: None,
                })
                .collect(),
        );
    }
    record.extras.insert("dgs.id".into(), line.id.into());
    Some(record)
}

fn read_switch(context: &mut Context<'_>, coupler: &DgsObject) -> Option<DistSwitch> {
    let ((bus_from, map_from), (bus_to, map_to), closed) = context.pair(coupler)?;
    let own = coupler.int("on_off").map_or(true, |state| state != 0);
    let mut record = DistSwitch::new(
        uid(context.doc, coupler),
        bus_from,
        bus_to,
        map_from,
        map_to,
        !(own && closed) || out_of_service(coupler),
    );
    if let Some(usage) = coupler.str("aUsage") {
        record.extras.insert("dgs.aUsage".into(), usage.into());
    }
    record.extras.insert("dgs.id".into(), coupler.id.into());
    Some(record)
}

/// The load connection an `ElmLod` phase technology declares, and how many
/// phase conductors it uses.
fn load_connection(phtech: i64) -> (Configuration, usize) {
    match phtech {
        0 => (Configuration::Delta, 3),
        1 | 2 => (Configuration::Wye, 3),
        3 | 4 => (Configuration::SinglePhase, 1),
        5 => (Configuration::Delta, 2),
        6 | 7 => (Configuration::Wye, 2),
        8 => (Configuration::Delta, 2),
        _ => (Configuration::Wye, 3),
    }
}

/// PowerFactory's `mode_inp` pair as the balanced reader resolves it.
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
    stated.or_else(|| {
        p.zip(q)
            .or_else(|| p.zip(s).and_then(|(p, s)| from_p_s(p, s)))
            .or_else(|| q.zip(s).and_then(|(q, s)| from_q_s(q, s)))
            .or_else(|| p.zip(cos_phi).and_then(|(p, c)| from_p_cos(p, c)))
            .or_else(|| q.zip(cos_phi).and_then(|(q, c)| from_q_cos(q, c)))
            .or_else(|| s.zip(cos_phi).and_then(|(s, c)| from_s_cos(s, c)))
    })
}

fn read_load(context: &mut Context<'_>, load: &DgsObject) -> Option<DistLoad> {
    let (bus, phases, closed) = context.single(load)?;
    let (configuration, count) = load_connection(load.int("phtech").unwrap_or(0));
    let terminal_map: Vec<String> = phases.into_iter().take(count).collect();
    let count = terminal_map.len().max(1);
    let scale = load.real("scale0").unwrap_or(1.0) * 1e6;
    let (p_nom, q_nom) = if load.int("i_sym") == Some(1) {
        let per_phase = |p: &str, q: &str| (load.real(p).unwrap_or(0.0), load.real(q).unwrap_or(0.0));
        let phases = [
            per_phase("plinir", "qlinir"),
            per_phase("plinis", "qlinis"),
            per_phase("plinit", "qlinit"),
        ];
        (
            phases.iter().take(count).map(|(p, _)| p * scale).collect(),
            phases.iter().take(count).map(|(_, q)| q * scale).collect(),
        )
    } else {
        let total = power_from_mode(
            load.str("mode_inp"),
            load.real("plini"),
            load.real("qlini"),
            load.real("slini"),
            load.real("coslini"),
        );
        let (p, q) = match total {
            Some(pair) => pair,
            None => {
                context.warn(
                    &codes::READ_DGS_VALUE_DEFAULTED,
                    format!(
                        "{} states no pair of quantities that determines its demand; zero \
                         demand assumed",
                        label(load)
                    ),
                    Some(load),
                );
                (0.0, 0.0)
            }
        };
        #[allow(clippy::cast_precision_loss)]
        let share = count as f64;
        (
            vec![p * scale / share; count],
            vec![q * scale / share; count],
        )
    };
    let terminal_id = context.connections(load)[0].terminal;
    let kv = context.bus_kv.get(&terminal_id).copied().unwrap_or(0.0);
    let v_nom = match configuration {
        Configuration::Delta => kv * 1e3,
        _ => kv * 1e3 / 3f64.sqrt(),
    };
    let mut record = DistLoad::new(
        uid(context.doc, load),
        bus,
        terminal_map,
        configuration,
        p_nom,
        q_nom,
    );
    record.voltage_model = DistLoadVoltageModel::ConstantPower {
        v_nom: vec![v_nom; count],
    };
    if !closed || out_of_service(load) {
        record.extras.insert("dgs.outserv".into(), true.into());
        context.warn(
            &codes::READ_DGS_FIELD_UNMAPPED,
            format!(
                "{} is out of service or open at its cubicle; the multiconductor load has no \
                 service flag and the state is kept in `extras`",
                label(load)
            ),
            Some(load),
        );
    }
    record.extras.insert("dgs.id".into(), load.id.into());
    record
        .extras
        .insert("dgs.class".into(), load.class().into());
    Some(record)
}

fn read_generator(context: &mut Context<'_>, machine: &DgsObject) -> Option<DistGenerator> {
    let (bus, phases, closed) = context.single(machine)?;
    let count = phases.len().max(1);
    #[allow(clippy::cast_precision_loss)]
    let share = count as f64;
    let p = machine.real("pgini").unwrap_or(0.0) * 1e6 / share;
    let q = machine.real("qgini").unwrap_or(0.0) * 1e6 / share;
    let mut record = DistGenerator::new(
        uid(context.doc, machine),
        bus,
        phases,
        Configuration::Wye,
        vec![p; count],
        vec![q; count],
    );
    if let (Some(low), Some(high)) = (machine.real("Pmin_uc"), machine.real("Pmax_uc")) {
        record.p_min = Some(vec![low * 1e6 / share; count]);
        record.p_max = Some(vec![high * 1e6 / share; count]);
    }
    let rated = machine
        .real("sgn")
        .or_else(|| context.doc.referenced(machine, "typ_id").and_then(|typ| typ.real("sgn")));
    if let (Some(low), Some(high), Some(rated)) =
        (machine.real("q_min"), machine.real("q_max"), rated)
    {
        record.q_min = Some(vec![low * rated * 1e6 / share; count]);
        record.q_max = Some(vec![high * rated * 1e6 / share; count]);
    }
    if let Some(rated) = rated {
        record.s_max = Some(vec![rated * 1e6 / share; count]);
    }
    if let Some(setpoint) = machine.real("usetp") {
        record.extras.insert("dgs.usetp".into(), setpoint.into());
    }
    if !closed || out_of_service(machine) {
        record.extras.insert("dgs.outserv".into(), true.into());
    }
    record.extras.insert("dgs.id".into(), machine.id.into());
    record
        .extras
        .insert("dgs.class".into(), machine.class().into());
    Some(record)
}

/// An external grid as a balanced voltage source behind its terminal.
fn read_source(context: &mut Context<'_>, grid: &DgsObject) -> Option<VoltageSource> {
    let (bus, phases, _closed) = context.single(grid)?;
    let terminal_id = context.connections(grid)[0].terminal;
    let kv = context.bus_kv.get(&terminal_id).copied().unwrap_or(0.0);
    let magnitude = grid.real("usetp").unwrap_or(1.0) * kv * 1e3 / 3f64.sqrt();
    let reference = grid.real("phiini").unwrap_or(0.0).to_radians();
    let count = phases.len();
    let angles = (0..count)
        .map(|k| {
            #[allow(clippy::cast_precision_loss)]
            let shift = -(k as f64) * 2.0 * std::f64::consts::PI / 3.0;
            reference + shift
        })
        .collect();
    let mut record = VoltageSource::new(
        uid(context.doc, grid),
        bus,
        phases,
        vec![magnitude; count],
        angles,
    );
    record.extras.insert("dgs.id".into(), grid.id.into());
    if let Some(short_circuit) = grid.real("snss") {
        record.extras.insert("dgs.snss_mva".into(), short_circuit.into());
    }
    Some(record)
}

fn read_shunt(context: &mut Context<'_>, shunt: &DgsObject) -> Option<DistShunt> {
    let (bus, phases, closed) = context.single(shunt)?;
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
                    _ => return None,
                }
            }
        }
        Some(2) => (
            shunt.real("gparac").unwrap_or(0.0) * 1e-6,
            shunt.real("bcap").unwrap_or(0.0) * 1e-6,
        ),
        other => {
            context.warn(
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
    #[allow(clippy::cast_precision_loss)]
    let sections = shunt.int("ncapa").unwrap_or(1).max(0) as f64;
    let n = phases.len();
    // The stated admittance is the three phase total on the line to line
    // voltage, which is the per phase wye admittance.
    let mut record = DistShunt::new(
        uid(context.doc, shunt),
        bus,
        phases,
        symmetric(n, g_section * sections, 0.0),
        symmetric(n, b_section * sections, 0.0),
    );
    if !closed || out_of_service(shunt) {
        record.extras.insert("dgs.outserv".into(), true.into());
    }
    record.extras.insert("dgs.id".into(), shunt.id.into());
    Some(record)
}

fn winding_connection(context: &mut Context<'_>, typ: &DgsObject, attr: &str) -> DistWindingConn {
    match typ.str(attr).map(|c| c.trim().to_ascii_uppercase()) {
        Some(ref conn) if conn.starts_with('D') => DistWindingConn::Delta,
        Some(ref conn) if conn.starts_with('Y') => DistWindingConn::Wye,
        Some(conn) => {
            context.warn(
                &codes::READ_DGS_VALUE_UNSUPPORTED,
                format!(
                    "{} states winding connection `{attr}={conn}`; wye and delta are read and \
                     the winding is wye",
                    label(typ)
                ),
                Some(typ),
            );
            DistWindingConn::Wye
        }
        None => DistWindingConn::Wye,
    }
}

/// Leakage impedance from the short circuit test, per unit on the rating.
fn short_circuit_impedance(uk_percent: f64, copper_kw: f64, rated_mva: f64) -> (f64, f64) {
    let z = uk_percent / 100.0;
    let r = copper_kw / (1000.0 * rated_mva);
    let x = (z * z - r * r).max(0.0).sqrt() * z.signum();
    (r, x)
}

fn tap_ratio(element: &DgsObject, typ: &DgsObject, position: &str, neutral: &str, step: &str) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let offset = (element.int(position).unwrap_or(0) - typ.int(neutral).unwrap_or(0)) as f64;
    1.0 + offset * typ.real(step).unwrap_or(0.0) / 100.0
}

fn read_two_winding(context: &mut Context<'_>, transformer: &DgsObject) -> Option<DistTransformer> {
    let doc = context.doc;
    let Some(typ) = doc.referenced(transformer, "typ_id") else {
        context.warn(
            &codes::READ_DGS_REFERENCE_DROPPED,
            format!(
                "{} names no TypTr2 transformer type in `typ_id`; the transformer was dropped",
                label(transformer)
            ),
            Some(transformer),
        );
        return None;
    };
    let ((bus_h, map_h), (bus_l, map_l), closed) = context.pair(transformer)?;
    let rated_mva = typ.real("strn").filter(|s| *s > 0.0)?;
    let (r, x) = short_circuit_impedance(
        typ.real("uktr").unwrap_or(0.0),
        typ.real("pcutr").unwrap_or(0.0),
        rated_mva,
    );
    let ratio = tap_ratio(transformer, typ, "nntap", "nntap0", "dutap");
    let tap_on_hv = typ.int("tap_side").unwrap_or(0) == 0;
    let phases = usize::try_from(typ.int("nt2ph").unwrap_or(3)).unwrap_or(3);
    let conn_h = winding_connection(context, typ, "tr2cn_h");
    let conn_l = winding_connection(context, typ, "tr2cn_l");
    let mut high = DistWinding::new(
        bus_h,
        map_h,
        conn_h,
        typ.real("utrn_h").unwrap_or(0.0) * 1e3,
        rated_mva * 1e6,
    );
    high.r_pct = r * 100.0 / 2.0;
    high.tap = if tap_on_hv { ratio } else { 1.0 };
    let mut low = DistWinding::new(
        bus_l,
        map_l,
        conn_l,
        typ.real("utrn_l").unwrap_or(0.0) * 1e3,
        rated_mva * 1e6,
    );
    low.r_pct = r * 100.0 / 2.0;
    low.tap = if tap_on_hv { 1.0 } else { ratio };
    let mut record = DistTransformer::new(
        uid(doc, transformer),
        vec![high, low],
        vec![x * 100.0],
        phases,
    );
    if let Some(clock) = typ.real("nt2ag").filter(|clock| *clock != 0.0) {
        record.extras.insert("dgs.nt2ag".into(), clock.into());
    }
    if let Some(current) = typ.real("curmg").filter(|c| *c != 0.0) {
        record.extras.insert("%imag".into(), current.into());
    }
    if let Some(iron) = typ.real("pfe").filter(|p| *p != 0.0) {
        record.extras.insert(
            "%noloadloss".into(),
            (iron / (1000.0 * rated_mva) * 100.0).into(),
        );
    }
    if !closed || out_of_service(transformer) {
        record.extras.insert("dgs.outserv".into(), true.into());
    }
    record.extras.insert("dgs.id".into(), transformer.id.into());
    Some(record)
}

fn read_three_winding(context: &mut Context<'_>, transformer: &DgsObject) -> Option<DistTransformer> {
    let doc = context.doc;
    let Some(typ) = doc.referenced(transformer, "typ_id") else {
        context.warn(
            &codes::READ_DGS_REFERENCE_DROPPED,
            format!(
                "{} names no TypTr3 transformer type in `typ_id`; the transformer was dropped",
                label(transformer)
            ),
            Some(transformer),
        );
        return None;
    };
    let connections = context.connections(transformer).to_vec();
    let [h, m, l] = connections.as_slice() else {
        let count = connections.len();
        context.warn(
            &codes::READ_DGS_RECORD_UNMAPPED,
            format!(
                "{} connects to {count} terminal(s) rather than three; the transformer was dropped",
                label(transformer)
            ),
            Some(transformer),
        );
        return None;
    };
    let rating = |name: &str| typ.real(name).filter(|s| *s > 0.0);
    let (s_h, s_m, s_l) = (rating("strn3_h")?, rating("strn3_m")?, rating("strn3_l")?);
    let pair = |uk: &str, pcu: &str, base: f64| {
        let (r, x) = short_circuit_impedance(
            typ.real(uk).unwrap_or(0.0),
            typ.real(pcu).unwrap_or(0.0),
            base,
        );
        // Per unit on the HV rating, so the star split is on one base.
        (r * s_h / base, x * s_h / base)
    };
    let (r_hm, x_hm) = pair("uktr3_h", "pcut3_h", s_h.min(s_m));
    let (r_ml, x_ml) = pair("uktr3_m", "pcut3_m", s_m.min(s_l));
    let (r_lh, x_lh) = pair("uktr3_l", "pcut3_l", s_l.min(s_h));
    let star = |ab: f64, ca: f64, bc: f64| (ab + ca - bc) / 2.0;
    let r_star = [
        star(r_hm, r_lh, r_ml),
        star(r_hm, r_ml, r_lh),
        star(r_ml, r_lh, r_hm),
    ];
    let taps = [
        tap_ratio(transformer, typ, "n3tap_h", "n3tp0_h", "du3tp_h"),
        tap_ratio(transformer, typ, "n3tap_m", "n3tp0_m", "du3tp_m"),
        tap_ratio(transformer, typ, "n3tap_l", "n3tp0_l", "du3tp_l"),
    ];
    let conns = [
        winding_connection(context, typ, "tr3cn_h"),
        winding_connection(context, typ, "tr3cn_m"),
        winding_connection(context, typ, "tr3cn_l"),
    ];
    let mut windings = Vec::with_capacity(3);
    let mut closed = true;
    for (index, (connection, rated_attr, rating)) in [
        (h, "utrn3_h", s_h),
        (m, "utrn3_m", s_m),
        (l, "utrn3_l", s_l),
    ]
    .into_iter()
    .enumerate()
    {
        closed &= connection.closed;
        let bus = context.bus_of(connection)?.clone();
        let mut winding = DistWinding::new(
            bus,
            connection.phases.clone(),
            conns[index],
            typ.real(rated_attr).unwrap_or(0.0) * 1e3,
            rating * 1e6,
        );
        winding.r_pct = r_star[index] * 100.0;
        winding.tap = taps[index];
        windings.push(winding);
    }
    // `xsc_pct` pairs in `pair_keys` order: HV-MV, HV-LV, MV-LV, percent on
    // the HV rating.
    let mut record = DistTransformer::new(
        uid(doc, transformer),
        windings,
        vec![x_hm * 100.0, x_lh * 100.0, x_ml * 100.0],
        3,
    );
    if !closed || out_of_service(transformer) {
        record.extras.insert("dgs.outserv".into(), true.into());
    }
    record.extras.insert("dgs.id".into(), transformer.id.into());
    Some(record)
}

/// Element classes the mapping never reads keep their rows as untyped
/// objects and are reported once per class.
fn report_unmapped_classes(context: &mut Context<'_>, net: &mut MulticonductorNetwork) {
    let doc = context.doc;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (class, rows) in doc.class_counts() {
        if HANDLED_CLASSES.contains(&class) || !(class.starts_with("Elm") || class.starts_with("Cha"))
        {
            continue;
        }
        counts.insert(class, rows);
        for object in doc.of_class(class) {
            let props = object
                .attributes()
                .map(|(name, value)| (Some(name.to_owned()), render(value)))
                .collect();
            net.untyped_mut()
                .push(UntypedObject::new(class, object.name(), props));
        }
    }
    for (class, rows) in counts {
        context.warn(
            &codes::READ_DGS_CLASS_UNMAPPED,
            format!(
                "{rows} `{class}` row(s) have no multiconductor network spelling and are kept \
                 as untyped objects"
            ),
            None,
        );
    }
}

fn render(value: &DgsValue) -> String {
    match value {
        DgsValue::Str(text) => text.clone(),
        DgsValue::Int(value) => value.to_string(),
        DgsValue::Real(value) => value.to_string(),
        DgsValue::Ref(key) => format!("{key:?}"),
        DgsValue::StrVec(values) => values.join(","),
        DgsValue::IntVec(values) => values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
        DgsValue::RealVec(values) => values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
        DgsValue::RefVec(values) => format!("{values:?}"),
        DgsValue::RealMatrix { rows, cols, data } => format!(
            "{rows}x{cols}:{}",
            data.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_values_become_phase_matrices() {
        let (self_z, mutual) = phase_from_sequence(0.9, 0.3);
        assert!((self_z - 0.5).abs() < 1e-12);
        assert!((mutual - 0.2).abs() < 1e-12);
        assert_eq!(symmetric(2, 1.0, 0.5), vec![vec![1.0, 0.5], vec![0.5, 1.0]]);
    }

    #[test]
    fn terminal_technologies_name_their_conductors() {
        assert_eq!(terminal_conductors(0), (vec!["1", "2", "3"], false));
        assert_eq!(terminal_conductors(1), (vec!["1", "2", "3"], true));
        assert_eq!(terminal_conductors(7), (vec!["1"], true));
        assert_eq!(terminal_conductors(8), (vec![], true));
    }
}

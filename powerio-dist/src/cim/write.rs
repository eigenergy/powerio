//! [`MulticonductorNetwork`] to one distribution CIM (CIM100) RDF/XML
//! document: the reader's inverse, with GridAPPS-D style per phase classes.
//!
//! mRIDs are deterministic. An element's imported CIM id (its `cim_mrid`
//! extra) passes through, and every other object takes a UUIDv5 from a fixed
//! PowerIO namespace, the object kind, and its name, so write, read, write is
//! byte stable. Wire coordinate terminal names map back to `SinglePhaseKind`
//! (`1`→A, `2`→B, `3`→C, `4`→N). The grounded neutral of a wye load, shunt,
//! or source is implied by its connection kind, so it is not written as a
//! phase.
//!
//! The document has no `md:FullModel` header: that header marks a CGMES
//! profile set, and the routing reads a header-less document with per phase
//! classes as distribution CIM.

use std::fmt::Write as _;
use std::sync::LazyLock;

use crate::convert::TextEmission;
use crate::diagnostics::{DiagnosticInfo, Diagnostics, codes as C};
use crate::model::{
    ConductorMatrix, Configuration, DistBus, DistLoadVoltageModel, DistTransformer, DistWinding,
    DistWindingConn, Extras, MulticonductorNetwork, VoltageSource,
};

use super::read::SWITCH_CLASSES;
use super::{
    LEAKAGE_READING, no_load_test_pct, sequence_to_phase, short_circuit_test_pct, tank_phase_clock,
    wrap_angle, z_base,
};

const CIM_NS: &str = "http://iec.ch/TC57/CIM100#";
const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// The UUIDv5 namespace of every mRID the writer derives.
static NAMESPACE: LazyLock<uuid::Uuid> = LazyLock::new(|| {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"https://powerio.dev/cim-dist")
});

/// A deterministic mRID from an object's kind and name.
fn det_mrid(kind: &str, name: &str) -> String {
    uuid::Uuid::new_v5(&NAMESPACE, format!("{kind}:{name}").as_bytes()).to_string()
}

/// Whether an imported id can stand in `rdf:ID="_<id>"` unchanged.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn esc_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A terminal name to its `SinglePhaseKind`: the OpenDSS node numbers
/// (`1` to `4`) or the phase letters BMOPF documents use (`a`, `b`, `c`,
/// `n`, either case).
fn phase_letter(terminal: &str) -> Option<&'static str> {
    match terminal {
        "1" | "a" | "A" => Some("A"),
        "2" | "b" | "B" => Some("B"),
        "3" | "c" | "C" => Some("C"),
        "4" | "n" | "N" => Some("N"),
        _ => None,
    }
}

/// Whether a terminal name is the neutral.
fn is_neutral(terminal: &str) -> bool {
    phase_letter(terminal) == Some("N")
}

/// The canonical position of a phase in a `PhaseCode`: A, B, C, then N.
fn phase_rank(terminal: &str) -> usize {
    match phase_letter(terminal) {
        Some("A") => 0,
        Some("B") => 1,
        Some("C") => 2,
        _ => 3,
    }
}

/// The `PhaseCode` value of a terminal map: its nameable terminals in the
/// canonical A, B, C, N order (`ABC`, `ABCN`, `BN`, …), with N added when
/// `neutral` is set, and whether the map already lists them in that order.
/// A `PhaseCode` names a set, not an order.
fn phase_code(map: &[String], neutral: bool) -> (Option<String>, bool) {
    let named: Vec<&str> = map
        .iter()
        .map(String::as_str)
        .filter(|t| phase_letter(t).is_some())
        .collect();
    let mut sorted = named.clone();
    sorted.sort_by_key(|t| phase_rank(t));
    sorted.dedup_by_key(|t| phase_rank(t));
    let in_order = named == sorted;
    let mut code: String = sorted.iter().filter_map(|t| phase_letter(t)).collect();
    if neutral && !code.ends_with('N') {
        code.push('N');
    }
    ((!code.is_empty()).then_some(code), in_order)
}

struct Doc {
    body: String,
    diags: Diagnostics,
}

impl Doc {
    fn warn(&mut self, info: &'static DiagnosticInfo, message: String) {
        self.diags.push(info, message);
    }

    fn open(&mut self, class: &str, id: &str) {
        let _ = writeln!(self.body, "  <cim:{class} rdf:ID=\"_{id}\">");
    }

    fn close(&mut self, class: &str) {
        let _ = writeln!(self.body, "  </cim:{class}>");
    }

    fn text(&mut self, prop: &str, value: &str) {
        let _ = writeln!(
            self.body,
            "    <cim:{prop}>{}</cim:{prop}>",
            esc_text(value)
        );
    }

    fn number(&mut self, what: &str, prop: &str, value: f64) {
        let value = if value.is_finite() {
            value
        } else {
            self.warn(
                &C::EMIT_CIM_DIST_VALUE_SUBSTITUTED,
                format!("{what}: {prop} is {value}, which CIM cannot state; 0 is written"),
            );
            0.0
        };
        let _ = writeln!(self.body, "    <cim:{prop}>{value}</cim:{prop}>");
    }

    fn integer(&mut self, prop: &str, value: usize) {
        let _ = writeln!(self.body, "    <cim:{prop}>{value}</cim:{prop}>");
    }

    fn boolean(&mut self, prop: &str, value: bool) {
        let _ = writeln!(self.body, "    <cim:{prop}>{value}</cim:{prop}>");
    }

    fn reference(&mut self, prop: &str, target: &str) {
        let _ = writeln!(self.body, "    <cim:{prop} rdf:resource=\"#_{target}\"/>");
    }

    fn enumeration(&mut self, prop: &str, value: &str) {
        let _ = writeln!(
            self.body,
            "    <cim:{prop} rdf:resource=\"{CIM_NS}{value}\"/>"
        );
    }

    /// Open an object with its mRID and name.
    fn named(&mut self, class: &str, id: &str, name: &str) {
        self.open(class, id);
        self.text("IdentifiedObject.mRID", id);
        self.text("IdentifiedObject.name", name);
    }

    /// One terminal of `owner` at sequence `seq` on `node`.
    fn terminal(&mut self, owner: &str, seq: usize, node: &str, phases: Option<&str>) -> String {
        let id = det_mrid("terminal", &format!("{owner}:{seq}"));
        self.named("Terminal", &id, &format!("{owner}_T{seq}"));
        self.reference("Terminal.ConductingEquipment", owner);
        self.reference("Terminal.ConnectivityNode", node);
        self.integer("ACDCTerminal.sequenceNumber", seq);
        if let Some(code) = phases {
            self.enumeration("Terminal.phases", &format!("PhaseCode.{code}"));
        }
        self.close("Terminal");
        id
    }

    /// The imported mRID (the `cim_mrid` extra) when it can stand as an
    /// `rdf:ID`, else a deterministic one.
    fn mrid_of(&mut self, extras: &Extras, kind: &str, name: &str) -> String {
        match extras.get("cim_mrid").and_then(serde_json::Value::as_str) {
            Some(id) if plain_id(id) => id.to_owned(),
            Some(id) => {
                let derived = det_mrid(kind, name);
                self.warn(
                    &C::EMIT_CIM_DIST_VALUE_SUBSTITUTED,
                    format!(
                        "{kind} {name}: mRID `{id}` cannot stand as an rdf:ID; `{derived}` is \
                         written"
                    ),
                );
                derived
            }
            None => det_mrid(kind, name),
        }
    }

    /// Report extras the writer does not regenerate.
    fn extras_dropped(&mut self, kind: &str, name: &str, extras: &Extras, used: &[&str]) {
        let keys: Vec<&str> = extras
            .keys()
            .map(String::as_str)
            .filter(|k| !used.contains(k))
            .collect();
        if !keys.is_empty() {
            self.warn(
                &C::EMIT_CIM_DIST_EXTRAS_DROPPED,
                format!(
                    "{kind} {name}: extras [{}] have no distribution CIM property",
                    keys.join(", ")
                ),
            );
        }
    }

    /// The `PhaseCode` of a map, reporting a terminal order the code cannot
    /// carry.
    fn ordered_code(
        &mut self,
        kind: &str,
        name: &str,
        map: &[String],
        neutral: bool,
    ) -> Option<String> {
        let (code, in_order) = phase_code(map, neutral);
        if !in_order {
            self.warn(
                &C::EMIT_CIM_DIST_VALUE_COLLAPSED,
                format!(
                    "{kind} {name}: terminal order [{}] is not expressible in a CIM PhaseCode; the \
                     phases are written in A, B, C, N order",
                    map.join(", ")
                ),
            );
        }
        code
    }

    /// Report a conductor that repeats a phase another conductor of the same
    /// element already states (bundled conductors on one terminal): CIM
    /// states each phase of an element once.
    fn repeated_phase(&mut self, kind: &str, name: &str, phase: &str) {
        self.warn(
            &C::EMIT_CIM_DIST_VALUE_COLLAPSED,
            format!(
                "{kind} {name}: more than one conductor is on phase {phase}; CIM states each \
                 phase once, so the repeat is not written"
            ),
        );
    }

    /// Report conductors whose terminal name has no `SinglePhaseKind`.
    fn unnamed_conductors(&mut self, kind: &str, name: &str, map: &[String]) {
        let unnamed: Vec<&str> = map
            .iter()
            .map(String::as_str)
            .filter(|t| phase_letter(t).is_none())
            .collect();
        if !unnamed.is_empty() {
            self.warn(
                &C::EMIT_CIM_DIST_FIELD_DROPPED,
                format!(
                    "{kind} {name}: conductors [{}] have no SinglePhaseKind (A, B, C, N) and are \
                     not written",
                    unnamed.join(", ")
                ),
            );
        }
    }
}

/// Serialize `net` as one distribution CIM document. Everything the profile
/// cannot carry is reported in the diagnostics.
// One element family per block, in model order; the exact float comparisons
// detect structure (asymmetry, unequal ratings) the profile cannot state.
#[allow(clippy::too_many_lines, clippy::float_cmp)]
pub(crate) fn emit_cim_dist_text(net: &MulticonductorNetwork) -> TextEmission {
    let mut doc = Doc {
        body: String::new(),
        diags: Diagnostics::new(),
    };

    let frequency = det_mrid("frequency", "base");
    doc.named(
        "BaseFrequency",
        &frequency,
        &format!("{} Hz", net.base_frequency()),
    );
    doc.number("network", "BaseFrequency.frequency", net.base_frequency());
    doc.close("BaseFrequency");

    if let Some(name) = net.name() {
        let feeder = det_mrid("feeder", name);
        doc.named("Feeder", &feeder, name);
        doc.close("Feeder");
    }

    let mut node_of = std::collections::HashMap::new();
    for bus in net.buses() {
        let id = doc.mrid_of(&bus.extras, "node", &bus.id);
        node_of.insert(bus.id.to_ascii_lowercase(), id.clone());
        doc.named("ConnectivityNode", &id, &bus.id);
        doc.close("ConnectivityNode");
        bus_losses(&mut doc, bus);
    }
    let node = |doc: &mut Doc, bus: &str| -> String {
        node_of
            .get(&bus.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| {
                doc.warn(
                    &C::EMIT_CIM_DIST_VALUE_SUBSTITUTED,
                    format!("bus `{bus}` is not declared; a connectivity node id is derived"),
                );
                det_mrid("node", bus)
            })
    };

    // The conductors each segment can state: the first conductor on each
    // phase, in terminal order.
    let mut columns_of: Vec<Vec<(usize, &'static str)>> = Vec::with_capacity(net.lines().len());
    for line in net.lines() {
        let mut written = std::collections::HashSet::new();
        let mut columns = Vec::new();
        for (index, terminal) in line.terminal_map_from.iter().enumerate() {
            let Some(phase) = phase_letter(terminal) else {
                continue;
            };
            if !written.insert(phase) {
                doc.repeated_phase("line", &line.name, phase);
                continue;
            }
            columns.push((index, phase));
        }
        columns_of.push(columns);
    }

    // Line codes as PerLengthPhaseImpedance with lower triangle data rows.
    let mut code_of = std::collections::HashMap::new();
    for code in net.line_codes() {
        if code.n_conductors == 0 {
            doc.warn(
                &C::EMIT_CIM_DIST_RECORD_DROPPED,
                format!("linecode {}: it has no conductors to write", code.name),
            );
            continue;
        }
        let id = doc.mrid_of(&code.extras, "plpi", &code.name);
        code_of.insert(code.name.to_ascii_lowercase(), id.clone());
        let all: Vec<usize> = (0..code.n_conductors).collect();
        write_phase_impedance(&mut doc, &id, &code.name, code, &all);
        if code.i_max.is_some() || code.s_max.is_some() {
            doc.warn(
                &C::EMIT_CIM_DIST_FIELD_DROPPED,
                format!(
                    "linecode {}: ampacity and apparent power ratings are not written (no \
                     operational limit mapping)",
                    code.name
                ),
            );
        }
        if code.source.is_some() {
            doc.warn(
                &C::EMIT_CIM_DIST_FIELD_DROPPED,
                format!(
                    "linecode {}: matrix provenance `source` has no CIM property",
                    code.name
                ),
            );
        }
        doc.extras_dropped("linecode", &code.name, &code.extras, &["cim_mrid"]);
    }

    // A segment naming other conductors than its line code holds refers to
    // the code's block for the conductors it names, so the reader takes back
    // exactly what was written. The blocks are written before every segment,
    // where a reread line code would stand.
    let mut projections: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut impedance_of: Vec<Option<String>> = Vec::with_capacity(net.lines().len());
    for (line, columns) in net.lines().iter().zip(&columns_of) {
        let impedance = match net.linecode(&line.linecode) {
            Some(code)
                if code.n_conductors > 0
                    && columns.len() == code.n_conductors
                    && columns
                        .iter()
                        .enumerate()
                        .all(|(k, (index, _))| k == *index) =>
            {
                code_of.get(&code.name.to_ascii_lowercase()).cloned()
            }
            Some(code) if !columns.is_empty() => {
                let phases: String = columns.iter().map(|(_, p)| *p).collect();
                let name = format!("{}_{phases}", code.name);
                let id = if let Some(id) = projections.get(&name) {
                    id.clone()
                } else {
                    let id = det_mrid("plpi", &name);
                    let rows: Vec<usize> = columns.iter().map(|(index, _)| *index).collect();
                    write_phase_impedance(&mut doc, &id, &name, code, &rows);
                    projections.insert(name, id.clone());
                    id
                };
                Some(id)
            }
            _ => None,
        };
        impedance_of.push(impedance);
    }

    for ((line, columns), impedance) in net.lines().iter().zip(&columns_of).zip(impedance_of) {
        if columns.is_empty() {
            doc.warn(
                &C::EMIT_CIM_DIST_RECORD_DROPPED,
                format!(
                    "line {}: no conductor has a SinglePhaseKind (A, B, C, N)",
                    line.name
                ),
            );
            continue;
        }
        let id = doc.mrid_of(&line.extras, "line", &line.name);
        doc.named("ACLineSegment", &id, &line.name);
        doc.number(
            &format!("line {}", line.name),
            "Conductor.length",
            line.length,
        );
        match impedance {
            Some(code) => doc.reference("ACLineSegment.PerLengthImpedance", &code),
            None => doc.warn(
                &C::EMIT_CIM_DIST_FIELD_DROPPED,
                format!(
                    "line {}: linecode `{}` is not defined; the segment is written without \
                     impedance",
                    line.name, line.linecode
                ),
            ),
        }
        doc.close("ACLineSegment");
        let from = node(&mut doc, &line.bus_from);
        let to = node(&mut doc, &line.bus_to);
        // The conductor order rides ACLineSegmentPhase.sequenceNumber.
        doc.terminal(
            &id,
            1,
            &from,
            phase_code(&line.terminal_map_from, false).0.as_deref(),
        );
        doc.terminal(
            &id,
            2,
            &to,
            phase_code(&line.terminal_map_to, false).0.as_deref(),
        );
        for (sequence, (_, phase)) in columns.iter().enumerate() {
            let pid = det_mrid("linephase", &format!("{id}:{phase}"));
            doc.named(
                "ACLineSegmentPhase",
                &pid,
                &format!("{}_{phase}", line.name),
            );
            doc.reference("ACLineSegmentPhase.ACLineSegment", &id);
            doc.enumeration(
                "ACLineSegmentPhase.phase",
                &format!("SinglePhaseKind.{phase}"),
            );
            doc.integer("ACLineSegmentPhase.sequenceNumber", sequence + 1);
            doc.close("ACLineSegmentPhase");
        }
        doc.unnamed_conductors("line", &line.name, &line.terminal_map_from);
        if line.terminal_map_from != line.terminal_map_to {
            doc.warn(
                &C::EMIT_CIM_DIST_VALUE_COLLAPSED,
                format!(
                    "line {}: the from and to terminal maps differ; the conductor phases are \
                     written from the from side",
                    line.name
                ),
            );
        }
        if line.i_max.is_some() || line.s_max.is_some() {
            doc.warn(
                &C::EMIT_CIM_DIST_FIELD_DROPPED,
                format!(
                    "line {}: ampacity and apparent power ratings are not written (no operational \
                     limit mapping)",
                    line.name
                ),
            );
        }
        doc.extras_dropped("line", &line.name, &line.extras, &["cim_mrid"]);
    }

    for load in net.loads() {
        // The phase conductors with their powers, in map order, and whether
        // the load closes between two phases rather than to a star point. A
        // single phase load states one power: its first conductor is the
        // phase and its second the return.
        let named = |t: &str| phase_letter(t).is_some_and(|p| p != "N");
        let (phases, phase_to_phase): (Vec<(&str, f64, f64)>, bool) =
            if load.configuration == Configuration::SinglePhase {
                let first = load.terminal_map.first().map(String::as_str);
                let second = load.terminal_map.get(1).map(String::as_str);
                let power = (
                    load.p_nom.first().copied().unwrap_or(0.0),
                    load.q_nom.first().copied().unwrap_or(0.0),
                );
                match first {
                    Some(first) if named(first) => {
                        (vec![(first, power.0, power.1)], second.is_some_and(named))
                    }
                    _ => (Vec::new(), false),
                }
            } else {
                let mut out = Vec::new();
                for (n, terminal) in load
                    .terminal_map
                    .iter()
                    .filter(|t| !is_neutral(t))
                    .enumerate()
                {
                    if named(terminal) {
                        out.push((
                            terminal.as_str(),
                            load.p_nom.get(n).copied().unwrap_or(0.0),
                            load.q_nom.get(n).copied().unwrap_or(0.0),
                        ));
                    }
                }
                (out, false)
            };
        if phases.is_empty() {
            doc.warn(
                &C::EMIT_CIM_DIST_RECORD_DROPPED,
                format!(
                    "load {}: no phase conductor has a SinglePhaseKind (A, B, C)",
                    load.name
                ),
            );
            continue;
        }
        let id = doc.mrid_of(&load.extras, "load", &load.name);
        let what = format!("load {}", load.name);
        doc.named("EnergyConsumer", &id, &load.name);
        let delta = load.configuration == Configuration::Delta || phase_to_phase;
        doc.enumeration(
            "EnergyConsumer.phaseConnection",
            if delta {
                "PhaseShuntConnectionKind.D"
            } else {
                "PhaseShuntConnectionKind.Y"
            },
        );
        doc.number(
            &what,
            "EnergyConsumer.p",
            phases.iter().map(|(_, p, _)| p).sum(),
        );
        doc.number(
            &what,
            "EnergyConsumer.q",
            phases.iter().map(|(_, _, q)| q).sum(),
        );
        doc.close("EnergyConsumer");
        let bus = node(&mut doc, &load.bus);
        // A phase to phase load states both phases on its terminal and its
        // one power on the load itself.
        let stated: Vec<String> = if phase_to_phase {
            load.terminal_map.iter().take(2).cloned().collect()
        } else {
            phases.iter().map(|(t, _, _)| (*t).to_owned()).collect()
        };
        let code = doc.ordered_code("load", &load.name, &stated, !delta);
        doc.terminal(&id, 1, &bus, code.as_deref());
        let children = if phase_to_phase { Vec::new() } else { phases };
        let mut written = std::collections::HashSet::new();
        for (terminal, p, q) in children {
            let Some(phase) = phase_letter(terminal) else {
                continue;
            };
            if !written.insert(phase) {
                doc.repeated_phase("load", &load.name, phase);
                continue;
            }
            let pid = det_mrid("loadphase", &format!("{id}:{phase}"));
            doc.named(
                "EnergyConsumerPhase",
                &pid,
                &format!("{}_{phase}", load.name),
            );
            doc.reference("EnergyConsumerPhase.EnergyConsumer", &id);
            doc.enumeration(
                "EnergyConsumerPhase.phase",
                &format!("SinglePhaseKind.{phase}"),
            );
            doc.number(&what, "EnergyConsumerPhase.p", p);
            doc.number(&what, "EnergyConsumerPhase.q", q);
            doc.close("EnergyConsumerPhase");
        }
        doc.unnamed_conductors("load", &load.name, &load.terminal_map);
        if !matches!(
            load.voltage_model,
            DistLoadVoltageModel::ConstantPower { .. }
        ) {
            doc.warn(
                &C::EMIT_CIM_DIST_FIELD_DROPPED,
                format!(
                    "load {}: the voltage dependence model is not written (no \
                     LoadResponseCharacteristic mapping)",
                    load.name
                ),
            );
        }
        doc.extras_dropped("load", &load.name, &load.extras, &["cim_mrid"]);
    }

    for shunt in net.shunts() {
        let id = doc.mrid_of(&shunt.extras, "shunt", &shunt.name);
        let what = format!("shunt {}", shunt.name);
        doc.named("LinearShuntCompensator", &id, &shunt.name);
        let delta = balanced_delta(&shunt.terminal_map, &shunt.g, &shunt.b);
        doc.enumeration(
            "ShuntCompensator.phaseConnection",
            if delta.is_some() {
                "PhaseShuntConnectionKind.D"
            } else {
                "PhaseShuntConnectionKind.Y"
            },
        );
        doc.integer("ShuntCompensator.sections", 1);
        doc.integer("ShuntCompensator.normalSections", 1);
        doc.integer("ShuntCompensator.maximumSections", 1);
        if let Some((g, b)) = delta {
            // The positive sequence (star equivalent) admittance of three
            // equal delta branches is three times a branch's.
            let per_section = |y: f64| exact_preimage(y, 3.0 * y, |v| v * 1.0 / 3.0);
            doc.number(&what, "LinearShuntCompensator.bPerSection", per_section(b));
            doc.number(&what, "LinearShuntCompensator.gPerSection", per_section(g));
        }
        doc.close("LinearShuntCompensator");
        let bus = node(&mut doc, &shunt.bus);
        let code = doc.ordered_code("shunt", &shunt.name, &shunt.terminal_map, false);
        doc.terminal(&id, 1, &bus, code.as_deref());
        if delta.is_some() {
            doc.extras_dropped("shunt", &shunt.name, &shunt.extras, &["cim_mrid"]);
            continue;
        }
        let diag =
            |m: &ConductorMatrix, n: usize| m.get(n).and_then(|r| r.get(n)).copied().unwrap_or(0.0);
        let mut written = std::collections::HashSet::new();
        for (n, terminal) in shunt.terminal_map.iter().enumerate() {
            let Some(phase) = phase_letter(terminal) else {
                continue;
            };
            if !written.insert(phase) {
                doc.repeated_phase("shunt", &shunt.name, phase);
                continue;
            }
            let pid = det_mrid("shuntphase", &format!("{id}:{phase}"));
            doc.named(
                "LinearShuntCompensatorPhase",
                &pid,
                &format!("{}_{phase}", shunt.name),
            );
            doc.reference("ShuntCompensatorPhase.ShuntCompensator", &id);
            doc.enumeration(
                "ShuntCompensatorPhase.phase",
                &format!("SinglePhaseKind.{phase}"),
            );
            doc.number(
                &what,
                "LinearShuntCompensatorPhase.bPerSection",
                diag(&shunt.b, n),
            );
            doc.number(
                &what,
                "LinearShuntCompensatorPhase.gPerSection",
                diag(&shunt.g, n),
            );
            doc.close("LinearShuntCompensatorPhase");
        }
        doc.unnamed_conductors("shunt", &shunt.name, &shunt.terminal_map);
        if has_off_diagonal(&shunt.b) || has_off_diagonal(&shunt.g) {
            doc.warn(
                &C::EMIT_CIM_DIST_VALUE_COLLAPSED,
                format!(
                    "shunt {}: off diagonal admittance has no per phase CIM property; the \
                     diagonal is written",
                    shunt.name
                ),
            );
        }
        doc.extras_dropped("shunt", &shunt.name, &shunt.extras, &["cim_mrid"]);
    }

    for switch in net.switches() {
        let id = doc.mrid_of(&switch.extras, "switch", &switch.name);
        let class = switch
            .extras
            .get("cim_class")
            .and_then(serde_json::Value::as_str)
            .filter(|c| SWITCH_CLASSES.contains(c))
            .unwrap_or("LoadBreakSwitch");
        doc.named(class, &id, &switch.name);
        doc.boolean("Switch.normalOpen", switch.open);
        doc.boolean("Switch.open", switch.open);
        if let Some(rated) = &switch.i_max {
            if let Some(first) = rated.first() {
                doc.number(
                    &format!("switch {}", switch.name),
                    "Switch.ratedCurrent",
                    *first,
                );
            }
            if rated.windows(2).any(|w| w[0] != w[1]) {
                doc.warn(
                    &C::EMIT_CIM_DIST_VALUE_COLLAPSED,
                    format!(
                        "switch {}: per conductor current ratings differ; the first is written as \
                         Switch.ratedCurrent",
                        switch.name
                    ),
                );
            }
        }
        doc.close(class);
        let from = node(&mut doc, &switch.bus_from);
        let to = node(&mut doc, &switch.bus_to);
        // The side pairing rides SwitchPhase.phaseSide1 and phaseSide2.
        doc.terminal(
            &id,
            1,
            &from,
            phase_code(&switch.terminal_map_from, false).0.as_deref(),
        );
        doc.terminal(
            &id,
            2,
            &to,
            phase_code(&switch.terminal_map_to, false).0.as_deref(),
        );
        let mut written = std::collections::HashSet::new();
        for (side1, side2) in switch.terminal_map_from.iter().zip(&switch.terminal_map_to) {
            let (Some(p1), Some(p2)) = (phase_letter(side1), phase_letter(side2)) else {
                continue;
            };
            if !written.insert(p1) {
                doc.repeated_phase("switch", &switch.name, p1);
                continue;
            }
            let pid = det_mrid("switchphase", &format!("{id}:{p1}"));
            doc.named("SwitchPhase", &pid, &format!("{}_{p1}", switch.name));
            doc.reference("SwitchPhase.Switch", &id);
            doc.enumeration("SwitchPhase.phaseSide1", &format!("SinglePhaseKind.{p1}"));
            doc.enumeration("SwitchPhase.phaseSide2", &format!("SinglePhaseKind.{p2}"));
            doc.boolean("SwitchPhase.normalOpen", switch.open);
            doc.boolean("SwitchPhase.closed", !switch.open);
            doc.close("SwitchPhase");
        }
        doc.unnamed_conductors("switch", &switch.name, &switch.terminal_map_from);
        doc.extras_dropped(
            "switch",
            &switch.name,
            &switch.extras,
            &["cim_mrid", "cim_class"],
        );
    }

    // Transformers that came from the tank form go back to it, grouped
    // under their PowerTransformer; every other transformer takes the
    // PowerTransformerEnd form.
    let tank_codes: Vec<Option<Vec<&'static str>>> = net
        .transformers()
        .iter()
        .map(|xf| tank_phase_codes(&mut doc, xf))
        .collect();
    let mut groups: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (k, xf) in net.transformers().iter().enumerate() {
        if tank_codes[k].is_some() {
            groups.entry(tank_group(xf)).or_default().push(k);
        }
    }
    let mut catalogs = TankCatalogs::default();
    for (k, xf) in net.transformers().iter().enumerate() {
        if xf.windings.len() < 2 {
            doc.warn(
                &C::EMIT_CIM_DIST_RECORD_DROPPED,
                format!(
                    "transformer {}: {} winding(s); a CIM transformer needs at least two",
                    xf.name,
                    xf.windings.len()
                ),
            );
            continue;
        }
        if tank_codes[k].is_some() {
            let Some(members) = groups.remove(&tank_group(xf)) else {
                continue;
            };
            let pt = match xf
                .extras
                .get("cim_power_transformer")
                .and_then(serde_json::Value::as_str)
            {
                Some(id) if plain_id(id) => id.to_owned(),
                _ => det_mrid("xf", &tank_group(xf)),
            };
            let pt_name = xf
                .extras
                .get("cim_power_transformer_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&xf.name)
                .to_owned();
            doc.named("PowerTransformer", &pt, &pt_name);
            doc.close("PowerTransformer");
            let mut sequence = 0;
            for m in members {
                let tank = &net.transformers()[m];
                let codes = tank_codes[m].as_deref().unwrap_or_default();
                let buses: Vec<String> = tank
                    .windings
                    .iter()
                    .map(|w| node(&mut doc, &w.bus))
                    .collect();
                write_tank(
                    &mut doc,
                    &mut catalogs,
                    tank,
                    codes,
                    &buses,
                    &pt,
                    &mut sequence,
                );
            }
            continue;
        }
        let buses: Vec<String> = xf.windings.iter().map(|w| node(&mut doc, &w.bus)).collect();
        write_end_form(&mut doc, xf, &buses);
    }
    catalogs.write(&mut doc);

    for source in net.sources() {
        let bus = node(&mut doc, &source.bus);
        write_source(&mut doc, source, &bus);
    }

    for (what, count) in [
        ("generator", net.generators().len()),
        ("inverter based resource", net.ibrs().len()),
        ("control profile", net.control_profiles().len()),
        ("capacitor", net.capacitors().len()),
        ("untyped object", net.untyped_objects().len()),
        ("source command", net.commands().len()),
        ("source option", net.options().len()),
    ] {
        if count > 0 {
            doc.warn(
                &C::EMIT_CIM_DIST_RECORD_DROPPED,
                format!("{count} {what}(s) have no distribution CIM mapping yet and are dropped"),
            );
        }
    }
    if net.geo().is_some() {
        doc.warn(
            &C::EMIT_CIM_DIST_FIELD_DROPPED,
            "the network coordinate metadata is not written (no Location mapping)".to_owned(),
        );
    }
    if !net.extras().is_empty() {
        let keys: Vec<&str> = net.extras().keys().map(String::as_str).collect();
        doc.warn(
            &C::EMIT_CIM_DIST_EXTRAS_DROPPED,
            format!(
                "network extras [{}] have no distribution CIM property",
                keys.join(", ")
            ),
        );
    }

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<rdf:RDF xmlns:rdf=\"{RDF_NS}\"\n         xmlns:cim=\"{CIM_NS}\">"
    );
    out.push_str(&doc.body);
    out.push_str("</rdf:RDF>\n");
    TextEmission::new(out, Vec::new(), doc.diags)
}

/// A numeric extra, stated as a number or as numeric text (the dss reader
/// keeps property values as written).
fn extras_number(extras: &Extras, key: &str) -> Option<f64> {
    let value = extras.get(key)?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|v: &f64| v.is_finite())
}

/// The no-load percentages `(%noloadloss, %imag)` a transformer's extras
/// state, either defaulting to 0 when only the other is stated.
fn no_load_extras(extras: &Extras) -> Option<(f64, f64)> {
    let loss = extras_number(extras, "%noloadloss");
    let imag = extras_number(extras, "%imag");
    (loss.is_some() || imag.is_some()).then(|| (loss.unwrap_or(0.0), imag.unwrap_or(0.0)))
}

/// A winding's grounding: a neutral impedance writes `grounded` with
/// `rground` and `xground`.
fn write_grounding(doc: &mut Doc, what: &str, winding: &DistWinding) {
    if let Some(r) = winding.r_neutral {
        doc.boolean("TransformerEnd.grounded", r >= 0.0);
        if r >= 0.0 {
            doc.number(what, "TransformerEnd.rground", r);
            doc.number(
                what,
                "TransformerEnd.xground",
                winding.x_neutral.unwrap_or(0.0),
            );
        }
    } else if winding.x_neutral.is_some() {
        doc.boolean("TransformerEnd.grounded", true);
        doc.number(what, "TransformerEnd.rground", 0.0);
        doc.number(
            what,
            "TransformerEnd.xground",
            winding.x_neutral.unwrap_or(0.0),
        );
    }
}

/// An off nominal tap as a `RatioTapChanger` on `end`: one step of the
/// exact offset, so the reader recovers the stated tap,
/// `1 + (step − neutralStep) · increment / 100`.
fn write_tap(doc: &mut Doc, what: &str, name: &str, end: &str, number: usize, tap: f64) {
    if (tap - 1.0).abs() > 0.0 {
        write_changer(doc, what, name, end, number, tap);
    }
}

fn write_changer(doc: &mut Doc, what: &str, name: &str, end: &str, number: usize, tap: f64) {
    let changer = det_mrid("tap", end);
    doc.named("RatioTapChanger", &changer, &format!("{name}_tap{number}"));
    doc.reference("RatioTapChanger.TransformerEnd", end);
    doc.integer("TapChanger.lowStep", 0);
    doc.integer("TapChanger.highStep", 1);
    doc.integer("TapChanger.neutralStep", 0);
    doc.integer("TapChanger.normalStep", 1);
    doc.number(what, "TapChanger.step", 1.0);
    let increment = exact_preimage(tap, (tap - 1.0) * 100.0, |i| 1.0 + (1.0 - 0.0) * i / 100.0);
    doc.number(what, "RatioTapChanger.stepVoltageIncrement", increment);
    doc.close("RatioTapChanger");
}

/// A transformer in the `PowerTransformerEnd` form: one end per winding,
/// the pair reactance on end 1 (two windings) or as mesh impedances (more),
/// and the no-load extras as a `TransformerCoreAdmittance` on end 1.
#[allow(clippy::too_many_lines)] // one object family per block, in output order
fn write_end_form(doc: &mut Doc, xf: &DistTransformer, buses: &[String]) {
    let id = doc.mrid_of(&xf.extras, "xf", &xf.name);
    let what = format!("transformer {}", xf.name);
    doc.named("PowerTransformer", &id, &xf.name);
    doc.close("PowerTransformer");
    let imported_ends = imported_end_ids(xf);
    let s_base = xf.windings[0].s_rating;
    short_circuit_count(doc, xf);
    let mut end_ids = Vec::with_capacity(xf.windings.len());
    for (index, winding) in xf.windings.iter().enumerate() {
        let number = index + 1;
        let code = doc.ordered_code(
            &what,
            &format!("winding {number}"),
            &winding.terminal_map,
            false,
        );
        let terminal = doc.terminal(&id, number, &buses[index], code.as_deref());
        let end = imported_ends
            .get(index)
            .cloned()
            .flatten()
            .unwrap_or_else(|| det_mrid("xfend", &format!("{id}:{number}")));
        doc.named(
            "PowerTransformerEnd",
            &end,
            &format!("{}_end{number}", xf.name),
        );
        doc.reference("PowerTransformerEnd.PowerTransformer", &id);
        doc.integer("TransformerEnd.endNumber", number);
        doc.reference("TransformerEnd.Terminal", &terminal);
        doc.number(&what, "PowerTransformerEnd.ratedU", winding.v_ref);
        doc.number(&what, "PowerTransformerEnd.ratedS", winding.s_rating);
        let kind = match winding.conn {
            DistWindingConn::Delta => "D",
            _ if winding.terminal_map.iter().any(|t| is_neutral(t)) => "Yn",
            _ => "Y",
        };
        doc.enumeration(
            "PowerTransformerEnd.connectionKind",
            &format!("WindingConnection.{kind}"),
        );
        doc.number(&what, "PowerTransformerEnd.r", winding_r_ohm(winding));
        // Two windings carry the pair reactance on end 1; more windings
        // state each pair as a TransformerMeshImpedance.
        let x = match z_base(winding.v_ref, s_base) {
            Some(base) if index == 0 && xf.windings.len() == 2 => {
                let xsc = xf.xsc_pct.first().copied().unwrap_or(0.0);
                exact_preimage(xsc, xsc / 100.0 * base, |x| (x / base + 0.0) * 100.0)
            }
            _ => 0.0,
        };
        doc.number(&what, "PowerTransformerEnd.x", x);
        write_grounding(doc, &what, winding);
        doc.close("PowerTransformerEnd");
        doc.unnamed_conductors(&what, &format!("winding {number}"), &winding.terminal_map);
        write_tap(doc, &what, &xf.name, &end, number, winding.tap);
        end_ids.push((end, winding.v_ref));
    }
    if xf.windings.len() > 2 {
        let mut pair = 0;
        for i in 0..end_ids.len() {
            for j in i + 1..end_ids.len() {
                let xsc = xf.xsc_pct.get(pair).copied().unwrap_or(0.0);
                pair += 1;
                let mesh = det_mrid("mesh", &format!("{id}:{i}:{j}"));
                doc.named(
                    "TransformerMeshImpedance",
                    &mesh,
                    &format!("{}_z{}{}", xf.name, i + 1, j + 1),
                );
                doc.reference("TransformerMeshImpedance.FromTransformerEnd", &end_ids[i].0);
                doc.reference("TransformerMeshImpedance.ToTransformerEnd", &end_ids[j].0);
                doc.number(&what, "TransformerMeshImpedance.r", 0.0);
                let x = match z_base(end_ids[i].1, s_base) {
                    Some(base) => exact_preimage(xsc, xsc / 100.0 * base, |x| x / base * 100.0),
                    None => 0.0,
                };
                doc.number(&what, "TransformerMeshImpedance.x", x);
                doc.close("TransformerMeshImpedance");
            }
        }
    }
    if let Some((loss, imag)) = no_load_extras(&xf.extras) {
        // g and b in siemens on end 1's impedance base, so the reader's
        // `100 · g · V²/S` recovers each percentage.
        let core = det_mrid("xfcore", &end_ids[0].0);
        doc.named(
            "TransformerCoreAdmittance",
            &core,
            &format!("{}_Yc", xf.name),
        );
        doc.reference("TransformerCoreAdmittance.TransformerEnd", &end_ids[0].0);
        let (g, b) = match z_base(end_ids[0].1, s_base) {
            Some(base) => (
                exact_preimage(loss, loss / 100.0 / base, |g| 100.0 * g * base),
                exact_preimage(imag, imag / 100.0 / base, |b| 100.0 * b * base),
            ),
            None => (0.0, 0.0),
        };
        doc.number(&what, "TransformerCoreAdmittance.g", g);
        doc.number(&what, "TransformerCoreAdmittance.b", b);
        doc.close("TransformerCoreAdmittance");
    }
    doc.extras_dropped(
        "transformer",
        &xf.name,
        &xf.extras,
        &["cim_mrid", "cim_end_mrids", "%noloadloss", "%imag"],
    );
}

/// The imported end mRIDs (the `cim_end_mrids` extra) that can stand as an
/// `rdf:ID`, by winding.
fn imported_end_ids(xf: &DistTransformer) -> Vec<Option<String>> {
    xf.extras
        .get("cim_end_mrids")
        .and_then(serde_json::Value::as_array)
        .map(|ends| {
            ends.iter()
                .map(|e| e.as_str().filter(|e| plain_id(e)).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Report a reactance count other than one per winding pair.
fn short_circuit_count(doc: &mut Doc, xf: &DistTransformer) {
    let pairs = xf.windings.len() * (xf.windings.len() - 1) / 2;
    if xf.xsc_pct.len() != pairs {
        doc.warn(
            &C::EMIT_CIM_DIST_VALUE_SUBSTITUTED,
            format!(
                "transformer {}: {} short circuit reactances for {pairs} winding pairs; missing \
                 pairs are written as 0",
                xf.name,
                xf.xsc_pct.len()
            ),
        );
    }
}

/// A winding's resistance in ohm on its own base, chosen so the reader's
/// percentage comes back exactly.
fn winding_r_ohm(winding: &DistWinding) -> f64 {
    match z_base(winding.v_ref, winding.s_rating) {
        Some(base) => exact_preimage(winding.r_pct, winding.r_pct / 100.0 * base, |r| {
            r / base * 100.0
        }),
        None => 0.0,
    }
}

/// The `PowerTransformer` a tank belongs to: its imported mRID, else the
/// tank's own name, so a tank without one stands alone.
fn tank_group(xf: &DistTransformer) -> String {
    xf.extras
        .get("cim_power_transformer")
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| format!("tank:{}", xf.name), str::to_owned)
}

/// The `TransformerTankEnd.phases` of each winding of a transformer whose
/// extras say `cim_form = "tank"` (decision D2), inverting the reader's
/// terminal maps. `None`, with a finding, when a winding has no tank phase
/// code; the transformer then takes the `PowerTransformerEnd` form.
fn tank_phase_codes(doc: &mut Doc, xf: &DistTransformer) -> Option<Vec<&'static str>> {
    if xf
        .extras
        .get("cim_form")
        .and_then(serde_json::Value::as_str)
        != Some("tank")
        || xf.windings.len() < 2
    {
        return None;
    }
    // A secondary written neutral first (`bus.0.2`) is the second half of a
    // center tap, and its sibling half is `s1`.
    let center_tap = xf.windings[1..]
        .iter()
        .any(|w| w.terminal_map.first().is_some_and(|t| is_neutral(t)));
    let mut codes = Vec::with_capacity(xf.windings.len());
    for (index, winding) in xf.windings.iter().enumerate() {
        let map: Vec<&str> = winding.terminal_map.iter().map(String::as_str).collect();
        let phase = |t: &str| phase_letter(t).filter(|p| *p != "N");
        let code = match (winding.conn, map.as_slice()) {
            (DistWindingConn::Wye, [p, n]) if index > 0 && center_tap && is_neutral(n) => {
                (phase(p) == Some("A")).then_some("s1")
            }
            (DistWindingConn::Wye, [n, p]) if index > 0 && center_tap && is_neutral(n) => {
                (phase(p) == Some("B")).then_some("s2")
            }
            (DistWindingConn::Wye, [p, n]) if is_neutral(n) => match phase(p) {
                Some("A") => Some("A"),
                Some("B") => Some("B"),
                Some("C") => Some("C"),
                _ => None,
            },
            (DistWindingConn::Delta, [p, q]) => match (phase(p), phase(q)) {
                (Some("A"), Some("B")) => Some("AB"),
                (Some("B"), Some("C")) => Some("BC"),
                (Some("A"), Some("C")) => Some("AC"),
                _ => None,
            },
            (DistWindingConn::Delta, [a, b, c]) | (DistWindingConn::Wye, [a, b, c, _]) => {
                let n_ok = map.len() == 3 || is_neutral(map[3]);
                (n_ok && phase(a) == Some("A") && phase(b) == Some("B") && phase(c) == Some("C"))
                    .then_some("ABC")
            }
            _ => None,
        };
        let Some(code) = code else {
            doc.warn(
                &C::EMIT_CIM_DIST_VALUE_COLLAPSED,
                format!(
                    "transformer {}: winding {} ({:?} on terminals [{}]) has no \
                     TransformerTankEnd phase code; the transformer is written in the \
                     PowerTransformerEnd form",
                    xf.name,
                    index + 1,
                    winding.conn,
                    winding.terminal_map.join(", ")
                ),
            );
            return None;
        };
        codes.push(code);
    }
    Some(codes)
}

/// One tank: the `TransformerTank`, a `TransformerTankEnd` per winding on
/// a terminal of the group's `PowerTransformer`, tap changers on its ends,
/// and its place in a shared catalog entry.
fn write_tank(
    doc: &mut Doc,
    catalogs: &mut TankCatalogs,
    xf: &DistTransformer,
    codes: &[&'static str],
    buses: &[String],
    pt: &str,
    sequence: &mut usize,
) {
    let id = doc.mrid_of(&xf.extras, "xftank", &xf.name);
    let what = format!("transformer {}", xf.name);
    doc.named("TransformerTank", &id, &xf.name);
    doc.reference("TransformerTank.PowerTransformer", pt);
    doc.close("TransformerTank");
    short_circuit_count(doc, xf);
    let imported_ends = imported_end_ids(xf);
    for (index, winding) in xf.windings.iter().enumerate() {
        let number = index + 1;
        *sequence += 1;
        let terminal = doc.terminal(pt, *sequence, &buses[index], None);
        let end = imported_ends
            .get(index)
            .cloned()
            .flatten()
            .unwrap_or_else(|| det_mrid("xftankend", &format!("{id}:{number}")));
        doc.named(
            "TransformerTankEnd",
            &end,
            &format!("{}_end{number}", xf.name),
        );
        doc.enumeration(
            "TransformerTankEnd.phases",
            &format!("PhaseCode.{}", codes[index]),
        );
        doc.reference("TransformerTankEnd.TransformerTank", &id);
        doc.integer("TransformerEnd.endNumber", number);
        doc.reference("TransformerEnd.Terminal", &terminal);
        write_grounding(doc, &what, winding);
        doc.close("TransformerTankEnd");
        write_tap(doc, &what, &xf.name, &end, number, winding.tap);
    }
    catalogs.add(doc, xf, codes, id);
    doc.extras_dropped(
        "transformer",
        &xf.name,
        &xf.extras,
        &[
            "cim_mrid",
            "cim_end_mrids",
            "cim_form",
            "cim_power_transformer",
            "cim_power_transformer_name",
            "cim_tank_info",
            "cim_phase_angle_clock",
            "%noloadloss",
            "%imag",
        ],
    );
}

/// One catalog winding: rated voltage and power, resistance (percent on its
/// own base), connection kind, and clock number.
#[derive(Clone, PartialEq)]
struct CatalogWinding {
    v_ref: f64,
    s_rating: f64,
    r_pct: f64,
    kind: &'static str,
    clock: f64,
}

/// One `TransformerTankInfo` to write, with the tanks its `Asset` lists.
struct TankCatalog {
    id: String,
    name: String,
    /// The bit pattern of everything the entry states, which decides
    /// sharing.
    signature: String,
    windings: Vec<CatalogWinding>,
    xsc_pct: Vec<f64>,
    no_load: Option<(f64, f64)>,
    tanks: Vec<String>,
}

/// The catalog entries tanks share, in order of first use.
#[derive(Default)]
struct TankCatalogs {
    entries: Vec<TankCatalog>,
}

impl TankCatalogs {
    /// File a tank under a catalog entry: its imported `cim_tank_info` when
    /// that entry states the same data, else the first entry with
    /// bit-identical data, else a new one.
    #[allow(clippy::float_cmp)] // phase clocks are discrete
    fn add(&mut self, doc: &mut Doc, xf: &DistTransformer, codes: &[&'static str], tank: String) {
        let clocks = xf
            .extras
            .get("cim_phase_angle_clock")
            .and_then(serde_json::Value::as_array);
        let three_phase = codes.first() == Some(&"ABC");
        let windings: Vec<CatalogWinding> = xf
            .windings
            .iter()
            .map(|w| CatalogWinding {
                v_ref: w.v_ref,
                s_rating: w.s_rating,
                r_pct: w.r_pct,
                kind: match (three_phase, w.conn) {
                    (true, DistWindingConn::Delta) => "D",
                    (true, _) => "Y",
                    (false, _) => "I",
                },
                clock: tank_phase_clock(&xf.windings[0], w, three_phase),
            })
            .collect();
        if clocks.is_some_and(|old| {
            old.iter()
                .zip(&windings)
                .any(|(v, w)| v.as_f64().is_some_and(|c| c != w.clock))
        }) {
            doc.warn(&C::EMIT_CIM_DIST_FIELD_DROPPED, format!(
                "transformer {}: retained phase-angle clocks disagree with the current winding layout; writing the represented clocks",
                xf.name
            ));
        }
        let pairs = xf.windings.len() * (xf.windings.len() - 1) / 2;
        let xsc_pct: Vec<f64> = (0..pairs)
            .map(|p| xf.xsc_pct.get(p).copied().unwrap_or(0.0))
            .collect();
        let no_load = no_load_extras(&xf.extras);
        let mut signature = String::new();
        for w in &windings {
            let _ = write!(
                signature,
                "{:x},{:x},{:x},{},{:x};",
                w.v_ref.to_bits(),
                w.s_rating.to_bits(),
                w.r_pct.to_bits(),
                w.kind,
                w.clock.to_bits()
            );
        }
        for x in &xsc_pct {
            let _ = write!(signature, "{:x},", x.to_bits());
        }
        if let Some((loss, imag)) = no_load {
            let _ = write!(signature, ";{:x},{:x}", loss.to_bits(), imag.to_bits());
        }
        let imported = xf
            .extras
            .get("cim_tank_info")
            .and_then(serde_json::Value::as_str)
            .filter(|id| plain_id(id));
        if let Some(id) = imported {
            match self.entries.iter_mut().find(|e| e.id == id) {
                Some(entry) if entry.signature == signature => {
                    entry.tanks.push(tank);
                    return;
                }
                Some(_) => doc.warn(
                    &C::EMIT_CIM_DIST_VALUE_SUBSTITUTED,
                    format!(
                        "transformer {}: its catalog data differs from another tank's under \
                         TransformerTankInfo `{id}`; it gets its own catalog entry",
                        xf.name
                    ),
                ),
                None => {
                    self.entries.push(TankCatalog {
                        id: id.to_owned(),
                        name: format!("{}_info", xf.name),
                        signature,
                        windings,
                        xsc_pct,
                        no_load,
                        tanks: vec![tank],
                    });
                    return;
                }
            }
        }
        if let Some(entry) = self.entries.iter_mut().find(|e| e.signature == signature) {
            entry.tanks.push(tank);
            return;
        }
        self.entries.push(TankCatalog {
            id: det_mrid("xftankinfo", &signature),
            name: format!("{}_info", xf.name),
            signature,
            windings,
            xsc_pct,
            no_load,
            tanks: vec![tank],
        });
    }

    /// Every catalog entry: its `PowerTransformerInfo` and
    /// `TransformerTankInfo`, a `TransformerEndInfo` per winding, a
    /// `ShortCircuitTest` per winding pair and a `NoLoadTest` when the
    /// no-load extras are present (each on winding 1's rating, so the
    /// reader's rescaling is the identity), and the `Asset` listing its
    /// tanks.
    #[allow(clippy::too_many_lines)] // one catalog class per block, in output order
    fn write(&self, doc: &mut Doc) {
        for entry in &self.entries {
            let what = format!("transformer catalog {}", entry.name);
            let container = det_mrid("xfpowerinfo", &entry.id);
            doc.named(
                "PowerTransformerInfo",
                &container,
                &format!("{}_PowerXfInfo", entry.name),
            );
            doc.close("PowerTransformerInfo");
            doc.named("TransformerTankInfo", &entry.id, &entry.name);
            doc.reference("TransformerTankInfo.PowerTransformerInfo", &container);
            doc.close("TransformerTankInfo");
            let s_1 = entry.windings[0].s_rating;
            let mut end_ids = Vec::with_capacity(entry.windings.len());
            for (index, w) in entry.windings.iter().enumerate() {
                let number = index + 1;
                let end = det_mrid("xfendinfo", &format!("{}:{number}", entry.id));
                doc.named(
                    "TransformerEndInfo",
                    &end,
                    &format!("{}_{number}", entry.name),
                );
                doc.reference("TransformerEndInfo.TransformerTankInfo", &entry.id);
                doc.integer("TransformerEndInfo.endNumber", number);
                doc.enumeration(
                    "TransformerEndInfo.connectionKind",
                    &format!("WindingConnection.{}", w.kind),
                );
                doc.number(&what, "TransformerEndInfo.phaseAngleClock", w.clock);
                doc.number(&what, "TransformerEndInfo.ratedU", w.v_ref);
                doc.number(&what, "TransformerEndInfo.ratedS", w.s_rating);
                let mut winding = DistWinding::new(
                    String::new(),
                    Vec::new(),
                    DistWindingConn::Wye,
                    w.v_ref,
                    w.s_rating,
                );
                winding.r_pct = w.r_pct;
                doc.number(&what, "TransformerEndInfo.r", winding_r_ohm(&winding));
                doc.close("TransformerEndInfo");
                end_ids.push(end);
            }
            // A percentage on a winding's own rating, on winding 1's.
            let on_base = |w: &CatalogWinding| {
                if w.s_rating > 0.0 {
                    w.r_pct * s_1 / w.s_rating
                } else {
                    0.0
                }
            };
            let mut pair = 0;
            for i in 0..entry.windings.len() {
                for j in i + 1..entry.windings.len() {
                    let x = entry.xsc_pct[pair];
                    pair += 1;
                    let (wi, wj) = (&entry.windings[i], &entry.windings[j]);
                    let loss = (on_base(wi) + on_base(wj)) * s_1 / 100.0 / 1000.0;
                    let leakage = test_leakage(x, loss, wi.v_ref, s_1);
                    let test = det_mrid("xfsctest", &format!("{}:{}:{}", entry.id, i + 1, j + 1));
                    doc.named(
                        "ShortCircuitTest",
                        &test,
                        &format!("{}_{}{}", entry.name, i + 1, j + 1),
                    );
                    doc.reference("ShortCircuitTest.EnergisedEnd", &end_ids[i]);
                    doc.reference("ShortCircuitTest.GroundedEnds", &end_ids[j]);
                    doc.number(&what, "ShortCircuitTest.leakageImpedance", leakage);
                    doc.number(&what, "ShortCircuitTest.loss", loss);
                    doc.number(&what, "TransformerTest.basePower", s_1);
                    doc.close("ShortCircuitTest");
                }
            }
            if let Some((loss_pct, imag)) = entry.no_load {
                let loss = exact_preimage(loss_pct, loss_pct * s_1 / 100.0 / 1000.0, |l| {
                    no_load_test_pct(l, 0.0, s_1, s_1).map_or(f64::NAN, |p| p.0)
                });
                let read_loss = no_load_test_pct(loss, 0.0, s_1, s_1).map_or(0.0, |p| p.0);
                let forward = if read_loss > 0.0 {
                    imag.hypot(read_loss)
                } else {
                    imag
                };
                let exciting = exact_preimage(imag, forward, |e| {
                    no_load_test_pct(loss, e, s_1, s_1).map_or(f64::NAN, |p| p.1)
                });
                let test = det_mrid("xfnltest", &entry.id);
                doc.named("NoLoadTest", &test, &format!("{}_1", entry.name));
                doc.reference("NoLoadTest.EnergisedEnd", &end_ids[0]);
                doc.number(
                    &what,
                    "NoLoadTest.energisedEndVoltage",
                    entry.windings[0].v_ref,
                );
                doc.number(&what, "NoLoadTest.excitingCurrent", exciting);
                doc.number(&what, "NoLoadTest.loss", loss);
                doc.number(&what, "TransformerTest.basePower", s_1);
                doc.close("NoLoadTest");
            }
            let asset = det_mrid("xfasset", &entry.id);
            doc.named("Asset", &asset, &format!("{}_asset", entry.name));
            doc.reference("Asset.AssetInfo", &entry.id);
            for tank in &entry.tanks {
                doc.reference("Asset.PowerSystemResources", tank);
            }
            doc.close("Asset");
        }
    }
}

/// The leakage impedance (ohm, referred to an energised end of rated
/// voltage `rated_u`, on a test base of `s_1`) whose reading with `loss`
/// recovers the pair reactance `x` exactly.
fn test_leakage(x: f64, loss: f64, rated_u: f64, s_1: f64) -> f64 {
    let Some(base) = z_base(rated_u, s_1) else {
        return 0.0;
    };
    let read = |leakage: f64| {
        short_circuit_test_pct(leakage, loss, rated_u, s_1, s_1, LEAKAGE_READING)
            .map_or(f64::NAN, |p| p.x)
    };
    let r =
        short_circuit_test_pct(0.0, loss, rated_u, s_1, s_1, LEAKAGE_READING).map_or(0.0, |p| p.r);
    let z = x.hypot(r);
    exact_preimage(x, z / 100.0 * base, read)
}

/// One `PerLengthPhaseImpedance` holding the rows and columns `rows` of a
/// line code, as lower triangle `PhaseImpedanceData` records.
#[allow(clippy::float_cmp)] // exact comparison detects asymmetry CIM cannot state
fn write_phase_impedance(
    doc: &mut Doc,
    id: &str,
    name: &str,
    code: &crate::model::DistLineCode,
    rows: &[usize],
) {
    doc.named("PerLengthPhaseImpedance", id, name);
    doc.integer("PerLengthPhaseImpedance.conductorCount", rows.len());
    doc.close("PerLengthPhaseImpedance");
    let at = |m: &ConductorMatrix, i: usize, j: usize| {
        m.get(i).and_then(|r| r.get(j)).copied().unwrap_or(0.0)
    };
    let has_g = code
        .g_from
        .iter()
        .chain(&code.g_to)
        .flatten()
        .any(|v| *v != 0.0);
    let what = format!("linecode {name}");
    let mut asymmetric = false;
    for (i, &row) in rows.iter().enumerate() {
        for (j, &col) in rows.iter().enumerate().take(i + 1) {
            let did = det_mrid("pid", &format!("{id}:{i}:{j}"));
            doc.open("PhaseImpedanceData", &did);
            doc.reference("PhaseImpedanceData.PhaseImpedance", id);
            doc.integer("PhaseImpedanceData.row", i + 1);
            doc.integer("PhaseImpedanceData.column", j + 1);
            doc.number(&what, "PhaseImpedanceData.r", at(&code.r_series, row, col));
            doc.number(&what, "PhaseImpedanceData.x", at(&code.x_series, row, col));
            if has_g {
                doc.number(
                    &what,
                    "PhaseImpedanceData.g",
                    at(&code.g_from, row, col) + at(&code.g_to, row, col),
                );
            }
            doc.number(
                &what,
                "PhaseImpedanceData.b",
                at(&code.b_from, row, col) + at(&code.b_to, row, col),
            );
            doc.close("PhaseImpedanceData");
            asymmetric |= at(&code.b_from, row, col) != at(&code.b_to, row, col)
                || at(&code.g_from, row, col) != at(&code.g_to, row, col)
                || at(&code.r_series, row, col) != at(&code.r_series, col, row)
                || at(&code.x_series, row, col) != at(&code.x_series, col, row);
        }
    }
    if asymmetric {
        doc.warn(
            &C::EMIT_CIM_DIST_VALUE_COLLAPSED,
            format!(
                "linecode {name}: CIM states one symmetric matrix and one total shunt admittance; \
                 the lower triangle and the sum of both ends are written"
            ),
        );
    }
}

/// What a bus states that a CIM connectivity node cannot: voltage bounds, a
/// location, grounding other than the implied neutral, and extras.
fn bus_losses(doc: &mut Doc, bus: &DistBus) {
    let bounds = bus.v_min.is_some()
        || bus.v_max.is_some()
        || bus.v_min_phase.is_some()
        || bus.v_max_phase.is_some()
        || bus.vpn_min.is_some()
        || bus.vpn_max.is_some()
        || bus.vpp_min.is_some()
        || bus.vpp_max.is_some()
        || bus.vpos_min.is_some()
        || bus.vpos_max.is_some()
        || bus.vneg_max.is_some()
        || bus.vzero_max.is_some()
        || bus.vn_max.is_some();
    if bounds {
        doc.warn(
            &C::EMIT_CIM_DIST_FIELD_DROPPED,
            format!(
                "bus {}: voltage bounds are not written (no operational limit mapping)",
                bus.id
            ),
        );
    }
    if bus.location.is_some() {
        doc.warn(
            &C::EMIT_CIM_DIST_FIELD_DROPPED,
            format!(
                "bus {}: the location is not written (no Location mapping)",
                bus.id
            ),
        );
    }
    // A reader grounds the neutral terminal and nothing else.
    let implied: Vec<&String> = bus.terminals.iter().filter(|t| is_neutral(t)).collect();
    let grounded: Vec<&String> = bus.grounded.iter().collect();
    if grounded != implied {
        doc.warn(
            &C::EMIT_CIM_DIST_VALUE_COLLAPSED,
            format!(
                "bus {}: grounded terminals [{}] are not written; a reader grounds the neutral \
                 terminal 4 when present",
                bus.id,
                bus.grounded.join(", ")
            ),
        );
    }
    doc.extras_dropped("bus", &bus.id, &bus.extras, &["cim_mrid"]);
}

fn write_source(doc: &mut Doc, source: &VoltageSource, node: &str) {
    let id = doc.mrid_of(&source.extras, "source", &source.name);
    let what = format!("source {}", source.name);
    let phases: Vec<(usize, &String)> = source
        .terminal_map
        .iter()
        .enumerate()
        .filter(|(_, t)| !is_neutral(t))
        .collect();
    let n = phases.len();
    let magnitude = |i: usize| source.v_magnitude.get(i).copied().unwrap_or(0.0);
    let angle = |i: usize| source.v_angle.get(i).copied().unwrap_or(0.0);
    let v_ln = phases.first().map_or(0.0, |(i, _)| magnitude(*i));
    let base_angle = phases.first().map_or(0.0, |(i, _)| angle(*i));
    let v_ll = if n >= 2 {
        let factor = 2.0 * (std::f64::consts::PI / n as f64).sin();
        exact_preimage(v_ln, v_ln * factor, |v| v / factor)
    } else {
        v_ln
    };
    let balanced = phases.iter().enumerate().all(|(k, (i, _))| {
        let expected = wrap_angle(base_angle - (k as f64) * 2.0 * std::f64::consts::PI / n as f64);
        (magnitude(*i) - v_ln).abs() <= 1e-9 * v_ln.abs().max(1.0)
            && wrap_angle(angle(*i) - expected).abs() <= 1e-9
    });
    if !balanced {
        doc.warn(
            &C::EMIT_CIM_DIST_VALUE_COLLAPSED,
            format!(
                "{what}: per terminal magnitudes or angles are not a balanced set; the first \
                 phase's magnitude and angle are written"
            ),
        );
    }
    doc.named("EnergySource", &id, &source.name);
    doc.number(&what, "EnergySource.voltageMagnitude", v_ll);
    doc.number(&what, "EnergySource.voltageAngle", base_angle);
    doc.number(&what, "EnergySource.nominalVoltage", v_ll);
    // `basekv` and `angle` are the dss reader's restatement of the nominal
    // voltage and angle written above.
    let mut used = vec!["cim_mrid", "basekv", "angle", "cim_sequence_impedance"];
    if let Some((r1, x1, r0, x0)) = sequence_impedance(source) {
        doc.number(&what, "EnergySource.r", r1);
        doc.number(&what, "EnergySource.x", x1);
        doc.number(&what, "EnergySource.r0", r0);
        doc.number(&what, "EnergySource.x0", x0);
        used.extend(["rs", "xs", "mvasc3", "mvasc1", "x1r1", "x0r0"]);
    }
    doc.close("EnergySource");
    let code = doc.ordered_code("source", &source.name, &source.terminal_map, true);
    doc.terminal(&id, 1, node, code.as_deref());
    doc.unnamed_conductors("source", &source.name, &source.terminal_map);
    if source.energy_cost_rate.is_some() {
        doc.warn(
            &C::EMIT_CIM_DIST_FIELD_DROPPED,
            format!("{what}: the energy cost rate has no CIM property"),
        );
    }
    doc.extras_dropped("source", &source.name, &source.extras, &used);
}

/// Positive and zero sequence series impedance, `z1 = self − mutual` and
/// `z0 = self + 2·mutual`, from the `rs`/`xs` phase matrices (the PMD
/// reader's and this reader's convention), else from a dss source's Thevenin
/// parameters with the OpenDSS defaults, as the PMD writer computes them.
/// `None` for a source that states neither (an ideal source).
fn sequence_impedance(source: &VoltageSource) -> Option<(f64, f64, f64, f64)> {
    if let Some(stated) = stated_sequence(source) {
        return Some(stated);
    }
    let row_parts = |m: &ConductorMatrix| -> Option<(f64, f64)> {
        let row = m.first()?;
        Some((*row.first()?, row.get(1).copied().unwrap_or(0.0)))
    };
    let parts = |key: &str| -> Option<(f64, f64)> {
        let row = source.extras.get(key)?.as_array()?.first()?.as_array()?;
        let self_z = row.first()?.as_f64()?;
        let mutual = row
            .get(1)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        Some((self_z, mutual))
    };
    let ((rs, rm), (xs, xm)) = match (parts("rs"), parts("xs")) {
        (Some(r), Some(x)) => (r, x),
        _ if source.extras.contains_key("basekv") => {
            let (r, x) = crate::pmd::thevenin(source, source.terminal_map.len());
            (row_parts(&r)?, row_parts(&x)?)
        }
        _ => return None,
    };
    Some((rs - rm, xs - xm, rs + 2.0 * rm, xs + 2.0 * xm))
}

/// The sequence values a CIM read stated, while the `rs`/`xs` matrices are
/// still exactly the ones it built from them.
#[allow(clippy::float_cmp)] // the matrices must be the reader's own, bit for bit
fn stated_sequence(source: &VoltageSource) -> Option<(f64, f64, f64, f64)> {
    let stated = source.extras.get("cim_sequence_impedance")?;
    let value = |key: &str| stated.get(key).and_then(serde_json::Value::as_f64);
    let (r1, x1, r0, x0) = (value("r")?, value("x")?, value("r0")?, value("x0")?);
    let n = source.terminal_map.len();
    let matrix = |key: &str| -> Option<Vec<Vec<f64>>> {
        source
            .extras
            .get(key)?
            .as_array()?
            .iter()
            .map(|row| {
                row.as_array()?
                    .iter()
                    .map(serde_json::Value::as_f64)
                    .collect()
            })
            .collect()
    };
    (matrix("rs")? == sequence_to_phase(r0, r1, n) && matrix("xs")? == sequence_to_phase(x0, x1, n))
        .then_some((r1, x1, r0, x0))
}

/// The branch conductance and susceptance of a balanced three phase delta
/// shunt: `2y` on the diagonal and `−y` off it, for both matrices.
fn balanced_delta(map: &[String], g: &ConductorMatrix, b: &ConductorMatrix) -> Option<(f64, f64)> {
    if map.len() != 3
        || !map
            .iter()
            .all(|t| phase_letter(t).is_some_and(|p| p != "N"))
    {
        return None;
    }
    let branch = |m: &ConductorMatrix| -> Option<f64> {
        if m.len() != 3 || m.iter().any(|row| row.len() != 3) {
            return None;
        }
        let y = -m[0][1];
        let tolerance = 1e-12 * y.abs().max(f64::MIN_POSITIVE);
        let matches = (0..3).all(|i| {
            (0..3).all(|j| {
                let expected = if i == j { 2.0 * y } else { -y };
                (m[i][j] - expected).abs() <= tolerance
            })
        });
        matches.then_some(y)
    };
    let b = branch(b).filter(|y| *y != 0.0)?;
    let g = if g.iter().flatten().all(|v| *v == 0.0) {
        0.0
    } else {
        branch(g)?
    };
    Some((g, b))
}

fn has_off_diagonal(m: &ConductorMatrix) -> bool {
    m.iter()
        .enumerate()
        .any(|(i, row)| row.iter().enumerate().any(|(j, v)| i != j && *v != 0.0))
}

/// A value near `forward` that the reader's `inverse` maps back to `target`
/// exactly, so a write, read, write cycle is byte stable. Unit conversions
/// round, so the plain forward value can come back one bit away; the search
/// steps outward a float at a time and keeps `forward` when no nearby value
/// is exact.
#[allow(clippy::float_cmp)] // exact equality is the property sought
fn exact_preimage(target: f64, forward: f64, inverse: impl Fn(f64) -> f64) -> f64 {
    if !forward.is_finite() || inverse(forward) == target {
        return forward;
    }
    let (mut below, mut above) = (forward, forward);
    for _ in 0..32 {
        below = below.next_down();
        above = above.next_up();
        if inverse(below) == target {
            return below;
        }
        if inverse(above) == target {
            return above;
        }
    }
    forward
}

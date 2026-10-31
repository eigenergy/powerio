//! [`DistNetwork`] → MG-RAVENS JSON.
//!
//! The writer regenerates the document shape the upstream OpenDSS-derived
//! examples carry: name-keyed hash tables nested under the CIM class
//! hierarchy, `Class::'name'` pointers, SI units, and the fixed
//! operational-limit type vocabulary. mRIDs are deterministic (imported ones
//! pass through the `ravens_mrid` extras), and every scaled emission
//! finishes on a read→write fixed point, so write → parse → write is byte
//! identical. Untyped objects a RAVENS parse preserved re-nest at their
//! recorded root paths; everything the schema cannot carry is warned by
//! element and field.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Number, Value};

use crate::convert::TextEmission;
use crate::diagnostics::codes as C;
use crate::model::{
    ConductorMatrix, Configuration, DistLoadVoltageModel, DistTransformer, DistWindingConn, Extras,
    IbrPrimeMover, MulticonductorNetwork,
};

use super::{MRID_KEY, NORMAL_OPEN_KEY, det_mrid, terminal_phase};

/// Schema release the writer targets; the upstream repository tags none, so
/// this mirrors the `RavensVersion` its own examples embed.
const RAVENS_VERSION: &str = "RAVENSv0.3.0-dev";
const RAVENS_VERSION_DATE: &str = "2025-03-11";
const CIM_VERSION: &str = "IEC61970CIM100";
const CIM_VERSION_DATE: &str = "2019-04-01";

/// The fixed operational-limit type names the upstream OpenDSS converter
/// keys every limit against: continuous (5e9 s) voltage bounds, and
/// continuous / one-day absolute-value ampacities.
const TYPE_V_LOW: &str = "lowType_5000000000.0s";
const TYPE_V_HIGH: &str = "highType_5000000000.0s";
const TYPE_I_NORM: &str = "absoluteValueType_5000000000.0s";
const TYPE_I_EMERG: &str = "absoluteValueType_86400.0s";

/// Serialize `net` as an MG-RAVENS JSON document, reporting every model
/// field the schema (as exercised by the upstream converters' conventions)
/// cannot carry.
///
/// # Panics
///
/// Never in practice: the document is maps, strings, and finite numbers (the
/// non-finite guard in `Writer::num` substitutes 0), which always serialize.
#[must_use]
pub(crate) fn emit_ravens_json_text(net: &MulticonductorNetwork) -> TextEmission {
    let mut w = Writer {
        warnings: crate::diagnostics::Diagnostics::new(),
        limit_sets: BTreeMap::new(),
        used_types: BTreeSet::new(),
        responses: BTreeMap::new(),
        profiles: BTreeMap::new(),
        bases: bus_bases(net),
        equipment_bases: BTreeMap::new(),
    };
    let doc = w.document(net);
    TextEmission::new(
        serde_json::to_string_pretty(&doc).expect("maps and finite numbers") + "\n",
        Vec::new(),
        w.warnings,
    )
}

/// Iterate the read→write cycle to its fixed point so a reparse reproduces
/// the emitted bits (one plain multiply/divide pair can drift one ULP, which
/// would break canonical-write idempotence; the loop is a guard that exits
/// on the first pass in practice).
#[allow(clippy::float_cmp)] // the fixed-point check is bit exact by design
fn stable_fixed(initial: f64, cycle: impl Fn(f64) -> f64) -> f64 {
    let mut y = initial;
    for _ in 0..4 {
        let cycled = cycle(y);
        if cycled == y || !cycled.is_finite() {
            break;
        }
        y = cycled;
    }
    y
}

fn pointer(class: &str, name: &str) -> Value {
    Value::String(format!("{class}::'{name}'"))
}

/// A `Terminal` entry: sequence number, phase code, node pointer, and an
/// optional operational limit set.
fn terminal(owner: &str, seq: u64, node: &str, phases: &str, limits: Option<&str>) -> Value {
    let mut t = identified("Terminal", &format!("{owner}_T{seq}"), None);
    t.insert("ACDCTerminal.sequenceNumber".into(), Value::from(seq));
    t.insert("Terminal.phases".into(), Value::String(phases.into()));
    t.insert(
        "Terminal.ConnectivityNode".into(),
        pointer("ConnectivityNode", node),
    );
    if let Some(set) = limits {
        t.insert(
            "ACDCTerminal.OperationalLimitSet".into(),
            pointer("OperationalLimitSet", set),
        );
    }
    Value::Object(t)
}

/// `Ravens.cimObjectType`, `IdentifiedObject.mRID`, `IdentifiedObject.name`:
/// the identity triple every emitted object starts from. The mRID is the
/// imported one when the element's extras carry it, else deterministic.
fn identified(kind: &str, name: &str, imported: Option<&str>) -> Map<String, Value> {
    let mut obj = Map::new();
    obj.insert("Ravens.cimObjectType".into(), Value::String(kind.into()));
    obj.insert(
        "IdentifiedObject.mRID".into(),
        Value::String(imported.map_or_else(|| det_mrid(kind, name), str::to_owned)),
    );
    obj.insert("IdentifiedObject.name".into(), Value::String(name.into()));
    obj
}

fn extras_mrid(extras: &Extras) -> Option<&str> {
    extras.get(MRID_KEY).and_then(Value::as_str)
}

/// An extras value as a number, whether the reader stored it as one or a
/// dss parse kept the source text.
fn xf64(extras: &Extras, key: &str) -> Option<f64> {
    let v = extras.get(key)?;
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Whether the bus grounds its neutral conductor.
fn neutral_grounded(net: &MulticonductorNetwork, bus: &str) -> bool {
    net.bus(bus)
        .is_some_and(|b| b.grounded.iter().any(|t| terminal_phase(t) == Some("N")))
}

/// Whether a terminal name is a phase conductor: anything the shared phase
/// vocabulary does not read as the neutral (`4`, `n`, `N`).
fn is_hot(terminal: &str) -> bool {
    terminal_phase(terminal) != Some("N")
}

/// PhaseCode states a set; ordering belongs to phase records, not this enum.
/// Repeated or unknown conductors cannot be represented as a phase set.
fn canonical_code(map: &[String]) -> Option<String> {
    let mut letters = BTreeSet::new();
    for t in map {
        let letter = terminal_phase(t)?;
        if !letters.insert(letter) {
            return None;
        }
    }
    (!letters.is_empty()).then(|| letters.into_iter().collect::<String>())
}

/// The CIM phase letters for a terminal map, in map order, neutral included.
fn letters(map: &[String]) -> Vec<&'static str> {
    map.iter().filter_map(|t| terminal_phase(t)).collect()
}

/// `PhaseCode.<letters>` over the map's hot terminals (the convention the
/// upstream equipment terminals carry).
fn hot_code(map: &[String]) -> String {
    let hot: String = letters(map)
        .into_iter()
        .filter(|l| *l != "N")
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    format!("PhaseCode.{hot}")
}

/// `PhaseCode.<letters>` over the full map, neutral included — for a shunt,
/// which can sit on the neutral alone (a grounding bank), where `hot_code`
/// would render an empty, un-invertible code.
fn neutral_inclusive_code(map: &[String]) -> String {
    let all: String = letters(map)
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    format!("PhaseCode.{all}")
}

/// `OrderedPhaseCodeKind.<letters>` over the full map, neutral included
/// (transformer tank ends).
fn ordered_code(map: &[String]) -> String {
    let all: String = letters(map).into_iter().collect();
    format!("OrderedPhaseCodeKind.{all}")
}

/// Display a float the way the emitted JSON will (shortest round trip), for
/// names like `BaseV_0.4` and `OpLimI_400.0_600.0` that embed values. The
/// upstream converters embed Python floats, which print `400.0` where Rust
/// prints `400`; the `.0` is restored for whole numbers to match the
/// vocabulary (the names are identity keys, not parsed numbers).
fn fdisp(v: f64) -> String {
    let s = format!("{v}");
    if s.contains(['.', 'e', 'E', 'n', 'i']) {
        s
    } else {
        format!("{s}.0")
    }
}

struct Writer {
    warnings: crate::diagnostics::Diagnostics,
    /// Emitted operational limit sets by name.
    limit_sets: BTreeMap<String, Value>,
    used_types: BTreeSet<&'static str>,
    /// Per-load response characteristics beyond the three canonical anchors.
    responses: BTreeMap<String, Value>,
    /// `EnergyConnectionProfile` rows keyed by name.
    profiles: BTreeMap<String, Value>,
    /// Propagated nominal line-to-line volts per bus id (lowercased).
    bases: BTreeMap<String, f64>,
    /// Equipment reference voltages may differ from the voltage of their bus.
    equipment_bases: BTreeMap<String, f64>,
}

impl Writer {
    fn warn_nonuniform(&mut self, what: &str, field: &str, values: &[f64]) {
        if values
            .windows(2)
            .any(|p| (p[0] - p[1]).abs() > 1e-9 * p[0].abs().max(p[1].abs()).max(1.0))
        {
            self.warn(&C::EMIT_RAVENS_DIST_VALUE_COLLAPSED, format!(
                "{what}: unequal per-phase {field} cannot be stated by one scalar; values are collapsed"
            ));
        }
    }
    fn representable_map(&mut self, what: &str, map: &[String]) -> bool {
        if canonical_code(map).is_some() {
            return true;
        }
        self.warn(&C::EMIT_RAVENS_DIST_RECORD_DROPPED, format!(
            "{what}: terminal map {map:?} contains repeated or unsupported CIM phase identities; record dropped"
        ));
        false
    }
    fn warn(&mut self, info: &'static crate::diagnostics::DiagnosticInfo, msg: impl Into<String>) {
        self.warnings.push(info, msg);
    }

    fn num(&mut self, v: f64, what: &str) -> Value {
        Number::from_f64(v).map_or_else(
            || {
                self.warn(
                    &C::EMIT_RAVENS_DIST_NOT_A_NUMBER,
                    format!("{what}: non-finite value cannot be carried by JSON; emitted 0"),
                );
                Value::from(0)
            },
            Value::Number,
        )
    }

    fn base_of(&self, bus: &str) -> Option<f64> {
        self.bases.get(&bus.to_ascii_lowercase()).copied()
    }

    fn base_pointer(&self, bus: &str) -> Option<(String, Value)> {
        let base = self.base_of(bus)?;
        let name = format!("BaseV_{}", fdisp(base / 1e3));
        Some((
            "ConductingEquipment.BaseVoltage".into(),
            pointer("BaseVoltage", &name),
        ))
    }

    fn warn_extras(&mut self, what: &str, extras: &Extras, consumed: &[&str]) {
        for key in extras.keys() {
            if consumed.contains(&key.as_str())
                || key.starts_with("ravens_")
                || key.starts_with("pmd_")
                || key == "bmopf_subtype"
            {
                continue;
            }
            self.warn(
                &C::EMIT_RAVENS_DIST_EXTRAS_DROPPED,
                format!(
                    "{what}: extra `{key}` has no MG-RAVENS slot; dropped (the schema closes \
                 every class with additionalProperties: false)"
                ),
            );
        }
    }

    /// Register (once) the ampacity limit set for a norm/emerg pair and
    /// return its name.
    fn current_limit_set(&mut self, norm: f64, emerg: f64) -> String {
        let name = format!("OpLimI_{}_{}", fdisp(norm), fdisp(emerg));
        if !self.limit_sets.contains_key(&name) {
            let mut set = identified("OperationalLimitSet", &name, None);
            let entry = |w: &mut Writer, suffix: &str, value: f64, ty: &'static str| {
                let mut lim = identified("CurrentLimit", &format!("{name}_{suffix}"), None);
                let value = w.num(value, "current limit");
                let norm = w.num(norm, "current limit");
                lim.insert("CurrentLimit.value".into(), value);
                lim.insert("CurrentLimit.normalValue".into(), norm);
                lim.insert(
                    "OperationalLimit.OperationalLimitType".into(),
                    pointer("OperationalLimitType", ty),
                );
                w.used_types.insert(ty);
                Value::Object(lim)
            };
            let values = vec![
                entry(self, "Norm", norm, TYPE_I_NORM),
                entry(self, "Emerg", emerg, TYPE_I_EMERG),
            ];
            set.insert(
                "OperationalLimitSet.OperationalLimitValue".into(),
                Value::Array(values),
            );
            self.limit_sets.insert(name.clone(), Value::Object(set));
        }
        name
    }

    /// Register (once) the voltage-bound limit set for a bus and return its
    /// name.
    fn voltage_limit_set(
        &mut self,
        low: Option<f64>,
        high: Option<f64>,
        normal: Option<f64>,
    ) -> String {
        let name = format!(
            "OpLimV_{}-{}",
            low.map_or_else(|| "none".into(), fdisp),
            high.map_or_else(|| "none".into(), fdisp)
        );
        if !self.limit_sets.contains_key(&name) {
            let mut set = identified("OperationalLimitSet", &name, None);
            let entry = |w: &mut Writer, suffix: &str, value: f64, ty: &'static str| {
                let mut lim = identified("VoltageLimit", &format!("{name}_{suffix}"), None);
                let value = w.num(value, "voltage limit");
                lim.insert("VoltageLimit.value".into(), value);
                if let Some(normal) = normal {
                    let normal = w.num(normal, "voltage limit");
                    lim.insert("VoltageLimit.normalValue".into(), normal);
                }
                lim.insert(
                    "OperationalLimit.OperationalLimitType".into(),
                    pointer("OperationalLimitType", ty),
                );
                w.used_types.insert(ty);
                Value::Object(lim)
            };
            let mut values = Vec::new();
            if let Some(low) = low {
                values.push(entry(self, "RangeAlow", low, TYPE_V_LOW));
            }
            if let Some(high) = high {
                values.push(entry(self, "RangeAhigh", high, TYPE_V_HIGH));
            }
            set.insert(
                "OperationalLimitSet.OperationalLimitValue".into(),
                Value::Array(values),
            );
            self.limit_sets.insert(name.clone(), Value::Object(set));
        }
        name
    }

    // ------------------------------------------------------------------
    // Document assembly
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_lines)] // assembling the nested class hierarchy is inherently long
    fn document(&mut self, net: &MulticonductorNetwork) -> Value {
        self.warn_unrepresented(net);

        // Element tables first: they register the limit sets, responses, and
        // profiles the bus pass points at.
        let linecodes = self.write_linecodes(net);
        let lines = self.write_lines(net);
        let switches = self.write_switches(net);
        let loads = self.write_loads(net);
        let shunts = self.write_shunts(net);
        let machines = self.write_machines(net);
        let pecs = self.write_power_electronics(net);
        let sources = self.write_sources(net);
        let (transformers, asset_info) = self.write_transformers(net);
        let (nodes, locations, grounds) = self.write_buses(net);

        let mut root = Map::new();
        root.insert("Versions".into(), versions());
        root.insert("ConnectivityNode".into(), Value::Object(nodes));
        if !locations.is_empty() {
            root.insert("Location".into(), Value::Object(locations));
        }

        // The CIM class-hierarchy nesting the schema requires: concrete hash
        // tables under their abstract ancestors, empty levels omitted (the
        // upstream documents omit absent classes entirely).
        let mut energy_connection = Map::new();
        if !loads.is_empty() {
            energy_connection.insert("EnergyConsumer".into(), Value::Object(loads));
        }
        if !sources.is_empty() {
            energy_connection.insert("EnergySource".into(), Value::Object(sources));
        }
        let mut regulating = Map::new();
        if !shunts.is_empty() {
            regulating.insert("ShuntCompensator".into(), Value::Object(shunts));
        }
        if !machines.is_empty() {
            regulating.insert("RotatingMachine".into(), Value::Object(machines));
        }
        if !pecs.is_empty() {
            regulating.insert("PowerElectronicsConnection".into(), Value::Object(pecs));
        }
        if !regulating.is_empty() {
            energy_connection.insert("RegulatingCondEq".into(), Value::Object(regulating));
        }
        let mut conducting = Map::new();
        if !lines.is_empty() {
            let mut conductor = Map::new();
            conductor.insert("ACLineSegment".into(), Value::Object(lines));
            conducting.insert("Conductor".into(), Value::Object(conductor));
        }
        if !switches.is_empty() {
            conducting.insert("Switch".into(), Value::Object(switches));
        }
        if !transformers.is_empty() {
            conducting.insert("PowerTransformer".into(), Value::Object(transformers));
        }
        if !grounds.is_empty() {
            conducting.insert("Ground".into(), Value::Object(grounds));
        }
        if !energy_connection.is_empty() {
            conducting.insert("EnergyConnection".into(), Value::Object(energy_connection));
        }
        if !conducting.is_empty() {
            let mut equipment = Map::new();
            equipment.insert("ConductingEquipment".into(), Value::Object(conducting));
            let mut psr = Map::new();
            psr.insert("Equipment".into(), Value::Object(equipment));
            root.insert("PowerSystemResource".into(), Value::Object(psr));
        }

        let bases: BTreeMap<String, f64> = self
            .bases
            .values()
            .map(|&v| (format!("BaseV_{}", fdisp(v / 1e3)), v))
            .chain(
                self.equipment_bases
                    .iter()
                    .map(|(name, &v)| (name.clone(), v)),
            )
            .collect();
        if !bases.is_empty() {
            let mut table = Map::new();
            for (name, volts) in bases {
                let mut base = identified("BaseVoltage", &name, None);
                let volts = self.num(volts, "base voltage");
                base.insert("BaseVoltage.nominalVoltage".into(), volts);
                table.insert(name, Value::Object(base));
            }
            root.insert("BaseVoltage".into(), Value::Object(table));
        }

        if !linecodes.is_empty() {
            let mut impedance = Map::new();
            impedance.insert("PerLengthPhaseImpedance".into(), Value::Object(linecodes));
            let mut parameter = Map::new();
            parameter.insert("PerLengthImpedance".into(), Value::Object(impedance));
            root.insert("PerLengthLineParameter".into(), Value::Object(parameter));
        }
        if !asset_info.is_empty() {
            let mut info = Map::new();
            info.insert("PowerTransformerInfo".into(), Value::Object(asset_info));
            root.insert("AssetInfo".into(), Value::Object(info));
        }
        if !self.responses.is_empty() {
            let table: Map<String, Value> =
                std::mem::take(&mut self.responses).into_iter().collect();
            root.insert("LoadResponseCharacteristic".into(), Value::Object(table));
        }
        if !self.limit_sets.is_empty() {
            let table: Map<String, Value> =
                std::mem::take(&mut self.limit_sets).into_iter().collect();
            root.insert("OperationalLimitSet".into(), Value::Object(table));
            root.insert("OperationalLimitType".into(), self.limit_types());
        }
        if !self.profiles.is_empty() {
            let table: Map<String, Value> =
                std::mem::take(&mut self.profiles).into_iter().collect();
            root.insert("EnergyConnectionProfile".into(), Value::Object(table));
        }

        self.restore_untyped(net, &mut root);
        Value::Object(root)
    }

    fn limit_types(&mut self) -> Value {
        let mut table = Map::new();
        let entries = [
            (TYPE_V_LOW, "low", 5_000_000_000.0),
            (TYPE_V_HIGH, "high", 5_000_000_000.0),
            (TYPE_I_NORM, "absoluteValue", 5_000_000_000.0),
            (TYPE_I_EMERG, "absoluteValue", 86_400.0),
        ];
        for (name, direction, duration) in entries {
            if !self.used_types.contains(name) {
                continue;
            }
            let mut ty = identified("OperationalLimitType", name, None);
            ty.insert(
                "OperationalLimitType.direction".into(),
                Value::String(format!("OperationalLimitDirectionKind.{direction}")),
            );
            let duration = self.num(duration, "limit type");
            ty.insert("OperationalLimitType.acceptableDuration".into(), duration);
            table.insert(name.into(), Value::Object(ty));
        }
        Value::Object(table)
    }

    // ------------------------------------------------------------------
    // Element tables
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_lines)] // matrix entries and their projection diagnostics belong together
    fn write_linecodes(&mut self, net: &MulticonductorNetwork) -> Map<String, Value> {
        let mut table = Map::new();
        for code in net.line_codes() {
            let what = format!("linecode {}", code.name);
            if let Some(limits) = &code.i_max {
                self.warn_nonuniform(&what, "ampacity", limits);
            }
            if code.g_from != code.g_to || code.b_from != code.b_to {
                self.warn(&C::EMIT_RAVENS_DIST_VALUE_COLLAPSED, format!(
                    "{what}: unequal end shunts become a symmetric pi model with the same total admittance"
                ));
            }
            if [
                &code.r_series,
                &code.x_series,
                &code.g_from,
                &code.g_to,
                &code.b_from,
                &code.b_to,
            ]
            .iter()
            .any(|matrix| {
                matrix.iter().enumerate().any(|(i, row)| {
                    row.iter().enumerate().any(|(j, v)| {
                        matrix
                            .get(j)
                            .and_then(|row| row.get(i))
                            .is_some_and(|other| (v - other).abs() > 0.0)
                    })
                })
            }) {
                self.warn(&C::EMIT_RAVENS_DIST_VALUE_COLLAPSED, format!(
                    "{what}: nonsymmetric conductor matrices are represented by their symmetric lower triangle"
                ));
            }
            if code.n_conductors > 4 {
                self.warn(
                    &C::EMIT_RAVENS_DIST_VALUE_COLLAPSED,
                    format!(
                        "linecode {}: {} conductors exceed the four CIM SinglePhaseKind identities \
                     (A/B/C/N); the impedance matrix is written in full, but conductors beyond \
                     the fourth have no phase representation on the lines that use it, so a \
                     reparse cannot recover their terminal assignment",
                        code.name, code.n_conductors
                    ),
                );
            }
            let mut obj = identified(
                "PerLengthPhaseImpedance",
                &code.name,
                extras_mrid(&code.extras),
            );
            obj.insert(
                "PerLengthPhaseImpedance.conductorCount".into(),
                Value::from(code.n_conductors as u64),
            );
            let total = |from: &ConductorMatrix, to: &ConductorMatrix, i: usize, j: usize| -> f64 {
                let at =
                    |m: &ConductorMatrix| m.get(i).and_then(|r| r.get(j)).copied().unwrap_or(0.0);
                at(from) + at(to)
            };
            let mut rows = Vec::new();
            // Column-major lower triangle, the upstream emission order.
            for col in 0..code.n_conductors {
                for row in col..code.n_conductors {
                    let mut entry = Map::new();
                    entry.insert(
                        "Ravens.cimObjectType".into(),
                        Value::String("PhaseImpedanceData".into()),
                    );
                    entry.insert("PhaseImpedanceData.row".into(), Value::from(row as u64 + 1));
                    entry.insert(
                        "PhaseImpedanceData.column".into(),
                        Value::from(col as u64 + 1),
                    );
                    let at = |m: &ConductorMatrix| {
                        m.get(row).and_then(|r| r.get(col)).copied().unwrap_or(0.0)
                    };
                    let what = format!("linecode {}", code.name);
                    let r = self.num(at(&code.r_series), &what);
                    let x = self.num(at(&code.x_series), &what);
                    let b = self.num(total(&code.b_from, &code.b_to, row, col), &what);
                    entry.insert("PhaseImpedanceData.r".into(), r);
                    entry.insert("PhaseImpedanceData.x".into(), x);
                    entry.insert("PhaseImpedanceData.b".into(), b);
                    let g = total(&code.g_from, &code.g_to, row, col);
                    if g.abs() > 0.0 {
                        let g = self.num(g, &what);
                        entry.insert("PhaseImpedanceData.g".into(), g);
                    }
                    rows.push(Value::Object(entry));
                }
            }
            obj.insert(
                "PerLengthPhaseImpedance.PhaseImpedanceData".into(),
                Value::Array(rows),
            );
            self.warn_extras(&format!("linecode {}", code.name), &code.extras, &[]);
            if code.s_max.is_some() {
                self.warn(
                    &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                    format!(
                        "linecode {}: `s_max` power rating has no MG-RAVENS slot; dropped \
                     (ampacity carries through the line current limits)",
                        code.name
                    ),
                );
            }
            table.insert(code.name.clone(), Value::Object(obj));
        }
        table
    }

    #[allow(clippy::too_many_lines)] // terminal order, impedance references and limits form one record
    fn write_lines(&mut self, net: &MulticonductorNetwork) -> Map<String, Value> {
        let mut table = Map::new();
        for line in net.lines() {
            let what = format!("line {}", line.name);
            if !self.representable_map(&what, &line.terminal_map_from)
                || !self.representable_map(&what, &line.terminal_map_to)
            {
                continue;
            }
            if line.terminal_map_from.len() > 1 && line.terminal_map_from != line.terminal_map_to {
                self.warn(&C::EMIT_RAVENS_DIST_RECORD_DROPPED, format!(
                    "{what}: different multi-conductor terminal maps cannot be stated by ACLineSegmentPhase; record dropped"
                ));
                continue;
            }
            let mut obj = identified("ACLineSegment", &line.name, extras_mrid(&line.extras));
            obj.insert("Equipment.inService".into(), Value::Bool(true));
            if let Some((key, value)) = self.base_pointer(&line.bus_from) {
                obj.insert(key, value);
            }
            let length = self.num(line.length, &what);
            obj.insert("Conductor.length".into(), length);
            obj.insert(
                "ACLineSegment.PerLengthImpedance".into(),
                pointer("PerLengthPhaseImpedance", &line.linecode),
            );

            // A segment's conductors are its `ACLineSegmentPhase` records,
            // the neutral among them, each a CIM phase identity (A, B, C,
            // N); a conductor outside that vocabulary (the fifth of a five
            // wire code) has no record, and the segment is not stated.
            let unspelled: Vec<&String> = line
                .terminal_map_from
                .iter()
                .filter(|t| terminal_phase(t).is_none())
                .collect();
            if !unspelled.is_empty() {
                self.warn(&C::EMIT_RAVENS_DIST_RECORD_DROPPED, format!(
                    "{what}: conductor(s) {unspelled:?} have no CIM phase identity (A, B, C, N), \
                     so the segment cannot be stated; dropped"
                ));
                continue;
            }
            // Each `Terminal.phases` is its own end's code, so a segment whose
            // ends land on different phases (a single phase lateral from
            // conductor 1 onto phase B) states both. The conductor order is
            // the from side's, in `ACLineSegmentPhase` sequence; a to side
            // the code cannot spell (an order other than ABCN, or a conductor
            // with no phase letter) collapses onto it.
            let code = neutral_inclusive_code(&line.terminal_map_from);
            let to_code = match canonical_code(&line.terminal_map_to) {
                Some(to_code) if line.terminal_map_to.len() == line.terminal_map_from.len() => {
                    format!("PhaseCode.{to_code}")
                }
                _ => {
                    if line.terminal_map_to != line.terminal_map_from {
                        self.warn(&C::EMIT_RAVENS_DIST_VALUE_COLLAPSED, format!(
                            "{what}: the to side's terminal map {:?} has no MG-RAVENS phase code \
                             (a code lists phases in ABC order), so the from side's phases stand \
                             for both ends",
                            line.terminal_map_to
                        ));
                    }
                    code.clone()
                }
            };
            let limit = {
                // A segment can override its shared linecode's ampacity.
                if let Some(limits) = &line.i_max {
                    self.warn_nonuniform(&what, "ampacity", limits);
                }
                let emerg = line
                    .i_max
                    .as_ref()
                    .or_else(|| net.linecode(&line.linecode).and_then(|c| c.i_max.as_ref()))
                    .and_then(|v| v.first().copied());
                let norm = xf64(&line.extras, "normamps");
                match (norm, emerg) {
                    (Some(n), Some(e)) => Some(self.current_limit_set(n, e)),
                    (None, Some(e)) => Some(self.current_limit_set(e, e)),
                    (Some(n), None) => Some(self.current_limit_set(n, n)),
                    (None, None) => None,
                }
            };
            let terminals = vec![
                terminal(&line.name, 1, &line.bus_from, &code, limit.as_deref()),
                terminal(&line.name, 2, &line.bus_to, &to_code, limit.as_deref()),
            ];
            obj.insert(
                "ConductingEquipment.Terminals".into(),
                Value::Array(terminals),
            );

            let phases: Vec<Value> = line
                .terminal_map_from
                .iter()
                .filter_map(|t| terminal_phase(t))
                .enumerate()
                .map(|(k, letter)| {
                    let mut p = identified(
                        "ACLineSegmentPhase",
                        &format!("{}_{letter}", line.name),
                        None,
                    );
                    p.insert(
                        "ACLineSegmentPhase.phase".into(),
                        Value::String(format!("SinglePhaseKind.{letter}")),
                    );
                    p.insert(
                        "ACLineSegmentPhase.sequenceNumber".into(),
                        Value::from(k as u64 + 1),
                    );
                    Value::Object(p)
                })
                .collect();
            obj.insert(
                "ACLineSegment.ACLineSegmentPhase".into(),
                Value::Array(phases),
            );
            self.warn_extras(&what, &line.extras, &["normamps"]);
            table.insert(line.name.clone(), Value::Object(obj));
        }
        table
    }

    fn write_switches(&mut self, net: &MulticonductorNetwork) -> Map<String, Value> {
        let mut table = Map::new();
        for s in net.switches() {
            let what = format!("switch {}", s.name);
            if let Some(limits) = &s.i_max {
                self.warn_nonuniform(&what, "ampacity", limits);
            }
            if !self.representable_map(&what, &s.terminal_map_from)
                || !self.representable_map(&what, &s.terminal_map_to)
            {
                continue;
            }
            let mut obj = identified("Switch", &s.name, extras_mrid(&s.extras));
            obj.insert("Equipment.inService".into(), Value::Bool(true));
            obj.insert("Switch.open".into(), Value::Bool(s.open));
            let normal = s
                .extras
                .get(NORMAL_OPEN_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(s.open);
            obj.insert("Switch.normalOpen".into(), Value::Bool(normal));
            let limit = {
                let emerg = s.i_max.as_ref().and_then(|v| v.first().copied());
                let norm = xf64(&s.extras, "normamps");
                match (norm, emerg) {
                    (Some(n), Some(e)) => Some(self.current_limit_set(n, e)),
                    (None, Some(e)) => Some(self.current_limit_set(e, e)),
                    (Some(n), None) => Some(self.current_limit_set(n, n)),
                    (None, None) => None,
                }
            };
            let terminals = vec![
                terminal(
                    &s.name,
                    1,
                    &s.bus_from,
                    &neutral_inclusive_code(&s.terminal_map_from),
                    limit.as_deref(),
                ),
                terminal(
                    &s.name,
                    2,
                    &s.bus_to,
                    &neutral_inclusive_code(&s.terminal_map_to),
                    limit.as_deref(),
                ),
            ];
            obj.insert(
                "ConductingEquipment.Terminals".into(),
                Value::Array(terminals),
            );

            // Always state conductor pairing, including the neutral and
            // noncanonical ordering, independently of the terminal phase set.
            {
                let sides: Vec<Value> = s
                    .terminal_map_from
                    .iter()
                    .zip(&s.terminal_map_to)
                    .filter_map(|(a, b)| Some((terminal_phase(a)?, terminal_phase(b)?)))
                    .map(|(a, b)| {
                        let mut p = identified("SwitchPhase", &format!("{}_{a}{b}", s.name), None);
                        p.insert(
                            "SwitchPhase.phaseSide1".into(),
                            Value::String(format!("SinglePhaseKind.{a}")),
                        );
                        p.insert(
                            "SwitchPhase.phaseSide2".into(),
                            Value::String(format!("SinglePhaseKind.{b}")),
                        );
                        Value::Object(p)
                    })
                    .collect();
                obj.insert("Switch.SwitchPhase".into(), Value::Array(sides));
            }
            self.warn_extras(&what, &s.extras, &["normamps"]);
            table.insert(s.name.clone(), Value::Object(obj));
        }
        table
    }

    #[allow(clippy::too_many_lines)] // one load emits in one pass: powers, phases, response, profile
    fn write_loads(&mut self, net: &MulticonductorNetwork) -> Map<String, Value> {
        let mut table = Map::new();
        for load in net.loads() {
            let what = format!("load {}", load.name);
            if !self.representable_map(&what, &load.terminal_map) {
                continue;
            }
            let mut obj = identified("EnergyConsumer", &load.name, extras_mrid(&load.extras));
            let p: f64 = load.p_nom.iter().sum();
            let q: f64 = load.q_nom.iter().sum();
            let p = self.num(p, &what);
            let q = self.num(q, &what);
            obj.insert("EnergyConsumer.p".into(), p);
            obj.insert("EnergyConsumer.q".into(), q);
            // A single phase load returning on the neutral is a wye phase; one
            // across two phase conductors is a delta pair (the PMD writer's
            // reading of `SinglePhase`).
            let neutral_return = load.configuration == Configuration::SinglePhase
                && load.terminal_map.last().is_some_and(|t| !is_hot(t));
            let delta = load.configuration == Configuration::Delta
                || (load.configuration == Configuration::SinglePhase && !neutral_return);
            // CIM `grounded`: the neutral is solidly grounded. A wye load on
            // a neutral conductor that is not grounded at its bus is not.
            let grounded = !delta
                && load.terminal_map.iter().any(|t| !is_hot(t))
                && neutral_grounded(net, &load.bus);
            obj.insert("EnergyConsumer.grounded".into(), Value::Bool(grounded));
            obj.insert("Equipment.inService".into(), Value::Bool(true));
            if let Some(&v) = load.voltage_model.v_nom().first() {
                let base = if delta { v } else { v * 3f64.sqrt() };
                let base = stable_fixed(base, |b| {
                    if delta {
                        b
                    } else {
                        (b / 3f64.sqrt()) * 3f64.sqrt()
                    }
                });
                let name = format!("BaseV_{}", fdisp(base / 1e3));
                self.equipment_bases.insert(name.clone(), base);
                obj.insert(
                    "ConductingEquipment.BaseVoltage".into(),
                    pointer("BaseVoltage", &name),
                );
                if load
                    .voltage_model
                    .v_nom()
                    .iter()
                    .any(|other| (other - v).abs() > 1e-9 * v.abs().max(1.0))
                {
                    self.warn(&C::EMIT_RAVENS_DIST_VALUE_COLLAPSED, format!(
                        "{what}: unequal per-phase nominal voltages; the first phase represents the load"
                    ));
                }
            } else if let Some((key, value)) = self.base_pointer(&load.bus) {
                obj.insert(key, value);
            }
            obj.insert(
                "EnergyConsumer.phaseConnection".into(),
                Value::String(format!(
                    "PhaseShuntConnectionKind.{}",
                    if delta { "D" } else { "Y" }
                )),
            );
            if let Some(response) = self.load_response(load) {
                obj.insert(
                    "EnergyConsumer.LoadResponse".into(),
                    pointer("LoadResponseCharacteristic", &response),
                );
            }
            self.attach_profile(load, &mut obj, net);

            let mut hot: Vec<&'static str> = load
                .terminal_map
                .iter()
                .filter_map(|t| terminal_phase(t))
                .filter(|l| *l != "N")
                .collect();
            if delta {
                let conductors = hot.clone();
                hot = (0..load.p_nom.len())
                    .filter_map(|k| {
                        let a = *conductors.get(k)?;
                        let b = *conductors.get((k + 1) % conductors.len())?;
                        match (a, b) {
                            ("A", "B") | ("B", "A") => Some("A"),
                            ("B", "C") | ("C", "B") => Some("B"),
                            ("C", "A") | ("A", "C") => Some("C"),
                            _ => None,
                        }
                    })
                    .collect();
                if hot.len() != load.p_nom.len() || hot.is_empty() {
                    self.warn(
                        &C::EMIT_RAVENS_DIST_RECORD_DROPPED,
                        format!(
                            "{what}: delta branch geometry has no CIM phase mapping; record dropped"
                        ),
                    );
                    continue;
                }
            }
            let phases: Vec<Value> = hot
                .iter()
                .enumerate()
                .map(|(k, letter)| {
                    let mut ph = identified(
                        "EnergyConsumerPhase",
                        &format!("{}_{letter}", load.name),
                        None,
                    );
                    let pk = load.p_nom.get(k).copied().unwrap_or(0.0);
                    let qk = load.q_nom.get(k).copied().unwrap_or(0.0);
                    let pk = self.num(pk, &what);
                    let qk = self.num(qk, &what);
                    ph.insert("EnergyConsumerPhase.p".into(), pk);
                    ph.insert("EnergyConsumerPhase.q".into(), qk);
                    ph.insert(
                        "EnergyConsumerPhase.phase".into(),
                        Value::String(format!("SinglePhaseKind.{letter}")),
                    );
                    Value::Object(ph)
                })
                .collect();
            if delta && hot.len() == 2 && load.p_nom.len() == 1 {
                // A one-phase delta load spans two hot terminals but has one
                // power record; only the first phase entry applies.
                obj.insert(
                    "EnergyConsumer.EnergyConsumerPhase".into(),
                    Value::Array(phases.into_iter().take(1).collect()),
                );
            } else {
                obj.insert(
                    "EnergyConsumer.EnergyConsumerPhase".into(),
                    Value::Array(phases),
                );
            }

            // The bus voltage bounds ride on the load terminal too, the
            // upstream convention.
            let limit = self.bus_voltage_limit(net, &load.bus);
            let terminals = vec![terminal(
                &load.name,
                1,
                &load.bus,
                &format!(
                    "PhaseCode.{}",
                    hot.iter()
                        .copied()
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<String>()
                ),
                limit.as_deref(),
            )];
            obj.insert(
                "ConductingEquipment.Terminals".into(),
                Value::Array(terminals),
            );

            if load.extras.contains_key("pf") {
                self.warn(
                    &C::EMIT_RAVENS_DIST_VALUE_SUBSTITUTED,
                    format!(
                        "{what}: the power-factor spec becomes an explicit reactive power \
                     (MG-RAVENS carries p and q, not pf)"
                    ),
                );
            }
            if load.extras.contains_key("vminpu") || load.extras.contains_key("vmaxpu") {
                self.warn(
                    &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                    format!(
                        "{what}: per-load voltage clamps (vminpu/vmaxpu) have no MG-RAVENS slot; \
                     dropped"
                    ),
                );
            }
            self.warn_extras(
                &what,
                &load.extras,
                &[
                    "kv", "phases", "daily", "yearly", "spectrum", "pf", "model", "zipv", "vminpu",
                    "vmaxpu",
                ],
            );
            table.insert(load.name.clone(), Value::Object(obj));
        }
        table
    }

    /// The response-characteristic table entry for a load's voltage model,
    /// returning the pointer name. Constant models share the three canonical
    /// anchors; ZIP and exponential models emit per-load records.
    #[allow(clippy::too_many_lines)] // one load law maps in one pass: anchors, ZIP, exponential
    fn load_response(&mut self, load: &crate::model::DistLoad) -> Option<String> {
        // The voltage band transitions, current limits, and dropout of the
        // bounded laws have no CIM load response field; the nominal law is
        // what the characteristic states.
        let nominal = load.voltage_model.without_voltage_bounds();
        if matches!(
            load.voltage_model,
            DistLoadVoltageModel::BoundedZip { .. }
                | DistLoadVoltageModel::BoundedExponential { .. }
                | DistLoadVoltageModel::BoundedConstantPower { .. }
        ) {
            self.warn(
                &C::EMIT_RAVENS_DIST_VALUE_SUBSTITUTED,
                format!(
                    "load {}: voltage-band transitions, current limits and dropout have no \
                     LoadResponseCharacteristic field; emitted the nominal law",
                    load.name
                ),
            );
        }
        let canonical = |w: &mut Writer, name: &str, field: &str| {
            if !w.responses.contains_key(name) {
                let mut lrc = identified("LoadResponseCharacteristic", name, None);
                lrc.insert(
                    format!("LoadResponseCharacteristic.p{field}"),
                    Value::from(100.0),
                );
                lrc.insert(
                    format!("LoadResponseCharacteristic.q{field}"),
                    Value::from(100.0),
                );
                w.responses.insert(name.into(), Value::Object(lrc));
            }
            Some(name.to_owned())
        };
        match &nominal {
            DistLoadVoltageModel::BoundedZip { .. }
            | DistLoadVoltageModel::BoundedExponential { .. }
            | DistLoadVoltageModel::BoundedConstantPower { .. } => {
                unreachable!("without_voltage_bounds returns the nominal law")
            }
            DistLoadVoltageModel::ConstantPower { .. } => {
                canonical(self, "Constant kVA", "ConstantPower")
            }
            DistLoadVoltageModel::ConstantCurrent { .. } => {
                canonical(self, "Constant I", "ConstantCurrent")
            }
            DistLoadVoltageModel::ConstantImpedance { .. } => {
                canonical(self, "Constant Z", "ConstantImpedance")
            }
            DistLoadVoltageModel::Zip {
                alpha_z,
                alpha_i,
                alpha_p,
                beta_z,
                beta_i,
                beta_p,
                ..
            } => {
                let name = format!("{}_response", load.name);
                let what = format!("load {}", load.name);
                let mut lrc = identified("LoadResponseCharacteristic", &name, None);
                let first = |v: &[f64]| v.first().copied().unwrap_or(0.0) * 100.0;
                for (key, coeffs) in [
                    ("pConstantImpedance", alpha_z),
                    ("pConstantCurrent", alpha_i),
                    ("pConstantPower", alpha_p),
                    ("qConstantImpedance", beta_z),
                    ("qConstantCurrent", beta_i),
                    ("qConstantPower", beta_p),
                ] {
                    let uniform = coeffs
                        .windows(2)
                        .all(|w| (w[0] - w[1]).abs() <= f64::EPSILON * w[0].abs().max(1.0));
                    if !uniform {
                        self.warn(
                            &C::EMIT_RAVENS_DIST_VALUE_COLLAPSED,
                            format!(
                                "{what}: per-phase ZIP coefficients are not uniform; the first \
                             phase's value represents the load"
                            ),
                        );
                    }
                    let value = self.num(first(coeffs), &what);
                    lrc.insert(format!("LoadResponseCharacteristic.{key}"), value);
                }
                self.responses.insert(name.clone(), Value::Object(lrc));
                Some(name)
            }
            DistLoadVoltageModel::Exponential {
                gamma_p, gamma_q, ..
            } => {
                let name = format!("{}_response", load.name);
                let what = format!("load {}", load.name);
                self.warn_nonuniform(&what, "active voltage exponent", gamma_p);
                self.warn_nonuniform(&what, "reactive voltage exponent", gamma_q);
                let mut lrc = identified("LoadResponseCharacteristic", &name, None);
                lrc.insert(
                    "LoadResponseCharacteristic.exponentModel".into(),
                    Value::Bool(true),
                );
                let gp = gamma_p.first().copied().unwrap_or(0.0);
                let gq = gamma_q.first().copied().unwrap_or(0.0);
                let gp = self.num(gp, &what);
                let gq = self.num(gq, &what);
                lrc.insert("LoadResponseCharacteristic.pVoltageExponent".into(), gp);
                lrc.insert("LoadResponseCharacteristic.qVoltageExponent".into(), gq);
                self.responses.insert(name.clone(), Value::Object(lrc));
                Some(name)
            }
        }
    }

    /// `daily`/`yearly`/`spectrum` extras → an `EnergyConnectionProfile` row
    /// (the upstream name convention joins the dss profile slots) plus the
    /// `LoadProfile` pointer when the schedule object itself survived a
    /// RAVENS parse untyped.
    fn attach_profile(
        &mut self,
        load: &crate::model::DistLoad,
        obj: &mut Map<String, Value>,
        net: &MulticonductorNetwork,
    ) {
        let text = |key: &str| {
            load.extras.get(key).and_then(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .or_else(|| v.as_f64().map(|f| format!("{f}")))
            })
        };
        let (daily, yearly, spectrum) = (text("daily"), text("yearly"), text("spectrum"));
        if daily.is_none() && yearly.is_none() && spectrum.is_none() {
            return;
        }
        let blank = String::new();
        let name = format!(
            "Load:{}:::{}::{}",
            daily.as_ref().unwrap_or(&blank),
            yearly.as_ref().unwrap_or(&blank),
            spectrum.as_ref().unwrap_or(&blank),
        );
        self.profiles.entry(name.clone()).or_insert_with(|| {
            let mut profile = identified("EnergyConnectionProfile", &name, None);
            for (key, value) in [
                ("EnergyConnectionProfile.dssDaily", &daily),
                ("EnergyConnectionProfile.dssYearly", &yearly),
                ("EnergyConnectionProfile.dssSpectrum", &spectrum),
            ] {
                if let Some(v) = value {
                    profile.insert(key.into(), Value::String(v.clone()));
                }
            }
            Value::Object(profile)
        });
        if let Some(schedule) = daily.or(yearly) {
            let preserved = net
                .untyped_objects()
                .iter()
                .any(|u| u.class == "EnergyConsumerSchedule" && u.name == schedule);
            if preserved {
                obj.insert(
                    "EnergyConsumer.LoadProfile".into(),
                    pointer("EnergyConsumerSchedule", &schedule),
                );
            } else {
                self.warn(
                    &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                    format!(
                        "load {}: shape `{schedule}` names a schedule this model does not carry; \
                     the profile row is emitted without a LoadProfile pointer",
                        load.name
                    ),
                );
            }
        }
    }

    #[allow(clippy::too_many_lines)] // one bank writes in one pass: admittance, connection kind, terminal
    fn write_shunts(&mut self, net: &MulticonductorNetwork) -> Map<String, Value> {
        let mut table = Map::new();
        for shunt in net.shunts() {
            let what = format!("shunt {}", shunt.name);
            if !self.representable_map(&what, &shunt.terminal_map) {
                continue;
            }
            let mut obj = identified(
                "LinearShuntCompensator",
                &shunt.name,
                extras_mrid(&shunt.extras),
            );
            let n = shunt.terminal_map.len().max(1);
            let diag = |m: &ConductorMatrix, k: usize| {
                m.get(k).and_then(|r| r.get(k)).copied().unwrap_or(0.0)
            };
            let b = diag(&shunt.b, 0);
            let g = diag(&shunt.g, 0);
            let uneven = (1..n).any(|k| {
                (diag(&shunt.b, k) - b).abs() > f64::EPSILON * b.abs().max(1.0)
                    || (diag(&shunt.g, k) - g).abs() > f64::EPSILON * g.abs().max(1.0)
            });
            if uneven {
                self.warn(
                    &C::EMIT_RAVENS_DIST_VALUE_COLLAPSED,
                    format!(
                        "{what}: per-phase susceptances differ; MG-RAVENS carries one per-section \
                     value, so the first phase represents the bank"
                    ),
                );
            }
            let off_diagonal = shunt
                .b
                .iter()
                .enumerate()
                .any(|(i, row)| row.iter().enumerate().any(|(j, v)| i != j && v.abs() > 0.0));
            if off_diagonal {
                self.warn(
                    &C::EMIT_RAVENS_DIST_VALUE_COLLAPSED,
                    format!(
                        "{what}: off-diagonal susceptance (a delta bank's geometry) has no \
                     MG-RAVENS slot; only the diagonal is carried"
                    ),
                );
            }

            let delta = shunt
                .extras
                .get("conn")
                .and_then(Value::as_str)
                .is_some_and(|c| c.eq_ignore_ascii_case("delta"));
            // nomU = cap.kV * 1000 (opendss2xml), i.e. the dss `kv` directly:
            // line-to-line for a multi-phase bank, line-to-neutral for a
            // single-phase one. No √3 — the reader inverts with nomU/1000.
            let nom_u =
                xf64(&shunt.extras, "kv").map(|kv| stable_fixed(kv * 1e3, |v| (v / 1e3) * 1e3));
            let nom_u = nom_u.or_else(|| {
                // Fallback from the bus base when no kv rode in: a single-phase
                // wye bank sits line-to-neutral, everything else line-to-line.
                self.base_of(&shunt.bus).map(|base| {
                    if !delta && n == 1 {
                        base / 3f64.sqrt()
                    } else {
                        base
                    }
                })
            });
            if let Some(u) = nom_u {
                let u = self.num(u, &what);
                obj.insert("ShuntCompensator.nomU".into(), u);
            }
            let bv = self.num(b, &what);
            let gv = self.num(g, &what);
            obj.insert("LinearShuntCompensator.bPerSection".into(), bv.clone());
            obj.insert("LinearShuntCompensator.gPerSection".into(), gv.clone());
            // `Yn` is a wye bank whose neutral is grounded at the bus, `Y` one
            // on an ungrounded neutral conductor; the reader grounds the node
            // for `Yn` only.
            obj.insert(
                "ShuntCompensator.phaseConnection".into(),
                Value::String(format!(
                    "PhaseShuntConnectionKind.{}",
                    if delta {
                        "D"
                    } else if neutral_grounded(net, &shunt.bus) {
                        "Yn"
                    } else {
                        "Y"
                    }
                )),
            );
            obj.insert("LinearShuntCompensator.b0PerSection".into(), bv);
            obj.insert("LinearShuntCompensator.g0PerSection".into(), gv);
            obj.insert("ShuntCompensator.normalSections".into(), Value::from(1));
            obj.insert("ShuntCompensator.maximumSections".into(), Value::from(1));
            obj.insert("Equipment.inService".into(), Value::Bool(true));
            obj.insert("ShuntCompensator.sections".into(), Value::from(1.0));
            if let Some((key, value)) = self.base_pointer(&shunt.bus) {
                obj.insert(key, value);
            }
            let terminals = vec![terminal(
                &shunt.name,
                1,
                &shunt.bus,
                &neutral_inclusive_code(&shunt.terminal_map),
                None,
            )];
            obj.insert(
                "ConductingEquipment.Terminals".into(),
                Value::Array(terminals),
            );
            self.warn_extras(&what, &shunt.extras, &["kv", "phases", "kvar", "conn"]);
            table.insert(shunt.name.clone(), Value::Object(obj));
        }
        table
    }

    #[allow(clippy::too_many_lines)] // ratings, limits, configuration and dispatch form one record
    fn write_machines(&mut self, net: &MulticonductorNetwork) -> Map<String, Value> {
        let mut table = Map::new();
        for machine in net.generators() {
            let what = format!("generator {}", machine.name);
            self.warn_nonuniform(&what, "active power", &machine.p_nom);
            self.warn_nonuniform(&what, "reactive power", &machine.q_nom);
            for (field, values) in [
                ("p_min", &machine.p_min),
                ("p_max", &machine.p_max),
                ("q_min", &machine.q_min),
                ("q_max", &machine.q_max),
            ] {
                if let Some(values) = values {
                    self.warn_nonuniform(&what, field, values);
                }
            }
            if !self.representable_map(&what, &machine.terminal_map) {
                continue;
            }
            if machine.configuration == Configuration::Delta
                || (machine.configuration == Configuration::SinglePhase
                    && machine.terminal_map.last().is_none_or(|t| is_hot(t)))
            {
                self.warn(&C::EMIT_RAVENS_DIST_RECORD_DROPPED, format!(
                    "{what}: delta generator connection has no distribution-profile field; record dropped"
                ));
                continue;
            }
            let mut obj = identified(
                "SynchronousMachine",
                &machine.name,
                extras_mrid(&machine.extras),
            );
            // CIM load convention (the upstream opendss2xml converter emits
            // `-gen.kW`): a machine's active/reactive power is injection
            // negative, so a positive generation setpoint writes as negative.
            // The maxQ/minQ and GeneratingUnit bounds below are NOT negated —
            // they are symmetric limits / positive operating ranges, matching
            // upstream.
            let p: f64 = machine.p_nom.iter().sum();
            let q: f64 = machine.q_nom.iter().sum();
            let p = self.num(-p, &what);
            let q = self.num(-q, &what);
            obj.insert("RotatingMachine.p".into(), p);
            obj.insert("RotatingMachine.q".into(), q);
            if let Some(kva) = xf64(&machine.extras, "kva") {
                let s = stable_fixed(kva * 1e3, |v| (v / 1e3) * 1e3);
                let s = self.num(s, &what);
                obj.insert("RotatingMachine.ratedS".into(), s);
            }
            if let Some(kv) = xf64(&machine.extras, "kv") {
                let u = stable_fixed(kv * 1e3, |v| (v / 1e3) * 1e3);
                let u = self.num(u, &what);
                obj.insert("RotatingMachine.ratedU".into(), u);
            }
            if let Some(max_q) = &machine.q_max {
                let total: f64 = max_q.iter().sum();
                let total = self.num(total, &what);
                obj.insert("SynchronousMachine.maxQ".into(), total);
            }
            if let Some(min_q) = &machine.q_min {
                let total: f64 = min_q.iter().sum();
                let total = self.num(total, &what);
                obj.insert("SynchronousMachine.minQ".into(), total);
            }
            if machine.p_max.is_some() || machine.p_min.is_some() {
                let mut unit =
                    identified("GeneratingUnit", &format!("{}_Unit", machine.name), None);
                if let Some(p_max) = &machine.p_max {
                    let total: f64 = p_max.iter().sum();
                    let total = self.num(total, &what);
                    unit.insert("GeneratingUnit.maxOperatingP".into(), total);
                }
                if let Some(p_min) = &machine.p_min {
                    let total: f64 = p_min.iter().sum();
                    let total = self.num(total, &what);
                    unit.insert("GeneratingUnit.minOperatingP".into(), total);
                }
                obj.insert("RotatingMachine.GeneratingUnit".into(), Value::Object(unit));
            }
            obj.insert("Equipment.inService".into(), Value::Bool(true));
            if let Some((key, value)) = self.base_pointer(&machine.bus) {
                obj.insert(key, value);
            }
            let terminals = vec![terminal(
                &machine.name,
                1,
                &machine.bus,
                &hot_code(&machine.terminal_map),
                None,
            )];
            obj.insert(
                "ConductingEquipment.Terminals".into(),
                Value::Array(terminals),
            );
            if machine.cost.is_some() {
                self.warn(
                    &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                    format!(
                        "{what}: generation cost has no slot in the distribution profile \
                     (cost functions belong to the balanced MG-RAVENS profile); dropped"
                    ),
                );
            }
            self.warn_extras(&what, &machine.extras, &["kv", "kva", "phases"]);
            table.insert(machine.name.clone(), Value::Object(obj));
        }
        table
    }

    #[allow(clippy::too_many_lines)] // one connection emits in one pass: ratings, limits, unit
    fn write_power_electronics(&mut self, net: &MulticonductorNetwork) -> Map<String, Value> {
        let mut table = Map::new();
        for ibr in net.ibrs() {
            let what = format!("ibr {}", ibr.name);
            self.warn_nonuniform(&what, "apparent-power rating", &ibr.s_max);
            for (field, values) in [
                ("p_min", &ibr.p_min),
                ("p_max", &ibr.p_max),
                ("q_min", &ibr.q_min),
                ("q_max", &ibr.q_max),
                ("i_max", &ibr.i_max),
            ] {
                if let Some(values) = values {
                    self.warn_nonuniform(&what, field, values);
                }
            }
            if !self.representable_map(&what, &ibr.terminal_map) {
                continue;
            }
            if ibr.terminal_map.last().is_none_or(|t| is_hot(t)) {
                self.warn(&C::EMIT_RAVENS_DIST_RECORD_DROPPED, format!(
                    "{what}: inverter connection without a neutral return has no distribution-profile field; record dropped"
                ));
                continue;
            }
            let mut obj = identified(
                "PowerElectronicsConnection",
                &ibr.name,
                extras_mrid(&ibr.extras),
            );
            let rated_s: f64 = ibr.s_max.iter().sum();
            // Injection-negative CIM load convention, consistent with the
            // SynchronousMachine path and the upstream opendss2xml PV export
            // (which writes the OpenDSS load-convention solved power, negative
            // for generation). The maxQ/minQ and PowerElectronicsUnit min/max
            // bounds below stay unnegated, as upstream emits them.
            if let Some(p) = ibr.p_avail {
                let p = self.num(-p, &what);
                obj.insert("PowerElectronicsConnection.p".into(), p);
            }
            if let Some(kvar) = xf64(&ibr.extras, "kvar") {
                let q = stable_fixed(-kvar * 1e3, |v| (v / 1e3) * 1e3);
                let q = self.num(q, &what);
                obj.insert("PowerElectronicsConnection.q".into(), q);
            }
            let s = self.num(rated_s, &what);
            obj.insert("PowerElectronicsConnection.ratedS".into(), s);
            let rated_u =
                xf64(&ibr.extras, "kv").map(|kv| stable_fixed(kv * 1e3, |v| (v / 1e3) * 1e3));
            if let Some(u) = rated_u {
                let u = self.num(u, &what);
                obj.insert("PowerElectronicsConnection.ratedU".into(), u);
            }
            if let Some(max_q) = &ibr.q_max {
                let total: f64 = max_q.iter().sum();
                let total = self.num(total, &what);
                obj.insert("PowerElectronicsConnection.maxQ".into(), total);
            }
            if let Some(min_q) = &ibr.q_min {
                let total: f64 = min_q.iter().sum();
                let total = self.num(total, &what);
                obj.insert("PowerElectronicsConnection.minQ".into(), total);
            }
            // maxIFault is per-unit of rated current at rated voltage: the
            // inverse of the reader's amps conversion, pinned to the fixed
            // point.
            let hot = ibr.terminal_map.iter().filter(|t| is_hot(t)).count().max(1);
            if let (Some(i_max), Some(u)) = (&ibr.i_max, rated_u)
                && let Some(amps) = i_max.first().copied()
                && rated_s > 0.0
                && u > 0.0
            {
                let rated_amps = if hot == 1 {
                    rated_s / u
                } else {
                    rated_s / (3f64.sqrt() * u)
                };
                let pu = stable_fixed(amps / rated_amps, |v| (v * rated_amps) / rated_amps);
                let pu = self.num(pu, &what);
                obj.insert("PowerElectronicsConnection.maxIFault".into(), pu);
            }

            let unit =
                match ibr.prime_mover {
                    IbrPrimeMover::Pv => {
                        let mut unit =
                            identified("PhotoVoltaicUnit", &format!("{}_PVPanels", ibr.name), None);
                        self.unit_bounds(&mut unit, ibr, &what);
                        Some(unit)
                    }
                    IbrPrimeMover::Battery => {
                        let mut unit =
                            identified("BatteryUnit", &format!("{}_Battery", ibr.name), None);
                        self.unit_bounds(&mut unit, ibr, &what);
                        for (key, prop) in [
                            ("ravens_rated_e", "BatteryUnit.ratedE"),
                            ("ravens_stored_e", "BatteryUnit.storedE"),
                        ] {
                            if let Some(v) = ibr.extras.get(key) {
                                unit.insert(prop.into(), v.clone());
                            }
                        }
                        if let Some(eff) = ibr.extras.get("ravens_battery_efficiency") {
                            unit.insert("BatteryUnit.BatteryUnitEfficiency".into(), eff.clone());
                        }
                        Some(unit)
                    }
                    _ => {
                        self.warn(&C::EMIT_RAVENS_DIST_FIELD_DROPPED, format!(
                        "{what}: prime mover {:?} has no MG-RAVENS unit class; the connection \
                         is emitted without a PowerElectronicsUnit",
                        ibr.prime_mover
                    ));
                        None
                    }
                };
            if let Some(unit) = unit {
                obj.insert(
                    "PowerElectronicsConnection.PowerElectronicsUnit".into(),
                    Value::Object(unit),
                );
            }
            obj.insert("Equipment.inService".into(), Value::Bool(true));
            if let Some((key, value)) = self.base_pointer(&ibr.bus) {
                obj.insert(key, value);
            }
            let terminals = vec![terminal(
                &ibr.name,
                1,
                &ibr.bus,
                &hot_code(&ibr.terminal_map),
                None,
            )];
            obj.insert(
                "ConductingEquipment.Terminals".into(),
                Value::Array(terminals),
            );
            if ibr.control_profile.is_some() {
                self.warn(
                    &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                    format!("{what}: inverter control profile has no MG-RAVENS slot; dropped"),
                );
            }
            if ibr.voltage_aggregation.is_some() {
                self.warn(
                    &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                    format!("{what}: voltage aggregation setting has no MG-RAVENS slot; dropped"),
                );
            }
            self.warn_extras(&what, &ibr.extras, &["kv", "kvar", "phases", "kva"]);
            table.insert(ibr.name.clone(), Value::Object(obj));
        }
        table
    }

    fn unit_bounds(
        &mut self,
        unit: &mut Map<String, Value>,
        ibr: &crate::model::DistIbr,
        what: &str,
    ) {
        if let Some(p_max) = &ibr.p_max {
            let total: f64 = p_max.iter().sum();
            let total = self.num(total, what);
            unit.insert("PowerElectronicsUnit.maxP".into(), total);
        }
        if let Some(p_min) = &ibr.p_min {
            let total: f64 = p_min.iter().sum();
            let total = self.num(total, what);
            unit.insert("PowerElectronicsUnit.minP".into(), total);
        }
    }

    #[allow(clippy::too_many_lines)] // one source writes in one pass, split per conductor when unbalanced
    fn write_sources(&mut self, net: &MulticonductorNetwork) -> Map<String, Value> {
        let mut table = Map::new();
        for source in net.sources() {
            let what = format!("vsource {}", source.name);
            if source.energy_cost_rate.is_some() {
                self.warn(
                    &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                    format!("{what}: energy_cost_rate has no distribution-profile field; dropped"),
                );
            }
            if !self.representable_map(&what, &source.terminal_map) {
                continue;
            }
            let mut obj = identified("EnergySource", &source.name, extras_mrid(&source.extras));
            let hot: Vec<usize> = source
                .terminal_map
                .iter()
                .enumerate()
                .filter(|(_, t)| is_hot(t))
                .map(|(k, _)| k)
                .collect();
            let has_neutral = source.terminal_map.iter().any(|t| !is_hot(t));
            let n = hot.len().max(1) as f64;
            let chord = if hot.len() <= 1 {
                1.0
            } else {
                2.0 * (std::f64::consts::PI / n).sin()
            };
            let v_ln = hot
                .first()
                .and_then(|&k| source.v_magnitude.get(k))
                .copied()
                .unwrap_or(0.0);
            // Emit the wrap-stable fixed point of the reference angle: the
            // reader re-wraps the emitted value into (-pi, pi], and a raw
            // angle can shift one ULP under that wrap, so stabilize here to
            // keep write → read → write byte identical.
            let angle = stable_fixed(
                hot.first()
                    .and_then(|&k| source.v_angle.get(k))
                    .copied()
                    .unwrap_or(0.0),
                wrap_angle,
            );
            // One magnitude and angle represent the source; warn when the
            // per-terminal values do not follow the positive-sequence n-gon
            // the reader will regenerate.
            let uneven = hot.windows(2).any(|pair| {
                terminal_phase(&source.terminal_map[pair[0]])
                    >= terminal_phase(&source.terminal_map[pair[1]])
            }) || hot.iter().enumerate().any(|(i, &k)| {
                let mag_dev = source.v_magnitude.get(k).map_or(0.0, |m| (m - v_ln).abs());
                let expected = wrap_angle(angle - std::f64::consts::TAU / n * i as f64);
                let ang_dev = source.v_angle.get(k).map_or(0.0, |a| (a - expected).abs());
                mag_dev > 1e-9 * v_ln.abs().max(1.0) || ang_dev > 1e-9
            });
            if uneven && hot.len() > 1 {
                // One magnitude and one angle is all an `EnergySource`
                // states. The per terminal phasors keep their values as one
                // single phase EnergySource per phase conductor at the same
                // node, the way the dss writer states an unbalanced source
                // as one ideal source per conductor; the neutral, the
                // Thevenin impedance, and the other extras ride on the first.
                self.warn(
                    &C::EMIT_RAVENS_DIST_SOURCE_SPLIT,
                    format!(
                        "{what}: the per terminal magnitudes or angles are not one balanced \
                         positive sequence set, and an MG-RAVENS EnergySource states one \
                         magnitude and one angle; written as {} single phase EnergySources \
                         `{}_<phase>` with the same terminal phasors",
                        hot.len(),
                        source.name
                    ),
                );
                for (i, &k) in hot.iter().enumerate() {
                    let letter = terminal_phase(&source.terminal_map[k]).unwrap_or("A");
                    let part_name = format!("{}_{letter}", source.name);
                    let part_what = format!("vsource {part_name}");
                    let mut part = identified("EnergySource", &part_name, None);
                    let magnitude = source.v_magnitude.get(k).copied().unwrap_or(0.0);
                    let angle =
                        stable_fixed(source.v_angle.get(k).copied().unwrap_or(0.0), wrap_angle);
                    // The whole source's line to line base, so every part
                    // states the same `nominalVoltage` and seeds the same
                    // BaseVoltage.
                    let ll = line_to_line_chord(hot.len());
                    let nominal = xf64(&source.extras, "basekv").map_or_else(
                        || stable_fixed(v_ln * ll, |v| (v / ll) * ll),
                        |kv| stable_fixed(kv * 1e3, |v| (v / 1e3) * 1e3),
                    );
                    let nominal_v = self.num(nominal, &part_what);
                    let magnitude_v = self.num(magnitude, &part_what);
                    let angle_v = self.num(angle, &part_what);
                    part.insert("EnergySource.nominalVoltage".into(), nominal_v);
                    part.insert("EnergySource.voltageMagnitude".into(), magnitude_v);
                    part.insert("EnergySource.voltageAngle".into(), angle_v);
                    if i == 0 {
                        for (key, prop) in [
                            ("r1", "EnergySource.r"),
                            ("x1", "EnergySource.x"),
                            ("r0", "EnergySource.r0"),
                            ("x0", "EnergySource.x0"),
                        ] {
                            if let Some(v) = xf64(&source.extras, key) {
                                let v = self.num(v, &part_what);
                                part.insert(prop.into(), v);
                            }
                        }
                    }
                    part.insert("Equipment.inService".into(), Value::Bool(true));
                    if let Some((key, value)) = self.base_pointer(&source.bus) {
                        part.insert(key, value);
                    }
                    let code = if i == 0 && has_neutral {
                        format!("PhaseCode.{letter}N")
                    } else {
                        format!("PhaseCode.{letter}")
                    };
                    part.insert(
                        "ConductingEquipment.Terminals".into(),
                        Value::Array(vec![terminal(&part_name, 1, &source.bus, &code, None)]),
                    );
                    table.insert(part_name, Value::Object(part));
                }
                self.warn_extras(
                    &what,
                    &source.extras,
                    &["basekv", "pu", "angle", "r1", "x1", "r0", "x0", "phases"],
                );
                continue;
            }

            let magnitude = stable_fixed(v_ln * chord, |v| (v / chord) * chord);
            // `nominalVoltage` is line to line in CIM: the stated base, or
            // the line to line equivalent of the first phasor.
            let ll = line_to_line_chord(hot.len());
            let nominal = xf64(&source.extras, "basekv").map_or_else(
                || stable_fixed(v_ln * ll, |v| (v / ll) * ll),
                |kv| stable_fixed(kv * 1e3, |v| (v / 1e3) * 1e3),
            );
            let nominal_v = self.num(nominal, &what);
            let magnitude_v = self.num(magnitude, &what);
            let angle_v = self.num(angle, &what);
            obj.insert("EnergySource.nominalVoltage".into(), nominal_v);
            obj.insert("EnergySource.voltageMagnitude".into(), magnitude_v);
            obj.insert("EnergySource.voltageAngle".into(), angle_v);
            for (key, prop) in [
                ("r1", "EnergySource.r"),
                ("x1", "EnergySource.x"),
                ("r0", "EnergySource.r0"),
                ("x0", "EnergySource.x0"),
            ] {
                if let Some(v) = xf64(&source.extras, key) {
                    let v = self.num(v, &what);
                    obj.insert(prop.into(), v);
                }
            }
            obj.insert("Equipment.inService".into(), Value::Bool(true));
            if let Some((key, value)) = self.base_pointer(&source.bus) {
                obj.insert(key, value);
            }
            // A source that states its neutral conductor (a BMOPF source at
            // 0 V on `n`) keeps it in the phase code; the reader gives it
            // back as a terminal at 0 V.
            let code = if has_neutral {
                neutral_inclusive_code(&source.terminal_map)
            } else {
                hot_code(&source.terminal_map)
            };
            let terminals = vec![terminal(&source.name, 1, &source.bus, &code, None)];
            obj.insert(
                "ConductingEquipment.Terminals".into(),
                Value::Array(terminals),
            );
            self.warn_extras(
                &what,
                &source.extras,
                &["basekv", "pu", "angle", "r1", "x1", "r0", "x0", "phases"],
            );
            table.insert(source.name.clone(), Value::Object(obj));
        }
        table
    }

    #[allow(clippy::too_many_lines)] // one tank emits in one pass: ends, catalog record, tests
    fn write_transformers(
        &mut self,
        net: &MulticonductorNetwork,
    ) -> (Map<String, Value>, Map<String, Value>) {
        let mut table = Map::new();
        let mut assets = Map::new();
        for xf in net.transformers() {
            let what = format!("transformer {}", xf.name);
            if xf
                .windings
                .iter()
                .any(|w| !self.representable_map(&what, &w.terminal_map))
            {
                continue;
            }
            if xf.windings.len() < 2 {
                self.warn(
                    &C::EMIT_RAVENS_DIST_RECORD_DROPPED,
                    format!("{what}: fewer than two windings; skipped"),
                );
                continue;
            }
            let info_name = format!("{}_info", xf.name);
            let mut obj = identified("PowerTransformer", &xf.name, extras_mrid(&xf.extras));
            obj.insert(
                "PowerTransformer.vectorGroup".into(),
                Value::String(vector_group(xf)),
            );

            let mut tank = identified("TransformerTank", &xf.name, None);
            tank.insert(
                "PowerSystemResource.AssetDatasheet".into(),
                pointer("TransformerTankInfo", &info_name),
            );
            let mut ends = Vec::new();
            let mut end_infos = Vec::new();
            for (k, w) in xf.windings.iter().enumerate() {
                let num = k as u64 + 1;
                let grounded =
                    w.conn == DistWindingConn::Wye && !w.r_neutral.is_some_and(|r| r < 0.0);
                let mut end = identified(
                    "TransformerTankEnd",
                    &format!("{}_End_{num}", xf.name),
                    None,
                );
                end.insert("TransformerEnd.grounded".into(), Value::Bool(grounded));
                if grounded {
                    let rg = w.r_neutral.unwrap_or(0.0).max(0.0);
                    let xg = w.x_neutral.unwrap_or(0.0);
                    let rg = self.num(rg, &what);
                    let xg = self.num(xg, &what);
                    end.insert("TransformerEnd.rground".into(), rg);
                    end.insert("TransformerEnd.xground".into(), xg);
                }
                // A grounded wye end states its neutral conductor whether or
                // not the winding map spells it: the reader materializes the
                // grounded neutral as terminal 4 (the wire coordinate
                // convention the dss and cim-dist readers share), so the
                // written code must already include it for write, read, write
                // to be byte stable.
                let mut ordered = w.terminal_map.clone();
                if grounded && !ordered.iter().any(|t| !is_hot(t)) {
                    ordered.push("4".to_owned());
                }
                end.insert(
                    "TransformerTankEnd.orderedPhases".into(),
                    Value::String(ordered_code(&ordered)),
                );
                end.insert("TransformerEnd.endNumber".into(), Value::from(num));
                if let Some(base) = self.base_of(&w.bus) {
                    end.insert(
                        "ConductingEquipment.BaseVoltage".into(),
                        pointer("BaseVoltage", &format!("BaseV_{}", fdisp(base / 1e3))),
                    );
                }
                let terminal = terminal(
                    &format!("{}_E{num}", xf.name),
                    1,
                    &w.bus,
                    &hot_code(&w.terminal_map),
                    None,
                );
                end.insert(
                    "ConductingEquipment.Terminals".into(),
                    Value::Array(vec![terminal]),
                );
                ends.push(Value::Object(end));

                // The catalog record: rated values verbatim, resistance in
                // ohms on the winding base pinned to the read→write fixed
                // point.
                let mut info =
                    identified("TransformerEndInfo", &format!("{info_name}_{num}"), None);
                info.insert("TransformerEndInfo.endNumber".into(), Value::from(num));
                info.insert(
                    "TransformerEndInfo.connectionKind".into(),
                    Value::String(format!(
                        "WindingConnection.{}",
                        match w.conn {
                            DistWindingConn::Delta => "D",
                            DistWindingConn::Wye => "Y",
                        }
                    )),
                );
                info.insert("TransformerEndInfo.phaseAngleClock".into(), Value::from(0));
                let rated_u = self.num(w.v_ref, &what);
                let rated_s = self.num(w.s_rating, &what);
                info.insert("TransformerEndInfo.ratedU".into(), rated_u);
                info.insert("TransformerEndInfo.ratedS".into(), rated_s);
                if let Some(z_base) = crate::model::n_winding_impedance_base(1, w.v_ref, w.s_rating)
                {
                    let r = stable_fixed(w.r_pct / 100.0 * z_base, |v| {
                        (v / z_base * 100.0) / 100.0 * z_base
                    });
                    let r = self.num(r, &what);
                    info.insert("TransformerEndInfo.r".into(), r);
                }
                if (w.tap - 1.0).abs() > 0.0 {
                    self.warn(
                        &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                        format!(
                            "{what}: winding {num} tap {} is dropped (tap changers are read from \
                         RAVENS documents but not synthesized into them)",
                            w.tap
                        ),
                    );
                }
                end_infos.push(info);
            }
            tank.insert(
                "TransformerTank.TransformerTankEnd".into(),
                Value::Array(ends),
            );
            obj.insert(
                "PowerTransformer.TransformerTank".into(),
                Value::Array(vec![Value::Object(tank)]),
            );

            // Tests ride on winding 1: the pair (1,2) short-circuit
            // reactance, and the no-load record when the extras carry one.
            let w1 = &xf.windings[0];
            if let Some(z_base) = crate::model::n_winding_impedance_base(1, w1.v_ref, w1.s_rating) {
                let xhl = xf.xsc_pct.first().copied().unwrap_or(0.0);
                let z = stable_fixed(xhl / 100.0 * z_base, |v| {
                    (v / z_base * 100.0) / 100.0 * z_base
                });
                let mut test = identified(
                    "ShortCircuitTest",
                    &format!("{info_name}_1_shortcircuit"),
                    None,
                );
                test.insert("ShortCircuitTest.energisedEndStep".into(), Value::from(1));
                test.insert("ShortCircuitTest.groundedEndStep".into(), Value::from(1));
                let z = self.num(z, &what);
                test.insert("ShortCircuitTest.leakageImpedance".into(), z.clone());
                test.insert("ShortCircuitTest.leakageImpedanceZero".into(), z);
                let base_power = self.num(w1.s_rating, &what);
                test.insert("TransformerTest.basePower".into(), base_power);
                if let Some(info) = end_infos.first_mut() {
                    info.insert(
                        "TransformerEndInfo.EnergisedEndShortCircuitTests".into(),
                        Value::Array(vec![Value::Object(test)]),
                    );
                }
                if xf.xsc_pct.len() > 1 {
                    self.warn(
                        &C::EMIT_RAVENS_DIST_VALUE_COLLAPSED,
                        format!(
                            "{what}: only the first winding pair's short-circuit reactance is \
                         carried; the document format does not encode the remaining pairs"
                        ),
                    );
                }
            }
            // A zero no-load figure states the default, and the reader keeps
            // only nonzero ones, so only those write a test record (write,
            // read, write stays byte stable).
            let noloadloss = xf64(&xf.extras, "%noloadloss").filter(|v| v.abs() > 0.0);
            let imag = xf64(&xf.extras, "%imag").filter(|v| v.abs() > 0.0);
            if noloadloss.is_some() || imag.is_some() {
                let mut test = identified("NoLoadTest", &format!("{info_name}_1_noload"), None);
                let energised = self.num(w1.v_ref, &what);
                test.insert("NoLoadTest.energisedEndVoltage".into(), energised);
                if let Some(pct) = imag {
                    let pct = self.num(pct, &what);
                    test.insert("NoLoadTest.excitingCurrent".into(), pct);
                }
                if let Some(pct) = noloadloss {
                    let loss = stable_fixed(pct / 100.0 * w1.s_rating, |v| {
                        (v / w1.s_rating * 100.0) / 100.0 * w1.s_rating
                    });
                    let loss = self.num(loss, &what);
                    test.insert("NoLoadTest.loss".into(), loss);
                }
                let base_power = self.num(w1.s_rating, &what);
                test.insert("TransformerTest.basePower".into(), base_power);
                if let Some(info) = end_infos.first_mut() {
                    info.insert(
                        "TransformerEndInfo.EnergisedEndNoLoadTests".into(),
                        Value::Array(vec![Value::Object(test)]),
                    );
                }
            }

            let mut tank_info = identified("TransformerTankInfo", &info_name, None);
            tank_info.insert(
                "TransformerTankInfo.TransformerEndInfos".into(),
                Value::Array(end_infos.into_iter().map(Value::Object).collect()),
            );
            let mut infos = Map::new();
            infos.insert(info_name.clone(), Value::Object(tank_info));
            let mut wrapper = Map::new();
            wrapper.insert(
                "PowerTransformerInfo.TransformerTankInfos".into(),
                Value::Object(infos),
            );
            assets.insert(info_name, Value::Object(wrapper));

            self.warn_extras(&what, &xf.extras, &["%noloadloss", "%imag"]);
            table.insert(xf.name.clone(), Value::Object(obj));
        }
        (table, assets)
    }

    fn bus_voltage_limit(&mut self, net: &MulticonductorNetwork, bus: &str) -> Option<String> {
        let b = net.bus(bus)?;
        if b.v_min.is_none() && b.v_max.is_none() {
            return None;
        }
        let normal = self.base_of(bus);
        Some(self.voltage_limit_set(b.v_min, b.v_max, normal))
    }

    #[allow(clippy::too_many_lines)] // one node writes in one pass: limits, ground, location
    fn write_buses(
        &mut self,
        net: &MulticonductorNetwork,
    ) -> (Map<String, Value>, Map<String, Value>, Map<String, Value>) {
        let mut nodes = Map::new();
        let mut locations = Map::new();
        let mut grounds = Map::new();
        for bus in net.buses() {
            let mut obj = identified("ConnectivityNode", &bus.id, extras_mrid(&bus.extras));
            if bus.v_min.is_some() || bus.v_max.is_some() {
                let normal = self.base_of(&bus.id);
                let set = self.voltage_limit_set(bus.v_min, bus.v_max, normal);
                obj.insert(
                    "ConnectivityNode.OperationalLimitSet".into(),
                    pointer("OperationalLimitSet", &set),
                );
            }
            for (field, value) in [
                (
                    "per-phase",
                    bus.v_min_phase.is_some() || bus.v_max_phase.is_some(),
                ),
                ("vpn", bus.vpn_min.is_some() || bus.vpn_max.is_some()),
                ("vpp", bus.vpp_min.is_some() || bus.vpp_max.is_some()),
                ("vpos", bus.vpos_min.is_some() || bus.vpos_max.is_some()),
                ("vneg", bus.vneg_max.is_some()),
                ("vzero", bus.vzero_max.is_some()),
                ("vn", bus.vn_max.is_some()),
            ] {
                if value {
                    self.warn(
                        &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                        format!(
                            "bus {}: {field} voltage bound family has no MG-RAVENS slot; dropped",
                            bus.id
                        ),
                    );
                }
            }
            // A grounded terminal is a CIM `Ground` at the node: one per bus,
            // its terminal's phase code naming the grounded conductors, so
            // a reparse recovers the grounding exactly. A conductor outside
            // the phase vocabulary has no code and is reported.
            if !bus.grounded.is_empty() {
                let unspelled: Vec<&String> = bus
                    .grounded
                    .iter()
                    .filter(|t| terminal_phase(t).is_none())
                    .collect();
                let mut letters: Vec<&str> = bus
                    .grounded
                    .iter()
                    .filter_map(|t| terminal_phase(t))
                    .collect();
                letters.sort_unstable();
                letters.dedup();
                if !unspelled.is_empty() {
                    self.warn(
                        &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                        format!(
                            "bus {}: grounded terminal(s) {unspelled:?} have no MG-RAVENS phase \
                         letter; {}",
                            bus.id,
                            if letters.is_empty() {
                                "the grounding is dropped"
                            } else {
                                "the Ground names the others"
                            }
                        ),
                    );
                }
                if !letters.is_empty() {
                    let name = format!("{}_Ground", bus.id);
                    let mut ground = identified("Ground", &name, None);
                    ground.insert("Equipment.inService".into(), Value::Bool(true));
                    let code = format!("PhaseCode.{}", letters.concat());
                    ground.insert(
                        "ConductingEquipment.Terminals".into(),
                        Value::Array(vec![terminal(&name, 1, &bus.id, &code, None)]),
                    );
                    grounds.insert(name, Value::Object(ground));
                }
            }
            if let Some(location) = &bus.location {
                let name = format!("{}_Location", bus.id);
                let mut loc = identified("Location", &name, None);
                let mut point = Map::new();
                point.insert(
                    "Ravens.cimObjectType".into(),
                    Value::String("PositionPoint".into()),
                );
                point.insert("PositionPoint.sequenceNumber".into(), Value::from(0));
                point.insert(
                    "PositionPoint.xPosition".into(),
                    Value::String(fdisp(location.x)),
                );
                point.insert(
                    "PositionPoint.yPosition".into(),
                    Value::String(fdisp(location.y)),
                );
                loc.insert(
                    "Location.PositionPoints".into(),
                    Value::Array(vec![Value::Object(point)]),
                );
                locations.insert(name, Value::Object(loc));
            }
            self.warn_extras(&format!("bus {}", bus.id), &bus.extras, &[]);
            nodes.insert(bus.id.clone(), Value::Object(obj));
        }
        (nodes, locations, grounds)
    }

    // ------------------------------------------------------------------
    // Untyped round trip and model-level losses
    // ------------------------------------------------------------------

    /// Re-nest objects a RAVENS parse preserved (they carry their raw JSON
    /// and root path); anything else untyped is a genuine loss.
    fn restore_untyped(&mut self, net: &MulticonductorNetwork, root: &mut Map<String, Value>) {
        for u in net.untyped_objects() {
            let prop = |key: &str| {
                u.props
                    .iter()
                    .find(|(k, _)| k.as_deref() == Some(key))
                    .map(|(_, v)| v.as_str())
            };
            let (Some(path), Some(raw)) = (prop("ravens_path"), prop("ravens_json")) else {
                self.warn(
                    &C::EMIT_RAVENS_DIST_RECORD_DROPPED,
                    format!(
                        "untyped {} {}: no MG-RAVENS representation; dropped",
                        u.class, u.name
                    ),
                );
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(raw) else {
                self.warn(
                    &C::EMIT_RAVENS_DIST_RECORD_DROPPED,
                    format!(
                        "untyped {} {}: preserved JSON no longer parses; dropped",
                        u.class, u.name
                    ),
                );
                continue;
            };
            let keys: Vec<&str> = path.split('/').filter(|k| !k.is_empty()).collect();
            if let Err(why) = insert_at(root, &keys, u.name.clone(), value) {
                self.warn(
                    &C::EMIT_RAVENS_DIST_RECORD_DROPPED,
                    format!("untyped {} {}: {why}; dropped", u.class, u.name),
                );
            }
        }
    }

    fn warn_unrepresented(&mut self, net: &MulticonductorNetwork) {
        if let Some(name) = net.name() {
            self.warn(
                &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                format!(
                    "network name `{name}` has no MG-RAVENS slot (the document carries no \
                 circuit record); dropped"
                ),
            );
        }
        #[allow(clippy::float_cmp)] // the default is the exact literal 60.0
        if net.base_frequency() != crate::dss::defaults::BASE_FREQUENCY {
            self.warn(
                &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                format!(
                    "base frequency {} Hz has no MG-RAVENS slot; a reparse assumes 60 Hz \
                 (admittances are stored in siemens, so the model is unaffected)",
                    net.base_frequency()
                ),
            );
        }
        if net.geo().is_some() {
            self.warn(
                &C::EMIT_RAVENS_DIST_FIELD_DROPPED,
                "coordinate-space metadata has no MG-RAVENS slot; bus positions are emitted \
                 as bare PositionPoints",
            );
        }
        for (verb, args) in net.commands().iter().chain(net.options()) {
            self.warn(
                &C::EMIT_RAVENS_DIST_RECORD_DROPPED,
                format!("source command `{verb} {args}` has no MG-RAVENS representation; dropped"),
            );
        }
        for profile in net.control_profiles() {
            self.warn(
                &C::EMIT_RAVENS_DIST_RECORD_DROPPED,
                format!(
                    "control profile {}: inverter control curves have no MG-RAVENS slot; dropped",
                    profile.name
                ),
            );
        }
        self.warn_extras("network", net.extras(), &[]);
    }
}

/// Descend `root` along `path` (creating empty object levels), then insert
/// `value` under `name`. A free function so the mutable borrow of the cursor
/// flows linearly through the descent and back out; the caller re-borrows
/// `root` on each call.
fn insert_at(
    root: &mut Map<String, Value>,
    path: &[&str],
    name: String,
    value: Value,
) -> Result<(), &'static str> {
    let mut cursor = root;
    for key in path {
        let entry = cursor
            .entry((*key).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        match entry.as_object_mut() {
            Some(next) => cursor = next,
            None => return Err("a document path segment is not an object"),
        }
    }
    if cursor.contains_key(&name) {
        return Err("its document slot is already taken");
    }
    cursor.insert(name, value);
    Ok(())
}

/// Wrap to `(-pi, pi]`, matching the reader's angle regeneration.
fn wrap_angle(a: f64) -> f64 {
    let shifted = (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU);
    if shifted <= 0.0 {
        std::f64::consts::PI
    } else {
        shifted - std::f64::consts::PI
    }
}

/// A conventional vector-group label (`Dyn0`, `Yy0`); decorative — the
/// reader types connections from the catalog records.
fn vector_group(xf: &DistTransformer) -> String {
    let mut out = String::new();
    for (k, w) in xf.windings.iter().enumerate() {
        let grounded = w.conn == DistWindingConn::Wye && !w.r_neutral.is_some_and(|r| r < 0.0);
        let letter = match (k, w.conn) {
            (0, DistWindingConn::Delta) => "D",
            (0, DistWindingConn::Wye) => "Y",
            (_, DistWindingConn::Delta) => "d",
            (_, DistWindingConn::Wye) => "y",
        };
        out.push_str(letter);
        if grounded && w.conn == DistWindingConn::Wye {
            out.push_str(if k == 0 { "N" } else { "n" });
        }
    }
    out.push('0');
    out
}

fn versions() -> Value {
    let mut cim = Map::new();
    cim.insert(
        "Ravens.cimObjectType".into(),
        Value::String("IEC61970CIMVersion".into()),
    );
    cim.insert(
        "IEC61970CIMVersion.version".into(),
        Value::String(CIM_VERSION.into()),
    );
    cim.insert(
        "IEC61970CIMVersion.date".into(),
        Value::String(CIM_VERSION_DATE.into()),
    );
    let mut ravens = Map::new();
    ravens.insert(
        "Ravens.cimObjectType".into(),
        Value::String("RavensVersion".into()),
    );
    ravens.insert(
        "RavensVersion.date".into(),
        Value::String(RAVENS_VERSION_DATE.into()),
    );
    ravens.insert(
        "RavensVersion.version".into(),
        Value::String(RAVENS_VERSION.into()),
    );
    let mut versions = Map::new();
    versions.insert("IEC61970CIMVersion".into(), Value::Object(cim));
    versions.insert("RavensVersion".into(), Value::Object(ravens));
    Value::Object(versions)
}

/// Is candidate seed `x` more authoritative than `y`? A seed carries the hot
/// phase count and the nominal line-to-line volts; more phases wins (a
/// three-phase element's line-to-line base is more authoritative than a
/// single-phase regulator's `v_ref·√3` estimate), higher volts breaks ties.
/// A total order, so seed selection is independent of element order.
/// The factor from a line to neutral magnitude to its line to line
/// equivalent for `hot` phase conductors: the chord of the positive sequence
/// n-gon (`sqrt(3)` at three), and `sqrt(3)` again for a single conductor,
/// whose magnitude is line to neutral.
fn line_to_line_chord(hot: usize) -> f64 {
    if hot <= 1 {
        3f64.sqrt()
    } else {
        2.0 * (std::f64::consts::PI / hot as f64).sin()
    }
}

fn seed_better(x: (usize, f64), y: (usize, f64)) -> bool {
    x.0 > y.0 || (x.0 == y.0 && x.1 > y.1)
}

/// Nominal line-to-line volts per bus (lowercased id): seeded by sources and
/// transformer windings, flooded across lines and switches. Loads do not
/// seed — a load's own base may legitimately differ from the bus base.
///
/// Selection is order-independent: each bus keeps the most authoritative seed
/// ([`seed_better`]) rather than the first one written, and the flood takes
/// the per-component maximum. Element order (a dss parse keeps file order, a
/// RAVENS parse sorts by name) therefore cannot change the result, so
/// write → read → write is byte stable.
fn bus_bases(net: &MulticonductorNetwork) -> BTreeMap<String, f64> {
    // Best (hot phase count, volts) seen per bus.
    let mut seeds: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    let mut seed = |bus: &str, hot: usize, volts: f64| {
        if volts > 0.0 {
            let key = bus.to_ascii_lowercase();
            let cand = (hot, volts);
            match seeds.get(&key) {
                Some(cur) if !seed_better(cand, *cur) => {}
                _ => {
                    seeds.insert(key, cand);
                }
            }
        }
    };
    for source in net.sources() {
        let hot = source.terminal_map.iter().filter(|t| is_hot(t)).count();
        // A source states the system base at its bus: `basekv` when it
        // carries one, else the line to line equivalent of its first
        // phasor, which is what the writer states as `nominalVoltage` and
        // the reader hands back as `basekv`, so both passes seed alike. The
        // seed outranks a winding's (a single phase part of a split source
        // still states the whole source's base).
        let volts = xf64(&source.extras, "basekv").map_or_else(
            || {
                let first = source
                    .terminal_map
                    .iter()
                    .position(|t| is_hot(t))
                    .and_then(|k| source.v_magnitude.get(k))
                    .copied()
                    .unwrap_or(0.0);
                first * line_to_line_chord(hot)
            },
            |kv| stable_fixed(kv * 1e3, |v| (v / 1e3) * 1e3),
        );
        seed(&source.bus, 3, volts);
    }
    for xf in net.transformers() {
        for w in &xf.windings {
            let hot = w.terminal_map.iter().filter(|t| is_hot(t)).count();
            let volts = if hot <= 1 {
                w.v_ref * 3f64.sqrt()
            } else {
                w.v_ref
            };
            seed(&w.bus, hot.max(1), volts);
        }
    }
    // Flood across zero-impedance-class edges (lines and switches share a
    // base). Take the per-edge maximum both ways and iterate to a fixed
    // point; max propagation converges to the per-component maximum
    // regardless of edge order.
    loop {
        let mut changed = false;
        let edges = net
            .lines()
            .iter()
            .map(|l| (&l.bus_from, &l.bus_to))
            .chain(net.switches().iter().map(|s| (&s.bus_from, &s.bus_to)));
        for (a, b) in edges {
            let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
            let best = match (seeds.get(&a).copied(), seeds.get(&b).copied()) {
                (Some(x), Some(y)) => {
                    if seed_better(x, y) {
                        x
                    } else {
                        y
                    }
                }
                (Some(x), None) => x,
                (None, Some(y)) => y,
                (None, None) => continue,
            };
            for end in [a, b] {
                if seeds.get(&end) != Some(&best) {
                    seeds.insert(end, best);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    seeds
        .into_iter()
        .map(|(bus, (_, volts))| (bus, volts))
        .collect()
}

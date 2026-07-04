//! `.dss` raw objects into the canonical [`DistNetwork`].
//!
//! Every OpenDSS default materializes into an explicit model value, recorded
//! in [`DistNetwork::defaulted`] under the `"class.name"` key. Specified
//! properties the typed fields do not capture go into the element's `extras`
//! verbatim (string values), so a later writer can reproduce them. Bus specs
//! resolve with the engine's fill rule: phase conductors default to nodes
//! `1..=phases`, every remaining conductor to ground (node 0), and the
//! written dot list overrides from the left. Ground connections become an
//! explicit perfectly grounded neutral terminal on the bus, named
//! `max(4, highest node + 1)` to match PowerModelsDistribution and the
//! public BMOPF examples.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use super::defaults as dd;
use super::lex::{BusSpec, Value, VarMap};
use super::raw::{RawDss, RawObject, parse_raw_with};
use crate::error::{Error, Result};
use crate::geo::{CoordinateSpace, CoordsKind, GeoMeta, Location};
use crate::model::{
    ActivePowerReference, ActivePowerUnit, Configuration, ControlVoltageReference, DistBus,
    DistControlProfile, DistGenerator, DistIbr, DistLine, DistLineCode, DistLoad,
    DistLoadVoltageModel, DistNetwork, DistShunt, DistSourceFormat, DistSwitch, DistTransformer,
    Extras, IbrPrimeMover, IbrTopology, Mat, PowerFactorControl, ReactivePowerReference,
    ReactivePowerUnit, UntypedObject, VoltVarControl, VoltWattControl, VoltageSource, Winding,
    WindingConn, pair_keys, square_from_rows,
};

const TYPED_DSS_CLASSES: &[&str] = &[
    "linecode",
    "vsource",
    "line",
    "transformer",
    "load",
    "capacitor",
    "reactor",
    "generator",
    "pvsystem",
    "xycurve",
    "invcontrol",
    "swtcontrol",
    "regcontrol",
];

/// Parses a `.dss` file, following includes, into the canonical model.
pub fn parse_dss_file(path: impl AsRef<Path>) -> Result<DistNetwork> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;
    let raw = parse_raw_with(&text, &path.display().to_string(), &mut |p: &Path| {
        std::fs::read_to_string(p)
    });
    Ok(network_from_raw(&raw, Arc::new(text)))
}

/// Parses `.dss` text; `Redirect`/`Compile` resolve relative to the working
/// directory.
pub fn parse_dss_str(text: &str) -> DistNetwork {
    let raw = parse_raw_with(text, "<string>", &mut |p: &Path| std::fs::read_to_string(p));
    network_from_raw(&raw, Arc::new(text.to_string()))
}

/// Lowers an executed raw script into the typed model.
pub fn network_from_raw(raw: &RawDss, source: Arc<String>) -> DistNetwork {
    let mut rd = Reader {
        net: DistNetwork {
            name: raw.circuit_name.clone(),
            base_frequency: dd::BASE_FREQUENCY,
            source: Some(source),
            source_format: Some(DistSourceFormat::Dss),
            warnings: raw.warnings.clone(),
            ..DistNetwork::default()
        },
        buses: BTreeMap::new(),
        bus_order: Vec::new(),
        linecode_units: BTreeMap::new(),
        xycurves: BTreeMap::new(),
        vars: &raw.vars,
    };

    for (name, value) in &raw.options {
        // Set option names resolve by first match in the engine's option
        // table order (Command.cpp Getcommand → HashList FindAbbrev), so
        // `Set defaultb=50` is DefaultBaseFrequency but anything shorter
        // ("default", "d") binds DefaultDaily; the bound sits at the unique
        // resolution point.
        if name.len() >= "defaultb".len() && "defaultbasefrequency".starts_with(name.as_str()) {
            if let Ok(f) = value.to_f64(Some(rd.vars)) {
                rd.net.base_frequency = f;
            }
        }
        rd.net.options.push((name.clone(), value.text.clone()));
    }
    for cmd in &raw.commands {
        rd.net.commands.push((cmd.verb.clone(), cmd.args.clone()));
    }

    // Linecodes first: lines reference them. Then everything else in script
    // order per class.
    for obj in raw.of_class("linecode") {
        let lc = rd.linecode(obj);
        rd.net.linecodes.push(lc);
    }
    for obj in raw.of_class("vsource") {
        let vs = rd.vsource(obj);
        rd.net.sources.push(vs);
    }
    for obj in raw.of_class("line") {
        rd.line(obj);
    }
    for obj in raw.of_class("transformer") {
        let t = rd.transformer(obj);
        rd.net.transformers.push(t);
    }
    for obj in raw.of_class("load") {
        let l = rd.load(obj);
        rd.net.loads.push(l);
    }
    for obj in raw.of_class("capacitor") {
        rd.capacitor(obj);
    }
    for obj in raw.of_class("reactor") {
        rd.reactor(obj);
    }
    for obj in raw.of_class("generator") {
        let g = rd.generator(obj);
        rd.net.generators.push(g);
    }
    read_ibr_objects(&mut rd, raw);
    for obj in raw.of_class("swtcontrol") {
        rd.swtcontrol(obj);
    }
    for obj in raw.of_class("regcontrol") {
        rd.regcontrol(obj);
    }
    for obj in &raw.objects {
        if !TYPED_DSS_CLASSES.contains(&obj.class.as_str()) {
            rd.net.untyped.push(UntypedObject::from(obj));
        }
    }

    // A dangling linecode reference would otherwise surface only at write
    // time; the engine refuses it at parse time.
    let known: std::collections::BTreeSet<String> = rd
        .net
        .linecodes
        .iter()
        .map(|c| c.name.to_ascii_lowercase())
        .collect();
    let missing: Vec<String> = rd
        .net
        .lines
        .iter()
        .filter(|l| !known.contains(&l.linecode.to_ascii_lowercase()))
        .map(|l| {
            format!(
                "line {} references unknown linecode `{}`",
                l.name, l.linecode
            )
        })
        .collect();
    rd.net.warnings.extend(missing);

    finish_buses(rd, raw)
}

/// Materializes the accumulated bus states, ground markers, and coordinates.
///
/// Element processing records ground connections (node 0) verbatim; here
/// each grounded bus gains an explicit perfectly grounded neutral terminal
/// named `max(4, highest node + 1)`, the number PowerModelsDistribution
/// and the public BMOPF examples give the materialized neutral, and every
/// element terminal map is rewritten from "0" to it.
fn finish_buses(mut rd: Reader, raw: &RawDss) -> DistNetwork {
    let mut coords: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for c in &raw.buscoords {
        coords.insert(c.bus.to_ascii_lowercase(), (c.x, c.y));
    }
    let buses = std::mem::take(&mut rd.bus_order);
    let states = std::mem::take(&mut rd.buses);
    let mut net = rd.net;
    let mut neutral_names: BTreeMap<String, String> = BTreeMap::new();
    for id in buses {
        let st = &states[&id];
        let mut terminals: Vec<i32> = st.nodes.iter().copied().filter(|&n| n != 0).collect();
        terminals.sort_unstable();
        let mut bus = DistBus {
            id: st.display.clone(),
            terminals: terminals.iter().map(ToString::to_string).collect(),
            ..DistBus::default()
        };
        if st.nodes.contains(&0) {
            let neutral = terminals.last().map_or(4, |&n| n.max(3) + 1);
            bus.terminals.push(neutral.to_string());
            bus.grounded.push(neutral.to_string());
            neutral_names.insert(id.clone(), neutral.to_string());
        }
        if let Some((x, y)) = coords.get(&id) {
            bus.location = Some(Location {
                x: *x,
                y: *y,
                kind: None,
            });
        }
        net.buses.push(bus);
    }
    if !coords.is_empty() {
        net.geo = Some(GeoMeta {
            space: CoordinateSpace::Unknown,
            kind: Some(CoordsKind::Source),
        });
        if coords
            .values()
            .all(|(x, y)| (-180.0..=180.0).contains(x) && (-90.0..=90.0).contains(y))
        {
            net.warnings.push(
                "OpenDSS buscoords fit longitude/latitude ranges; coordinate space remains unknown because Buscoords does not declare a CRS".to_owned(),
            );
        }
    }

    let rewrite = |bus: &str, map: &mut [String]| {
        if let Some(neutral) = neutral_names.get(&bus.to_ascii_lowercase()) {
            for t in map.iter_mut().filter(|t| *t == "0") {
                t.clone_from(neutral);
            }
        }
    };
    for l in &mut net.lines {
        rewrite(&l.bus_from, &mut l.terminal_map_from);
        rewrite(&l.bus_to, &mut l.terminal_map_to);
    }
    for s in &mut net.switches {
        rewrite(&s.bus_from, &mut s.terminal_map_from);
        rewrite(&s.bus_to, &mut s.terminal_map_to);
    }
    for l in &mut net.loads {
        rewrite(&l.bus, &mut l.terminal_map);
    }
    for g in &mut net.generators {
        rewrite(&g.bus, &mut g.terminal_map);
    }
    for s in &mut net.shunts {
        rewrite(&s.bus, &mut s.terminal_map);
    }
    for v in &mut net.sources {
        rewrite(&v.bus, &mut v.terminal_map);
    }
    for t in &mut net.transformers {
        for w in &mut t.windings {
            rewrite(&w.bus, &mut w.terminal_map);
        }
    }
    net
}

fn read_ibr_objects(rd: &mut Reader<'_>, raw: &RawDss) {
    for obj in raw.of_class("pvsystem") {
        rd.pvsystem(obj);
    }
    for obj in raw.of_class("xycurve") {
        rd.xycurve(obj);
    }
    for obj in raw.of_class("invcontrol") {
        rd.invcontrol(obj);
    }
}

impl From<&RawObject> for UntypedObject {
    fn from(obj: &RawObject) -> Self {
        UntypedObject {
            class: obj.class.clone(),
            name: obj.name.clone(),
            props: obj
                .props
                .iter()
                .map(|p| (p.name.clone(), p.value.text.clone()))
                .collect(),
        }
    }
}

struct BusState {
    display: String,
    nodes: std::collections::BTreeSet<i32>,
}

struct Reader<'a> {
    net: DistNetwork,
    buses: BTreeMap<String, BusState>,
    bus_order: Vec<String>,
    /// Linecode name (lowercase) → meters per its length unit, `None` when
    /// the linecode has no units. Lines need it: `ConvertLineUnits` couples
    /// the two sides' units.
    linecode_units: BTreeMap<String, Option<f64>>,
    xycurves: BTreeMap<String, XyCurveRaw>,
    vars: &'a VarMap,
}

/// Last-wins view of an object's resolved properties, plus the set of names
/// actually written (for provenance and extras).
struct Props<'a> {
    by_name: BTreeMap<&'a str, &'a Value>,
    consumed: std::cell::RefCell<Vec<&'a str>>,
}

impl<'a> Props<'a> {
    fn new(obj: &'a RawObject) -> Self {
        let mut by_name = BTreeMap::new();
        for p in &obj.props {
            if let Some(n) = &p.name {
                by_name.insert(n.as_str(), &p.value);
            }
        }
        Props {
            by_name,
            consumed: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn get(&self, name: &'a str) -> Option<&'a Value> {
        self.consumed.borrow_mut().push(name);
        self.by_name.get(name).copied()
    }

    /// Specified properties the typed fields did not consume, for extras.
    fn leftovers(&self) -> Vec<(&str, &Value)> {
        let consumed = self.consumed.borrow();
        self.by_name
            .iter()
            .filter(|(k, _)| !consumed.contains(*k) && **k != "like")
            .map(|(k, v)| (*k, *v))
            .collect()
    }
}

/// Reactor properties that set the impedance directly. When any is present
/// the engine takes its SpecType from the impedance and ignores `kvar`/`kv`,
/// so the kvar-shunt typing does not apply and the object stays untyped.
/// `parallel` (series vs parallel R-X) and `rp` (a parallel damping
/// resistance) are modifiers, not a SpecType of their own: a `kvar` reactor
/// that also sets them is still a kvar shunt, so they are not listed here.
const REACTOR_IMPEDANCE_FORMS: &[&str] = &[
    "rmatrix", "xmatrix", "r", "x", "z1", "z2", "z0", "z", "rcurve", "lcurve", "lmh",
];

#[derive(Clone, Copy)]
struct KvarShuntSpec {
    class: &'static str,
    series_name: &'static str,
    default_phases: usize,
    default_kvar: f64,
    default_kv: f64,
    b_sign: f64,
}

const CAPACITOR_KVAR_SHUNT: KvarShuntSpec = KvarShuntSpec {
    class: "capacitor",
    series_name: "capacitors",
    default_phases: dd::capacitor::PHASES,
    default_kvar: dd::capacitor::KVAR,
    default_kv: dd::capacitor::KV,
    b_sign: 1.0,
};

const REACTOR_KVAR_SHUNT: KvarShuntSpec = KvarShuntSpec {
    class: "reactor",
    series_name: "reactors",
    default_phases: dd::reactor::PHASES,
    default_kvar: dd::reactor::KVAR,
    default_kv: dd::reactor::KV,
    b_sign: -1.0,
};

#[derive(Clone, Debug)]
struct XyCurveRaw {
    x: Vec<f64>,
    y: Vec<f64>,
}

impl Reader<'_> {
    fn warn(&mut self, msg: impl Into<String>) {
        self.net.warnings.push(msg.into());
    }

    fn defaulted(&mut self, class: &str, name: &str, field: &'static str) {
        let fields = self
            .net
            .defaulted
            .entry(format!("{class}.{name}"))
            .or_default();
        if !fields.contains(&field) {
            fields.push(field);
        }
    }

    fn f64_prop(&mut self, p: Option<&Value>) -> Option<f64> {
        p.and_then(|v| v.to_f64(Some(self.vars)).ok())
    }

    fn usize_prop(&mut self, p: Option<&Value>) -> Option<usize> {
        p.and_then(|v| v.to_i64(Some(self.vars)).ok())
            .map(|i| usize::try_from(i).unwrap_or(0))
    }

    /// Meters per source length unit, or `None` when no conversion applies:
    /// the property is missing, `none`, or a code `GetUnitsCode`
    /// (Shared/LineUnits.cpp) does not recognize — the engine maps unknown
    /// codes to UNITS_NONE. Unknown codes warn.
    fn units_code(&mut self, units: Option<&str>, class: &str, name: &str) -> Option<f64> {
        let u = units?;
        if let Some(f) = dd::unit_to_meters(u) {
            return Some(f);
        }
        if !u.to_ascii_lowercase().starts_with("no") {
            self.net.warnings.push(format!(
                "{class} {name}: unknown units `{u}`; treated as none"
            ));
        }
        None
    }

    /// Extras value for a written numeric token: the literal text when it
    /// is already a plain number, otherwise the evaluated value — RPN or
    /// `@var` text is no use to the dss writer, which needs an argument the
    /// engine can read back.
    fn stash_numeric(&self, v: &Value) -> serde_json::Value {
        if v.text.parse::<f64>().is_ok() {
            v.text.clone().into()
        } else {
            match v.to_f64(Some(self.vars)) {
                Ok(n) => n.into(),
                Err(_) => v.text.clone().into(),
            }
        }
    }

    /// `kv` and `phases` for the dss writer: the written token (evaluated
    /// when not a plain number), the materialized default otherwise.
    fn stash_kv_and_phases(&self, props: &Props, extras: &mut Extras, kv: f64, phases: usize) {
        let kv_value = match props.by_name.get("kv") {
            Some(written) => self.stash_numeric(written),
            None => kv.into(),
        };
        extras.insert("kv".into(), kv_value);
        let phases_value = match props.by_name.get("phases") {
            Some(written) => self.stash_numeric(written),
            None => (phases as u64).into(),
        };
        extras.insert("phases".into(), phases_value);
        // A 1 phase delta types as SinglePhase, indistinguishable from a wye
        // spot load without the written token; the writer reads this stash to
        // re-emit conn=delta.
        if let Some(written) = props.by_name.get("conn") {
            extras.insert("conn".into(), written.text.clone().into());
        }
    }

    /// The property's value, or the class default recorded with provenance.
    fn f64_or(
        &mut self,
        props: &Props,
        key: &'static str,
        class: &str,
        name: &str,
        default: f64,
    ) -> f64 {
        if let Some(v) = self.f64_prop(props.get(key)) {
            v
        } else {
            self.defaulted(class, name, key);
            default
        }
    }

    fn usize_or(
        &mut self,
        props: &Props,
        key: &'static str,
        class: &str,
        name: &str,
        default: usize,
    ) -> usize {
        if let Some(v) = self.usize_prop(props.get(key)) {
            v
        } else {
            self.defaulted(class, name, key);
            default
        }
    }

    /// Registers a bus connection and returns the terminal names for the
    /// element. `phases` conductors default to nodes 1..=phases; conductors
    /// beyond that default to ground. `keep` limits how many conductors the
    /// terminal map lists (delta maps exclude the unused trailing conductor).
    fn terminals(
        &mut self,
        spec: &BusSpec,
        phases: usize,
        nconds: usize,
        keep: usize,
    ) -> Vec<String> {
        let mut nodes: Vec<i32> = (1..=i32::try_from(nconds).unwrap_or(i32::MAX)).collect();
        for n in nodes.iter_mut().skip(phases) {
            *n = 0;
        }
        for (i, &n) in spec.nodes.iter().enumerate().take(nconds) {
            nodes[i] = n.max(0); // parser marks bad nodes -1; treat as ground
        }
        let key = spec.name.to_ascii_lowercase();
        let state = self.buses.entry(key.clone()).or_insert_with(|| {
            self.bus_order.push(key.clone());
            BusState {
                display: spec.name.clone(),
                nodes: std::collections::BTreeSet::new(),
            }
        });
        for &n in nodes.iter().take(keep) {
            state.nodes.insert(n);
        }
        nodes.truncate(keep);
        nodes.iter().map(ToString::to_string).collect()
    }

    // ----- linecode ------------------------------------------------------

    fn linecode(&mut self, obj: &RawObject) -> DistLineCode {
        let props = Props::new(obj);
        let n = self.usize_or(
            &props,
            "nphases",
            "linecode",
            &obj.name,
            dd::linecode::NPHASES,
        );
        let units = props.get("units").map(|v| v.text.clone());
        let units_m = self.units_code(units.as_deref(), "linecode", &obj.name);
        let per_meter = units_m.unwrap_or(1.0);
        self.linecode_units
            .insert(obj.name.to_ascii_lowercase(), units_m);

        let freq = self
            .f64_prop(props.get("basefreq"))
            .unwrap_or(self.net.base_frequency);

        let z = self.impedance_matrices(
            &props,
            n,
            "linecode",
            &obj.name,
            dd::line::R1,
            dd::line::X1,
            dd::line::R0,
            dd::line::X0,
            dd::line::C1_NF,
            dd::line::C0_NF,
        );
        if z.all_default {
            self.defaulted("linecode", &obj.name, "rmatrix");
        }

        // Half the total line charging susceptance at each end; OpenDSS
        // carries one C matrix for the whole pi section.
        let b_half = scale_mat(
            &z.c_nf,
            std::f64::consts::TAU * freq * 1e-9 / per_meter / 2.0,
        );
        let zero = vec![vec![0.0; n]; n];

        // i_max carries the emergency rating: PMD's cm_ub and the public
        // BMOPF examples both use emergamps. normamps stays in extras.
        let amps = self.f64_or(
            &props,
            "emergamps",
            "linecode",
            &obj.name,
            dd::line::EMERGAMPS,
        );
        let i_max = Some(vec![amps; n]);

        let mut extras = extras_from_leftovers(&props);
        if let Some(u) = units {
            extras.insert("units".into(), u.into());
        }
        for (key, text) in z.malformed {
            extras.insert(key.to_string(), text.into());
        }
        DistLineCode {
            name: obj.name.clone(),
            n_conductors: n,
            r_series: scale_mat(&z.r, 1.0 / per_meter),
            x_series: scale_mat(&z.x, 1.0 / per_meter),
            g_from: zero.clone(),
            b_from: b_half.clone(),
            g_to: zero,
            b_to: b_half,
            i_max,
            s_max: None,
            extras,
        }
    }

    /// R, X (ohm per unit length) and C (nF per unit length) matrices from
    /// either explicit matrices or sequence values.
    #[allow(clippy::too_many_arguments)]
    fn impedance_matrices(
        &mut self,
        props: &Props,
        n: usize,
        class: &str,
        name: &str,
        r1d: f64,
        x1d: f64,
        r0d: f64,
        x0d: f64,
        c1d: f64,
        c0d: f64,
    ) -> SeriesImpedance {
        let mut malformed: Vec<(&'static str, String)> = Vec::new();
        let mut rows = |key: &'static str| -> Option<Mat> {
            let v = props.get(key)?;
            let parsed = v
                .to_rows(Some(self.vars))
                .ok()
                .and_then(|rows| square_from_rows(&rows, n));
            if parsed.is_none() {
                malformed.push((key, v.text.clone()));
            }
            parsed
        };
        let rm = rows("rmatrix");
        let xm = rows("xmatrix");
        let cm = rows("cmatrix");
        // The engine rejects the whole script on a bad matrix; the liberal
        // reader falls back to the sequence values but says so and keeps
        // the text. A written property is never reported as defaulted.
        for (key, _) in &malformed {
            self.warn(format!(
                "{class} {name}: `{key}` does not parse as a {n}x{n} matrix; \
                 sequence values apply and the text is kept in extras"
            ));
        }
        let any_written = [
            "rmatrix", "xmatrix", "cmatrix", "r1", "x1", "r0", "x0", "c1", "c0", "b1", "b0",
        ]
        .iter()
        .any(|k| props.by_name.contains_key(*k));

        let seq = |props: &Props, k1: &'static str, k0: &'static str, d1: f64, d0: f64| {
            let v1 = props
                .get(k1)
                .and_then(|v| v.to_f64(Some(self.vars)).ok())
                .unwrap_or(d1);
            let v0 = props
                .get(k0)
                .and_then(|v| v.to_f64(Some(self.vars)).ok())
                .unwrap_or(d0);
            if n == 1 {
                return vec![vec![v1]];
            }
            // Symmetric component to phase: diag (2 z1 + z0)/3, off
            // diagonal (z0 - z1)/3.
            let s = (2.0 * v1 + v0) / 3.0;
            let m = (v0 - v1) / 3.0;
            let mut mat = vec![vec![m; n]; n];
            for (i, row) in mat.iter_mut().enumerate() {
                row[i] = s;
            }
            mat
        };

        SeriesImpedance {
            r: rm.unwrap_or_else(|| seq(props, "r1", "r0", r1d, r0d)),
            x: xm.unwrap_or_else(|| seq(props, "x1", "x0", x1d, x0d)),
            c_nf: cm.unwrap_or_else(|| seq(props, "c1", "c0", c1d, c0d)),
            all_default: !any_written,
            malformed,
        }
    }

    // ----- vsource -------------------------------------------------------

    fn vsource(&mut self, obj: &RawObject) -> VoltageSource {
        let props = Props::new(obj);
        let phases = self.usize_or(&props, "phases", "vsource", &obj.name, dd::vsource::PHASES);
        let basekv = self.f64_or(&props, "basekv", "vsource", &obj.name, dd::vsource::BASEKV);
        let pu = self.f64_or(&props, "pu", "vsource", &obj.name, dd::vsource::PU);
        let angle_deg = self.f64_or(
            &props,
            "angle",
            "vsource",
            &obj.name,
            dd::vsource::ANGLE_DEG,
        );
        let spec = if let Some(v) = props.get("bus1") {
            v.to_bus_spec()
        } else {
            self.defaulted("vsource", &obj.name, "bus1");
            Value::new(dd::vsource::BUS1).to_bus_spec()
        };
        let map = self.terminals(&spec, phases, phases + 1, phases + 1);

        // VSource.cpp ~995-1003: one phase takes basekv outright, otherwise
        // the per phase magnitude is basekv / (2 sin(pi/n)) — the chord of
        // the n-gon, which is sqrt(3) only at n = 3. Angles space at
        // -360/n degrees (positive sequence, ~1272), wrapped to (-180, 180]
        // in radians, matching the reference conversion.
        let v_ln = if phases == 1 {
            basekv * 1e3 * pu
        } else {
            basekv * 1e3 * pu / (2.0 * (std::f64::consts::PI / phases as f64).sin())
        };
        let mut v_magnitude = vec![v_ln; phases];
        let mut v_angle: Vec<f64> = (0..phases)
            .map(|k| {
                let deg = angle_deg - 360.0 / phases as f64 * k as f64;
                let a = deg.to_radians();
                // rem_euclid yields [0, tau); shifting puts the result in
                // [-pi, pi), and the reference maps the open end to +pi.
                let shifted = (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU);
                if shifted <= 0.0 {
                    std::f64::consts::PI
                } else {
                    shifted - std::f64::consts::PI
                }
            })
            .collect();
        // The neutral conductor rides at ground.
        v_magnitude.push(0.0);
        v_angle.push(0.0);

        // The raw base voltage rides in extras: the magnitudes fold in pu,
        // and downstream writers need the unscaled base.
        let mut extras = extras_from_leftovers(&props);
        extras.insert("basekv".into(), basekv.into());
        extras.insert("angle".into(), angle_deg.into());
        if (pu - 1.0).abs() > 0.0 {
            extras.insert("pu".into(), pu.into());
        }
        VoltageSource {
            name: obj.name.clone(),
            bus: spec.name,
            terminal_map: map,
            v_magnitude,
            v_angle,
            extras,
        }
    }

    // ----- line / switch -------------------------------------------------

    fn line(&mut self, obj: &RawObject) {
        let props = Props::new(obj);
        let phases = self
            .usize_prop(props.get("phases"))
            .unwrap_or(dd::line::PHASES);
        let spec1 = bus_spec(props.get("bus1"), "");
        let spec2 = bus_spec(props.get("bus2"), "");
        // A line has no neutral conductor of its own: nconds == phases.
        let map_from = self.terminals(&spec1, phases, phases, phases);
        let map_to = self.terminals(&spec2, phases, phases, phases);

        let is_switch = props.get("switch").is_some_and(super::lex::Value::to_bool);
        if is_switch {
            let amps = self.f64_or(&props, "emergamps", "line", &obj.name, dd::line::EMERGAMPS);
            let i_max = Some(vec![amps; phases]);
            let mut extras = extras_from_leftovers(&props);
            // OpenDSS replaces a switch line's impedance with fixed dummy
            // values; record anything written so nothing drops silently.
            for k in ["linecode", "length", "r1", "x1", "rmatrix", "xmatrix"] {
                if let Some(v) = props.by_name.get(k) {
                    extras.insert(k.to_string(), v.text.clone().into());
                    self.warn(format!(
                        "line {}: `{k}` is ignored by OpenDSS on switch=yes; kept in extras",
                        obj.name
                    ));
                }
            }
            self.net.switches.push(DistSwitch {
                name: obj.name.clone(),
                bus_from: spec1.name,
                bus_to: spec2.name,
                terminal_map_from: map_from,
                terminal_map_to: map_to,
                open: false,
                i_max,
                extras,
            });
            return;
        }

        let length_units = props.get("units").map(|v| v.text.clone());
        let line_units_m = self.units_code(length_units.as_deref(), "line", &obj.name);
        let length = self.f64_or(&props, "length", "line", &obj.name, dd::line::LENGTH);

        // ConvertLineUnits (Shared/LineUnits.cpp ~166) is 1.0 when either
        // side is UNITS_NONE, and the engine scales the linecode matrices
        // by Len / FUnitsConvert (Line.cpp ~1177). A unitless line length
        // is therefore in the linecode's units, and a unitless linecode is
        // per line length unit, so the raw length preserves the Z·length
        // product.
        let mut malformed: Vec<(&'static str, String)> = Vec::new();
        let (linecode, length_factor) = if let Some(code) = props.get("linecode") {
            let lc_units_m = self
                .linecode_units
                .get(&code.text.to_ascii_lowercase())
                .copied()
                .flatten();
            let factor = match (lc_units_m, line_units_m) {
                (Some(_), Some(lf)) => lf,
                (Some(lcf), None) => lcf,
                (None, _) => 1.0,
            };
            (code.text.clone(), factor)
        } else {
            let factor = line_units_m.unwrap_or(1.0);
            let (code, bad) = self.synthesize_linecode(&props, phases, factor, &obj.name);
            malformed = bad;
            (code, factor)
        };

        let mut extras = extras_from_leftovers(&props);
        if let Some(u) = length_units {
            extras.insert("units".into(), u.into());
        }
        for (key, text) in malformed {
            extras.insert(key.to_string(), text.into());
        }
        self.net.lines.push(DistLine {
            name: obj.name.clone(),
            bus_from: spec1.name,
            bus_to: spec2.name,
            terminal_map_from: map_from,
            terminal_map_to: map_to,
            linecode,
            length: length * length_factor,
            extras,
        });
    }

    /// A line without `linecode=` carries inline or default impedance;
    /// materialize it as a linecode named `_line_<name>` in the line's own
    /// length units. Malformed matrix texts return for the line's extras.
    fn synthesize_linecode(
        &mut self,
        props: &Props,
        phases: usize,
        length_factor: f64,
        line_name: &str,
    ) -> (String, Vec<(&'static str, String)>) {
        let z = self.impedance_matrices(
            props,
            phases,
            "line",
            line_name,
            dd::line::R1,
            dd::line::X1,
            dd::line::R0,
            dd::line::X0,
            dd::line::C1_NF,
            dd::line::C0_NF,
        );
        if z.all_default {
            self.defaulted("line", line_name, "r1");
            self.defaulted("line", line_name, "x1");
        }
        let b_half = scale_mat(
            &z.c_nf,
            std::f64::consts::TAU * self.net.base_frequency * 1e-9 / length_factor / 2.0,
        );
        let zero = vec![vec![0.0; phases]; phases];
        let amps = self.f64_or(props, "emergamps", "line", line_name, dd::line::EMERGAMPS);
        let i_max = Some(vec![amps; phases]);
        let name = format!("_line_{line_name}");
        self.net.linecodes.push(DistLineCode {
            name: name.clone(),
            n_conductors: phases,
            r_series: scale_mat(&z.r, 1.0 / length_factor),
            x_series: scale_mat(&z.x, 1.0 / length_factor),
            g_from: zero.clone(),
            b_from: b_half.clone(),
            g_to: zero,
            b_to: b_half,
            i_max,
            s_max: None,
            extras: Extras::new(),
        });
        (name, z.malformed)
    }

    // ----- load ----------------------------------------------------------

    /// Final (kWBase, kvarBase, PFNominal, LoadSpecType) after the last
    /// edit boundary, with write provenance for kw and pf.
    ///
    /// Load.cpp runs RecalcElementData at the end of EVERY Edit (~773), so
    /// kw/kvar/pf fold per edit, not flat. Within an edit, kw (case 4,
    /// ~691) sets LoadSpecType 0 (kW + PF), kvar (case 12, ~753) sets 1
    /// (kW + kvar), and pf (case 5, ~699) updates PFNominal without
    /// touching the spec. The boundary recalc (~1342) rederives kvar from
    /// kW and PF under spec 0, and PFNominal from kW and kvar under spec 1
    /// (~1352-1360). like= splices the source's boundaries in the raw
    /// layer, matching MakeLike's copy of the recalced state.
    fn load_power(&mut self, obj: &RawObject) -> LoadPower {
        let mut s = LoadPower {
            kw: dd::load::KW,
            // Constructor kvarBase is 5.0, never observable: spec 1
            // requires a kvar write and the first spec 0 boundary
            // overwrites the seed.
            kvar: 0.0,
            pf: dd::load::PF,
            spec_kvar: false, // LoadSpecType: false = 0, true = 1
            kw_written: false,
            pf_written: false,
        };
        let mut start = 0;
        for end in obj.edit_bounds() {
            for p in &obj.props[start..end] {
                let Some(key @ ("kw" | "kvar" | "pf")) = p.name.as_deref() else {
                    continue;
                };
                let Some(v) = self.f64_prop(Some(&p.value)) else {
                    continue;
                };
                match key {
                    "kw" => {
                        s.kw = v;
                        s.spec_kvar = false;
                        s.kw_written = true;
                    }
                    "kvar" => {
                        s.kvar = v;
                        s.spec_kvar = true;
                    }
                    _ => {
                        s.pf = v;
                        s.pf_written = true;
                    }
                }
            }
            start = end;
            // RecalcElementData at the edit boundary.
            if s.spec_kvar {
                let kva = s.kw.hypot(s.kvar);
                if kva > 0.0 {
                    s.pf = s.kw / kva;
                    // Mixed signs make PF negative (Sign(kWBase*kvarBase)).
                    if s.kw * s.kvar < 0.0 {
                        s.pf = -s.pf;
                    }
                }
            } else {
                s.kvar = s.kw * (1.0 / (s.pf * s.pf) - 1.0).sqrt();
                if s.pf < 0.0 {
                    s.kvar = -s.kvar;
                }
            }
        }
        s
    }

    fn load(&mut self, obj: &RawObject) -> DistLoad {
        let props = Props::new(obj);
        let phases = self.usize_or(&props, "phases", "load", &obj.name, dd::load::PHASES);
        let conn_delta = props.get("conn").is_some_and(|v| {
            v.text.to_ascii_lowercase().starts_with('d') || v.text.eq_ignore_ascii_case("ll")
        });
        let kv = self.f64_or(&props, "kv", "load", &obj.name, dd::load::KV);
        let LoadPower {
            kw,
            kvar: q_total,
            pf,
            spec_kvar,
            kw_written,
            pf_written,
        } = self.load_power(obj);
        if !kw_written {
            self.defaulted("load", &obj.name, "kw");
        }
        // Mark the walked properties consumed so they stay out of extras.
        let _ = (props.get("kw"), props.get("kvar"), props.get("pf"));
        // When the final spec is 0, q derives from the power factor; the
        // source pf rides in extras so the dss writer can emit pf= and let
        // the engine do its own trigonometry — transcendental rounding
        // across implementations would otherwise leak into regenerated
        // cases. Under spec 1 the writer emits kvar=.
        let mut pf_source: Option<f64> = None;
        if !spec_kvar {
            if !pf_written {
                self.defaulted("load", &obj.name, "pf");
            }
            pf_source = Some(pf);
        }
        let model = self
            .usize_prop(props.get("model"))
            .map_or(dd::load::MODEL, |m| i64::try_from(m).unwrap_or(i64::MAX));

        let spec = bus_spec(props.get("bus1"), "");
        let nconds = if conn_delta && phases == 3 {
            phases
        } else {
            phases + 1
        };
        let map = self.terminals(&spec, phases, nconds, nconds);

        let configuration = if phases == 1 {
            Configuration::SinglePhase
        } else if conn_delta {
            Configuration::Delta
        } else {
            Configuration::Wye
        };

        // kv is the load's own base and model its dss load model code;
        // both ride in extras for the writers (the kv default materializes
        // here like every other constructor default), while the typed
        // fields hold explicit power per phase. phases rides too: a 2
        // phase delta load also has 3 conductors, so the terminal map
        // alone cannot reconstruct `phases=`.
        let mut extras = extras_from_leftovers(&props);
        self.stash_kv_and_phases(&props, &mut extras, kv, phases);
        if let Some(pf) = pf_source {
            extras.insert("pf".into(), pf.into());
        }
        if model != 1 {
            extras.insert("model".into(), model.into());
        }
        let v_phase = if phases >= 2 && configuration == Configuration::Wye {
            kv * 1e3 / 3f64.sqrt()
        } else {
            kv * 1e3
        };
        let v_nom = vec![v_phase; phases];
        let zipv = props
            .get("zipv")
            .and_then(|v| v.to_vector(Some(self.vars)).ok())
            .unwrap_or_default();
        let voltage_model = match model {
            2 => DistLoadVoltageModel::ConstantImpedance { v_nom },
            5 => DistLoadVoltageModel::ConstantCurrent { v_nom },
            8 if zipv.len() >= 6 => DistLoadVoltageModel::Zip {
                v_nom,
                alpha_z: vec![zipv[0]; phases],
                alpha_i: vec![zipv[1]; phases],
                alpha_p: vec![zipv[2]; phases],
                beta_z: vec![zipv[3]; phases],
                beta_i: vec![zipv[4]; phases],
                beta_p: vec![zipv[5]; phases],
            },
            8 => DistLoadVoltageModel::Zip {
                v_nom,
                alpha_z: Vec::new(),
                alpha_i: Vec::new(),
                alpha_p: Vec::new(),
                beta_z: Vec::new(),
                beta_i: Vec::new(),
                beta_p: Vec::new(),
            },
            _ => DistLoadVoltageModel::ConstantPower { v_nom },
        };
        DistLoad {
            name: obj.name.clone(),
            bus: spec.name,
            terminal_map: map,
            configuration,
            p_nom: vec![kw * 1e3 / phases as f64; phases],
            q_nom: vec![q_total * 1e3 / phases as f64; phases],
            voltage_model,
            extras,
        }
    }

    // ----- transformer ---------------------------------------------------

    #[allow(clippy::too_many_lines)] // OpenDSS transformer edits must be replayed in order
    fn transformer(&mut self, obj: &RawObject) -> DistTransformer {
        // Order matters: wdg= switches the winding under edit, windings=
        // reallocates. Walk assignments sequentially.
        let mut phases = dd::transformer::PHASES;
        let mut n_windings = dd::transformer::WINDINGS;
        let mut windings = vec![WindingRaw::default(); n_windings];
        let mut active = 0usize;
        let mut xhl = dd::transformer::XHL;
        let mut xht = dd::transformer::XHT;
        let mut xlt = dd::transformer::XLT;
        let mut xhl_specified = false;
        let mut x_pairs: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        let mut extras = Extras::new();
        let conn_is_delta =
            |t: &str| t.to_ascii_lowercase().starts_with('d') || t.eq_ignore_ascii_case("ll");
        for p in &obj.props {
            let Some(name) = &p.name else { continue };
            let v = &p.value;
            match name.as_str() {
                "phases" => {
                    phases = self.usize_prop(Some(v)).unwrap_or(phases);
                }
                "windings" => {
                    n_windings = self.usize_prop(Some(v)).unwrap_or(n_windings).max(1);
                    windings = vec![WindingRaw::default(); n_windings];
                    active = 0;
                }
                "wdg" => {
                    let k = self.usize_prop(Some(v)).unwrap_or(1).max(1);
                    grow(&mut windings, k, &mut n_windings);
                    active = k - 1;
                }
                "bus" => windings[active].bus = Some(v.to_bus_spec()),
                "conn" => windings[active].conn_delta = conn_is_delta(&v.text),
                "kv" | "kva" | "tap" | "%r" | "rneut" | "xneut" => {
                    let parsed = self.f64_prop(Some(v));
                    let w = &mut windings[active];
                    match name.as_str() {
                        "kv" => {
                            w.kv = parsed.unwrap_or(w.kv);
                            w.kv_specified = true;
                        }
                        "kva" => {
                            w.kva = parsed.unwrap_or(w.kva);
                            w.kva_specified = true;
                        }
                        "tap" => w.tap = parsed.unwrap_or(w.tap),
                        "%r" => w.r_pct = parsed.unwrap_or(w.r_pct),
                        "rneut" => w.r_neutral = parsed,
                        "xneut" => w.x_neutral = parsed,
                        _ => unreachable!("matched transformer scalar property"),
                    }
                }
                "buses" | "conns" => {
                    let items = v.to_string_list(Some(self.vars));
                    grow(&mut windings, items.len(), &mut n_windings);
                    apply_winding_strings(&mut windings, name, &items);
                }
                "kvs" | "kvas" | "taps" | "%rs" => match v.to_vector(Some(self.vars)) {
                    Ok(items) => {
                        grow(&mut windings, items.len(), &mut n_windings);
                        apply_winding_numbers(&mut windings, name, &items);
                    }
                    Err(e) => self.warn(format!("transformer {}: {name}: {e}", obj.name)),
                },
                "%loadloss" => {
                    // The engine splits load loss across the first two
                    // windings: %R each = %loadloss / 2 (Transformer.cpp,
                    // property 26). The written value also rides in extras
                    // for the canonical echo.
                    if let Some(ll) = self.f64_prop(Some(v)) {
                        for w in windings.iter_mut().take(2) {
                            w.r_pct = ll / 2.0;
                        }
                    }
                    extras.insert("%loadloss".to_string(), v.text.clone().into());
                }
                "xhl" | "x12" => {
                    xhl = self.f64_prop(Some(v)).unwrap_or(xhl);
                    xhl_specified = true;
                    x_pairs.insert((0, 1), xhl);
                }
                "xht" | "x13" => {
                    xht = self.f64_prop(Some(v)).unwrap_or(xht);
                    x_pairs.insert((0, 2), xht);
                }
                "xlt" | "x23" => {
                    xlt = self.f64_prop(Some(v)).unwrap_or(xlt);
                    x_pairs.insert((1, 2), xlt);
                }
                other if x_pair_key(other).is_some() => {
                    if let Some((i, j)) = x_pair_key(other) {
                        let x = self.f64_prop(Some(v)).unwrap_or(0.0);
                        x_pairs.insert((i, j), x);
                    }
                }
                other => {
                    extras.insert(other.to_string(), v.text.clone().into());
                }
            }
        }

        if !xhl_specified {
            self.defaulted("transformer", &obj.name, "xhl");
        }
        let out = self.finish_windings(&windings, phases, &obj.name);

        let xsc_pct = if n_windings >= 3 {
            pair_keys(n_windings)
                .into_iter()
                .map(|pair| {
                    x_pairs.get(&pair).copied().unwrap_or(match pair {
                        (0, 1) => xhl,
                        (0, 2) => xht,
                        (1, 2) => xlt,
                        _ => 0.0,
                    })
                })
                .collect()
        } else {
            vec![xhl]
        };
        DistTransformer {
            name: obj.name.clone(),
            windings: out,
            xsc_pct,
            phases,
            extras,
        }
    }

    /// Resolves winding bus specs, terminal maps, and SI ratings, recording
    /// provenance for defaulted kv/kva.
    fn finish_windings(
        &mut self,
        windings: &[WindingRaw],
        phases: usize,
        name: &str,
    ) -> Vec<Winding> {
        let mut out = Vec::with_capacity(windings.len());
        for (i, w) in windings.iter().enumerate() {
            if !w.kv_specified {
                self.defaulted("transformer", name, "kv");
            }
            if !w.kva_specified {
                self.defaulted("transformer", name, "kva");
            }
            let spec = w
                .bus
                .clone()
                .unwrap_or_else(|| Value::new(format!("{name}_w{}", i + 1)).to_bus_spec());
            // Each winding terminal has phases + 1 conductors; wye keeps the
            // neutral in the map, delta leaves the unused conductor out. A
            // delta winding is wired line to line, so a single phase delta leg
            // (an open delta secondary, bus spec `.1.2`) still spans two phase
            // terminals; keep both rather than collapsing to one.
            let keep = if w.conn_delta {
                phases.max(2)
            } else {
                phases + 1
            };
            let map = self.terminals(&spec, phases, phases + 1, keep);
            out.push(Winding {
                bus: spec.name,
                terminal_map: map,
                conn: if w.conn_delta {
                    WindingConn::Delta
                } else {
                    WindingConn::Wye
                },
                v_ref: w.kv * 1e3,
                s_rating: w.kva * 1e3,
                r_pct: w.r_pct,
                tap: w.tap,
                r_neutral: w.r_neutral,
                x_neutral: w.x_neutral,
            });
        }
        out
    }

    // ----- capacitor → shunt ---------------------------------------------

    fn capacitor(&mut self, obj: &RawObject) {
        self.kvar_shunt(obj, CAPACITOR_KVAR_SHUNT);
    }

    // ----- reactor → shunt -----------------------------------------------

    /// A grounding (shunt) reactor specified by `kvar`/`kv` maps to a shunt
    /// with inductive (negative) susceptance, the sign mirror of a capacitor.
    /// A reactor from a bus terminal to the same bus's node 0 is also a shunt;
    /// when it uses `r`/`x`, store the equivalent conductance and susceptance.
    /// Other `bus2` reactors are series elements and stay untyped.
    fn reactor(&mut self, obj: &RawObject) {
        let props = Props::new(obj);
        let phases = self.usize_or(&props, "phases", "reactor", &obj.name, dd::reactor::PHASES);
        if phases == 0 {
            self.warn(format!(
                "reactor {}: nonpositive `phases` value is not a typed shunt; kept untyped",
                obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }
        let bus = bus_spec(props.get("bus1"), "");
        let bus2 = props.get("bus2").map(super::lex::Value::to_bus_spec);
        let explicit_single_grounding = bus2
            .as_ref()
            .is_some_and(|return_bus| explicit_single_terminal_ground_return(&bus, return_bus));
        let grounding_return = explicit_single_grounding
            || bus2
                .as_ref()
                .is_some_and(|return_bus| same_bus_ground_return(&bus, return_bus, phases));

        if bus2.is_some() && !grounding_return {
            self.warn(format!(
                "reactor {}: series reactors (bus2) are not typed yet; kept untyped",
                obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }

        if let Some(form) = REACTOR_IMPEDANCE_FORMS
            .iter()
            .find(|k| !matches!(**k, "r" | "x") && props.by_name.contains_key(**k))
        {
            self.warn(format!(
                "reactor {}: impedance form (`{form}`) is not typed yet; kept untyped",
                obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }
        let has_rx = props.by_name.contains_key("r") || props.by_name.contains_key("x");
        if has_rx {
            if grounding_return {
                let grounding_phases = if explicit_single_grounding { 1 } else { phases };
                self.grounding_impedance_reactor(obj, &props, &bus, grounding_phases);
            } else {
                let form = if props.by_name.contains_key("r") {
                    "r"
                } else {
                    "x"
                };
                self.warn(format!(
                    "reactor {}: impedance form (`{form}`) is not typed yet; kept untyped",
                    obj.name
                ));
                self.net.untyped.push(UntypedObject::from(obj));
            }
            return;
        }

        self.kvar_shunt_with_props(obj, &props, REACTOR_KVAR_SHUNT);
    }

    fn grounding_impedance_reactor(
        &mut self,
        obj: &RawObject,
        props: &Props<'_>,
        bus: &BusSpec,
        phases: usize,
    ) {
        // An absent `r`/`x` key defaults to 0, but a key whose token fails to
        // evaluate keeps the object untyped instead of silently substituting 0,
        // which would emit a lossless grounding reactor with no warning that
        // the resistance was dropped.
        let term = |v: Option<&Value>| v.map_or(Ok(0.0), |val| val.to_f64(Some(self.vars)));
        let (Ok(resistance), Ok(reactance)) = (term(props.get("r")), term(props.get("x"))) else {
            self.warn(format!(
                "reactor {}: `r`/`x` does not evaluate to a number; kept untyped",
                obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        };
        let denom = resistance * resistance + reactance * reactance;
        if !denom.is_finite() || denom <= 0.0 {
            self.warn(format!(
                "reactor {}: zero impedance grounding reactor is not a typed shunt; kept untyped",
                obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }
        let map = self.terminals(bus, phases, phases + 1, phases);
        let dim = map.len();
        let mut conductance = vec![vec![0.0; dim]; dim];
        let mut susceptance = vec![vec![0.0; dim]; dim];
        let y_g = resistance / denom;
        let y_b = -reactance / denom;
        for idx in 0..dim {
            conductance[idx][idx] = y_g;
            susceptance[idx][idx] = y_b;
        }
        self.net.shunts.push(DistShunt {
            name: obj.name.clone(),
            bus: bus.name.clone(),
            terminal_map: map,
            g: conductance,
            b: susceptance,
            extras: extras_from_leftovers(props),
        });
    }

    fn kvar_shunt(&mut self, obj: &RawObject, spec: KvarShuntSpec) {
        let props = Props::new(obj);
        self.kvar_shunt_with_props(obj, &props, spec);
    }

    fn kvar_shunt_with_props(&mut self, obj: &RawObject, props: &Props<'_>, spec: KvarShuntSpec) {
        let phases = self.usize_or(props, "phases", spec.class, &obj.name, spec.default_phases);
        if phases == 0 {
            self.warn(format!(
                "{} {}: nonpositive `phases` value is not a typed shunt; kept untyped",
                spec.class, obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }
        // InterpretConnection: `d*` and `ll` are delta for both Capacitor and
        // Reactor. Delta banks are line to line shunts represented by a nodal
        // admittance matrix.
        let conn_delta = props.get("conn").is_some_and(|v| {
            v.text.to_ascii_lowercase().starts_with('d') || v.text.eq_ignore_ascii_case("ll")
        });
        let bus = bus_spec(props.get("bus1"), "");
        if let Some(return_bus) = props.get("bus2").map(super::lex::Value::to_bus_spec) {
            if !same_bus_ground_return(&bus, &return_bus, phases) {
                self.warn(format!(
                    "{} {}: series {} (bus2) are not typed yet; kept untyped",
                    spec.class, obj.name, spec.series_name
                ));
                self.net.untyped.push(UntypedObject::from(obj));
                return;
            }
        }

        if conn_delta && phases == 1 && bus.nodes.len() < 2 {
            self.warn(format!(
                "{} {}: single phase delta shunt needs two bus nodes; kept untyped",
                spec.class, obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }
        // Read the first kvar array entry, as the DSS engine does for a
        // grounding shunt bank.
        let kvar = props
            .get("kvar")
            .and_then(|v| v.to_vector(Some(self.vars)).ok())
            .and_then(|v| v.first().copied())
            .unwrap_or_else(|| {
                self.defaulted(spec.class, &obj.name, "kvar");
                spec.default_kvar
            });
        let kv = self.f64_or(props, "kv", spec.class, &obj.name, spec.default_kv);
        // A wye bank's kv is line to line for 2 or 3 phases, line to neutral
        // otherwise. A delta bank's kv is line to line across each branch.
        let v_ref = if conn_delta {
            kv * 1e3
        } else if phases == 2 || phases == 3 {
            kv * 1e3 / 3f64.sqrt()
        } else {
            kv * 1e3
        };
        // `kvar_shunt_matrix` divides by `v_ref * v_ref`; a positive but tiny
        // `v_ref` can square to zero (or a non-finite) and turn the admittance
        // into an infinity, so reject the squared value here too.
        let v_sq = v_ref * v_ref;
        if !v_ref.is_finite() || v_ref <= 0.0 || !v_sq.is_finite() || v_sq == 0.0 {
            self.warn(format!(
                "{} {}: invalid `kv` value is not a typed shunt; kept untyped",
                spec.class, obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }

        let (nconds, keep) = if conn_delta {
            let keep = match phases {
                1 => 2,
                2 => 3,
                _ => phases,
            };
            (keep, keep)
        } else {
            // The default return is the same bus's ground; register the ground
            // connection but keep the map and matrices phase only, the shape a
            // shunt-to-ground admittance has downstream.
            (phases + 1, phases)
        };
        let map = self.terminals(&bus, phases, nconds, keep);
        let Some(susceptance) =
            kvar_shunt_matrix(&map, phases, conn_delta, kvar, v_ref, spec.b_sign)
        else {
            self.warn(format!(
                "{} {}: delta shunt terminal map is not typed; kept untyped",
                spec.class, obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        };
        let mut extras = extras_from_leftovers(props);
        self.stash_kv_and_phases(props, &mut extras, kv, phases);
        extras.insert("kvar".into(), kvar.into());
        if conn_delta {
            extras.insert("conn".into(), "delta".into());
        }
        self.net.shunts.push(DistShunt {
            name: obj.name.clone(),
            bus: bus.name,
            terminal_map: map,
            g: vec![vec![0.0; susceptance.len()]; susceptance.len()],
            b: susceptance,
            extras,
        });
    }

    // ----- generator -----------------------------------------------------

    fn generator(&mut self, obj: &RawObject) -> DistGenerator {
        let props = Props::new(obj);
        let phases = self.usize_or(
            &props,
            "phases",
            "generator",
            &obj.name,
            dd::generator::PHASES,
        );
        // InterpretConnection (generator.cpp ~299): `d*` and `ll` are delta.
        let conn_delta = props.get("conn").is_some_and(|v| {
            v.text.to_ascii_lowercase().starts_with('d') || v.text.eq_ignore_ascii_case("ll")
        });
        // generator.cpp: kw and pf writes (props 4-5, side effect ~588)
        // call SyncUpPowerQuantities (~3879), rederiving kvar from kW and
        // PF; a kvar write (Set_Presentkvar, ~3857) stores kvar and
        // rederives PF from kW and kvar. The state carries across writes
        // in source order, seeded by the constructor values. Verified
        // asymmetry with Load: the generator resyncs eagerly AT each write
        // and has no end-of-edit recalc, so a flat fold over all writes is
        // correct here while loads need the per edit boundary walk above.
        let mut kw = dd::generator::KW;
        let mut kvar = dd::generator::KVAR;
        let mut pf = dd::generator::PF;
        let (mut kw_written, mut q_written) = (false, false);
        for p in &obj.props {
            let Some(key @ ("kw" | "kvar" | "pf")) = p.name.as_deref() else {
                continue;
            };
            let Some(v) = self.f64_prop(Some(&p.value)) else {
                continue;
            };
            match key {
                "kw" | "pf" => {
                    if key == "kw" {
                        kw = v;
                        kw_written = true;
                    } else {
                        pf = v;
                        q_written = true;
                    }
                    if pf != 0.0 {
                        kvar = kw * (pf.acos().tan()).copysign(pf);
                    }
                }
                _ => {
                    kvar = v;
                    q_written = true;
                    let kva = kw.hypot(kvar);
                    pf = if kva == 0.0 { 1.0 } else { kw / kva };
                    if kw * kvar < 0.0 {
                        pf = -pf;
                    }
                }
            }
        }
        if !kw_written {
            self.defaulted("generator", &obj.name, "kw");
        }
        if !q_written {
            self.defaulted("generator", &obj.name, "kvar");
        }
        // Mark the walked properties consumed so they stay out of extras.
        let _ = (props.get("kw"), props.get("kvar"), props.get("pf"));
        let kv = self.f64_or(&props, "kv", "generator", &obj.name, dd::generator::KV);
        let maxkvar = self.f64_prop(props.get("maxkvar"));
        let minkvar = self.f64_prop(props.get("minkvar"));

        let spec = bus_spec(props.get("bus1"), "");
        let nconds = if conn_delta && phases == 3 {
            phases
        } else {
            phases + 1
        };
        let map = self.terminals(&spec, phases, nconds, nconds);

        let per_phase = |total_kw: f64| vec![total_kw * 1e3 / phases as f64; phases];
        let mut extras = extras_from_leftovers(&props);
        self.stash_kv_and_phases(&props, &mut extras, kv, phases);
        DistGenerator {
            name: obj.name.clone(),
            bus: spec.name,
            terminal_map: map,
            configuration: if phases == 1 {
                Configuration::SinglePhase
            } else if conn_delta {
                Configuration::Delta
            } else {
                Configuration::Wye
            },
            p_nom: per_phase(kw),
            q_nom: per_phase(kvar),
            p_min: None,
            p_max: None,
            q_min: minkvar.map(per_phase),
            q_max: maxkvar.map(per_phase),
            cost: None,
            extras,
        }
    }

    // ----- PVSystem / InvControl ----------------------------------------

    fn pvsystem(&mut self, obj: &RawObject) {
        let props = Props::new(obj);
        let phases = self.usize_or(
            &props,
            "phases",
            "pvsystem",
            &obj.name,
            dd::pvsystem::PHASES,
        );
        if phases == 0 {
            self.warn(format!(
                "pvsystem {}: nonpositive `phases` value is not typed; kept untyped",
                obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }
        let conn_delta = props.get("conn").is_some_and(|v| {
            v.text.to_ascii_lowercase().starts_with('d') || v.text.eq_ignore_ascii_case("ll")
        });
        let spec = bus_spec(props.get("bus1"), "");
        let nconds = if conn_delta && phases == 3 {
            phases
        } else {
            phases + 1
        };
        let map = self.terminals(&spec, phases, nconds, nconds);
        let kv = self.f64_or(&props, "kv", "pvsystem", &obj.name, dd::pvsystem::KV);
        let irradiance = self.f64_or(
            &props,
            "irradiance",
            "pvsystem",
            &obj.name,
            dd::pvsystem::IRRADIANCE,
        );
        let pmpp = self.f64_or(&props, "pmpp", "pvsystem", &obj.name, dd::pvsystem::PMPP);
        let pct_pmpp = self
            .f64_prop(props.get("%pmpp"))
            .or_else(|| self.f64_prop(props.get("pctpmpp")))
            .unwrap_or(dd::pvsystem::PCT_PMPP);
        let kva = self
            .f64_prop(props.get("kva"))
            .unwrap_or(pmpp.max(f64::EPSILON));
        let per_phase = |total_kw: f64| vec![total_kw * 1e3 / phases as f64; phases];
        let p_avail = pmpp * irradiance * pct_pmpp / 100.0 * 1e3;
        let q_max = self
            .f64_prop(props.get("kvarmax"))
            .map(per_phase)
            .or_else(|| Some(vec![kva * 1e3 / phases as f64; phases]));
        let q_min = self
            .f64_prop(props.get("kvarmaxabs"))
            .map(|v| vec![-v * 1e3 / phases as f64; phases])
            .or_else(|| q_max.as_ref().map(|v| v.iter().map(|x| -*x).collect()));
        let topology = if phases == 1 {
            IbrTopology::SinglePhase
        } else if conn_delta {
            IbrTopology::ThreeLeg
        } else {
            IbrTopology::FourLeg
        };
        let pf = self.f64_prop(props.get("pf"));
        let mut extras = extras_from_leftovers(&props);
        self.stash_kv_and_phases(&props, &mut extras, kv, phases);
        extras.remove("conn");
        let mut ibr = DistIbr {
            name: obj.name.clone(),
            bus: spec.name,
            terminal_map: map,
            topology,
            prime_mover: IbrPrimeMover::Pv,
            s_max: vec![kva * 1e3 / phases as f64; phases],
            i_max: None,
            p_avail: Some(p_avail),
            p_min: Some(vec![0.0; phases]),
            p_max: Some(per_phase(pmpp * pct_pmpp / 100.0)),
            q_min,
            q_max,
            control_profile: None,
            voltage_aggregation: None,
            extras,
        };
        if let Some(pf) = pf {
            let profile = format!("{}_pf", obj.name);
            ibr.control_profile = Some(profile.clone());
            self.net.control_profiles.push(DistControlProfile {
                name: profile,
                power_factor: Some(PowerFactorControl { pf }),
                volt_var: None,
                volt_watt: None,
                extras: Extras::new(),
            });
        }
        self.net.ibrs.push(ibr);
    }

    fn xycurve(&mut self, obj: &RawObject) {
        let props = Props::new(obj);
        let x = props
            .get("xarray")
            .and_then(|v| v.to_vector(Some(self.vars)).ok())
            .unwrap_or_default();
        let y = props
            .get("yarray")
            .and_then(|v| v.to_vector(Some(self.vars)).ok())
            .unwrap_or_default();
        if x.is_empty() || y.is_empty() {
            self.warn(format!(
                "xycurve {}: xarray/yarray are incomplete; kept untyped",
                obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }
        self.xycurves
            .insert(obj.name.to_ascii_lowercase(), XyCurveRaw { x, y });
    }

    fn invcontrol(&mut self, obj: &RawObject) {
        let props = Props::new(obj);
        let derlist = props
            .get("derlist")
            .map(|v| dss_name_list(&v.text))
            .unwrap_or_default();
        let mode = props
            .get("mode")
            .map(|v| v.text.to_ascii_lowercase())
            .unwrap_or_default();
        let combimode = props
            .get("combimode")
            .map(|v| v.text.to_ascii_lowercase())
            .unwrap_or_default();
        let mon = props
            .get("monvoltagecalc")
            .map(|v| v.text.to_ascii_lowercase())
            .unwrap_or_default();
        let voltage_reference = if mon.contains("avg") {
            ControlVoltageReference::PgAveraged
        } else {
            ControlVoltageReference::PgPerPhase
        };
        let mut profile = DistControlProfile::new(obj.name.clone());
        profile.volt_var =
            self.invcontrol_volt_var(obj, &props, &derlist, voltage_reference, &mode, &combimode);
        profile.volt_watt =
            self.invcontrol_volt_watt(&props, &derlist, voltage_reference, &mode, &combimode);
        if profile.power_factor.is_none()
            && profile.volt_var.is_none()
            && profile.volt_watt.is_none()
        {
            self.warn(format!(
                "invcontrol {}: control mode is not typed; kept untyped",
                obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }
        for der in derlist {
            let name = der.rsplit_once('.').map_or(der.as_str(), |(_, name)| name);
            if let Some(ibr) = self
                .net
                .ibrs
                .iter_mut()
                .find(|ibr| ibr.name.eq_ignore_ascii_case(name))
            {
                ibr.control_profile = Some(profile.name.clone());
                if profile.volt_var.is_some() {
                    ibr.extras.remove("%pminnovars");
                    ibr.extras.remove("%pminkvarmax");
                }
            } else {
                self.warn(format!(
                    "invcontrol {}: DER `{der}` does not match a typed PVSystem",
                    obj.name
                ));
            }
        }
        self.net.control_profiles.push(profile);
    }

    fn invcontrol_volt_var(
        &mut self,
        obj: &RawObject,
        props: &Props<'_>,
        derlist: &[String],
        voltage_reference: ControlVoltageReference,
        mode: &str,
        combimode: &str,
    ) -> Option<VoltVarControl> {
        if !(mode.contains("voltvar") || combimode.contains("vv")) {
            return None;
        }
        let curve_name = props.get("vvc_curve1").map(|v| v.text.clone())?;
        let curve = self
            .xycurves
            .get(&curve_name.to_ascii_lowercase())
            .cloned()?;
        let base_v = self.control_base_voltage(derlist).unwrap_or_else(|| {
            self.warn(format!(
                "invcontrol {}: no rated voltage found for vvc_curve1; using 1 pu as 1 V",
                obj.name
            ));
            1.0
        });
        let q_ref = props
            .get("refreactivepower")
            .map(|v| v.text.to_ascii_uppercase())
            .filter(|s| s.contains("VARAVAL"))
            .map_or(ReactivePowerReference::VarMax, |_| {
                ReactivePowerReference::VarAvailable
            });
        let p_min_for_q = self
            .ibr_extra_f64(derlist, "%pminnovars")
            .or_else(|| self.f64_prop(props.get("%pminnovars")));
        let p_min_for_q_max = self
            .ibr_extra_f64(derlist, "%pminkvarmax")
            .or_else(|| self.f64_prop(props.get("%pminkvarmax")));
        Some(VoltVarControl {
            voltage_reference: Some(voltage_reference),
            breakpoints: curve.x.iter().map(|x| x * base_v).collect(),
            q_limits: if curve.y.len() >= 4 {
                vec![curve.y[3], curve.y[0]]
            } else {
                curve.y
            },
            q_unit: Some(ReactivePowerUnit::VaFraction),
            q_ref: Some(q_ref),
            p_min_for_q,
            p_min_for_q_max,
        })
    }

    fn invcontrol_volt_watt(
        &mut self,
        props: &Props<'_>,
        derlist: &[String],
        voltage_reference: ControlVoltageReference,
        mode: &str,
        combimode: &str,
    ) -> Option<VoltWattControl> {
        if !(mode.contains("voltwatt") || combimode.contains("vw")) {
            return None;
        }
        let curve_name = props
            .get("voltwatt_curve")
            .or_else(|| props.get("volt_watt_curve"))
            .map(|v| v.text.clone())?;
        let curve = self
            .xycurves
            .get(&curve_name.to_ascii_lowercase())
            .cloned()?;
        let base_v = self.control_base_voltage(derlist).unwrap_or(1.0);
        let p_ref = props
            .get("voltwattyaxis")
            .map(|v| v.text.to_ascii_uppercase())
            .map_or(ActivePowerReference::SMax, |s| {
                if s.contains("PAVAILABLE") {
                    ActivePowerReference::PAvailable
                } else if s.contains("PMPP") {
                    ActivePowerReference::PMax
                } else {
                    ActivePowerReference::SMax
                }
            });
        Some(VoltWattControl {
            voltage_reference: Some(voltage_reference),
            breakpoints: curve.x.iter().map(|x| x * base_v).collect(),
            p_limits: if curve.y.len() >= 2 {
                vec![curve.y[1], curve.y[0]]
            } else {
                curve.y
            },
            p_unit: Some(ActivePowerUnit::VaFraction),
            p_ref: Some(p_ref),
        })
    }

    fn ibr_extra_f64(&self, derlist: &[String], key: &str) -> Option<f64> {
        derlist.iter().find_map(|der| {
            let name = der.rsplit_once('.').map_or(der.as_str(), |(_, name)| name);
            self.net
                .ibrs
                .iter()
                .find(|ibr| ibr.name.eq_ignore_ascii_case(name))
                .and_then(|ibr| ibr.extras.get(key))
                .and_then(json_value_f64)
        })
    }

    fn control_base_voltage(&self, derlist: &[String]) -> Option<f64> {
        let der = derlist.first()?;
        let name = der.rsplit_once('.').map_or(der.as_str(), |(_, name)| name);
        let ibr = self
            .net
            .ibrs
            .iter()
            .find(|ibr| ibr.name.eq_ignore_ascii_case(name))?;
        let kv = ibr
            .extras
            .get("kv")
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(dd::pvsystem::KV);
        Some(match ibr.topology {
            IbrTopology::FourLeg => kv * 1e3 / 3f64.sqrt(),
            IbrTopology::SinglePhase | IbrTopology::ThreeLeg => kv * 1e3,
        })
    }

    // ----- controls ------------------------------------------------------

    fn swtcontrol(&mut self, obj: &RawObject) {
        let props = Props::new(obj);
        let Some(target) = props.get("switchedobj").map(|v| v.text.clone()) else {
            self.warn(format!("swtcontrol {}: no SwitchedObj; ignored", obj.name));
            return;
        };
        // Element references compare class names case insensitively, like
        // every dss identifier.
        let line_name = match target.split_once('.') {
            Some((class, rest)) if class.eq_ignore_ascii_case("line") => rest,
            _ => target.as_str(),
        };
        // The present state follows the last `action`/`state` assignment in
        // source order; `normal` applies only when neither was written.
        let mut open = None;
        for p in &obj.props {
            match p.name.as_deref() {
                Some("action" | "state") => {
                    open = Some(p.value.text.to_ascii_lowercase().starts_with('o'));
                }
                Some("normal") if open.is_none() => {
                    open = Some(p.value.text.to_ascii_lowercase().starts_with('o'));
                }
                _ => {}
            }
        }
        let open = open.unwrap_or(false);
        match self
            .net
            .switches
            .iter_mut()
            .find(|s| s.name.eq_ignore_ascii_case(line_name))
        {
            Some(sw) => sw.open = open,
            None => self.warn(format!(
                "swtcontrol {}: switched object `{target}` is not a switch line",
                obj.name
            )),
        }
    }

    fn regcontrol(&mut self, obj: &RawObject) {
        let props = Props::new(obj);
        let target = props
            .get("transformer")
            .map_or_else(String::new, |v| v.text.clone());
        self.warn(format!(
            "regcontrol {}: voltage regulation is ignored; transformer `{target}` keeps its written taps",
            obj.name
        ));
        self.net.untyped.push(UntypedObject::from(obj));
    }
}

/// Every entry times `k`.
fn scale_mat(m: &Mat, k: f64) -> Mat {
    m.iter()
        .map(|row| row.iter().map(|v| v * k).collect())
        .collect()
}

fn filled_phase_nodes(spec: &BusSpec, phases: usize) -> Vec<i32> {
    let mut nodes: Vec<i32> = (1..=i32::try_from(phases).unwrap_or(i32::MAX)).collect();
    for (idx, &node) in spec.nodes.iter().enumerate().take(phases) {
        nodes[idx] = node.max(0);
    }
    nodes
}

fn same_bus_ground_return(bus: &BusSpec, return_bus: &BusSpec, phases: usize) -> bool {
    bus.name.eq_ignore_ascii_case(&return_bus.name)
        && !return_bus.nodes.is_empty()
        && filled_phase_nodes(return_bus, phases)
            .iter()
            .all(|&n| n <= 0)
}

fn explicit_single_terminal_ground_return(bus: &BusSpec, return_bus: &BusSpec) -> bool {
    bus.name.eq_ignore_ascii_case(&return_bus.name)
        && bus.nodes.len() == 1
        && return_bus.nodes.len() == 1
        && return_bus.nodes[0] <= 0
}

fn dss_name_list(text: &str) -> Vec<String> {
    text.trim_matches(|c: char| matches!(c, '[' | ']' | '(' | ')'))
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn json_value_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

/// The line to line branches of a delta bank over `n` terminals: a closed
/// ring for a 3+ phase bank, an open chain otherwise. Shared with the writer
/// so the reader and writer cannot disagree on the branch topology.
pub(super) fn delta_edges(n: usize, phases: usize) -> Vec<(usize, usize)> {
    if n < 2 {
        Vec::new()
    } else if phases >= 3 && n >= 3 {
        (0..n).map(|i| (i, (i + 1) % n)).collect()
    } else {
        let branches = phases.max(1).min(n - 1);
        (0..branches).map(|i| (i, i + 1)).collect()
    }
}

fn kvar_shunt_matrix(
    map: &[String],
    phases: usize,
    conn_delta: bool,
    kvar: f64,
    v_ref: f64,
    b_sign: f64,
) -> Option<Mat> {
    let dim = map.len();
    let mut susceptance = vec![vec![0.0; dim]; dim];
    if conn_delta {
        let edges = delta_edges(dim, phases);
        if edges.is_empty() || map.iter().any(|t| t == "0") {
            return None;
        }
        let b_branch = b_sign * kvar * 1e3 / edges.len() as f64 / (v_ref * v_ref);
        for (from, to) in edges {
            susceptance[from][from] += b_branch;
            susceptance[to][to] += b_branch;
            susceptance[from][to] -= b_branch;
            susceptance[to][from] -= b_branch;
        }
    } else {
        let b_phase = b_sign * kvar * 1e3 / phases as f64 / (v_ref * v_ref);
        for (idx, row) in susceptance.iter_mut().enumerate().take(phases) {
            row[idx] = b_phase;
        }
    }
    Some(susceptance)
}

fn bus_spec(v: Option<&Value>, fallback: &str) -> BusSpec {
    v.map_or_else(
        || Value::new(fallback).to_bus_spec(),
        super::lex::Value::to_bus_spec,
    )
}

fn extras_from_leftovers(props: &Props) -> Extras {
    let mut extras = Extras::new();
    for (k, v) in props.leftovers() {
        extras.insert(k.to_string(), v.text.clone().into());
    }
    extras
}

/// `buses=(...)` / `conns=(...)` applied across windings.
fn apply_winding_strings(windings: &mut [WindingRaw], name: &str, items: &[String]) {
    let conn_is_delta =
        |t: &str| t.to_ascii_lowercase().starts_with('d') || t.eq_ignore_ascii_case("ll");
    for (i, item) in items.iter().enumerate() {
        let w = &mut windings[i];
        if name == "buses" {
            w.bus = Some(Value::new(item.clone()).to_bus_spec());
        } else {
            w.conn_delta = conn_is_delta(item);
        }
    }
}

/// A numeric transformer array (`kvs=(...)`, RPN entries included) applied
/// across windings.
fn apply_winding_numbers(windings: &mut [WindingRaw], name: &str, items: &[f64]) {
    for (i, &item) in items.iter().enumerate() {
        let w = &mut windings[i];
        match name {
            "kvs" => {
                w.kv = item;
                w.kv_specified = true;
            }
            "kvas" => {
                w.kva = item;
                w.kva_specified = true;
            }
            "taps" => w.tap = item,
            _ => w.r_pct = item,
        }
    }
}

fn x_pair_key(name: &str) -> Option<(usize, usize)> {
    let rest = name.strip_prefix('x')?;
    if rest.len() != 2 || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut chars = rest.chars();
    let i = chars.next()?.to_digit(10)? as usize;
    let j = chars.next()?.to_digit(10)? as usize;
    if i == 0 || j == 0 || i == j {
        return None;
    }
    Some((i.min(j) - 1, i.max(j) - 1))
}

/// A load's power state after the last edit boundary: the engine's
/// (kWBase, kvarBase, PFNominal, LoadSpecType), plus which of kw/pf were
/// ever written (for default provenance).
struct LoadPower {
    kw: f64,
    kvar: f64,
    pf: f64,
    /// LoadSpecType: false = 0 (kW + PF), true = 1 (kW + kvar).
    spec_kvar: bool,
    kw_written: bool,
    pf_written: bool,
}

/// Series impedance of a linecode or inline line, per source length unit.
struct SeriesImpedance {
    r: Mat,
    x: Mat,
    c_nf: Mat,
    /// No matrix or sequence property was written at all.
    all_default: bool,
    /// Matrix properties written but unparseable as n x n, with their raw
    /// text (the engine rejects the whole script; the reader keeps them
    /// in extras).
    malformed: Vec<(&'static str, String)>,
}

#[derive(Clone)]
struct WindingRaw {
    bus: Option<BusSpec>,
    conn_delta: bool,
    kv: f64,
    kva: f64,
    tap: f64,
    r_pct: f64,
    r_neutral: Option<f64>,
    x_neutral: Option<f64>,
    kv_specified: bool,
    kva_specified: bool,
}

impl Default for WindingRaw {
    fn default() -> Self {
        WindingRaw {
            bus: None,
            conn_delta: false,
            kv: dd::transformer::KV,
            kva: dd::transformer::KVA,
            tap: dd::transformer::TAP,
            r_pct: dd::transformer::PCT_R,
            r_neutral: None,
            x_neutral: None,
            kv_specified: false,
            kva_specified: false,
        }
    }
}

/// Grows the winding list to at least `n`, tracking the winding count.
fn grow(windings: &mut Vec<WindingRaw>, n: usize, count: &mut usize) {
    if n > windings.len() {
        windings.resize(n, WindingRaw::default());
        *count = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_warning(net: &DistNetwork, needle: &str) -> bool {
        net.warnings.iter().any(|w| w.contains(needle))
    }

    #[test]
    fn vsource_magnitude_is_the_polygon_chord() {
        // VSource.cpp ~999-1002: one phase takes basekv outright, n > 1
        // divides by 2 sin(pi/n); sqrt(3) is the n = 3 special case.
        let net = parse_dss_str(
            "New Circuit.c basekv=12.47 pu=1.05 phases=2 bus1=src.1.2\n\
             New Vsource.aux basekv=12.47 phases=4 bus1=b2\n\
             New Vsource.solo basekv=2.4 phases=1 bus1=b3.1",
        );
        let two = &net.sources[0];
        assert!((two.v_magnitude[0] - 12.47e3 * 1.05 / 2.0).abs() < 1e-9);
        // Spacing is -360/n degrees: the second phase of a 2 phase source
        // wraps to +pi.
        assert!((two.v_angle[1] - std::f64::consts::PI).abs() < 1e-12);
        let four = &net.sources[1];
        let chord = 2.0 * (std::f64::consts::PI / 4.0).sin();
        assert!((four.v_magnitude[0] - 12.47e3 / chord).abs() < 1e-9);
        let solo = &net.sources[2];
        assert!((solo.v_magnitude[0] - 2.4e3).abs() < 1e-9);
    }

    #[test]
    fn vsource_defaults_are_recorded() {
        let net = parse_dss_str("New Circuit.c1");
        let fields = net.defaulted.get("vsource.source").expect("entry");
        for key in ["phases", "pu", "angle", "basekv", "bus1"] {
            assert!(fields.contains(&key), "missing {key}");
        }
    }

    /// One single phase linecode + line; (r per meter, length meters).
    fn r_and_length(lc_tail: &str, line_tail: &str) -> (f64, f64) {
        let net = parse_dss_str(&format!(
            "New Circuit.c\n\
             New Linecode.lc nphases=1 rmatrix=(0.5){lc_tail}\n\
             New Line.l1 bus1=a.1 bus2=b.1 phases=1 linecode=lc{line_tail}"
        ));
        let line = net.lines.iter().find(|l| l.name == "l1").unwrap();
        let code = net.linecode(&line.linecode).unwrap();
        (code.r_series[0][0], line.length)
    }

    #[test]
    fn unitless_line_length_is_in_linecode_units() {
        // ConvertLineUnits is 1.0 when the line has no units, so the
        // engine reads `length=2` against a km linecode as 2 km:
        // 0.5 ohm/km * 2 km = 1 ohm total.
        let (r, len) = r_and_length(" units=km", " length=2");
        assert!((len - 2000.0).abs() < 1e-9);
        assert!((r * len - 1.0).abs() < 1e-12);
    }

    #[test]
    fn unitless_linecode_is_per_line_unit() {
        // The mirror case: a unitless linecode is per line length unit,
        // so the raw length carries and the total is again 1 ohm.
        let (r, len) = r_and_length("", " length=2 units=km");
        assert!((len - 2.0).abs() < 1e-12);
        assert!((r * len - 1.0).abs() < 1e-12);
    }

    #[test]
    fn written_units_on_both_sides_convert() {
        // 0.5 ohm/km over 500 m = 0.25 ohm.
        let (r, len) = r_and_length(" units=km", " length=500 units=m");
        assert!((len - 500.0).abs() < 1e-9);
        assert!((r * len - 0.25).abs() < 1e-12);
    }

    #[test]
    fn one_phase_inline_sequence_values_stay_positive_sequence() {
        let net = parse_dss_str(
            "New Circuit.c\n\
             New Line.l1 bus1=a.1 bus2=b.1 phases=1 length=0.5 units=km r1=0.5 x1=0.2 c1=3",
        );
        let line = net.lines.iter().find(|l| l.name == "l1").unwrap();
        let code = net.linecode(&line.linecode).unwrap();
        assert!((line.length - 500.0).abs() < 1e-9);
        assert!((code.r_series[0][0] * line.length - 0.25).abs() < 1e-12);
        assert!((code.x_series[0][0] * line.length - 0.1).abs() < 1e-12);
    }

    #[test]
    fn no_units_anywhere_takes_the_raw_product() {
        let (r, len) = r_and_length("", " length=2");
        assert!((len - 2.0).abs() < 1e-12);
        assert!((r * len - 1.0).abs() < 1e-12);
    }

    #[test]
    fn two_phase_wye_capacitor_kv_is_line_to_line() {
        // Capacitor.cpp ~621-630: PhasekV = kv/sqrt(3) for 2 AND 3 phase
        // wye banks, kv outright otherwise.
        let net = parse_dss_str(
            "New Circuit.c\n\
             New Capacitor.c2 bus1=b.1.2 phases=2 kv=12.47 kvar=600\n\
             New Capacitor.c1 bus1=b.3 phases=1 kv=7.2 kvar=300",
        );
        let c2 = net.shunts.iter().find(|s| s.name == "c2").unwrap();
        let v2 = 12.47e3 / 3f64.sqrt();
        assert!((c2.b[0][0] * v2 * v2 / 300e3 - 1.0).abs() < 1e-12);
        let c1 = net.shunts.iter().find(|s| s.name == "c1").unwrap();
        let v1 = 7.2e3;
        assert!((c1.b[0][0] * v1 * v1 / 300e3 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn capacitor_and_reactor_kvar_shunts_share_magnitude_with_opposite_sign() {
        let net = parse_dss_str(
            "New Circuit.c\n\
             New Capacitor.cap bus1=b.1 phases=1 kv=7.2 kvar=300\n\
             New Reactor.rea bus1=b.2 phases=1 kv=7.2 kvar=300",
        );
        let cap = net.shunts.iter().find(|s| s.name == "cap").unwrap();
        let rea = net.shunts.iter().find(|s| s.name == "rea").unwrap();
        assert!(cap.b[0][0] > 0.0);
        assert!(rea.b[0][0] < 0.0);
        assert!((cap.b[0][0] + rea.b[0][0]).abs() < 1e-18);
    }

    #[test]
    fn kvar_shunts_with_nonpositive_phases_stay_untyped() {
        let net = parse_dss_str(
            "New Circuit.c\n\
             New Capacitor.cap bus1=b.1 phases=0 kv=7.2 kvar=300\n\
             New Reactor.rea bus1=b.2 phases=0 kv=7.2 kvar=300",
        );
        assert!(net.shunts.is_empty());
        assert!(
            net.untyped
                .iter()
                .any(|u| u.class.eq_ignore_ascii_case("capacitor") && u.name == "cap")
        );
        assert!(
            net.untyped
                .iter()
                .any(|u| u.class.eq_ignore_ascii_case("reactor") && u.name == "rea")
        );
        assert!(
            net.warnings
                .iter()
                .any(|w| w.contains("capacitor cap: nonpositive `phases`"))
        );
        assert!(
            net.warnings
                .iter()
                .any(|w| w.contains("reactor rea: nonpositive `phases`"))
        );
    }

    #[test]
    fn ll_connection_means_delta() {
        // InterpretConnection maps `ll` to delta for every class.
        let net = parse_dss_str(
            "New Circuit.c\n\
             New Generator.g bus1=b.1.2.3 phases=3 conn=ll kw=90 kvar=30 kv=4.16\n\
             New Capacitor.cap bus1=b.1.2.3 phases=3 conn=ll kvar=600 kv=4.16",
        );
        assert_eq!(net.generators[0].configuration, Configuration::Delta);
        // `ll` capacitor banks use the delta shunt path.
        assert_eq!(net.shunts.len(), 1);
        let sh = &net.shunts[0];
        assert!(sh.b[0][1] < 0.0, "{:?}", sh.b);
        assert_eq!(sh.terminal_map, vec!["1", "2", "3"]);
        assert!(
            net.untyped
                .iter()
                .all(|u| !(u.class.eq_ignore_ascii_case("capacitor") && u.name == "cap"))
        );
    }

    #[test]
    fn load_kw_after_kvar_reverts_to_pf() {
        // Load.cpp: kw flips LoadSpecType back to 0 (kW + PF), so the
        // earlier kvar is discarded and q comes from the default pf 0.88.
        let net =
            parse_dss_str("New Circuit.c\nNew Load.l bus1=b.1 phases=1 kv=2.4 kvar=20 kw=100");
        let l = &net.loads[0];
        let q: f64 = l.q_nom.iter().sum();
        assert!((q - 100e3 * 0.88f64.acos().tan()).abs() < 1e-6);
        assert_eq!(
            l.extras.get("pf").and_then(serde_json::Value::as_f64),
            Some(0.88)
        );
        assert!(
            net.defaulted
                .get("load.l")
                .is_some_and(|f| f.contains(&"pf"))
        );
    }

    #[test]
    fn load_like_replays_the_sources_recalced_pf() {
        // Load.a ends its New under spec 1: recalc derives
        // PFNominal = 10/sqrt(10² + 20²) = 0.4472 (kw still the constructor
        // 10). MakeLike copies that recalced state, so b's kw=100 flips to
        // spec 0 and the end-of-edit recalc lands kvar =
        // 100·tan(acos(0.4472)) = 200, not the 53.97 a flat walk against
        // pf 0.88 would give. Confirmed against opendssdirect.
        let net = parse_dss_str(
            "New Circuit.c\n\
             New Load.a bus1=b.1 phases=1 kv=2.4 kvar=20\n\
             New Load.b like=a kw=100",
        );
        let b = net.loads.iter().find(|l| l.name == "b").unwrap();
        let q: f64 = b.q_nom.iter().sum();
        assert!((q - 200e3).abs() < 1e-6);
        // Final spec is 0: the writer emits pf=, the recalced 0.4472.
        let pf = b.extras.get("pf").and_then(serde_json::Value::as_f64);
        assert!((pf.unwrap() - 0.447_213_595_499_957_9).abs() < 1e-12);
        // The source itself keeps its written kvar.
        let a = net.loads.iter().find(|l| l.name == "a").unwrap();
        let qa: f64 = a.q_nom.iter().sum();
        assert!((qa - 20e3).abs() < 1e-9);
    }

    #[test]
    fn load_tilde_continuation_recalcs_at_each_edit() {
        // Same numbers via `~`: the New line's recalc fixes pf at 0.4472,
        // the continuation's kw=100 reverts to spec 0 and its own recalc
        // gives kvar = 200. A flat last-write walk would say 53.97.
        let net = parse_dss_str(
            "New Circuit.c\n\
             New Load.l bus1=b.1 phases=1 kv=2.4 kvar=20\n\
             ~ kw=100",
        );
        let q: f64 = net.loads[0].q_nom.iter().sum();
        assert!((q - 200e3).abs() < 1e-6);
    }

    #[test]
    fn load_pf_between_kvar_and_kw_applies() {
        // pf (case 5) updates PFNominal without touching the spec; the
        // later kw sets spec 0, so the single recalc uses pf 0.95:
        // q = 100·tan(acos(0.95)) = 32.868. Confirmed against
        // opendssdirect.
        let net = parse_dss_str(
            "New Circuit.c\nNew Load.l bus1=b.1 phases=1 kv=2.4 kvar=20 pf=0.95 kw=100",
        );
        let l = &net.loads[0];
        let q: f64 = l.q_nom.iter().sum();
        assert!((q - 100e3 * 0.95f64.acos().tan()).abs() < 1e-6);
        assert_eq!(
            l.extras.get("pf").and_then(serde_json::Value::as_f64),
            Some(0.95)
        );
        assert!(
            !net.defaulted
                .get("load.l")
                .is_some_and(|f| f.contains(&"pf"))
        );
    }

    #[test]
    fn load_kvar_after_kw_stays() {
        let net =
            parse_dss_str("New Circuit.c\nNew Load.l bus1=b.1 phases=1 kv=2.4 kw=100 kvar=20");
        let l = &net.loads[0];
        let q: f64 = l.q_nom.iter().sum();
        assert!((q - 20e3).abs() < 1e-9);
        // The writer must emit kvar=, not pf=.
        assert!(!l.extras.contains_key("pf"));
    }

    #[test]
    fn generator_kw_after_kvar_resyncs_q() {
        // Set_Presentkvar rederives PF from kW and kvar; the later kw
        // write resyncs kvar from that PF. Constructor kW is 1000, so
        // kvar=20 kw=100 scales q to 100 * 20/1000 = 2 kvar.
        let net =
            parse_dss_str("New Circuit.c\nNew Generator.g bus1=b.1 phases=1 kv=2.4 kvar=20 kw=100");
        let q: f64 = net.generators[0].q_nom.iter().sum();
        assert!((q - 2e3).abs() < 1e-6);
    }

    #[test]
    fn generator_kvar_after_kw_stays() {
        let net =
            parse_dss_str("New Circuit.c\nNew Generator.g bus1=b.1 phases=1 kv=2.4 kw=100 kvar=20");
        let q: f64 = net.generators[0].q_nom.iter().sum();
        assert!((q - 20e3).abs() < 1e-9);
    }

    #[test]
    fn generator_pf_after_kvar_wins() {
        // pf calls SyncUpPowerQuantities: kvar = kW tan(acos(pf)) with the
        // constructor kW 1000.
        let net = parse_dss_str(
            "New Circuit.c\nNew Generator.g bus1=b.1.2.3 phases=3 kv=4.16 kvar=20 pf=0.9",
        );
        let q: f64 = net.generators[0].q_nom.iter().sum();
        assert!((q - 1000e3 * 0.9f64.acos().tan()).abs() < 1e-3);
    }

    #[test]
    fn malformed_matrix_warns_and_keeps_text() {
        // The engine rejects a bad rmatrix outright; the reader keeps
        // going on sequence values but must not call the property
        // defaulted, and the text must survive in extras.
        let net = parse_dss_str(
            "New Circuit.c\n\
             New Linecode.bad nphases=2 rmatrix=(1 2 3) units=m\n\
             New Line.l2 bus1=a.1.2 bus2=b.1.2 phases=2 rmatrix=(bogus) length=10",
        );
        assert!(has_warning(&net, "linecode bad") && has_warning(&net, "rmatrix"));
        assert!(
            !net.defaulted
                .get("linecode.bad")
                .is_some_and(|f| f.contains(&"rmatrix"))
        );
        let code = net.linecode("bad").unwrap();
        assert!(
            code.extras
                .get("rmatrix")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| s.contains("1 2 3"))
        );
        // Sequence defaults filled in: diag (2 r1 + r0) / 3.
        let diag = (2.0 * dd::line::R1 + dd::line::R0) / 3.0;
        assert!((code.r_series[0][0] - diag).abs() < 1e-12);
        // The inline line path lands the text on the line's extras.
        assert!(has_warning(&net, "line l2"));
        let l2 = net.lines.iter().find(|l| l.name == "l2").unwrap();
        assert!(
            l2.extras
                .get("rmatrix")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| s.contains("bogus"))
        );
    }

    #[test]
    fn switchedobj_class_prefix_is_case_insensitive() {
        let net = parse_dss_str(
            "New Circuit.c\n\
             New Line.sw1 bus1=a.1 bus2=b.1 phases=1 switch=y\n\
             New SwtControl.s1 SwitchedObj=LINE.sw1 Action=open",
        );
        assert!(net.switches[0].open);
    }

    #[test]
    fn phases_token_rides_in_extras() {
        // A 2 phase delta load has 3 conductors, indistinguishable from a
        // 3 phase delta by terminal map alone.
        let net = parse_dss_str(
            "New Circuit.c\n\
             New Load.l bus1=b.1.2 phases=2 conn=delta kw=50 kvar=10 kv=4.8\n\
             New Generator.g bus1=b.1.2.3 kw=10 kvar=2 kv=4.16\n\
             New Capacitor.cap bus1=b.1.2.3 phases=3 kvar=600 kv=4.16",
        );
        let l = &net.loads[0];
        assert_eq!(l.terminal_map.len(), 3);
        assert_eq!(
            l.extras.get("phases").and_then(serde_json::Value::as_str),
            Some("2")
        );
        // An unwritten phases= materializes the class default.
        assert_eq!(
            net.generators[0]
                .extras
                .get("phases")
                .and_then(serde_json::Value::as_u64),
            Some(3)
        );
        assert_eq!(
            net.shunts[0]
                .extras
                .get("phases")
                .and_then(serde_json::Value::as_str),
            Some("3")
        );
    }

    #[test]
    fn rpn_kv_token_stashes_the_evaluated_value() {
        // The writer needs a number; RPN text would not read back.
        let net = parse_dss_str("New Circuit.c\nNew Load.l bus1=b.1 phases=1 kw=10 kv={4.8 2 /}");
        assert_eq!(
            net.loads[0]
                .extras
                .get("kv")
                .and_then(serde_json::Value::as_f64),
            Some(2.4)
        );
    }
}

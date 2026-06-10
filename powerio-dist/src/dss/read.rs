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
use crate::model::{
    Configuration, DistBus, DistGenerator, DistLine, DistLineCode, DistLoad, DistNetwork,
    DistShunt, DistSourceFormat, DistSwitch, DistTransformer, Extras, Mat, UntypedObject,
    VoltageSource, Winding, WindingConn, square_from_rows,
};

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
        vars: &raw.vars,
    };

    for (name, value) in &raw.options {
        if name == "defaultbasefrequency" {
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
    for obj in raw.of_class("generator") {
        let g = rd.generator(obj);
        rd.net.generators.push(g);
    }
    for obj in raw.of_class("swtcontrol") {
        rd.swtcontrol(obj);
    }
    for obj in raw.of_class("regcontrol") {
        rd.regcontrol(obj);
    }
    for obj in &raw.objects {
        if !matches!(
            obj.class.as_str(),
            "linecode"
                | "vsource"
                | "line"
                | "transformer"
                | "load"
                | "capacitor"
                | "generator"
                | "swtcontrol"
                | "regcontrol"
        ) {
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
            bus.extras.insert("x".into(), (*x).into());
            bus.extras.insert("y".into(), (*y).into());
        }
        net.buses.push(bus);
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

    /// Meters per source length unit; `none` and missing stay at 1 (the
    /// value is taken as meters), unknown codes warn.
    fn units_factor(&mut self, units: Option<&str>, class: &str, name: &str) -> f64 {
        match units {
            None => 1.0,
            Some(u) => dd::unit_to_meters(u).unwrap_or_else(|| {
                if !u.eq_ignore_ascii_case("none") {
                    self.net.warnings.push(format!(
                        "{class} {name}: unknown units `{u}`; treated as meters"
                    ));
                }
                1.0
            }),
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
        let per_meter = self.units_factor(units.as_deref(), "linecode", &obj.name);

        let freq = self
            .f64_prop(props.get("basefreq"))
            .unwrap_or(self.net.base_frequency);

        let (r, x, c_nf, matrix_defaulted) = self.impedance_matrices(
            &props,
            n,
            dd::line::R1,
            dd::line::X1,
            dd::line::R0,
            dd::line::X0,
            dd::line::C1_NF,
            dd::line::C0_NF,
        );
        if matrix_defaulted {
            self.defaulted("linecode", &obj.name, "rmatrix");
        }

        // Half the total line charging susceptance at each end; OpenDSS
        // carries one C matrix for the whole pi section.
        let b_half = scale_mat(&c_nf, std::f64::consts::TAU * freq * 1e-9 / per_meter / 2.0);
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
        DistLineCode {
            name: obj.name.clone(),
            n_conductors: n,
            r_series: scale_mat(&r, 1.0 / per_meter),
            x_series: scale_mat(&x, 1.0 / per_meter),
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
    /// either explicit matrices or sequence values. Returns whether the
    /// impedance came entirely from defaults.
    #[allow(clippy::too_many_arguments)]
    fn impedance_matrices(
        &mut self,
        props: &Props,
        n: usize,
        r1d: f64,
        x1d: f64,
        r0d: f64,
        x0d: f64,
        c1d: f64,
        c0d: f64,
    ) -> (Mat, Mat, Mat, bool) {
        let rows = |v: Option<&Value>| -> Option<Mat> {
            v.and_then(|v| v.to_rows(Some(self.vars)).ok())
                .and_then(|rows| square_from_rows(&rows, n))
        };
        let rm = rows(props.get("rmatrix"));
        let xm = rows(props.get("xmatrix"));
        let cm = rows(props.get("cmatrix"));
        let any_matrix = rm.is_some() || xm.is_some() || cm.is_some();
        let any_seq = ["r1", "x1", "r0", "x0", "c1", "c0", "b1", "b0"]
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

        let r = rm.unwrap_or_else(|| seq(props, "r1", "r0", r1d, r0d));
        let x = xm.unwrap_or_else(|| seq(props, "x1", "x0", x1d, x0d));
        let c = cm.unwrap_or_else(|| seq(props, "c1", "c0", c1d, c0d));
        (r, x, c, !any_matrix && !any_seq)
    }

    // ----- vsource -------------------------------------------------------

    fn vsource(&mut self, obj: &RawObject) -> VoltageSource {
        let props = Props::new(obj);
        let phases = self
            .usize_prop(props.get("phases"))
            .unwrap_or(dd::vsource::PHASES);
        let basekv = self.f64_or(&props, "basekv", "vsource", &obj.name, dd::vsource::BASEKV);
        let pu = self.f64_prop(props.get("pu")).unwrap_or(dd::vsource::PU);
        let angle_deg = self
            .f64_prop(props.get("angle"))
            .unwrap_or(dd::vsource::ANGLE_DEG);
        let spec = if let Some(v) = props.get("bus1") {
            v.to_bus_spec()
        } else {
            self.defaulted("vsource", &obj.name, "bus1");
            Value::new(dd::vsource::BUS1).to_bus_spec()
        };
        let map = self.terminals(&spec, phases, phases + 1, phases + 1);

        // The engine's convention: per phase magnitude basekv/sqrt(phases),
        // angles spaced -360/phases degrees, wrapped to (-180, 180] (the
        // wrap is in radians, matching the reference conversion).
        let v_ln = basekv * 1e3 / (phases as f64).sqrt() * pu;
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
        let length_factor = self.units_factor(length_units.as_deref(), "line", &obj.name);
        let length = self.f64_or(&props, "length", "line", &obj.name, dd::line::LENGTH);

        let linecode = if let Some(code) = props.get("linecode") {
            code.text.clone()
        } else {
            self.synthesize_linecode(&props, phases, length_factor, &obj.name)
        };

        let mut extras = extras_from_leftovers(&props);
        if let Some(u) = length_units {
            extras.insert("units".into(), u.into());
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
    /// length units.
    fn synthesize_linecode(
        &mut self,
        props: &Props,
        phases: usize,
        length_factor: f64,
        line_name: &str,
    ) -> String {
        let (r, x, c_nf, all_default) = self.impedance_matrices(
            props,
            phases,
            dd::line::R1,
            dd::line::X1,
            dd::line::R0,
            dd::line::X0,
            dd::line::C1_NF,
            dd::line::C0_NF,
        );
        if all_default {
            self.defaulted("line", line_name, "r1");
            self.defaulted("line", line_name, "x1");
        }
        let b_half = scale_mat(
            &c_nf,
            std::f64::consts::TAU * self.net.base_frequency * 1e-9 / length_factor / 2.0,
        );
        let zero = vec![vec![0.0; phases]; phases];
        let amps = self.f64_or(props, "emergamps", "line", line_name, dd::line::EMERGAMPS);
        let i_max = Some(vec![amps; phases]);
        let name = format!("_line_{line_name}");
        self.net.linecodes.push(DistLineCode {
            name: name.clone(),
            n_conductors: phases,
            r_series: scale_mat(&r, 1.0 / length_factor),
            x_series: scale_mat(&x, 1.0 / length_factor),
            g_from: zero.clone(),
            b_from: b_half.clone(),
            g_to: zero,
            b_to: b_half,
            i_max,
            s_max: None,
            extras: Extras::new(),
        });
        name
    }

    // ----- load ----------------------------------------------------------

    fn load(&mut self, obj: &RawObject) -> DistLoad {
        let props = Props::new(obj);
        let phases = self.usize_or(&props, "phases", "load", &obj.name, dd::load::PHASES);
        let conn_delta = props.get("conn").is_some_and(|v| {
            v.text.to_ascii_lowercase().starts_with('d') || v.text.eq_ignore_ascii_case("ll")
        });
        let kw = self.f64_or(&props, "kw", "load", &obj.name, dd::load::KW);
        let kv = self.f64_or(&props, "kv", "load", &obj.name, dd::load::KV);
        let kvar = self.f64_prop(props.get("kvar"));
        // When q derives from the power factor, the source pf rides in
        // extras so the dss writer can emit pf= and let the engine do its
        // own trigonometry; transcendental rounding across implementations
        // would otherwise leak into regenerated cases.
        let mut pf_source: Option<f64> = None;
        let q_total = if let Some(q) = kvar {
            q
        } else {
            let pf = self.f64_or(&props, "pf", "load", &obj.name, dd::load::PF);
            pf_source = Some(pf);
            kw * (pf.acos().tan()).copysign(pf)
        };
        let model = self
            .usize_prop(props.get("model"))
            .map_or(dd::load::MODEL, |m| i64::try_from(m).unwrap_or(i64::MAX));
        if model != 1 {
            self.warn(format!(
                "load {}: model={model} is not constant power; downstream formats treat it as constant power",
                obj.name
            ));
        }

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
        // fields hold explicit power per phase.
        let mut extras = extras_from_leftovers(&props);
        match props.by_name.get("kv") {
            Some(written) => {
                extras.insert("kv".into(), written.text.clone().into());
            }
            None => {
                extras.insert("kv".into(), kv.into());
            }
        }
        if let Some(pf) = pf_source {
            extras.insert("pf".into(), pf.into());
        }
        if model != 1 {
            extras.insert("model".into(), model.into());
        }
        DistLoad {
            name: obj.name.clone(),
            bus: spec.name,
            terminal_map: map,
            configuration,
            p_nom: vec![kw * 1e3 / phases as f64; phases],
            q_nom: vec![q_total * 1e3 / phases as f64; phases],
            extras,
        }
    }

    // ----- transformer ---------------------------------------------------

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
                "kv" | "kva" | "tap" | "%r" => {
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
                        _ => w.r_pct = parsed.unwrap_or(w.r_pct),
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
                }
                "xht" | "x13" => xht = self.f64_prop(Some(v)).unwrap_or(xht),
                "xlt" | "x23" => xlt = self.f64_prop(Some(v)).unwrap_or(xlt),
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
            vec![xhl, xht, xlt]
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
            // neutral in the map, delta leaves the unused conductor out.
            let keep = if w.conn_delta { phases } else { phases + 1 };
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
            });
        }
        out
    }

    // ----- capacitor → shunt ---------------------------------------------

    fn capacitor(&mut self, obj: &RawObject) {
        let props = Props::new(obj);
        if props.by_name.contains_key("bus2") {
            self.warn(format!(
                "capacitor {}: series capacitors (bus2) are not typed yet; kept untyped",
                obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }
        let phases = self.usize_or(
            &props,
            "phases",
            "capacitor",
            &obj.name,
            dd::capacitor::PHASES,
        );
        let conn_delta = props
            .get("conn")
            .is_some_and(|v| v.text.to_ascii_lowercase().starts_with('d'));
        if conn_delta {
            self.warn(format!(
                "capacitor {}: delta connection is not typed yet; kept untyped",
                obj.name
            ));
            self.net.untyped.push(UntypedObject::from(obj));
            return;
        }
        let kvar_first = props
            .get("kvar")
            .and_then(|v| v.to_vector(Some(self.vars)).ok())
            .and_then(|v| v.first().copied());
        let kvar = if let Some(q) = kvar_first {
            q
        } else {
            self.defaulted("capacitor", &obj.name, "kvar");
            dd::capacitor::KVAR
        };
        let kv = self.f64_or(&props, "kv", "capacitor", &obj.name, dd::capacitor::KV);
        let v_phase = if phases == 3 {
            kv * 1e3 / 3f64.sqrt()
        } else {
            kv * 1e3
        };
        let b_phase = kvar * 1e3 / phases as f64 / (v_phase * v_phase);

        let spec = bus_spec(props.get("bus1"), "");
        // The default return (bus2) is the same bus's ground; register the
        // ground connection but keep the map and matrices phase only, the
        // shape a shunt-to-ground admittance has downstream.
        let map = self.terminals(&spec, phases, phases + 1, phases);
        let n = map.len();
        let mut b = vec![vec![0.0; n]; n];
        for (i, row) in b.iter_mut().enumerate().take(phases) {
            row[i] = b_phase;
        }
        // The written pair regenerates verbatim in the dss writer; the b
        // matrix is the model truth either way.
        let mut extras = extras_from_leftovers(&props);
        extras.insert("kv".into(), kv.into());
        extras.insert("kvar".into(), kvar.into());
        self.net.shunts.push(DistShunt {
            name: obj.name.clone(),
            bus: spec.name,
            terminal_map: map,
            g: vec![vec![0.0; n]; n],
            b,
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
        let conn_delta = props
            .get("conn")
            .is_some_and(|v| v.text.to_ascii_lowercase().starts_with('d'));
        let kw = self.f64_or(&props, "kw", "generator", &obj.name, dd::generator::KW);
        let kvar = match (
            self.f64_prop(props.get("kvar")),
            self.f64_prop(props.get("pf")),
        ) {
            (Some(q), _) => q,
            (None, Some(pf)) => kw * (pf.acos().tan()).copysign(pf),
            (None, None) => {
                self.defaulted("generator", &obj.name, "kvar");
                dd::generator::KVAR
            }
        };
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
        match props.by_name.get("kv") {
            Some(written) => {
                extras.insert("kv".into(), written.text.clone().into());
            }
            None => {
                extras.insert("kv".into(), kv.into());
            }
        }
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

    // ----- controls ------------------------------------------------------

    fn swtcontrol(&mut self, obj: &RawObject) {
        let props = Props::new(obj);
        let Some(target) = props.get("switchedobj").map(|v| v.text.clone()) else {
            self.warn(format!("swtcontrol {}: no SwitchedObj; ignored", obj.name));
            return;
        };
        let line_name = target
            .strip_prefix("Line.")
            .or_else(|| target.strip_prefix("line."))
            .unwrap_or(&target);
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

#[derive(Clone)]
struct WindingRaw {
    bus: Option<BusSpec>,
    conn_delta: bool,
    kv: f64,
    kva: f64,
    tap: f64,
    r_pct: f64,
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

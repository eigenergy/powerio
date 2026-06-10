//! [`DistNetwork`] into OpenDSS `.dss` text.
//!
//! The canonical writer regenerates a solvable case from the typed model:
//! a `Clear`/`Set DefaultBaseFrequency` header, the circuit with its
//! source, linecodes in meters, elements with explicit bus dots (a
//! terminal in the bus's perfectly grounded set emits as node 0, the exact
//! inverse of the reader's materialization), `Set VoltageBases`,
//! `Calcvoltagebases`, and `Solve`. Element extras whose keys appear in
//! the class property tables emit verbatim; everything else is reported.
//!
//! Floats print through Rust's shortest round trip formatting; OpenDSS
//! reads the full precision back.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::convert::Conversion;
use crate::model::{Configuration, DistBus, DistNetwork, Mat, WindingConn};

use super::prop;

/// Writes canonical `.dss` text from the model.
pub fn write_dss(net: &DistNetwork) -> Conversion {
    let mut w = DssWriter {
        out: String::new(),
        warnings: Vec::new(),
        grounded: net
            .buses
            .iter()
            .map(|b| (b.id.to_ascii_lowercase(), b.grounded.clone()))
            .collect(),
        kv_estimate: estimate_bus_kv(net),
    };
    w.network(net);
    Conversion {
        text: w.out,
        warnings: w.warnings,
    }
}

struct DssWriter {
    out: String,
    warnings: Vec<String>,
    /// Bus id (lowercase) → perfectly grounded terminal names.
    grounded: BTreeMap<String, Vec<String>>,
    /// Bus id (lowercase) → phase to neutral voltage estimate, volts.
    kv_estimate: BTreeMap<String, f64>,
}

/// Phase to neutral voltage per bus, propagated from the sources through
/// lines and switches (same level) and transformers (winding ratios). The
/// estimate feeds load/capacitor `kv` and `Set VoltageBases` when the
/// source format did not carry them.
fn estimate_bus_kv(net: &DistNetwork) -> BTreeMap<String, f64> {
    let mut kv: BTreeMap<String, f64> = BTreeMap::new();
    for vs in &net.sources {
        let vln = vs.v_magnitude.iter().copied().fold(0.0_f64, f64::max);
        if vln > 0.0 {
            kv.insert(vs.bus.to_ascii_lowercase(), vln);
        }
    }
    for _ in 0..net.buses.len() {
        let mut changed = false;
        for l in &net.lines {
            let (f, t) = (
                l.bus_from.to_ascii_lowercase(),
                l.bus_to.to_ascii_lowercase(),
            );
            match (kv.get(&f).copied(), kv.get(&t).copied()) {
                (Some(v), None) => {
                    kv.insert(t, v);
                    changed = true;
                }
                (None, Some(v)) => {
                    kv.insert(f, v);
                    changed = true;
                }
                _ => {}
            }
        }
        for s in &net.switches {
            let (f, t) = (
                s.bus_from.to_ascii_lowercase(),
                s.bus_to.to_ascii_lowercase(),
            );
            match (kv.get(&f).copied(), kv.get(&t).copied()) {
                (Some(v), None) => {
                    kv.insert(t, v);
                    changed = true;
                }
                (None, Some(v)) => {
                    kv.insert(f, v);
                    changed = true;
                }
                _ => {}
            }
        }
        for t in &net.transformers {
            // Propagate by winding voltage ratio from any known winding bus.
            let known: Option<(usize, f64)> = t
                .windings
                .iter()
                .enumerate()
                .find_map(|(i, w)| kv.get(&w.bus.to_ascii_lowercase()).map(|v| (i, *v)));
            if let Some((i, v_known)) = known {
                let v_ref_known = t.windings[i].v_ref;
                if v_ref_known > 0.0 {
                    for (j, w) in t.windings.iter().enumerate() {
                        if j != i && !kv.contains_key(&w.bus.to_ascii_lowercase()) {
                            kv.insert(w.bus.to_ascii_lowercase(), v_known * w.v_ref / v_ref_known);
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    kv
}

/// A float in the shortest form Rust round trips.
fn num(v: f64) -> String {
    format!("{v}")
}

impl DssWriter {
    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    fn line_out(&mut self, s: &str) {
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// `bus.1.2.0` syntax: terminals in the bus's perfectly grounded set
    /// emit as node 0, the inverse of the reader's neutral naming.
    fn bus_ref(&self, bus: &str, map: &[String]) -> String {
        let grounded = self.grounded.get(&bus.to_ascii_lowercase());
        let nodes: Vec<String> = map
            .iter()
            .map(|t| {
                if grounded.is_some_and(|g| g.contains(t)) {
                    "0".to_string()
                } else {
                    t.clone()
                }
            })
            .collect();
        if nodes.is_empty() {
            bus.to_string()
        } else {
            format!("{bus}.{}", nodes.join("."))
        }
    }

    /// Extras whose keys are dss properties of `class` emit as written;
    /// the rest are reported per key.
    fn extras_tail(&mut self, class: &str, name: &str, extras: &crate::model::Extras) -> String {
        let table = prop::class_by_name(class);
        let mut tail = String::new();
        for (key, value) in extras {
            if matches!(key.as_str(), "bmopf_subtype") || key.starts_with("pmd_") {
                continue; // converter bookkeeping
            }
            let known = table.is_some_and(|t| t.props.contains(&key.as_str()));
            let text = value
                .as_str()
                .map(ToString::to_string)
                .or_else(|| value.as_f64().map(num))
                .or_else(|| value.as_i64().map(|v| v.to_string()));
            match (known, text) {
                (true, Some(text)) => {
                    let quoted = if text.contains(' ') || text.contains(',') {
                        format!("({text})")
                    } else {
                        text
                    };
                    let _ = write!(tail, " {key}={quoted}");
                }
                _ => self.warn(format!(
                    "{class} {name}: extra `{key}` is not a dss property; dropped from the output"
                )),
            }
        }
        tail
    }

    fn matrix_arg(m: &Mat) -> String {
        let rows: Vec<String> = m
            .iter()
            .enumerate()
            .map(|(i, row)| {
                row[..=i]
                    .iter()
                    .map(|v| num(*v))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        format!("({})", rows.join(" | "))
    }

    fn network(&mut self, net: &DistNetwork) {
        self.line_out("Clear");
        self.line_out(&format!(
            "Set DefaultBaseFrequency={}",
            num(net.base_frequency)
        ));
        self.out.push('\n');

        self.sources(net);
        self.linecodes(net);
        self.lines(net);
        self.switches(net);
        self.transformers(net);
        self.loads(net);
        self.shunts(net);
        self.generators(net);

        for u in &net.untyped {
            self.warn(format!(
                "{} {}: untyped object is not regenerated in canonical dss output",
                u.class, u.name
            ));
        }
        for b in &net.buses {
            self.bus_extras(b);
        }

        self.out.push('\n');
        let mut bases: Vec<f64> = self
            .kv_estimate
            .values()
            .map(|v| v * 3f64.sqrt() / 1e3)
            .collect();
        bases.sort_by(f64::total_cmp);
        bases.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        if !bases.is_empty() {
            let list: Vec<String> = bases.iter().map(|v| num(*v)).collect();
            self.line_out(&format!("Set VoltageBases=[{}]", list.join(", ")));
            self.line_out("Calcvoltagebases");
        }
        self.line_out("Solve");
    }

    fn bus_extras(&mut self, b: &DistBus) {
        for key in b.extras.keys() {
            if key == "x" || key == "y" {
                continue; // coordinates have no command in canonical output yet
            }
            self.warnings.push(format!(
                "bus {}: extra `{key}` is not regenerated in canonical dss output",
                b.id
            ));
        }
        for (field, present) in [
            ("v_min", b.v_min.is_some()),
            ("v_max", b.v_max.is_some()),
            ("vpn_min", b.vpn_min.is_some()),
            ("vpn_max", b.vpn_max.is_some()),
            ("vpp_min", b.vpp_min.is_some()),
            ("vpp_max", b.vpp_max.is_some()),
            ("vsym_min", b.vsym_min.is_some()),
            ("vsym_max", b.vsym_max.is_some()),
        ] {
            if present {
                self.warnings.push(format!(
                    "bus {}: `{field}` voltage bounds have no dss expression; dropped",
                    b.id
                ));
            }
        }
    }

    fn sources(&mut self, net: &DistNetwork) {
        for (i, vs) in net.sources.iter().enumerate() {
            let phases = vs.v_magnitude.iter().filter(|&&v| v > 0.0).count().max(1);
            let basekv = vs
                .extras
                .get("basekv")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or_else(|| {
                    vs.v_magnitude.iter().copied().fold(0.0_f64, f64::max) * (phases as f64).sqrt()
                        / 1e3
                });
            let pu = vs
                .extras
                .get("pu")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0);
            let angle = vs
                .extras
                .get("angle")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or_else(|| vs.v_angle.first().copied().unwrap_or(0.0).to_degrees());
            let head = if i == 0 {
                let name = net.name.clone().unwrap_or_else(|| "converted".into());
                format!("New Circuit.{name}")
            } else {
                format!("New Vsource.{}", vs.name)
            };
            let mut s = format!(
                "{head} basekv={} pu={} angle={} phases={phases} bus1={}",
                num(basekv),
                num(pu),
                num(angle),
                self.bus_ref(&vs.bus, &vs.terminal_map),
            );
            let mut extras = vs.extras.clone();
            extras.remove("basekv");
            extras.remove("pu");
            extras.remove("angle");
            // A source that came through the ENGINEERING model carries its
            // Thevenin impedance as rs/xs matrices; sequence values
            // reconstruct exactly (z1 = self - mutual, z0 = self + 2 mutual).
            let take_seq = |key: &str, extras: &mut crate::model::Extras| -> Option<(f64, f64)> {
                let m = extras.remove(key)?;
                let row = m.as_array()?.first()?.as_array()?;
                let self_v = row.first()?.as_f64()?;
                let mutual = row
                    .get(1)
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0);
                Some((self_v - mutual, self_v + 2.0 * mutual))
            };
            let r = take_seq("rs", &mut extras);
            let x = take_seq("xs", &mut extras);
            if let (Some((r1, r0)), Some((x1, x0))) = (r, x) {
                // Lowercase keys in sorted order: a reparse keeps these in
                // extras and the next write emits them from there verbatim.
                let _ = write!(
                    s,
                    " z0=({}, {}) z1=({}, {})",
                    num(r0),
                    num(x0),
                    num(r1),
                    num(x1)
                );
            }
            s.push_str(&self.extras_tail("vsource", &vs.name, &extras));
            self.line_out(&s);
        }
        self.out.push('\n');
    }

    fn linecodes(&mut self, net: &DistNetwork) {
        let omega_nf = std::f64::consts::TAU * net.base_frequency * 1e-9;
        for c in &net.linecodes {
            let n = c.n_conductors;
            let mut s = format!("New Linecode.{} nphases={n} units=m", c.name);
            let _ = write!(s, " rmatrix={}", Self::matrix_arg(&c.r_series));
            let _ = write!(s, " xmatrix={}", Self::matrix_arg(&c.x_series));
            // cmatrix in nF per meter: each half is omega C / 2, so
            // C_nF = 2 b / (omega 1e-9).
            let c_nf: Mat = c
                .b_from
                .iter()
                .map(|row| row.iter().map(|b| 2.0 * b / omega_nf).collect())
                .collect();
            let _ = write!(s, " cmatrix={}", Self::matrix_arg(&c_nf));
            if let Some(i_max) = &c.i_max {
                let _ = write!(s, " emergamps={}", num(i_max[0]));
            }
            if !c.g_from.iter().flatten().all(|&g| g == 0.0) {
                self.warn(format!(
                    "linecode {}: shunt conductance has no dss linecode field; dropped",
                    c.name
                ));
            }
            let mut extras = c.extras.clone();
            extras.remove("units"); // canonical output is in meters
            s.push_str(&self.extras_tail("linecode", &c.name, &extras));
            self.line_out(&s);
        }
        self.out.push('\n');
    }

    fn lines(&mut self, net: &DistNetwork) {
        for l in &net.lines {
            let phases = l.terminal_map_from.len();
            let mut s = format!(
                "New Line.{} bus1={} bus2={} phases={phases} linecode={} length={} units=m",
                l.name,
                self.bus_ref(&l.bus_from, &l.terminal_map_from),
                self.bus_ref(&l.bus_to, &l.terminal_map_to),
                l.linecode,
                num(l.length),
            );
            let mut extras = l.extras.clone();
            extras.remove("units"); // canonical output is in meters
            s.push_str(&self.extras_tail("line", &l.name, &extras));
            self.line_out(&s);
        }
        self.out.push('\n');
    }

    fn switches(&mut self, net: &DistNetwork) {
        for sw in &net.switches {
            let phases = sw.terminal_map_from.len();
            let mut s = format!(
                "New Line.{} bus1={} bus2={} phases={phases} switch=y",
                sw.name,
                self.bus_ref(&sw.bus_from, &sw.terminal_map_from),
                self.bus_ref(&sw.bus_to, &sw.terminal_map_to),
            );
            if let Some(i_max) = &sw.i_max {
                let _ = write!(s, " emergamps={}", num(i_max[0]));
            }
            s.push_str(&self.extras_tail("line", &sw.name, &sw.extras));
            self.line_out(&s);
            self.line_out(&format!(
                "New SwtControl.{}_state SwitchedObj=Line.{} Action={}",
                sw.name,
                sw.name,
                if sw.open { "open" } else { "close" },
            ));
        }
        self.out.push('\n');
    }

    fn transformers(&mut self, net: &DistNetwork) {
        for t in &net.transformers {
            let nw = t.windings.len();
            let buses: Vec<String> = t
                .windings
                .iter()
                .map(|w| self.bus_ref(&w.bus, &w.terminal_map))
                .collect();
            let conns: Vec<&str> = t
                .windings
                .iter()
                .map(|w| match w.conn {
                    WindingConn::Wye => "wye",
                    WindingConn::Delta => "delta",
                })
                .collect();
            let kvs: Vec<String> = t.windings.iter().map(|w| num(w.v_ref / 1e3)).collect();
            let kvas: Vec<String> = t.windings.iter().map(|w| num(w.s_rating / 1e3)).collect();
            let rs: Vec<String> = t.windings.iter().map(|w| num(w.r_pct)).collect();
            let taps: Vec<String> = t.windings.iter().map(|w| num(w.tap)).collect();
            let mut s = format!(
                "New Transformer.{} phases={} windings={nw} buses=({}) conns=({}) kvs=({}) kvas=({}) %Rs=({}) taps=({})",
                t.name,
                t.phases,
                buses.join(", "),
                conns.join(", "),
                kvs.join(", "),
                kvas.join(", "),
                rs.join(", "),
                taps.join(", "),
            );
            let _ = write!(s, " xhl={}", num(t.xsc_pct[0]));
            if t.xsc_pct.len() >= 3 {
                let _ = write!(s, " xht={} xlt={}", num(t.xsc_pct[1]), num(t.xsc_pct[2]));
            }
            s.push_str(&self.extras_tail("transformer", &t.name, &t.extras));
            self.line_out(&s);
        }
        self.out.push('\n');
    }

    fn loads(&mut self, net: &DistNetwork) {
        for l in &net.loads {
            let phases = match l.configuration {
                Configuration::Delta if l.terminal_map.len() == 3 => 3,
                Configuration::Wye => l.terminal_map.len().saturating_sub(1).max(1),
                _ => 1,
            };
            let conn = match l.configuration {
                Configuration::Delta => "delta",
                _ => "wye",
            };
            let kw: f64 = l.p_nom.iter().sum::<f64>() / 1e3;
            let kvar: f64 = l.q_nom.iter().sum::<f64>() / 1e3;
            let kv = self.element_kv(&l.extras, &l.bus, phases, l.configuration, &l.name, "load");
            let mut extras = l.extras.clone();
            extras.remove("kv");
            // q that came from a power factor goes back as pf=, so the
            // engine recomputes its own kvar bit for bit.
            let reactive = match extras.remove("pf").and_then(|v| v.as_f64()) {
                Some(pf) => format!("pf={}", num(pf)),
                None => format!("kvar={}", num(kvar)),
            };
            let mut s = format!(
                "New Load.{} bus1={} phases={phases} conn={conn} kv={} kw={} {reactive}",
                l.name,
                self.bus_ref(&l.bus, &l.terminal_map),
                num(kv),
                num(kw),
            );
            s.push_str(&self.extras_tail("load", &l.name, &extras));
            self.line_out(&s);
        }
        self.out.push('\n');
    }

    /// `kv` for a load or capacitor: the recorded value when the source
    /// carried one, otherwise the propagated bus estimate.
    fn element_kv(
        &mut self,
        extras: &crate::model::Extras,
        bus: &str,
        phases: usize,
        configuration: Configuration,
        name: &str,
        class: &str,
    ) -> f64 {
        if let Some(kv) = extras.get("kv").and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        }) {
            return kv;
        }
        if let Some(vln) = self.kv_estimate.get(&bus.to_ascii_lowercase()).copied() {
            // OpenDSS convention: line to line for 2 and 3 phase, line to
            // neutral for single phase.
            let v = if phases >= 2 || configuration == Configuration::Delta {
                vln * 3f64.sqrt()
            } else {
                vln
            };
            v / 1e3
        } else {
            self.warn(format!(
                "{class} {name}: no kv in the source and no bus voltage estimate; \
                 emitted 12.47"
            ));
            12.47
        }
    }

    fn shunts(&mut self, net: &DistNetwork) {
        for sh in &net.shunts {
            let phases = sh.terminal_map.len();
            let b_phase = (0..phases.min(sh.b.len()))
                .map(|i| sh.b[i][i])
                .fold(0.0_f64, f64::max);
            if b_phase <= 0.0 {
                self.warn(format!(
                    "shunt {}: no positive susceptance; dropped from the output",
                    sh.name
                ));
                continue;
            }
            let off_diag =
                sh.b.iter()
                    .enumerate()
                    .any(|(i, row)| row.iter().enumerate().any(|(j, &v)| i != j && v != 0.0));
            if off_diag {
                self.warn(format!(
                    "shunt {}: off diagonal susceptance has no capacitor expression; \
                     only the diagonal is regenerated",
                    sh.name
                ));
            }
            // Any (kv, kvar) pair with kvar = b v^2 reproduces the same
            // admittance; the recorded pair (when the source carried one)
            // emits verbatim, keeping the text stable across round trips.
            let kv = self.element_kv(
                &sh.extras,
                &sh.bus,
                phases,
                Configuration::Wye,
                &sh.name,
                "capacitor",
            );
            let kvar = sh
                .extras
                .get("kvar")
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or_else(|| {
                    let v_phase = if phases >= 2 {
                        kv * 1e3 / 3f64.sqrt()
                    } else {
                        kv * 1e3
                    };
                    b_phase * v_phase * v_phase * phases as f64 / 1e3
                });
            let mut extras = sh.extras.clone();
            extras.remove("kv");
            extras.remove("kvar");
            let mut s = format!(
                "New Capacitor.{} bus1={} phases={phases} conn=wye kv={} kvar={}",
                sh.name,
                self.bus_ref(&sh.bus, &sh.terminal_map),
                num(kv),
                num(kvar),
            );
            s.push_str(&self.extras_tail("capacitor", &sh.name, &extras));
            self.line_out(&s);
        }
        self.out.push('\n');
    }

    fn generators(&mut self, net: &DistNetwork) {
        for g in &net.generators {
            let phases = match g.configuration {
                Configuration::Delta if g.terminal_map.len() == 3 => 3,
                Configuration::Wye => g.terminal_map.len().saturating_sub(1).max(1),
                _ => 1,
            };
            let conn = match g.configuration {
                Configuration::Delta => "delta",
                _ => "wye",
            };
            let kw: f64 = g.p_nom.iter().sum::<f64>() / 1e3;
            let kvar: f64 = g.q_nom.iter().sum::<f64>() / 1e3;
            let kv = self.element_kv(
                &g.extras,
                &g.bus,
                phases,
                g.configuration,
                &g.name,
                "generator",
            );
            let mut s = format!(
                "New Generator.{} bus1={} phases={phases} conn={conn} kv={} kw={} kvar={}",
                g.name,
                self.bus_ref(&g.bus, &g.terminal_map),
                num(kv),
                num(kw),
                num(kvar),
            );
            if let Some(q) = &g.q_max {
                let _ = write!(s, " maxkvar={}", num(q.iter().sum::<f64>() / 1e3));
            }
            if let Some(q) = &g.q_min {
                let _ = write!(s, " minkvar={}", num(q.iter().sum::<f64>() / 1e3));
            }
            if g.cost.is_some() {
                self.warn(format!(
                    "generator {}: generation cost has no dss field; dropped",
                    g.name
                ));
            }
            let mut extras = g.extras.clone();
            extras.remove("kv");
            s.push_str(&self.extras_tail("generator", &g.name, &extras));
            self.line_out(&s);
        }
    }
}

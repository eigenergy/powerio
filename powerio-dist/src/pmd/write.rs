//! [`DistNetwork`] into PMD ENGINEERING JSON.
//!
//! The output reproduces what PMD's own dss2eng emits for the same network
//! wherever the model carries the data: terminal integers, `ENABLED`
//! status, `source_id`, the materialized grounded neutral with zero
//! `rg`/`xg`, linecode `cm_ub` from the emergency rating, transformer
//! `tm_*` tap fields, the delta wye barrel roll with `polarity` -1 on the
//! lagging wye winding, and the voltage source Thevenin matrices computed
//! from the short circuit data when the source format carried it. The
//! reader's `pmd_*` stashes (status, settings, files, grounding and switch
//! impedance, tap arrays, polarity, inline line impedance) win over the
//! recomputed defaults, so PMD in, PMD out does not alter fields.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::convert::Conversion;
use crate::geo::CoordinateSpace;
use crate::model::{
    Configuration, DistBus, DistLineCode, DistLoadVoltageModel, DistNetwork, DistTransformer,
    Extras, Mat, VoltageSource, Winding, WindingConn,
};

/// Writes the ENGINEERING document.
///
/// # Panics
///
/// Never in practice: the document is maps, strings, finite numbers, and
/// nulls, which always serialize.
pub fn write_pmd_json(net: &DistNetwork) -> Conversion {
    let mut w = Writer {
        warnings: Vec::new(),
    };
    let doc = w.document(net);
    Conversion {
        text: serde_json::to_string_pretty(&doc).expect("maps and finite numbers") + "\n",
        sidecars: Vec::new(),
        warnings: w.warnings,
        diagnostics: Vec::new(),
    }
}

struct Writer {
    warnings: Vec<String>,
}

/// Terminal names as PMD integer connections; non numeric names count from
/// 90 upward (PMD requires ints; the warning names the rename).
fn conns(map: &[String], warnings: &mut Vec<String>, what: &str) -> Vec<i64> {
    map.iter()
        .enumerate()
        .map(|(k, t)| {
            t.parse::<i64>().unwrap_or_else(|_| {
                let fallback = 90 + i64::try_from(k).unwrap_or(0);
                warnings.push(format!(
                    "{what}: terminal `{t}` is not numeric; emitted as {fallback}"
                ));
                fallback
            })
        })
        .collect()
}

/// A matrix as PMD serializes it: array of columns (`hcat` rebuilds it).
fn matrix(m: &Mat) -> Value {
    let n = m.len();
    let cols: Vec<Value> = (0..n)
        .map(|j| Value::Array((0..n).map(|i| json!(m[i][j])).collect()))
        .collect();
    Value::Array(cols)
}

fn zero_matrix(n: usize) -> Mat {
    vec![vec![0.0; n]; n]
}

/// A shunt whose stashed `conn` marks it a delta (line to line) bank.
fn shunt_is_delta(extras: &Extras) -> bool {
    extras
        .get("conn")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t.to_ascii_lowercase().starts_with('d') || t.eq_ignore_ascii_case("ll"))
}

fn scale(m: &Mat, k: f64) -> Mat {
    m.iter()
        .map(|row| row.iter().map(|v| v * k).collect())
        .collect()
}

impl Writer {
    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// Reports extras the ENGINEERING model has no field for. `consumed`
    /// names keys a field already represents; `pmd_*` bookkeeping and the
    /// BMOPF subtype marker pass silently.
    fn extras_dropped(&mut self, extras: &crate::model::Extras, consumed: &[&str], what: &str) {
        for key in extras.keys() {
            if consumed.contains(&key.as_str()) || key.starts_with("pmd_") || key == "bmopf_subtype"
            {
                continue;
            }
            self.warn(format!(
                "{what}: `{key}` has no ENGINEERING field; dropped from the output"
            ));
        }
    }

    fn extras_f64(extras: &Extras, key: &str) -> Option<f64> {
        extras.get(key).and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
    }

    /// The element status: the reader's stash when the source carried a non
    /// ENABLED status, `ENABLED` otherwise.
    fn status(extras: &Extras) -> Value {
        extras
            .get("pmd_status")
            .cloned()
            .unwrap_or_else(|| json!("ENABLED"))
    }

    fn bus_coordinates(&mut self, o: &mut Map<String, Value>, b: &DistBus, net: &DistNetwork) {
        if let Some(location) = b.location {
            if !matches!(
                net.geo.as_ref().map(|geo| &geo.space),
                Some(CoordinateSpace::Geographic { .. })
            ) {
                self.warnings.push(format!(
                    "bus {}: non-geographic or undeclared location is not emitted to PMD lon/lat",
                    b.id
                ));
                return;
            }
            if location.x.is_finite() && location.y.is_finite() {
                o.insert("lon".into(), json!(location.x));
                o.insert("lat".into(), json!(location.y));
            } else {
                self.warnings.push(format!(
                    "bus {}: nonfinite location is not emitted to PMD JSON",
                    b.id
                ));
            }
            return;
        }
        if let Some(x) = Self::extras_f64(&b.extras, "x") {
            o.insert("lon".into(), json!(x));
        }
        if let Some(y) = Self::extras_f64(&b.extras, "y") {
            o.insert("lat".into(), json!(y));
        }
    }

    fn document(&mut self, net: &DistNetwork) -> Value {
        let mut doc = Map::new();
        doc.insert("data_model".into(), json!("ENGINEERING"));
        doc.insert(
            "name".into(),
            json!(net.name.clone().unwrap_or_default().to_lowercase()),
        );
        doc.insert(
            "files".into(),
            net.extras
                .get("pmd_files")
                .cloned()
                .unwrap_or_else(|| json!([])),
        );

        // The reader's stash wins; synthesis covers dss/bmopf sourced
        // models.
        let settings = net
            .extras
            .get("pmd_settings")
            .cloned()
            .unwrap_or_else(|| synthesized_settings(net));
        doc.insert("settings".into(), settings);

        let max_conductor = net
            .buses
            .iter()
            .flat_map(|b| &b.terminals)
            .filter_map(|t| t.parse::<i64>().ok())
            .max()
            .unwrap_or(4)
            .max(4);
        doc.insert(
            "conductor_ids".into(),
            Value::Array((1..=max_conductor).map(|i| json!(i)).collect()),
        );

        let mut buses = Map::new();
        for b in &net.buses {
            let mut o = Map::new();
            o.insert(
                "terminals".into(),
                json!(conns(
                    &b.terminals,
                    &mut self.warnings,
                    &format!("bus {}", b.id)
                )),
            );
            let grounded = conns(&b.grounded, &mut self.warnings, &format!("bus {}", b.id));
            // Nonzero grounding impedance rides in extras (the reader's
            // stash); zero vectors are the materialized default.
            for key in ["rg", "xg"] {
                let v = b
                    .extras
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| json!(vec![0.0; grounded.len()]));
                o.insert(key.into(), v);
            }
            o.insert("grounded".into(), json!(grounded));
            o.insert("status".into(), Self::status(&b.extras));
            self.bus_coordinates(&mut o, b, net);
            // Voltage bound families have no ENGINEERING fields in volts;
            // they drop loudly (PMD bounds are per unit).
            for (key, present) in [
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
                    self.warn(format!(
                        "bus {}: `{key}` volt bounds have no ENGINEERING field; dropped",
                        b.id
                    ));
                }
            }
            buses.insert(b.id.to_lowercase(), Value::Object(o));
        }
        doc.insert("bus".into(), Value::Object(buses));

        Self::linecodes(net, &mut doc);
        self.branches(net, &mut doc);
        self.injections(net, &mut doc);
        self.transformers(net, &mut doc);

        for u in &net.untyped {
            self.warn(format!(
                "{} {}: class is not converted to ENGINEERING; dropped from the output",
                u.class, u.name
            ));
        }
        Value::Object(doc)
    }

    fn linecodes(net: &DistNetwork, doc: &mut Map<String, Value>) {
        // Linecodes the reader materialized from inline line impedance
        // re-inline on the line; they are skipped here unless a line
        // without the marker also references them.
        let inlined = inlined_codes(net);
        let mut codes = Map::new();
        for c in &net.linecodes {
            if inlined.contains(&c.name.to_lowercase()) {
                continue;
            }
            let mut o = Map::new();
            insert_impedance_matrices(&mut o, c, net.base_frequency);
            if let Some(i_max) = &c.i_max {
                o.insert("cm_ub".into(), json!(i_max));
            }
            if let Some(s_max) = &c.s_max {
                o.insert("sm_ub".into(), json!(s_max));
            }
            codes.insert(c.name.to_lowercase(), Value::Object(o));
        }
        if !codes.is_empty() {
            doc.insert("linecode".into(), Value::Object(codes));
        }
    }

    fn branches(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        if !net.lines.is_empty() {
            let mut lines = Map::new();
            for l in &net.lines {
                let mut o = Map::new();
                o.insert("f_bus".into(), json!(l.bus_from.to_lowercase()));
                o.insert("t_bus".into(), json!(l.bus_to.to_lowercase()));
                let what = format!("line {}", l.name);
                o.insert(
                    "f_connections".into(),
                    json!(conns(&l.terminal_map_from, &mut self.warnings, &what)),
                );
                o.insert(
                    "t_connections".into(),
                    json!(conns(&l.terminal_map_to, &mut self.warnings, &what)),
                );
                o.insert("length".into(), json!(l.length));
                // A line the reader materialized a linecode for re-inlines
                // its impedance, the dss2eng shape for rmatrix defined
                // lines: matrices on the line, no linecode key.
                let inline = l.extras.get("pmd_inline").and_then(Value::as_bool) == Some(true);
                match net.linecode(&l.linecode) {
                    Some(c) if inline => {
                        insert_impedance_matrices(&mut o, c, net.base_frequency);
                        if let Some(i_max) = &c.i_max {
                            o.insert("cm_ub".into(), json!(i_max));
                        }
                    }
                    _ => {
                        if inline {
                            self.warn(format!(
                                "{what}: linecode `{}` is missing; emitted the reference instead of inline impedance",
                                l.linecode
                            ));
                        }
                        o.insert("linecode".into(), json!(l.linecode.to_lowercase()));
                    }
                }
                o.insert("status".into(), Self::status(&l.extras));
                o.insert(
                    "source_id".into(),
                    json!(format!("line.{}", l.name.to_lowercase())),
                );
                self.extras_dropped(&l.extras, &["units"], &what);
                lines.insert(l.name.to_lowercase(), Value::Object(o));
            }
            doc.insert("line".into(), Value::Object(lines));
        }

        if !net.switches.is_empty() {
            let mut switches = Map::new();
            for s in &net.switches {
                let mut o = Map::new();
                let n = s.terminal_map_from.len();
                let what = format!("switch {}", s.name);
                o.insert("f_bus".into(), json!(s.bus_from.to_lowercase()));
                o.insert("t_bus".into(), json!(s.bus_to.to_lowercase()));
                o.insert(
                    "f_connections".into(),
                    json!(conns(&s.terminal_map_from, &mut self.warnings, &what)),
                );
                o.insert(
                    "t_connections".into(),
                    json!(conns(&s.terminal_map_to, &mut self.warnings, &what)),
                );
                // The reader's stash carries the source's series matrices;
                // otherwise PMD models a dss switch as a tiny series
                // resistance, 1e-4 ohm/m over the forced 0.001 m length
                // (the product form keeps the value bit identical).
                let rs = s.extras.get("pmd_rs").cloned().unwrap_or_else(|| {
                    let mut rs = zero_matrix(n);
                    for (i, row) in rs.iter_mut().enumerate() {
                        row[i] = 1e-4 * 0.001;
                    }
                    matrix(&rs)
                });
                o.insert("rs".into(), rs);
                let xs = s
                    .extras
                    .get("pmd_xs")
                    .cloned()
                    .unwrap_or_else(|| matrix(&zero_matrix(n)));
                o.insert("xs".into(), xs);
                o.insert("g_fr".into(), matrix(&zero_matrix(n)));
                o.insert("g_to".into(), matrix(&zero_matrix(n)));
                o.insert("b_fr".into(), matrix(&zero_matrix(n)));
                o.insert("b_to".into(), matrix(&zero_matrix(n)));
                if let Some(i_max) = &s.i_max {
                    o.insert("cm_ub".into(), json!(i_max));
                }
                o.insert(
                    "state".into(),
                    json!(if s.open { "OPEN" } else { "CLOSED" }),
                );
                o.insert("dispatchable".into(), json!("YES"));
                o.insert("status".into(), Self::status(&s.extras));
                o.insert(
                    "source_id".into(),
                    json!(format!("line.{}", s.name.to_lowercase())),
                );
                self.extras_dropped(&s.extras, &[], &what);
                switches.insert(s.name.to_lowercase(), Value::Object(o));
            }
            doc.insert("switch".into(), Value::Object(switches));
        }
    }

    fn loads(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        if !net.loads.is_empty() {
            let mut loads = Map::new();
            for l in &net.loads {
                let mut o = Map::new();
                let what = format!("load {}", l.name);
                let connections = conns(&l.terminal_map, &mut self.warnings, &what);
                // PMD types a two terminal load WYE when the return is the
                // bus's grounded neutral and DELTA otherwise.
                let configuration = match l.configuration {
                    Configuration::Delta => "DELTA",
                    Configuration::Wye => "WYE",
                    Configuration::SinglePhase => {
                        let grounded_return = l
                            .terminal_map
                            .last()
                            .zip(net.bus(&l.bus))
                            .is_some_and(|(t, b)| b.grounded.contains(t));
                        if grounded_return { "WYE" } else { "DELTA" }
                    }
                };
                o.insert("configuration".into(), json!(configuration));
                o.insert("connections".into(), json!(connections));
                o.insert(
                    "pd_nom".into(),
                    json!(l.p_nom.iter().map(|p| p / 1e3).collect::<Vec<_>>()),
                );
                o.insert(
                    "qd_nom".into(),
                    json!(l.q_nom.iter().map(|q| q / 1e3).collect::<Vec<_>>()),
                );
                o.insert("bus".into(), json!(l.bus.to_lowercase()));
                let mut insert_vm_nom = |v_nom: &[f64]| {
                    if let Some(value) = source_vm_nom(&l.extras, v_nom) {
                        o.insert("vm_nom".into(), value);
                    } else if !v_nom.is_empty() {
                        let value = if v_nom.len() == 1 {
                            json!(v_nom[0] / 1e3)
                        } else {
                            json!(v_nom.iter().map(|v| v / 1e3).collect::<Vec<_>>())
                        };
                        o.insert("vm_nom".into(), value);
                    } else if let Some(kv) = Self::extras_f64(&l.extras, "kv") {
                        o.insert("vm_nom".into(), json!(kv));
                    }
                };
                let model = match &l.voltage_model {
                    DistLoadVoltageModel::ConstantImpedance { v_nom } => {
                        insert_vm_nom(v_nom);
                        "IMPEDANCE"
                    }
                    DistLoadVoltageModel::ConstantCurrent { v_nom } => {
                        insert_vm_nom(v_nom);
                        "CURRENT"
                    }
                    DistLoadVoltageModel::Zip { v_nom, .. } => {
                        insert_vm_nom(v_nom);
                        "ZIPV"
                    }
                    DistLoadVoltageModel::Exponential { v_nom, .. } => {
                        insert_vm_nom(v_nom);
                        self.warn(format!(
                            "{what}: exponential load model has no ENGINEERING field; emitted POWER"
                        ));
                        "POWER"
                    }
                    DistLoadVoltageModel::ConstantPower { v_nom } => {
                        insert_vm_nom(v_nom);
                        "POWER"
                    }
                };
                o.insert("model".into(), json!(model));
                o.insert("dispatchable".into(), json!("NO"));
                o.insert("status".into(), Self::status(&l.extras));
                o.insert(
                    "source_id".into(),
                    json!(format!("load.{}", l.name.to_lowercase())),
                );
                self.extras_dropped(&l.extras, &["kv", "model", "pf"], &what);
                loads.insert(l.name.to_lowercase(), Value::Object(o));
            }
            doc.insert("load".into(), Value::Object(loads));
        }
    }

    fn generators(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        if !net.generators.is_empty() {
            let mut gens = Map::new();
            for g in &net.generators {
                let mut o = Map::new();
                let what = format!("generator {}", g.name);
                o.insert("bus".into(), json!(g.bus.to_lowercase()));
                o.insert(
                    "connections".into(),
                    json!(conns(&g.terminal_map, &mut self.warnings, &what)),
                );
                o.insert(
                    "configuration".into(),
                    json!(match g.configuration {
                        Configuration::Delta => "DELTA",
                        _ => "WYE",
                    }),
                );
                let kw = |w: &[f64]| w.iter().map(|v| v / 1e3).collect::<Vec<_>>();
                o.insert("pg".into(), json!(kw(&g.p_nom)));
                o.insert("qg".into(), json!(kw(&g.q_nom)));
                if let Some(b) = &g.q_min {
                    o.insert("qg_lb".into(), json!(kw(b)));
                }
                if let Some(b) = &g.q_max {
                    o.insert("qg_ub".into(), json!(kw(b)));
                }
                if let Some(b) = &g.p_min {
                    o.insert("pg_lb".into(), json!(kw(b)));
                }
                if let Some(b) = &g.p_max {
                    o.insert("pg_ub".into(), json!(kw(b)));
                }
                if g.cost.is_some() {
                    self.warn(format!(
                        "{what}: generation cost has no ENGINEERING field; dropped"
                    ));
                }
                o.insert("control_mode".into(), json!("FREQUENCYDROOP"));
                o.insert("status".into(), Self::status(&g.extras));
                o.insert(
                    "source_id".into(),
                    json!(format!("generator.{}", g.name.to_lowercase())),
                );
                self.extras_dropped(&g.extras, &["kv"], &what);
                gens.insert(g.name.to_lowercase(), Value::Object(o));
            }
            doc.insert("generator".into(), Value::Object(gens));
        }
    }

    fn injections(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        self.loads(net, doc);
        self.generators(net, doc);
        if !net.shunts.is_empty() {
            let mut shunts = Map::new();
            for s in &net.shunts {
                let mut o = Map::new();
                let what = format!("shunt {}", s.name);
                o.insert("bus".into(), json!(s.bus.to_lowercase()));
                o.insert(
                    "connections".into(),
                    json!(conns(&s.terminal_map, &mut self.warnings, &what)),
                );
                o.insert("gs".into(), matrix(&s.g));
                o.insert("bs".into(), matrix(&s.b));
                // A delta bank carries a `conn` marker and an off diagonal B
                // matrix; emitting it as WYE would describe a line to line
                // admittance as line to ground.
                let configuration = if shunt_is_delta(&s.extras) {
                    "DELTA"
                } else {
                    "WYE"
                };
                o.insert("configuration".into(), json!(configuration));
                o.insert("model".into(), json!("CAPACITOR"));
                o.insert("dispatchable".into(), json!("NO"));
                o.insert("status".into(), Self::status(&s.extras));
                o.insert(
                    "source_id".into(),
                    json!(format!("capacitor.{}", s.name.to_lowercase())),
                );
                self.extras_dropped(&s.extras, &["kv", "kvar", "conn"], &what);
                shunts.insert(s.name.to_lowercase(), Value::Object(o));
            }
            doc.insert("shunt".into(), Value::Object(shunts));
        }

        let mut sources = Map::new();
        for vs in &net.sources {
            sources.insert(vs.name.to_lowercase(), self.voltage_source(vs));
        }
        doc.insert("voltage_source".into(), Value::Object(sources));
    }

    fn voltage_source(&mut self, vs: &VoltageSource) -> Value {
        let mut o = Map::new();
        let what = format!("voltage source {}", vs.name);
        let connections = conns(&vs.terminal_map, &mut self.warnings, &what);
        let n = connections.len();
        o.insert("bus".into(), json!(vs.bus.to_lowercase()));
        o.insert("connections".into(), json!(connections));
        o.insert("configuration".into(), json!("WYE"));
        o.insert(
            "vm".into(),
            json!(vs.v_magnitude.iter().map(|v| v / 1e3).collect::<Vec<_>>()),
        );
        o.insert(
            "va".into(),
            json!(
                vs.v_angle
                    .iter()
                    .map(|a| a.to_degrees())
                    .collect::<Vec<_>>()
            ),
        );
        // The Thevenin matrices: verbatim when the source carried them
        // (an ENGINEERING round trip), recomputed with the engine's
        // formulas from short circuit data otherwise.
        if let (Some(rs), Some(xs)) = (vs.extras.get("rs"), vs.extras.get("xs")) {
            o.insert("rs".into(), rs.clone());
            o.insert("xs".into(), xs.clone());
        } else {
            let (rs, xs) = thevenin(vs, n);
            if rs.iter().flatten().all(|&v| v == 0.0) {
                self.warn(format!(
                    "{what}: no short circuit data; emitted an ideal source (zero rs/xs)"
                ));
            }
            o.insert("rs".into(), matrix(&rs));
            o.insert("xs".into(), matrix(&xs));
        }
        o.insert("status".into(), Self::status(&vs.extras));
        o.insert(
            "source_id".into(),
            json!(format!("vsource.{}", vs.name.to_lowercase())),
        );
        // The short circuit form (basekv/pu/angle/MVAsc/X-R ratios) is
        // represented by vm/va and the Thevenin matrices.
        self.extras_dropped(
            &vs.extras,
            &[
                "basekv",
                "basemva",
                "pu",
                "angle",
                "mvasc1",
                "mvasc3",
                "x1r1",
                "x0r0",
                "rs",
                "xs",
                "isc1",
                "isc3",
                "configuration",
            ],
            &what,
        );
        Value::Object(o)
    }

    fn transformers(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        if net.transformers.is_empty() {
            return;
        }
        let mut out = Map::new();
        for t in &net.transformers {
            out.insert(t.name.to_lowercase(), self.transformer(t));
        }
        doc.insert("transformer".into(), Value::Object(out));
    }

    fn transformer(&mut self, t: &DistTransformer) -> Value {
        let mut o = Map::new();
        let what = format!("transformer {}", t.name);
        let phases = t.phases;

        // The reader's stash carries a source polarity/connections pair the
        // lag convention does not reproduce (euro/lead, reversed windings);
        // emit it verbatim. Otherwise apply the ANSI lag convention the
        // reference dss2eng uses: barrel roll the wye phase conductors
        // under a delta primary and reverse the winding polarity.
        let stashed = t.extras.contains_key("pmd_polarity");
        let mut buses = Vec::new();
        let mut connections: Vec<Value> = Vec::new();
        for (w_idx, w) in t.windings.iter().enumerate() {
            buses.push(json!(w.bus.to_lowercase()));
            let mut c = conns(&w.terminal_map, &mut self.warnings, &what);
            if !stashed
                && w_idx > 0
                && t.windings[0].conn == WindingConn::Delta
                && w.conn == WindingConn::Wye
                && c.len() > 1
            {
                let phases_part = c.len() - 1;
                c[..phases_part].rotate_left(1);
            }
            connections.push(json!(c));
        }
        o.insert("bus".into(), Value::Array(buses));
        o.insert(
            "connections".into(),
            t.extras
                .get("pmd_connections")
                .cloned()
                .unwrap_or(Value::Array(connections)),
        );
        o.insert(
            "polarity".into(),
            t.extras
                .get("pmd_polarity")
                .cloned()
                .unwrap_or_else(|| json!(lag_polarity(&t.windings))),
        );
        o.insert(
            "configuration".into(),
            Value::Array(
                t.windings
                    .iter()
                    .map(|w| {
                        json!(match w.conn {
                            WindingConn::Wye => "WYE",
                            WindingConn::Delta => "DELTA",
                        })
                    })
                    .collect(),
            ),
        );
        o.insert(
            "rw".into(),
            json!(
                t.windings
                    .iter()
                    .map(|w| w.r_pct / 100.0)
                    .collect::<Vec<_>>()
            ),
        );
        o.insert(
            "xsc".into(),
            json!(t.xsc_pct.iter().map(|x| x / 100.0).collect::<Vec<_>>()),
        );
        o.insert(
            "sm_nom".into(),
            json!(
                t.windings
                    .iter()
                    .map(|w| w.s_rating / 1e3)
                    .collect::<Vec<_>>()
            ),
        );
        o.insert(
            "vm_nom".into(),
            json!(t.windings.iter().map(|w| w.v_ref / 1e3).collect::<Vec<_>>()),
        );
        let sm_ub =
            Self::extras_f64(&t.extras, "emerghkva").unwrap_or(t.windings[0].s_rating / 1e3 * 1.5);
        o.insert("sm_ub".into(), json!(sm_ub));
        insert_tap_fields(&mut o, t, phases);
        if let Some(controls) = t.extras.get("controls") {
            o.insert("controls".into(), controls.clone());
        }
        let noloadloss = Self::extras_f64(&t.extras, "%noloadloss").unwrap_or(0.0) / 100.0;
        let cmag = Self::extras_f64(&t.extras, "%imag").unwrap_or(0.0) / 100.0;
        o.insert("noloadloss".into(), json!(noloadloss));
        o.insert("cmag".into(), json!(cmag));
        o.insert("status".into(), Self::status(&t.extras));
        o.insert(
            "source_id".into(),
            json!(format!("transformer.{}", t.name.to_lowercase())),
        );
        self.extras_dropped(
            &t.extras,
            &["controls", "%loadloss", "%noloadloss", "%imag", "emerghkva"],
            &what,
        );
        Value::Object(o)
    }
}

/// The per winding per phase tap arrays. The reader's `pmd_tm_*` stashes
/// win (per phase taps, custom bounds, regulator fix flags); the defaults
/// for the rest are the engine's bounds (0.9..1.1) and step (1/32).
fn insert_tap_fields(o: &mut Map<String, Value>, t: &DistTransformer, phases: usize) {
    let nw = t.windings.len();
    let mut insert = |key: &str, default: fn(&DistTransformer, usize, usize) -> Value| {
        let v = t
            .extras
            .get(&format!("pmd_{key}"))
            .cloned()
            .unwrap_or_else(|| default(t, nw, phases));
        o.insert(key.into(), v);
    };
    insert("tm_set", |t, _, phases| {
        Value::Array(
            t.windings
                .iter()
                .map(|w| json!(vec![w.tap; phases]))
                .collect(),
        )
    });
    insert("tm_fix", |_, nw, phases| {
        Value::Array((0..nw).map(|_| json!(vec![true; phases])).collect())
    });
    insert("tm_lb", |_, nw, phases| {
        Value::Array((0..nw).map(|_| json!(vec![0.9; phases])).collect())
    });
    insert("tm_ub", |_, nw, phases| {
        Value::Array((0..nw).map(|_| json!(vec![1.1; phases])).collect())
    });
    insert("tm_step", |_, nw, phases| {
        Value::Array((0..nw).map(|_| json!(vec![1.0 / 32.0; phases])).collect())
    });
}

/// The ENGINEERING settings for a model without the reader's stash (dss or
/// bmopf sourced), following the dss2eng conventions: the per bus vbase is
/// the source's nominal line to neutral kV without the pu factor folded
/// in, and sbase is basemva in kVA (default 100 MVA).
fn synthesized_settings(net: &DistNetwork) -> Value {
    let mut settings = Map::new();
    settings.insert("base_frequency".into(), json!(net.base_frequency));
    settings.insert("power_scale_factor".into(), json!(1000.0));
    settings.insert("voltage_scale_factor".into(), json!(1000.0));
    let sbase = net
        .sources
        .first()
        .and_then(|vs| Writer::extras_f64(&vs.extras, "basemva"))
        .map_or(100_000.0, |mva| mva * 1e3);
    settings.insert("sbase_default".into(), json!(sbase));
    let mut vbases = Map::new();
    for vs in &net.sources {
        let phases = count_phases(vs).max(1) as f64;
        let vln_kv = Writer::extras_f64(&vs.extras, "basekv").map_or_else(
            || {
                let pu = Writer::extras_f64(&vs.extras, "pu").unwrap_or(1.0);
                vs.v_magnitude.first().copied().unwrap_or(0.0) / 1e3 / pu
            },
            |kv| kv / phases.sqrt(),
        );
        vbases.insert(vs.bus.to_lowercase(), json!(vln_kv));
    }
    settings.insert("vbases_default".into(), Value::Object(vbases));
    Value::Object(settings)
}

/// The polarity vector the ANSI lag convention produces for these windings:
/// -1 with a barrel roll on each wye winding under a delta primary, -1 on
/// the reversed second half of a center tap secondary, 1 elsewhere. The
/// reader compares the source against this to decide whether the file's
/// polarity needs an extras stash.
pub(super) fn lag_polarity(windings: &[Winding]) -> Vec<i64> {
    let nw = windings.len();
    let mut polarity = vec![1i64; nw];
    for (w_idx, w) in windings.iter().enumerate().skip(1) {
        if windings[0].conn == WindingConn::Delta
            && w.conn == WindingConn::Wye
            && w.terminal_map.len() > 1
        {
            polarity[w_idx] = -1;
        }
        // Center tap: the second half winding is reversed.
        if w_idx == 2 && nw == 3 && windings[1].terminal_map.last() == w.terminal_map.first() {
            polarity[w_idx] = -1;
        }
    }
    polarity
}

/// Names (lowercased) of linecodes that re-inline on their lines: every
/// referencing line carries the reader's `pmd_inline` marker.
fn inlined_codes(net: &DistNetwork) -> BTreeSet<String> {
    let mut inlined = BTreeSet::new();
    for c in &net.linecodes {
        let mut refs = net
            .lines
            .iter()
            .filter(|l| l.linecode.eq_ignore_ascii_case(&c.name))
            .peekable();
        if refs.peek().is_some()
            && refs.all(|l| l.extras.get("pmd_inline").and_then(Value::as_bool) == Some(true))
        {
            inlined.insert(c.name.to_lowercase());
        }
    }
    inlined
}

/// The six ENGINEERING impedance matrices of a linecode, emitted onto a
/// `linecode` entry or re-inlined onto a line. The b_fr/b_to numbers are
/// the dss cmatrix halves in nanofarads per meter (the susceptance follows
/// as 2 pi f C); the model holds true siemens per meter, so divide the
/// omega back out — or emit the reader's raw stash, which is bit exact.
fn insert_impedance_matrices(o: &mut Map<String, Value>, c: &DistLineCode, base_frequency: f64) {
    o.insert("rs".into(), matrix(&c.r_series));
    o.insert("xs".into(), matrix(&c.x_series));
    o.insert("g_fr".into(), matrix(&c.g_from));
    o.insert("g_to".into(), matrix(&c.g_to));
    if let (Some(fr), Some(to)) = (c.extras.get("pmd_b_fr"), c.extras.get("pmd_b_to")) {
        o.insert("b_fr".into(), fr.clone());
        o.insert("b_to".into(), to.clone());
    } else {
        let to_nf = 1.0 / (std::f64::consts::TAU * base_frequency * 1e-9);
        o.insert("b_fr".into(), matrix(&scale(&c.b_from, to_nf)));
        o.insert("b_to".into(), matrix(&scale(&c.b_to, to_nf)));
    }
}

/// The engine's Thevenin computation from MVAsc3/MVAsc1 and the X/R ratios
/// (the same math the reference dss2eng inherits): sequence impedances from
/// the short circuit levels, then self/mutual phase values filled over all
/// conductors including the neutral.
fn thevenin(vs: &VoltageSource, n_cond: usize) -> (Mat, Mat) {
    let get = |key: &str| Writer::extras_f64(&vs.extras, key);
    let basekv = get("basekv").unwrap_or_else(|| {
        // Reconstruct from the magnitude when basekv was defaulted.
        vs.v_magnitude.first().copied().unwrap_or(0.0) / 1e3 * (count_phases(vs) as f64).sqrt()
    });
    let phases = count_phases(vs);
    if basekv <= 0.0 || phases == 0 {
        return (zero_matrix(n_cond), zero_matrix(n_cond));
    }
    let mvasc3 = get("mvasc3").unwrap_or(2000.0);
    let mvasc1 = get("mvasc1").unwrap_or(2100.0);
    let x1r1 = get("x1r1").unwrap_or(4.0);
    let x0r0 = get("x0r0").unwrap_or(3.0);
    let factor = if phases == 1 { 1.0 } else { 3f64.sqrt() };

    let isc1 = mvasc1 * 1e3 / (basekv * factor);
    let x1 = basekv * basekv / mvasc3 / (1.0 + 1.0 / (x1r1 * x1r1)).sqrt();
    let r1 = x1 / x1r1;
    let a = 1.0 + x0r0 * x0r0;
    let b = 4.0 * (r1 + x1 * x0r0);
    let c = 4.0 * (r1 * r1 + x1 * x1) - (3.0 * basekv * 1000.0 / factor / isc1).powi(2);
    let disc = (b * b - 4.0 * a * c).max(0.0).sqrt();
    let r0 = ((-b + disc) / (2.0 * a)).max((-b - disc) / (2.0 * a));
    let x0 = r0 * x0r0;

    let r_self = (2.0 * r1 + r0) / 3.0;
    let x_self = (2.0 * x1 + x0) / 3.0;
    let r_mutual = (r0 - r1) / 3.0;
    let x_mutual = (x0 - x1) / 3.0;

    let mut r_mat = vec![vec![r_mutual; n_cond]; n_cond];
    let mut x_mat = vec![vec![x_mutual; n_cond]; n_cond];
    for i in 0..n_cond {
        r_mat[i][i] = r_self;
        x_mat[i][i] = x_self;
    }
    (r_mat, x_mat)
}

fn count_phases(vs: &VoltageSource) -> usize {
    vs.v_magnitude.iter().filter(|&&v| v > 0.0).count()
}

fn source_vm_nom(extras: &Extras, v_nom: &[f64]) -> Option<Value> {
    let raw = extras.get("kv")?;
    if v_nom.is_empty() {
        return Some(raw.clone());
    }
    if let Some(kv) = raw
        .as_f64()
        .or_else(|| raw.as_str().and_then(|s| s.parse().ok()))
    {
        if v_nom.iter().all(|v| same_voltage(*v, kv * 1e3)) {
            return Some(json!(kv));
        }
    }
    let vals: Vec<f64> = raw
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_f64)
        .collect();
    if vals.len() == 1 && v_nom.iter().all(|v| same_voltage(*v, vals[0] * 1e3)) {
        return Some(raw.clone());
    }
    if vals.len() == v_nom.len()
        && vals
            .iter()
            .zip(v_nom)
            .all(|(a, b)| same_voltage(*b, *a * 1e3))
    {
        return Some(raw.clone());
    }
    None
}

fn same_voltage(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

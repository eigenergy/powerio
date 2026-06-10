//! [`DistNetwork`] into PMD ENGINEERING JSON.
//!
//! The output reproduces what PMD's own dss2eng emits for the same network
//! wherever the model carries the data: terminal integers, `ENABLED`
//! status, `source_id`, the materialized grounded neutral with zero
//! `rg`/`xg`, linecode `cm_ub` from the emergency rating, transformer
//! `tm_*` tap fields, the delta-wye barrel roll with `polarity` -1 on the
//! lagging wye winding, and the voltage source Thevenin matrices computed
//! from the short circuit data when the source format carried it.

use serde_json::{Map, Value, json};

use crate::convert::Conversion;
use crate::model::{Configuration, DistNetwork, DistTransformer, Mat, VoltageSource, WindingConn};

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
        warnings: w.warnings,
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

fn scale(m: &Mat, k: f64) -> Mat {
    m.iter()
        .map(|row| row.iter().map(|v| v * k).collect())
        .collect()
}

impl Writer {
    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    fn extras_f64(extras: &crate::model::Extras, key: &str) -> Option<f64> {
        extras.get(key).and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
    }

    fn document(&mut self, net: &DistNetwork) -> Value {
        let mut doc = Map::new();
        doc.insert("data_model".into(), json!("ENGINEERING"));
        doc.insert(
            "name".into(),
            json!(net.name.clone().unwrap_or_default().to_lowercase()),
        );
        doc.insert("files".into(), json!([]));

        let mut settings = Map::new();
        settings.insert("base_frequency".into(), json!(net.base_frequency));
        settings.insert("power_scale_factor".into(), json!(1000.0));
        settings.insert("voltage_scale_factor".into(), json!(1000.0));
        settings.insert("sbase_default".into(), json!(100_000.0));
        let mut vbases = Map::new();
        for vs in &net.sources {
            let vln_kv = vs.v_magnitude.first().copied().unwrap_or(0.0) / 1e3;
            vbases.insert(vs.bus.to_lowercase(), json!(vln_kv));
        }
        settings.insert("vbases_default".into(), Value::Object(vbases));
        doc.insert("settings".into(), Value::Object(settings));

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
            o.insert("rg".into(), json!(vec![0.0; grounded.len()]));
            o.insert("xg".into(), json!(vec![0.0; grounded.len()]));
            o.insert("grounded".into(), json!(grounded));
            o.insert("status".into(), json!("ENABLED"));
            if let Some(x) = Self::extras_f64(&b.extras, "x") {
                o.insert("lon".into(), json!(x));
            }
            if let Some(y) = Self::extras_f64(&b.extras, "y") {
                o.insert("lat".into(), json!(y));
            }
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
        if net.linecodes.is_empty() {
            return;
        }
        // The ENGINEERING b_fr/b_to numbers are the dss cmatrix halves in
        // nanofarads per meter (the susceptance follows as 2 pi f C); the
        // model holds true siemens per meter, so divide the omega back out.
        let to_nf = 1.0 / (std::f64::consts::TAU * net.base_frequency * 1e-9);
        let mut codes = Map::new();
        for c in &net.linecodes {
            let mut o = Map::new();
            o.insert("rs".into(), matrix(&c.r_series));
            o.insert("xs".into(), matrix(&c.x_series));
            o.insert("g_fr".into(), matrix(&c.g_from));
            o.insert("g_to".into(), matrix(&c.g_to));
            if let (Some(fr), Some(to)) = (c.extras.get("pmd_b_fr"), c.extras.get("pmd_b_to")) {
                o.insert("b_fr".into(), fr.clone());
                o.insert("b_to".into(), to.clone());
            } else {
                o.insert("b_fr".into(), matrix(&scale(&c.b_from, to_nf)));
                o.insert("b_to".into(), matrix(&scale(&c.b_to, to_nf)));
            }
            if let Some(i_max) = &c.i_max {
                o.insert("cm_ub".into(), json!(i_max));
            }
            codes.insert(c.name.to_lowercase(), Value::Object(o));
        }
        doc.insert("linecode".into(), Value::Object(codes));
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
                o.insert("linecode".into(), json!(l.linecode.to_lowercase()));
                o.insert("status".into(), json!("ENABLED"));
                o.insert(
                    "source_id".into(),
                    json!(format!("line.{}", l.name.to_lowercase())),
                );
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
                // PMD models a dss switch as a tiny series resistance,
                // computed as 1e-4 ohm/m over the forced 0.001 m length;
                // the product form keeps the value bit identical.
                let mut rs = zero_matrix(n);
                for (i, row) in rs.iter_mut().enumerate() {
                    row[i] = 1e-4 * 0.001;
                }
                o.insert("rs".into(), matrix(&rs));
                o.insert("xs".into(), matrix(&zero_matrix(n)));
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
                o.insert("status".into(), json!("ENABLED"));
                o.insert(
                    "source_id".into(),
                    json!(format!("line.{}", s.name.to_lowercase())),
                );
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
                if let Some(kv) = Self::extras_f64(&l.extras, "kv") {
                    o.insert("vm_nom".into(), json!(kv));
                }
                let model = match Self::extras_f64(&l.extras, "model").map(|m| m as i64) {
                    Some(2) => "IMPEDANCE",
                    Some(5) => "CURRENT",
                    Some(8) => "ZIPV",
                    _ => "POWER",
                };
                o.insert("model".into(), json!(model));
                o.insert("dispatchable".into(), json!("NO"));
                o.insert("status".into(), json!("ENABLED"));
                o.insert(
                    "source_id".into(),
                    json!(format!("load.{}", l.name.to_lowercase())),
                );
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
                o.insert("status".into(), json!("ENABLED"));
                o.insert(
                    "source_id".into(),
                    json!(format!("generator.{}", g.name.to_lowercase())),
                );
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
                o.insert("configuration".into(), json!("WYE"));
                o.insert("model".into(), json!("CAPACITOR"));
                o.insert("dispatchable".into(), json!("NO"));
                o.insert("status".into(), json!("ENABLED"));
                o.insert(
                    "source_id".into(),
                    json!(format!("capacitor.{}", s.name.to_lowercase())),
                );
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
        o.insert("status".into(), json!("ENABLED"));
        o.insert(
            "source_id".into(),
            json!(format!("vsource.{}", vs.name.to_lowercase())),
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
        let nw = t.windings.len();
        let phases = t.phases;

        let mut buses = Vec::new();
        let mut connections: Vec<Value> = Vec::new();
        let mut polarity = vec![1i64; nw];
        for (w_idx, w) in t.windings.iter().enumerate() {
            buses.push(json!(w.bus.to_lowercase()));
            let mut c = conns(&w.terminal_map, &mut self.warnings, &what);
            if w_idx > 0 {
                let prim_delta = t.windings[0].conn == WindingConn::Delta;
                if prim_delta && w.conn == WindingConn::Wye && c.len() > 1 {
                    // The lag (ansi) connection: barrel roll the phase
                    // conductors by one and reverse the winding polarity,
                    // as the reference dss2eng does.
                    let phases_part = c.len() - 1;
                    c[..phases_part].rotate_left(1);
                    polarity[w_idx] = -1;
                }
                // Center tap: the second half winding is reversed.
                if w_idx == 2
                    && nw == 3
                    && t.windings[1].terminal_map.last() == w.terminal_map.first()
                {
                    polarity[w_idx] = -1;
                }
            }
            connections.push(json!(c));
        }
        o.insert("bus".into(), Value::Array(buses));
        o.insert("connections".into(), Value::Array(connections));
        o.insert("polarity".into(), json!(polarity));
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
        o.insert("status".into(), json!("ENABLED"));
        o.insert(
            "source_id".into(),
            json!(format!("transformer.{}", t.name.to_lowercase())),
        );
        Value::Object(o)
    }
}

/// The per winding per phase tap arrays, with the engine's defaults for the
/// bounds (0.9..1.1) and step (1/32).
fn insert_tap_fields(o: &mut Map<String, Value>, t: &DistTransformer, phases: usize) {
    let nw = t.windings.len();
    o.insert(
        "tm_set".into(),
        Value::Array(
            t.windings
                .iter()
                .map(|w| json!(vec![w.tap; phases]))
                .collect(),
        ),
    );
    o.insert(
        "tm_fix".into(),
        Value::Array((0..nw).map(|_| json!(vec![true; phases])).collect()),
    );
    o.insert(
        "tm_lb".into(),
        Value::Array((0..nw).map(|_| json!(vec![0.9; phases])).collect()),
    );
    o.insert(
        "tm_ub".into(),
        Value::Array((0..nw).map(|_| json!(vec![1.1; phases])).collect()),
    );
    o.insert(
        "tm_step".into(),
        Value::Array((0..nw).map(|_| json!(vec![1.0 / 32.0; phases])).collect()),
    );
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

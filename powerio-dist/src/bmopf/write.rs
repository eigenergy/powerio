//! [`DistNetwork`] into strict BMOPF JSON.
//!
//! Output is schema valid wherever the schema permits the data; the one
//! deliberate exception is linecodes and shunts wider than 9 conductors,
//! whose matrix keys (`R_series_10_10`) the draft schema's single digit
//! key patterns reject. The writer emits them anyway: the data is valid,
//! the pattern is the limitation, and the conversion warns.
//!
//! Numbers serialize through serde_json (shortest round trip form).
//! Nonfinite values cannot appear in JSON; they emit as 0 with a warning
//! naming the element and field.

use serde_json::{Map, Value, json};

use crate::convert::Conversion;
use crate::model::{Configuration, DistNetwork, DistTransformer, Mat, Winding, WindingConn};

/// Writes the strict BMOPF document. Every field the schema cannot carry
/// is reported in the warnings.
///
/// # Panics
///
/// Never in practice: the document is maps, strings, and finite numbers,
/// which always serialize.
pub fn write_bmopf_json(net: &DistNetwork) -> Conversion {
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

impl Writer {
    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// Finite number guard (the jnum pattern): JSON has no Inf/NaN.
    fn num(&mut self, v: f64, what: &str) -> Value {
        if v.is_finite() {
            json!(v)
        } else {
            self.warn(format!("{what}: nonfinite value emitted as 0"));
            json!(0.0)
        }
    }

    fn nums(&mut self, vs: &[f64], what: &str) -> Value {
        Value::Array(vs.iter().map(|&v| self.num(v, what)).collect())
    }

    fn extras_dropped(&mut self, extras: &crate::model::Extras, what: &str) {
        for key in extras.keys() {
            if key == "bmopf_subtype" {
                continue; // reader bookkeeping, not source data
            }
            self.warn(format!(
                "{what}: `{key}` has no place in the BMOPF schema; dropped from the output"
            ));
        }
    }

    fn document(&mut self, net: &DistNetwork) -> Value {
        let mut doc = Map::new();
        if let Some(name) = &net.name {
            doc.insert("name".into(), json!(name));
        }

        let mut buses = Map::new();
        for b in &net.buses {
            let mut o = Map::new();
            o.insert("terminal_names".into(), json!(b.terminals));
            if !b.grounded.is_empty() {
                o.insert("perfectly_grounded_terminals".into(), json!(b.grounded));
            }
            if let Some(v) = b.v_min {
                o.insert("v_min".into(), self.num(v, "bus v_min"));
            }
            if let Some(v) = b.v_max {
                o.insert("v_max".into(), self.num(v, "bus v_max"));
            }
            for (key, bound) in [
                ("vpn_min", &b.vpn_min),
                ("vpn_max", &b.vpn_max),
                ("vpp_min", &b.vpp_min),
                ("vpp_max", &b.vpp_max),
                ("vsym_min", &b.vsym_min),
                ("vsym_max", &b.vsym_max),
            ] {
                if let Some(v) = bound {
                    o.insert(key.into(), self.nums(v, &format!("bus {key}")));
                }
            }
            // Coordinates and other extras have no bus fields in the schema.
            self.extras_dropped(&b.extras, &format!("bus {}", b.id));
            buses.insert(b.id.clone(), Value::Object(o));
        }
        doc.insert("bus".into(), Value::Object(buses));

        if !net.linecodes.is_empty() {
            let mut codes = Map::new();
            for c in &net.linecodes {
                let mut o = Map::new();
                let n = c.n_conductors;
                if n > 9 {
                    self.warn(format!(
                        "linecode {}: {n} conductors produce double digit matrix keys, \
                         which the draft schema's `^R_series_\\d_\\d` patterns reject; \
                         emitted anyway",
                        c.name
                    ));
                }
                self.flat_matrix(&mut o, "R_series", &c.r_series, &c.name);
                self.flat_matrix(&mut o, "X_series", &c.x_series, &c.name);
                self.flat_matrix(&mut o, "G_from", &c.g_from, &c.name);
                self.flat_matrix(&mut o, "G_to", &c.g_to, &c.name);
                self.flat_matrix(&mut o, "B_from", &c.b_from, &c.name);
                self.flat_matrix(&mut o, "B_to", &c.b_to, &c.name);
                if let Some(i_max) = &c.i_max {
                    o.insert("i_max".into(), self.nums(i_max, "linecode i_max"));
                }
                if let Some(s_max) = &c.s_max {
                    o.insert("s_max".into(), self.nums(s_max, "linecode s_max"));
                }
                self.extras_dropped(&c.extras, &format!("linecode {}", c.name));
                codes.insert(c.name.clone(), Value::Object(o));
            }
            doc.insert("linecode".into(), Value::Object(codes));
        }

        self.branches(net, &mut doc);
        self.injections(net, &mut doc);

        let transformers = self.transformers(net);
        if !transformers.is_empty() {
            doc.insert("transformer".into(), Value::Object(transformers));
        }

        for u in &net.untyped {
            self.warn(format!(
                "{} {}: class is not represented in BMOPF; dropped from the output",
                u.class, u.name
            ));
        }
        Value::Object(doc)
    }

    /// Lines and switches.
    fn branches(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        if !net.lines.is_empty() {
            let mut lines = Map::new();
            for l in &net.lines {
                let mut o = Map::new();
                o.insert("length".into(), self.num(l.length, "line length"));
                o.insert("linecode".into(), json!(l.linecode));
                o.insert("bus_from".into(), json!(l.bus_from));
                o.insert("bus_to".into(), json!(l.bus_to));
                o.insert("terminal_map_from".into(), json!(l.terminal_map_from));
                o.insert("terminal_map_to".into(), json!(l.terminal_map_to));
                self.extras_dropped(&l.extras, &format!("line {}", l.name));
                lines.insert(l.name.clone(), Value::Object(o));
            }
            doc.insert("line".into(), Value::Object(lines));
        }
        if !net.switches.is_empty() {
            let mut switches = Map::new();
            for s in &net.switches {
                let mut o = Map::new();
                o.insert("bus_from".into(), json!(s.bus_from));
                o.insert("bus_to".into(), json!(s.bus_to));
                o.insert("terminal_map_from".into(), json!(s.terminal_map_from));
                o.insert("terminal_map_to".into(), json!(s.terminal_map_to));
                o.insert("open_switch".into(), json!(s.open));
                if let Some(i_max) = &s.i_max {
                    o.insert("i_max".into(), self.nums(i_max, "switch i_max"));
                }
                self.extras_dropped(&s.extras, &format!("switch {}", s.name));
                switches.insert(s.name.clone(), Value::Object(o));
            }
            doc.insert("switch".into(), Value::Object(switches));
        }
    }

    /// Loads, generators, shunts, and the voltage sources.
    fn injections(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        if !net.loads.is_empty() {
            let mut loads = Map::new();
            for l in &net.loads {
                let mut o = Map::new();
                o.insert("configuration".into(), json!(config_str(l.configuration)));
                o.insert("p_nom".into(), self.nums(&l.p_nom, "load p_nom"));
                o.insert("q_nom".into(), self.nums(&l.q_nom, "load q_nom"));
                o.insert("bus".into(), json!(l.bus));
                o.insert("terminal_map".into(), json!(l.terminal_map));
                self.extras_dropped(&l.extras, &format!("load {}", l.name));
                loads.insert(l.name.clone(), Value::Object(o));
            }
            doc.insert("load".into(), Value::Object(loads));
        }
        if !net.generators.is_empty() {
            let mut gens = Map::new();
            for g in &net.generators {
                gens.insert(g.name.clone(), self.generator(g));
            }
            doc.insert("generator".into(), Value::Object(gens));
        }
        if !net.shunts.is_empty() {
            let mut shunts = Map::new();
            for s in &net.shunts {
                let mut o = Map::new();
                o.insert("bus".into(), json!(s.bus));
                o.insert("terminal_map".into(), json!(s.terminal_map));
                self.flat_matrix(&mut o, "G", &s.g, &s.name);
                self.flat_matrix(&mut o, "B", &s.b, &s.name);
                self.extras_dropped(&s.extras, &format!("shunt {}", s.name));
                shunts.insert(s.name.clone(), Value::Object(o));
            }
            doc.insert("shunt".into(), Value::Object(shunts));
        }
        let mut sources = Map::new();
        for (i, vs) in net.sources.iter().enumerate() {
            if i > 0 {
                self.warn(format!(
                    "voltage source {}: the BMOPF formulation expects exactly one source; \
                     this network has {}",
                    vs.name,
                    net.sources.len()
                ));
            }
            let mut o = Map::new();
            o.insert(
                "v_magnitude".into(),
                self.nums(&vs.v_magnitude, "voltage_source v_magnitude"),
            );
            o.insert(
                "v_angle".into(),
                self.nums(&vs.v_angle, "voltage_source v_angle"),
            );
            o.insert("bus".into(), json!(vs.bus));
            o.insert("terminal_map".into(), json!(vs.terminal_map));
            self.extras_dropped(&vs.extras, &format!("voltage source {}", vs.name));
            sources.insert(vs.name.clone(), Value::Object(o));
        }
        doc.insert("voltage_source".into(), Value::Object(sources));
    }

    fn generator(&mut self, g: &crate::model::DistGenerator) -> Value {
        let mut o = Map::new();
        // BMOPF generators carry bounds and cost, no dispatch setpoint: a
        // fixed injection becomes pinned bounds. Explicit source bounds win
        // over the setpoint, which then has nowhere to go.
        let what = format!("generator {}", g.name);
        for (key_lo, key_hi, lo, hi, nom) in [
            ("p_min", "p_max", &g.p_min, &g.p_max, &g.p_nom),
            ("q_min", "q_max", &g.q_min, &g.q_max, &g.q_nom),
        ] {
            if lo.is_some() || hi.is_some() {
                // Pinned bounds ARE the setpoint; only a setpoint that
                // differs from the bounds has nowhere to go.
                let pinned = lo.as_deref() == Some(nom) && hi.as_deref() == Some(nom);
                if !nom.is_empty() && !nom.iter().all(|&v| v == 0.0) && !pinned {
                    self.warn(format!(
                        "{what}: explicit {key_lo}/{key_hi} bounds win over the setpoint, \
                         which has no BMOPF field"
                    ));
                }
                if let Some(v) = lo {
                    o.insert(key_lo.into(), self.nums(v, key_lo));
                }
                if let Some(v) = hi {
                    o.insert(key_hi.into(), self.nums(v, key_hi));
                }
            } else if !nom.is_empty() {
                // A fixed injection becomes pinned bounds.
                o.insert(key_lo.into(), self.nums(nom, key_lo));
                o.insert(key_hi.into(), self.nums(nom, key_hi));
            }
        }
        let cost = g.cost.unwrap_or_else(|| {
            self.warnings.push(format!(
                "{what}: no generation cost in the source; emitted cost 0"
            ));
            0.0
        });
        o.insert("cost".into(), self.num(cost, "generator cost"));
        o.insert("bus".into(), json!(g.bus));
        o.insert("configuration".into(), json!(config_str(g.configuration)));
        o.insert("terminal_map".into(), json!(g.terminal_map));
        if g.configuration == Configuration::Delta {
            self.warn(format!(
                "{what}: the BMOPF formulation covers WYE generators; DELTA emitted as written"
            ));
        }
        self.extras_dropped(&g.extras, &what);
        Value::Object(o)
    }

    /// Transformers keyed by subtype; wye-wye three phase units decompose
    /// into one single_phase entry per phase, the convention the public
    /// example networks use.
    fn transformers(&mut self, net: &DistNetwork) -> Map<String, Value> {
        let mut by_subtype: Map<String, Value> = Map::new();
        let insert = |sub: &str, name: String, v: Value, map: &mut Map<String, Value>| {
            map.entry(sub.to_string())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("subtype maps are objects")
                .insert(name, v);
        };
        for t in &net.transformers {
            self.extras_dropped(&t.extras, &format!("transformer {}", t.name));
            match classify(t) {
                Kind::SinglePhase => {
                    let v = self.two_winding(t, &t.windings[0], &t.windings[1], 1.0);
                    insert("single_phase", t.name.clone(), v, &mut by_subtype);
                }
                Kind::SinglePhaseShape(sub) => {
                    let v = self.two_winding(t, &t.windings[0], &t.windings[1], 1.0);
                    insert(sub, t.name.clone(), v, &mut by_subtype);
                }
                Kind::CenterTap => {
                    let v = self.center_tap(t);
                    insert("center_tap", t.name.clone(), v, &mut by_subtype);
                }
                Kind::WyeDelta => {
                    let v = self.three_phase(t, 0);
                    insert("wye_delta", t.name.clone(), v, &mut by_subtype);
                }
                Kind::DeltaWye => {
                    let v = self.three_phase(t, 1);
                    insert("delta_wye", t.name.clone(), v, &mut by_subtype);
                }
                Kind::WyeWye3 => {
                    for (k, v) in self.decompose_wye_wye(t) {
                        insert("single_phase", k, v, &mut by_subtype);
                    }
                }
                Kind::Unsupported(why) => {
                    self.warn(format!(
                        "transformer {}: {why}; not representable in the four BMOPF \
                         subtypes, dropped from the output",
                        t.name
                    ));
                }
            }
        }
        by_subtype
    }

    /// Shared single_phase / center_tap shape. `to_scale` rescales the to
    /// side ratings (used by the wye-wye decomposition).
    fn two_winding(
        &mut self,
        t: &DistTransformer,
        from: &Winding,
        to: &Winding,
        s_scale: f64,
    ) -> Value {
        let s = from.s_rating * s_scale;
        let zb_from = from.v_ref * from.v_ref / s;
        let zb_to = to.v_ref * to.v_ref / s;
        let mut o = Map::new();
        o.insert("bus_from".into(), json!(from.bus));
        o.insert("bus_to".into(), json!(to.bus));
        o.insert("s_rating".into(), self.num(s, "transformer s_rating"));
        o.insert(
            "v_ref_from".into(),
            self.num(from.v_ref, "transformer v_ref_from"),
        );
        o.insert(
            "v_ref_to".into(),
            self.num(to.v_ref, "transformer v_ref_to"),
        );
        o.insert(
            "r_series_from".into(),
            self.num(from.r_pct / 100.0 * zb_from, "transformer r_series_from"),
        );
        o.insert(
            "r_series_to".into(),
            self.num(to.r_pct / 100.0 * zb_to, "transformer r_series_to"),
        );
        // The whole leakage reactance rides on the from side, the
        // convention the public example uses.
        o.insert(
            "x_series_from".into(),
            self.num(t.xsc_pct[0] / 100.0 * zb_from, "transformer x_series_from"),
        );
        o.insert("x_series_to".into(), json!(0.0));
        o.insert("terminal_map_from".into(), json!(from.terminal_map));
        o.insert("terminal_map_to".into(), json!(to.terminal_map));
        self.taps_dropped(t);
        o.into()
    }

    fn center_tap(&mut self, t: &DistTransformer) -> Value {
        // The split secondary collapses to one to side winding: voltage is
        // the full 240 V across the outer terminals, the center tap is the
        // shared terminal, listed last.
        let from = &t.windings[0];
        let (w2, w3) = (&t.windings[1], &t.windings[2]);
        let common = w2
            .terminal_map
            .iter()
            .find(|term| w3.terminal_map.contains(term))
            .cloned()
            .unwrap_or_default();
        let mut hots: Vec<String> = Vec::new();
        for term in w2.terminal_map.iter().chain(&w3.terminal_map) {
            if *term != common && !hots.contains(term) {
                hots.push(term.clone());
            }
        }
        let to = Winding {
            bus: w2.bus.clone(),
            terminal_map: {
                let mut m = hots;
                m.push(common);
                m
            },
            conn: WindingConn::Wye,
            v_ref: w2.v_ref + w3.v_ref,
            s_rating: from.s_rating,
            r_pct: w2.r_pct + w3.r_pct,
            tap: 1.0,
        };
        self.warn(format!(
            "transformer {}: center tap secondary collapsed to one winding; the \
             xht/xlt impedance split is not representable and was dropped",
            t.name
        ));
        self.two_winding(t, from, &to, 1.0)
    }

    /// `wye_delta` / `delta_wye`: one series impedance in ohms on the wye
    /// side. `wye_idx` names which winding is the wye one.
    fn three_phase(&mut self, t: &DistTransformer, wye_idx: usize) -> Value {
        let from = &t.windings[0];
        let to = &t.windings[1];
        let wye = &t.windings[wye_idx];
        let s = from.s_rating;
        let zb_wye = wye.v_ref * wye.v_ref / s;
        let mut o = Map::new();
        o.insert("bus_from".into(), json!(from.bus));
        o.insert("bus_to".into(), json!(to.bus));
        o.insert("s_rating".into(), self.num(s, "transformer s_rating"));
        o.insert(
            "v_ref_from".into(),
            self.num(from.v_ref, "transformer v_ref_from"),
        );
        o.insert(
            "v_ref_to".into(),
            self.num(to.v_ref, "transformer v_ref_to"),
        );
        o.insert(
            "r_series".into(),
            self.num(
                (from.r_pct + to.r_pct) / 100.0 * zb_wye,
                "transformer r_series",
            ),
        );
        o.insert(
            "x_series".into(),
            self.num(t.xsc_pct[0] / 100.0 * zb_wye, "transformer x_series"),
        );
        o.insert("terminal_map_from".into(), json!(from.terminal_map));
        o.insert("terminal_map_to".into(), json!(to.terminal_map));
        self.taps_dropped(t);
        o.into()
    }

    /// A three phase wye-wye unit becomes one single_phase entry per phase
    /// (`name_1`..), each at line to neutral voltage and a third of the
    /// rating, the convention the public example networks use.
    fn decompose_wye_wye(&mut self, t: &DistTransformer) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        let (from, to) = (&t.windings[0], &t.windings[1]);
        let sqrt3 = 3f64.sqrt();
        for k in 0..t.phases {
            let per = |w: &Winding| {
                let neutral = w.terminal_map.last().cloned().unwrap_or_default();
                Winding {
                    bus: w.bus.clone(),
                    terminal_map: vec![w.terminal_map[k].clone(), neutral],
                    conn: WindingConn::Wye,
                    v_ref: w.v_ref / sqrt3,
                    s_rating: w.s_rating / 3.0,
                    r_pct: w.r_pct,
                    tap: w.tap,
                }
            };
            let f = per(from);
            let to_1 = per(to);
            let mut t1 = t.clone();
            t1.windings = vec![f.clone(), to_1.clone()];
            let v = self.two_winding(&t1, &f, &to_1, 1.0);
            out.push((format!("{}_{}", t.name, k + 1), v));
        }
        self.warn(format!(
            "transformer {}: three phase wye-wye decomposed into {} single_phase units",
            t.name, t.phases
        ));
        out
    }

    fn taps_dropped(&mut self, t: &DistTransformer) {
        for w in &t.windings {
            if (w.tap - 1.0).abs() > 1e-12 {
                self.warn(format!(
                    "transformer {}: off nominal tap {} has no BMOPF field; dropped",
                    t.name, w.tap
                ));
            }
        }
    }

    fn flat_matrix(&mut self, o: &mut Map<String, Value>, prefix: &str, m: &Mat, name: &str) {
        for (i, row) in m.iter().enumerate() {
            for (j, &v) in row.iter().enumerate() {
                o.insert(
                    format!("{prefix}_{}_{}", i + 1, j + 1),
                    self.num(v, &format!("{name} {prefix}")),
                );
            }
        }
    }
}

enum Kind {
    SinglePhase,
    /// Two windings already in the shared single_phase/center_tap shape,
    /// emitted under the named subtype.
    SinglePhaseShape(&'static str),
    CenterTap,
    WyeDelta,
    DeltaWye,
    WyeWye3,
    Unsupported(String),
}

fn classify(t: &DistTransformer) -> Kind {
    // A network read from BMOPF records its subtype; trust it so writing
    // back reproduces the grouping (center tap reads as two windings).
    if let Some(sub) = t.extras.get("bmopf_subtype").and_then(|v| v.as_str()) {
        match sub {
            "single_phase" => return Kind::SinglePhase,
            "center_tap" if t.windings.len() == 2 => return Kind::SinglePhaseShape("center_tap"),
            "wye_delta" => return Kind::WyeDelta,
            "delta_wye" => return Kind::DeltaWye,
            _ => {}
        }
    }
    let conns: Vec<WindingConn> = t.windings.iter().map(|w| w.conn).collect();
    match (t.phases, conns.as_slice()) {
        (1, [WindingConn::Wye, WindingConn::Wye]) => Kind::SinglePhase,
        (1, [WindingConn::Wye, WindingConn::Wye, WindingConn::Wye]) => Kind::CenterTap,
        (3, [WindingConn::Wye, WindingConn::Delta]) => Kind::WyeDelta,
        (3, [WindingConn::Delta, WindingConn::Wye]) => Kind::DeltaWye,
        (3, [WindingConn::Wye, WindingConn::Wye]) => Kind::WyeWye3,
        _ => Kind::Unsupported(format!(
            "{} phase with {} windings ({:?})",
            t.phases,
            t.windings.len(),
            conns
        )),
    }
}

fn config_str(c: Configuration) -> &'static str {
    match c {
        Configuration::Wye => "WYE",
        Configuration::Delta => "DELTA",
        Configuration::SinglePhase => "SINGLE_PHASE",
    }
}

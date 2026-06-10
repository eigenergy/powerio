//! [`DistNetwork`] into OpenDSS `.dss` text.
//!
//! The canonical writer regenerates a solvable case from the typed model:
//! a `Clear`/`Set DefaultBaseFrequency` header, the circuit with its
//! source, linecodes in meters, elements with explicit bus dots (a
//! terminal in the bus's perfectly grounded set emits as node 0, the exact
//! inverse of the reader's materialization), the source `Set` options the
//! writer does not derive itself, `Set VoltageBases`, `Calcvoltagebases`,
//! and `Solve`. Element extras whose keys appear in the class property
//! tables emit verbatim; everything else is reported.
//!
//! Floats print through Rust's shortest round trip formatting; OpenDSS
//! reads the full precision back.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::convert::Conversion;
use crate::model::{Configuration, DistBus, DistNetwork, Extras, Mat, WindingConn};

use super::{lex, prop};

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
        terminals: net
            .buses
            .iter()
            .map(|b| (b.id.to_ascii_lowercase(), b.terminals.clone()))
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
    /// Bus id (lowercase) → ordered terminal names.
    terminals: BTreeMap<String, Vec<String>>,
    /// Bus id (lowercase) → phase to neutral voltage estimate, volts.
    kv_estimate: BTreeMap<String, f64>,
}

/// Phase to neutral voltage per bus, propagated from the sources through
/// lines and switches (same level) and transformers (winding ratios). The
/// estimate feeds load/capacitor `kv` and `Set VoltageBases` when the
/// source format did not carry them.
///
/// The seed is not the model voltage directly: it is the basekv the writer
/// will emit (the stashed token when the source carried one), run through
/// the reader's basekv → per phase formula. A reparse then reproduces the
/// same floats bit for bit; seeding from `v_magnitude` is not a fixed
/// point of the sqrt round trip and `Set VoltageBases` would drift one ulp
/// per write. Transformer ratios use `(v_ref / 1e3) * 1e3`, the value a
/// reparse of the emitted `kvs=` rebuilds, for the same reason.
fn estimate_bus_kv(net: &DistNetwork) -> BTreeMap<String, f64> {
    let mut kv: BTreeMap<String, f64> = BTreeMap::new();
    for vs in &net.sources {
        let phases = source_phases(net, vs);
        let basekv = extras_f64(&vs.extras, "basekv").unwrap_or_else(|| source_basekv(vs, phases));
        let pu = extras_f64(&vs.extras, "pu").unwrap_or(1.0);
        let vln = basekv * 1e3 * pu / source_chord(phases);
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
                let v_ref_known = (t.windings[i].v_ref / 1e3) * 1e3;
                if v_ref_known > 0.0 {
                    for (j, w) in t.windings.iter().enumerate() {
                        if j != i && !kv.contains_key(&w.bus.to_ascii_lowercase()) {
                            kv.insert(
                                w.bus.to_ascii_lowercase(),
                                v_known * ((w.v_ref / 1e3) * 1e3) / v_ref_known,
                            );
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

/// VSource.cpp's per phase magnitude divisor: the chord of the n-gon
/// (1 for a single phase source, sqrt(3) at n = 3). Division by the
/// 1 phase chord is exact, so one expression serves both reader branches.
fn source_chord(phases: usize) -> f64 {
    if phases <= 1 {
        1.0
    } else {
        2.0 * (std::f64::consts::PI / phases as f64).sin()
    }
}

/// The basekv a source without a stashed token emits: the model magnitude
/// through the inverse of the reader's chord formula.
fn source_basekv(vs: &crate::model::VoltageSource, phases: usize) -> f64 {
    vs.v_magnitude.iter().copied().fold(0.0_f64, f64::max) * source_chord(phases) / 1e3
}

/// An extra as a number: the reader stashes written tokens as strings and
/// materialized defaults as numbers.
fn extras_f64(extras: &Extras, key: &str) -> Option<f64> {
    let v = extras.get(key)?;
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn extras_usize(extras: &Extras, key: &str) -> Option<usize> {
    let v = extras.get(key)?;
    v.as_u64()
        .and_then(|u| usize::try_from(u).ok())
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .or_else(|| {
            v.as_f64()
                .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                .map(|f| f as usize)
        })
}

/// Whether the dss tokenizer would split this name: its delimiters, quote
/// pair characters, comment openers, and (in bus ids) the node dot.
fn name_breaks_dss(name: &str, is_bus_id: bool) -> bool {
    name.contains("//")
        || name.chars().any(|c| {
            matches!(
                c,
                ' ' | '\t' | ',' | '=' | '!' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}'
            ) || (is_bus_id && c == '.')
        })
}

/// A `key=value` value as dss text. A value the lexer scans back as one
/// bare token emits bare; anything else wraps in the first quote pair
/// whose closer is absent from the value. The lexer honors all five pairs,
/// and its quoted scan runs to the closer without checking delimiters or
/// comment openers, so the wrapper protects spaces, commas, `=`, `!`, and
/// `//`. The choice depends only on the value: the reader strips the
/// wrapper, so the next write sees the bare value and picks the same form.
/// `false` means nothing reparses to the value — every closer appears in
/// it and bare scanning splits it — and the caller must warn.
fn dss_value_out(value: &str) -> (String, bool) {
    let mut scan = lex::Scanner::new(value, None);
    let bare = match scan.next_param() {
        None => value.is_empty(),
        Some(p) => {
            p.name.is_none()
                && !p.value.quoted
                && p.value.text == value
                && scan.next_param().is_none()
        }
    };
    if bare {
        return (value.to_string(), true);
    }
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\'')] {
        if !value.contains(close) {
            return (format!("{open}{value}{close}"), true);
        }
    }
    (value.to_string(), false)
}

/// Emitted source `phases=`: the stashed token when the source carried
/// one, otherwise the terminal map entries outside the bus's grounded
/// set. The engine counts conductors, not energized phases, so a phase
/// at v_magnitude 0 keeps its place on the dot list; the emission site
/// warns about the disagreement.
fn source_phases(net: &DistNetwork, vs: &crate::model::VoltageSource) -> usize {
    if let Some(p) = extras_usize(&vs.extras, "phases") {
        return p.max(1);
    }
    let grounded = net
        .buses
        .iter()
        .find(|b| b.id.eq_ignore_ascii_case(&vs.bus))
        .map(|b| b.grounded.as_slice())
        .unwrap_or_default();
    vs.terminal_map
        .iter()
        .filter(|t| !grounded.contains(t))
        .count()
        .max(1)
}

/// First row (self, mutual) of a series matrix extra, without consuming it.
fn seq_parts(extras: &Extras, key: &str) -> Option<(f64, f64)> {
    let row = extras.get(key)?.as_array()?.first()?.as_array()?;
    let self_v = row.first()?.as_f64()?;
    let mutual = row
        .get(1)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    Some((self_v, mutual))
}

impl DssWriter {
    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// The engine's bus fill rule gives every conductor the dot list does
    /// not cover a default — nodes 1..=phases for the phase conductors,
    /// ground for the rest — so a map shorter than the class's conductor
    /// count comes back from a reparse one grounded neutral longer. The
    /// first write of such a model is not a fixed point; the second is.
    fn warn_short_map(&mut self, class: &str, name: &str, map_len: usize, nconds: usize) {
        if map_len < nconds {
            self.warn(format!(
                "{class} {name}: terminal map lists {map_len} of {nconds} conductors; \
                 dss materializes a grounded neutral terminal and the reparsed model \
                 gains one"
            ));
        }
    }

    /// A numeric source extra. A present token that does not parse warns;
    /// the derived value substitutes and the extra is consumed either way.
    fn source_extra_f64(&mut self, vs: &crate::model::VoltageSource, key: &str) -> Option<f64> {
        let v = vs.extras.get(key)?;
        let parsed = v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        if parsed.is_none() {
            self.warn(format!(
                "vsource {}: {key} extra `{v}` does not parse as a number; \
                 using the derived value",
                vs.name
            ));
        }
        parsed
    }

    fn line_out(&mut self, s: &str) {
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn check_name(&mut self, class: &str, name: &str) {
        if name_breaks_dss(name, false) {
            self.warn(format!(
                "{class} `{name}`: name contains characters dss cannot represent; \
                 output will not reparse identically"
            ));
        }
    }

    /// `bus.1.2.0` syntax: terminals in the bus's perfectly grounded set
    /// emit as node 0, the inverse of the reader's neutral naming. dss
    /// nodes are positional integers, so a non numeric terminal name emits
    /// as its 1 based position on the bus (the element map position when
    /// the bus does not list it), reported, keeping the conductor structure
    /// intact across the trip.
    fn bus_ref(&mut self, bus: &str, map: &[String]) -> String {
        let key = bus.to_ascii_lowercase();
        if name_breaks_dss(bus, true) {
            self.warn(format!(
                "bus `{bus}`: id contains characters dss cannot represent; \
                 output will not reparse identically"
            ));
        }
        let grounded = self.grounded.get(&key).cloned();
        let terminals = self.terminals.get(&key).cloned().unwrap_or_default();
        let nodes: Vec<String> = map
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if grounded.as_ref().is_some_and(|g| g.contains(t)) {
                    "0".to_string()
                } else if t.parse::<u32>().is_ok() {
                    t.clone()
                } else {
                    let pos = terminals.iter().position(|x| x == t).unwrap_or(i) + 1;
                    self.warn(format!(
                        "bus {bus}: terminal `{t}` is not a dss node number; \
                         emitted as node {pos}, its position on the bus"
                    ));
                    pos.to_string()
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
    fn extras_tail(&mut self, class: &str, name: &str, extras: &Extras) -> String {
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
                    let (out, representable) = dss_value_out(&text);
                    if !representable {
                        self.warn(format!(
                            "{class} {name}: extra `{key}` value `{text}` contains every \
                             dss quote closer and splits when scanned bare; emitted as \
                             written and a reparse will not see the same value"
                        ));
                    }
                    let _ = write!(tail, " {key}={out}");
                }
                _ => self.warn(format!(
                    "{class} {name}: extra `{key}` is not a dss property; dropped from the output"
                )),
            }
        }
        tail
    }

    /// Lower triangle matrix text. Rows shorter than the triangle pad
    /// with 0 instead of panicking, and the padding is reported.
    fn matrix_arg(&mut self, m: &Mat, what: &str) -> String {
        let mut short = false;
        let rows: Vec<String> = m
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let take = row.len().min(i + 1);
                let mut vals: Vec<String> = row[..take].iter().map(|v| num(*v)).collect();
                if take < i + 1 {
                    short = true;
                    vals.resize(i + 1, "0".to_string());
                }
                vals.join(" ")
            })
            .collect();
        if short {
            self.warn(format!(
                "{what}: matrix rows are shorter than the lower triangle; \
                 missing entries emitted as 0"
            ));
        }
        format!("({})", rows.join(" | "))
    }

    /// Consumes an rs/xs extras pair only when both first rows parse; a
    /// half present or unusable pair stays in extras and is reported.
    fn take_seq_pair(
        &mut self,
        extras: &mut Extras,
        r_key: &str,
        x_key: &str,
        what: &str,
    ) -> Option<((f64, f64), (f64, f64))> {
        let r = seq_parts(extras, r_key);
        let x = seq_parts(extras, x_key);
        if let (Some(r), Some(x)) = (r, x) {
            extras.remove(r_key);
            extras.remove(x_key);
            return Some((r, x));
        }
        if extras.contains_key(r_key) || extras.contains_key(x_key) {
            let state = |key: &str, parsed: bool| {
                if !extras.contains_key(key) {
                    format!("`{key}` is missing")
                } else if parsed {
                    format!("`{key}` is usable")
                } else {
                    format!("`{key}` is not a numeric matrix")
                }
            };
            self.warn(format!(
                "{what}: series impedance extras unusable ({}, {}); left in extras",
                state(r_key, r.is_some()),
                state(x_key, x.is_some()),
            ));
        }
        None
    }

    /// Emitted `phases=`: the reader's stash when present, otherwise
    /// inferred from the terminal map shape. A delta map with 3 conductors
    /// is 2 or 3 phase; without the stash the 3 phase reading wins, loudly.
    fn element_phases(
        &mut self,
        extras: &Extras,
        terminal_map: &[String],
        configuration: Configuration,
        class: &str,
        name: &str,
    ) -> usize {
        if let Some(p) = extras_usize(extras, "phases") {
            return p.max(1);
        }
        match configuration {
            Configuration::Delta => match terminal_map.len() {
                2 => 1,
                3 => {
                    self.warn(format!(
                        "{class} {name}: a delta terminal map with 3 conductors is 2 or 3 \
                         phase and no phases record disambiguates; emitted phases=3"
                    ));
                    3
                }
                n => {
                    self.warn(format!(
                        "{class} {name}: a delta terminal map with {n} conductors has no \
                         dss phases mapping; emitted phases={}",
                        n.max(1)
                    ));
                    n.max(1)
                }
            },
            Configuration::Wye => terminal_map.len().saturating_sub(1).max(1),
            _ => 1,
        }
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
        // Source options re-emit in stored order, except the keys this
        // writer derives itself (the DefaultBaseFrequency header, the
        // VoltageBases tail). Commands do not re-emit: their position in
        // the script matters and the canonical element order does not
        // preserve it, so each drop is reported instead.
        for (key, value) in &net.options {
            if key.is_empty() {
                self.warn(format!(
                    "option `{value}` has no name; not regenerated in canonical dss output"
                ));
                continue;
            }
            // The engine resolves Set names exact-then-unique-prefix
            // (Command.cpp Getcommand → HashList FindAbbrev), so `Set volt=`
            // already IS Voltagebases and `Set defaultb=` DefaultBaseFrequency.
            // Very short prefixes bind by the engine's option table order;
            // this check intentionally covers only the three keys the writer
            // derives itself.
            let key_lc = key.to_ascii_lowercase();
            if ["voltagebases", "defaultbasefrequency", "calcvoltagebases"]
                .iter()
                .any(|derived| derived.starts_with(&key_lc))
            {
                continue;
            }
            let (text, representable) = dss_value_out(value);
            if !representable {
                self.warn(format!(
                    "option `{key}`: value `{value}` contains every dss quote closer \
                     and splits when scanned bare; emitted as written and a reparse \
                     will not see the same value"
                ));
            }
            self.line_out(&format!("Set {key}={text}"));
        }
        for (verb, args) in &net.commands {
            if verb.eq_ignore_ascii_case("calcvoltagebases") || verb.eq_ignore_ascii_case("solve") {
                continue; // the tail emits these
            }
            let shown = if args.is_empty() {
                verb.clone()
            } else {
                format!("{verb} {args}")
            };
            self.warn(format!(
                "command `{shown}` is not regenerated in canonical dss output"
            ));
        }
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
            let phases = source_phases(net, vs);
            let energized = vs.v_magnitude.iter().filter(|&&v| v > 0.0).count();
            if energized > 0 && energized != phases {
                self.warn(format!(
                    "vsource {}: emitted phases={phases} but {energized} v_magnitude \
                     entries are positive; a reparse energizes all {phases}",
                    vs.name
                ));
            }
            self.warn_short_map("vsource", &vs.name, vs.terminal_map.len(), phases + 1);
            let basekv = self
                .source_extra_f64(vs, "basekv")
                .unwrap_or_else(|| source_basekv(vs, phases));
            let pu = self.source_extra_f64(vs, "pu").unwrap_or(1.0);
            let angle = self
                .source_extra_f64(vs, "angle")
                .unwrap_or_else(|| vs.v_angle.first().copied().unwrap_or(0.0).to_degrees());
            let head = if i == 0 {
                let name = net.name.clone().unwrap_or_else(|| "converted".into());
                self.check_name("circuit", &name);
                format!("New Circuit.{name}")
            } else {
                self.check_name("vsource", &vs.name);
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
            extras.remove("phases"); // the head already prints phases=
            // A source that came through the ENGINEERING model carries its
            // Thevenin impedance as rs/xs matrices; sequence values
            // reconstruct exactly (z1 = self - mutual, z0 = self + 2 mutual).
            let what = format!("vsource {}", vs.name);
            if let Some(((rs, rm), (xs, xm))) = self.take_seq_pair(&mut extras, "rs", "xs", &what) {
                // Lowercase keys in sorted order: a reparse keeps these in
                // extras and the next write emits them from there verbatim.
                let _ = write!(
                    s,
                    " z0=({}, {}) z1=({}, {})",
                    num(rs + 2.0 * rm),
                    num(xs + 2.0 * xm),
                    num(rs - rm),
                    num(xs - xm)
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
            self.check_name("linecode", &c.name);
            let n = c.n_conductors;
            let what = format!("linecode {}", c.name);
            let mut s = format!("New Linecode.{} nphases={n} units=m", c.name);
            let rm = self.matrix_arg(&c.r_series, &what);
            let _ = write!(s, " rmatrix={rm}");
            let xm = self.matrix_arg(&c.x_series, &what);
            let _ = write!(s, " xmatrix={xm}");
            // cmatrix in nF per meter: each half is omega C / 2, so
            // C_nF = 2 b / (omega 1e-9).
            let c_nf: Mat = c
                .b_from
                .iter()
                .map(|row| row.iter().map(|b| 2.0 * b / omega_nf).collect())
                .collect();
            let cm = self.matrix_arg(&c_nf, &what);
            let _ = write!(s, " cmatrix={cm}");
            match c.i_max.as_deref() {
                Some([amps, ..]) => {
                    let _ = write!(s, " emergamps={}", num(*amps));
                }
                Some([]) => self.warn(format!(
                    "linecode {}: i_max is empty; emergamps not emitted",
                    c.name
                )),
                None => {}
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
            self.check_name("line", &l.name);
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
            self.check_name("line", &sw.name);
            let phases = sw.terminal_map_from.len();
            let mut s = format!(
                "New Line.{} bus1={} bus2={} phases={phases} switch=y",
                sw.name,
                self.bus_ref(&sw.bus_from, &sw.terminal_map_from),
                self.bus_ref(&sw.bus_to, &sw.terminal_map_to),
            );
            match sw.i_max.as_deref() {
                Some([amps, ..]) => {
                    let _ = write!(s, " emergamps={}", num(*amps));
                }
                Some([]) => self.warn(format!(
                    "line {}: i_max is empty; emergamps not emitted",
                    sw.name
                )),
                None => {}
            }
            // A switch that came through the ENGINEERING model carries its
            // total series matrices; sequence overrides reproduce them over
            // the forced 0.001 length (the engine's switch dummy values
            // would otherwise apply).
            let mut extras = sw.extras.clone();
            let what = format!("line {}", sw.name);
            if let Some(((rs, rm), (xs, xm))) =
                self.take_seq_pair(&mut extras, "pmd_rs", "pmd_xs", &what)
            {
                let _ = write!(
                    s,
                    " c0=0 c1=0 r0={} r1={} x0={} x1={}",
                    num((rs + 2.0 * rm) / 0.001),
                    num((rs - rm) / 0.001),
                    num((xs + 2.0 * xm) / 0.001),
                    num((xs - xm) / 0.001)
                );
            }
            s.push_str(&self.extras_tail("line", &sw.name, &extras));
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
            self.check_name("transformer", &t.name);
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
            if let Some(xhl) = t.xsc_pct.first() {
                let _ = write!(s, " xhl={}", num(*xhl));
                if t.xsc_pct.len() >= 3 {
                    let _ = write!(s, " xht={} xlt={}", num(t.xsc_pct[1]), num(t.xsc_pct[2]));
                }
            } else {
                self.warn(format!(
                    "transformer {}: xsc_pct is empty; emitted xhl=0",
                    t.name
                ));
                s.push_str(" xhl=0");
            }
            s.push_str(&self.extras_tail("transformer", &t.name, &t.extras));
            self.line_out(&s);
        }
        self.out.push('\n');
    }

    fn loads(&mut self, net: &DistNetwork) {
        for l in &net.loads {
            self.check_name("load", &l.name);
            let phases =
                self.element_phases(&l.extras, &l.terminal_map, l.configuration, "load", &l.name);
            let conn = element_conn(&l.extras, l.configuration);
            // The reader's nconds: a 3 phase delta has no neutral conductor,
            // every other connection carries phases + 1.
            let nconds = if conn == "delta" && phases == 3 {
                phases
            } else {
                phases + 1
            };
            self.warn_short_map("load", &l.name, l.terminal_map.len(), nconds);
            let kw: f64 = l.p_nom.iter().sum::<f64>() / 1e3;
            let kvar: f64 = l.q_nom.iter().sum::<f64>() / 1e3;
            let kv = self.element_kv(&l.extras, &l.bus, phases, l.configuration, &l.name, "load");
            let mut extras = l.extras.clone();
            extras.remove("kv");
            extras.remove("phases");
            extras.remove("conn");
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
        extras: &Extras,
        bus: &str,
        phases: usize,
        configuration: Configuration,
        name: &str,
        class: &str,
    ) -> f64 {
        if let Some(v) = extras.get("kv") {
            match v
                .as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            {
                Some(kv) => return kv,
                None => self.warn(format!(
                    "{class} {name}: kv extra `{v}` does not parse as a number; \
                     using the bus voltage estimate"
                )),
            }
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
            self.check_name("capacitor", &sh.name);
            let phases = extras_usize(&sh.extras, "phases").unwrap_or(sh.terminal_map.len());
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
            let kvar = extras_f64(&sh.extras, "kvar").unwrap_or_else(|| {
                // The reader's wye capacitor convention: line to line kv
                // for 2 and 3 phase, line to neutral for single phase.
                let v_phase = if matches!(phases, 2 | 3) {
                    kv * 1e3 / 3f64.sqrt()
                } else {
                    kv * 1e3
                };
                b_phase * v_phase * v_phase * phases as f64 / 1e3
            });
            let mut extras = sh.extras.clone();
            extras.remove("kv");
            extras.remove("kvar");
            extras.remove("phases");
            extras.remove("conn");
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
            self.check_name("generator", &g.name);
            let phases = self.element_phases(
                &g.extras,
                &g.terminal_map,
                g.configuration,
                "generator",
                &g.name,
            );
            let conn = element_conn(&g.extras, g.configuration);
            let nconds = if conn == "delta" && phases == 3 {
                phases
            } else {
                phases + 1
            };
            self.warn_short_map("generator", &g.name, g.terminal_map.len(), nconds);
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
            extras.remove("phases");
            extras.remove("conn");
            s.push_str(&self.extras_tail("generator", &g.name, &extras));
            self.line_out(&s);
        }
    }
}

/// Emitted `conn=`: delta for a typed delta, and for a single phase
/// element whose stashed conn token was delta (the reader types 1 phase
/// delta as `SinglePhase`, which would otherwise re-emit as wye).
fn element_conn(extras: &Extras, configuration: Configuration) -> &'static str {
    let stash_delta = extras
        .get("conn")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t.to_ascii_lowercase().starts_with('d') || t.eq_ignore_ascii_case("ll"));
    match configuration {
        Configuration::Delta => "delta",
        Configuration::SinglePhase if stash_delta => "delta",
        _ => "wye",
    }
}

#[cfg(test)]
mod tests {
    use super::super::read::parse_dss_str;
    use super::*;
    use crate::model::{
        DistGenerator, DistLine, DistLineCode, DistLoad, DistShunt, DistSwitch, DistTransformer,
        VoltageSource, Winding,
    };

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    fn bus(id: &str, terminals: &[&str], grounded: &[&str]) -> DistBus {
        DistBus {
            id: id.into(),
            terminals: strings(terminals),
            grounded: strings(grounded),
            ..DistBus::default()
        }
    }

    fn three_phase_source(vln: f64) -> (DistBus, VoltageSource) {
        let third = 2.0 * std::f64::consts::FRAC_PI_3;
        (
            bus("sb", &["1", "2", "3", "4"], &["4"]),
            VoltageSource {
                name: "source".into(),
                bus: "sb".into(),
                terminal_map: strings(&["1", "2", "3", "4"]),
                v_magnitude: vec![vln, vln, vln, 0.0],
                v_angle: vec![0.0, -third, third, 0.0],
                extras: Extras::new(),
            },
        )
    }

    fn load_on(bus: &str, map: &[&str], configuration: Configuration) -> DistLoad {
        let phases = map.len();
        DistLoad {
            name: "ld".into(),
            bus: bus.into(),
            terminal_map: strings(map),
            configuration,
            p_nom: vec![1e3; phases],
            q_nom: vec![0.0; phases],
            extras: Extras::from([("kv".to_string(), serde_json::json!("0.4"))]),
        }
    }

    fn roundtrip(net: &DistNetwork) -> (String, String) {
        let first = write_dss(net);
        let second = write_dss(&parse_dss_str(&first.text));
        (first.text, second.text)
    }

    #[test]
    fn voltage_bases_survive_the_sqrt_round_trip() {
        // basekv = vln*sqrt(3)/1e3 then vln' = basekv*1e3/sqrt(3) is not a
        // float fixed point for this PMD shaped value; the second write must
        // reuse the stashed basekv instead of re-deriving the entry.
        let vln = 9_336.235_056_420_312_f64;
        let basekv = vln * 3f64.sqrt() / 1e3;
        assert!(
            (basekv * 1e3 / 3f64.sqrt()).to_bits() != vln.to_bits(),
            "test value no longer reproduces the drift"
        );
        let (b, vs) = three_phase_source(vln);
        let net = DistNetwork {
            name: Some("t".into()),
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ..DistNetwork::default()
        };
        let (first, second) = roundtrip(&net);
        assert!(first.contains("Set VoltageBases="), "{first}");
        assert_eq!(first, second);
    }

    #[test]
    fn load_phases_prefer_the_reader_stash() {
        let (b, vs) = three_phase_source(2400.0);
        let mut load = load_on("sb", &["1", "2", "3"], Configuration::Delta);
        load.extras.insert("phases".into(), serde_json::json!("2"));
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        let line = out.text.lines().find(|l| l.contains("Load.ld")).unwrap();
        assert!(line.contains("phases=2 conn=delta"), "{line}");
        // The stash must not double emit through the extras tail.
        assert_eq!(line.matches("phases=").count(), 1, "{line}");
        assert!(!out.warnings.iter().any(|w| w.contains("2 or 3 phase")));
    }

    #[test]
    fn ambiguous_delta_keeps_three_phases_loudly() {
        let (b, vs) = three_phase_source(2400.0);
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load_on("sb", &["1", "2", "3"], Configuration::Delta)],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        let line = out.text.lines().find(|l| l.contains("Load.ld")).unwrap();
        assert!(line.contains("phases=3 conn=delta"), "{line}");
        assert!(
            out.warnings.iter().any(|w| w.contains("2 or 3 phase")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn single_phase_delta_emits_conn_delta() {
        let (b, vs) = three_phase_source(2400.0);
        // Two conductor delta typed as Delta: phases=1 conn=delta.
        let two_wire = load_on("sb", &["1", "2"], Configuration::Delta);
        // The reader types 1 phase delta as SinglePhase; the stashed conn
        // token carries the delta.
        let mut stashed = load_on("sb", &["1", "2"], Configuration::SinglePhase);
        stashed.name = "ld2".into();
        stashed
            .extras
            .insert("conn".into(), serde_json::json!("delta"));
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![two_wire, stashed],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        let l1 = out.text.lines().find(|l| l.contains("Load.ld ")).unwrap();
        assert!(l1.contains("phases=1 conn=delta"), "{l1}");
        let l2 = out.text.lines().find(|l| l.contains("Load.ld2 ")).unwrap();
        assert!(l2.contains("phases=1 conn=delta"), "{l2}");
        assert_eq!(l2.matches("conn=").count(), 1, "{l2}");
    }

    #[test]
    fn unrepresentable_names_are_reported() {
        let (b, vs) = three_phase_source(2400.0);
        let mut load = load_on("sb", &["1", "2", "3", "4"], Configuration::Wye);
        load.name = "load 1".into();
        let net = DistNetwork {
            name: Some("my circuit".into()),
            base_frequency: 60.0,
            buses: vec![b, bus("a=b", &["1"], &[])],
            sources: vec![vs],
            loads: vec![load],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        let hits = |needle: &str| {
            out.warnings
                .iter()
                .any(|w| w.contains(needle) && w.contains("cannot represent"))
        };
        assert!(hits("load 1"), "{:?}", out.warnings);
        assert!(hits("my circuit"), "{:?}", out.warnings);
        // The bad bus id warns at its bus_ref emission site.
        let mut net2 = net.clone();
        net2.lines.push(DistLine {
            name: "l1".into(),
            bus_from: "sb".into(),
            bus_to: "a=b".into(),
            terminal_map_from: strings(&["1"]),
            terminal_map_to: strings(&["1"]),
            linecode: "lc".into(),
            length: 1.0,
            extras: Extras::new(),
        });
        let out2 = write_dss(&net2);
        assert!(
            out2.warnings
                .iter()
                .any(|w| w.contains("a=b") && w.contains("cannot represent")),
            "{:?}",
            out2.warnings
        );
    }

    #[test]
    fn unparseable_kv_extra_warns_instead_of_silently_substituting() {
        let (b, vs) = three_phase_source(2400.0);
        let mut load = load_on("sb", &["1", "2", "3", "4"], Configuration::Wye);
        load.extras.insert("kv".into(), serde_json::json!("@kv"));
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("@kv") && w.contains("does not parse")),
            "{:?}",
            out.warnings
        );
        // The estimate substitutes: 2400*sqrt(3)/1e3 line to line.
        let line = out.text.lines().find(|l| l.contains("Load.ld")).unwrap();
        assert!(
            line.contains(&format!("kv={}", num(2400.0 * 3f64.sqrt() / 1e3))),
            "{line}"
        );
    }

    #[test]
    fn options_reemit_and_commands_warn() {
        let src = "Clear\n\
                   New Circuit.c1 basekv=12.47 pu=1 angle=0 phases=3 bus1=sb\n\
                   Set mode=snapshot\n\
                   Set controlmode=OFF\n\
                   Disable Line.l1\n\
                   Set VoltageBases=[12.47]\n\
                   Calcvoltagebases\n\
                   Solve\n";
        let out = write_dss(&parse_dss_str(src));
        assert!(out.text.contains("Set mode=snapshot"), "{}", out.text);
        assert!(out.text.contains("Set controlmode=OFF"), "{}", out.text);
        // The writer derives these; the stored options must not double them.
        assert_eq!(out.text.matches("Set VoltageBases").count(), 1);
        assert_eq!(out.text.matches("Calcvoltagebases").count(), 1);
        assert_eq!(out.text.matches("DefaultBaseFrequency").count(), 1);
        assert!(!out.text.to_lowercase().contains("disable"));
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("disable Line.l1") && w.contains("not regenerated")),
            "{:?}",
            out.warnings
        );
        // Solve and Calcvoltagebases re-derive; no warning claims they drop.
        assert!(!out.warnings.iter().any(|w| w.contains("`solve`")));
        let again = write_dss(&parse_dss_str(&out.text));
        assert_eq!(out.text, again.text);
    }

    #[test]
    fn non_numeric_terminal_positionalizes() {
        let mut load = load_on("b1", &["a", "n"], Configuration::Wye);
        load.extras.insert("kv".into(), serde_json::json!("0.23"));
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![bus("b1", &["a", "n"], &["n"])],
            loads: vec![load],
            ..DistNetwork::default()
        };
        let (first, second) = roundtrip(&net);
        let line = first.lines().find(|l| l.contains("Load.ld")).unwrap();
        assert!(line.contains("bus1=b1.1.0"), "{line}");
        let out = write_dss(&net);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("`a`") && w.contains("position")),
            "{:?}",
            out.warnings
        );
        assert_eq!(first, second);
    }

    #[test]
    fn half_present_thevenin_pair_stays_and_warns() {
        let (b, mut vs) = three_phase_source(2400.0);
        vs.extras
            .insert("rs".into(), serde_json::json!([[1.0, 0.1], [0.1, 1.0]]));
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        assert!(!out.text.contains("z1="), "{}", out.text);
        assert!(
            out.warnings.iter().any(|w| w.contains("`xs` is missing")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn unusable_switch_sequence_extras_warn() {
        let (b, vs) = three_phase_source(2400.0);
        let sw = DistSwitch {
            name: "sw1".into(),
            bus_from: "sb".into(),
            bus_to: "b2".into(),
            terminal_map_from: strings(&["1", "2", "3"]),
            terminal_map_to: strings(&["1", "2", "3"]),
            open: false,
            i_max: Some(Vec::new()),
            extras: Extras::from([("pmd_rs".to_string(), serde_json::json!("oops"))]),
        };
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b, bus("b2", &["1", "2", "3"], &[])],
            sources: vec![vs],
            switches: vec![sw],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        assert!(!out.text.contains("r0="), "{}", out.text);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("pmd_rs") && w.contains("not a numeric matrix")),
            "{:?}",
            out.warnings
        );
        assert!(
            out.warnings.iter().any(|w| w.contains("i_max is empty")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn degenerate_shapes_warn_instead_of_panicking() {
        let (b, vs) = three_phase_source(2400.0);
        let lc = DistLineCode {
            name: "lc1".into(),
            n_conductors: 2,
            r_series: vec![vec![1.0], vec![0.5]], // second row short
            x_series: vec![vec![1.0, 0.0], vec![0.0, 1.0]],
            g_from: vec![vec![0.0; 2]; 2],
            b_from: vec![vec![0.0; 2]; 2],
            g_to: vec![vec![0.0; 2]; 2],
            b_to: vec![vec![0.0; 2]; 2],
            i_max: Some(Vec::new()),
            s_max: None,
            extras: Extras::new(),
        };
        let t = DistTransformer {
            name: "t1".into(),
            windings: vec![
                Winding {
                    bus: "sb".into(),
                    terminal_map: strings(&["1", "2"]),
                    conn: WindingConn::Wye,
                    v_ref: 2400.0,
                    s_rating: 25e3,
                    r_pct: 0.5,
                    tap: 1.0,
                },
                Winding {
                    bus: "b2".into(),
                    terminal_map: strings(&["1", "2"]),
                    conn: WindingConn::Wye,
                    v_ref: 240.0,
                    s_rating: 25e3,
                    r_pct: 0.5,
                    tap: 1.0,
                },
            ],
            xsc_pct: Vec::new(),
            phases: 1,
            extras: Extras::new(),
        };
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b, bus("b2", &["1", "2"], &[])],
            sources: vec![vs],
            linecodes: vec![lc],
            transformers: vec![t],
            ..DistNetwork::default()
        };
        let out = write_dss(&net); // must not panic
        assert!(out.text.contains("rmatrix=(1 | 0.5 0)"), "{}", out.text);
        assert!(out.text.contains("xhl=0"), "{}", out.text);
        let has = |needle: &str| out.warnings.iter().any(|w| w.contains(needle));
        assert!(has("shorter than the lower triangle"), "{:?}", out.warnings);
        assert!(has("xsc_pct is empty"), "{:?}", out.warnings);
        assert!(has("i_max is empty"), "{:?}", out.warnings);
    }

    #[test]
    fn two_phase_capacitor_kvar_uses_line_to_line_kv() {
        // The reader treats wye capacitor kv as line to line for 2 and 3
        // phase; the kvar fallback must invert with the same convention.
        let (b, vs) = three_phase_source(2400.0);
        let b_phase = 1e-3;
        let sh = DistShunt {
            name: "c1".into(),
            bus: "sb".into(),
            terminal_map: strings(&["1", "2"]),
            g: vec![vec![0.0; 2]; 2],
            b: vec![vec![b_phase, 0.0], vec![0.0, b_phase]],
            extras: Extras::new(),
        };
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            shunts: vec![sh],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        let kv = 2400.0 * 3f64.sqrt() / 1e3;
        let v_phase = kv * 1e3 / 3f64.sqrt();
        let expected = b_phase * v_phase * v_phase * 2.0 / 1e3;
        let line = out
            .text
            .lines()
            .find(|l| l.contains("Capacitor.c1"))
            .unwrap();
        assert!(line.contains(&format!("kvar={}", num(expected))), "{line}");
    }

    #[test]
    fn option_values_choose_a_wrapper_the_lexer_undoes() {
        let src = "Clear\n\
                   New Circuit.c1 basekv=12.47 pu=1 angle=0 phases=3 bus1=sb\n\
                   Set foo=[a!b]\n\
                   Set bar=[(abc]\n\
                   Set baz=(x ] y)\n\
                   Set qux=[a ) b]\n\
                   Solve\n";
        let net = parse_dss_str(src);
        let first = write_dss(&net);
        for line in [
            "Set foo=(a!b)",
            "Set bar=((abc)",
            "Set baz=(x ] y)",
            "Set qux=[a ) b]",
        ] {
            assert!(
                first.text.contains(line),
                "{line} missing in {}",
                first.text
            );
        }
        assert!(
            !first
                .warnings
                .iter()
                .any(|w| w.contains("emitted as written")),
            "{:?}",
            first.warnings
        );
        // The reader strips the wrapper back off...
        let reparsed = parse_dss_str(&first.text);
        let opt = |k: &str| {
            reparsed
                .options
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(opt("foo"), Some("a!b"));
        assert_eq!(opt("bar"), Some("(abc"));
        assert_eq!(opt("baz"), Some("x ] y"));
        assert_eq!(opt("qux"), Some("a ) b"));
        // ...and the second write picks the same wrapper from the bare value.
        let second = write_dss(&reparsed);
        assert_eq!(first.text, second.text);
    }

    #[test]
    fn extras_tail_values_wrap_like_options() {
        let (b, vs) = three_phase_source(2400.0);
        let mut load = load_on("sb", &["1", "2", "3", "4"], Configuration::Wye);
        load.extras
            .insert("daily".into(), serde_json::json!("a ) b"));
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..DistNetwork::default()
        };
        let (first, second) = roundtrip(&net);
        // A paren wrapper would close at the `)` and land `b)` on the next
        // positional property (duty); brackets survive.
        assert!(first.contains("daily=[a ) b]"), "{first}");
        assert_eq!(first, second);
        let back = parse_dss_str(&first);
        assert_eq!(
            back.loads[0]
                .extras
                .get("daily")
                .and_then(serde_json::Value::as_str),
            Some("a ) b")
        );
    }

    #[test]
    fn unrepresentable_values_emit_as_written_and_warn() {
        // Every quote closer appears, and the spaces split a bare scan: no
        // emitted form reparses to this value.
        let bad = "a )]}\"' b";
        let (b, vs) = three_phase_source(2400.0);
        let mut load = load_on("sb", &["1", "2", "3", "4"], Configuration::Wye);
        load.extras.insert("daily".into(), serde_json::json!(bad));
        let mut net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..DistNetwork::default()
        };
        net.options.push(("foo".into(), bad.into()));
        let out = write_dss(&net);
        assert!(out.text.contains(&format!("Set foo={bad}")), "{}", out.text);
        assert!(out.text.contains(&format!("daily={bad}")), "{}", out.text);
        let warned = |needle: &str| {
            out.warnings
                .iter()
                .any(|w| w.contains(needle) && w.contains("emitted as written"))
        };
        assert!(warned("option `foo`"), "{:?}", out.warnings);
        assert!(warned("`daily`"), "{:?}", out.warnings);
    }

    #[test]
    fn abbreviated_derived_options_skip_and_set_the_frequency() {
        // The engine resolves Set names by unique prefix, so volt= IS
        // Voltagebases and defaultb= IS DefaultBaseFrequency.
        let src = "Clear\n\
                   New Circuit.c1 basekv=12.47 pu=1 angle=0 phases=3 bus1=sb\n\
                   Set volt=[115, 132]\n\
                   Set defaultb=50\n\
                   Solve\n";
        let net = parse_dss_str(src);
        assert!((net.base_frequency - 50.0).abs() < 1e-12);
        let out = write_dss(&net);
        assert!(
            out.text.contains("Set DefaultBaseFrequency=50"),
            "{}",
            out.text
        );
        assert_eq!(
            out.text
                .to_lowercase()
                .matches("defaultbasefrequency")
                .count(),
            1,
            "{}",
            out.text
        );
        assert_eq!(
            out.text.matches("Set VoltageBases").count(),
            1,
            "{}",
            out.text
        );
        assert!(!out.text.contains("Set volt="), "{}", out.text);
        assert!(!out.text.contains("Set defaultb="), "{}", out.text);
        let second = write_dss(&parse_dss_str(&out.text));
        assert_eq!(out.text, second.text);
    }

    #[test]
    fn non_numeric_source_extras_warn_before_falling_back() {
        let (b, mut vs) = three_phase_source(2400.0);
        vs.extras
            .insert("basekv".into(), serde_json::json!("@base"));
        vs.extras.insert("pu".into(), serde_json::json!("unity"));
        vs.extras.insert("angle".into(), serde_json::json!([0.0]));
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        for key in ["basekv", "pu", "angle"] {
            assert!(
                out.warnings
                    .iter()
                    .any(|w| w.contains(&format!("{key} extra")) && w.contains("does not parse")),
                "{key}: {:?}",
                out.warnings
            );
        }
        // The derived values substitute.
        let line = out.text.lines().find(|l| l.contains("Circuit.")).unwrap();
        assert!(line.contains("pu=1 angle=0"), "{line}");
    }

    #[test]
    fn de_energized_source_phase_keeps_its_conductor() {
        let (b, mut vs) = three_phase_source(2400.0);
        vs.v_magnitude[2] = 0.0; // de-energized, but still a phase conductor
        let net = DistNetwork {
            name: Some("t".into()),
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ..DistNetwork::default()
        };
        let (first, second) = roundtrip(&net);
        let line = first.lines().find(|l| l.contains("Circuit.")).unwrap();
        // phases=2 against the 4 node dot list would drop a node on reparse.
        assert!(line.contains("phases=3"), "{line}");
        assert!(line.contains("bus1=sb.1.2.3.0"), "{line}");
        assert_eq!(first, second);
        let out = write_dss(&net);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("phases=3") && w.contains("positive")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn source_phases_stash_wins_and_does_not_double_emit() {
        let (b, mut vs) = three_phase_source(2400.0);
        vs.extras.insert("phases".into(), serde_json::json!("3"));
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        let line = out.text.lines().find(|l| l.contains("Circuit.")).unwrap();
        assert!(line.contains("phases=3"), "{line}");
        assert_eq!(line.matches("phases=").count(), 1, "{line}");
    }

    #[test]
    fn foreign_maps_without_a_neutral_warn_and_converge_at_write2() {
        // A vsource/wye load map with no grounded terminal: the engine's
        // nconds fill extends the reparsed bus with a grounded neutral, so
        // write1 is not a fixed point. The writer must say so.
        let third = 2.0 * std::f64::consts::FRAC_PI_3;
        let vs = VoltageSource {
            name: "source".into(),
            bus: "sb".into(),
            terminal_map: strings(&["1", "2", "3"]),
            v_magnitude: vec![2400.0; 3],
            v_angle: vec![0.0, -third, third],
            extras: Extras::new(),
        };
        let load = load_on("sb", &["1"], Configuration::Wye);
        let net = DistNetwork {
            name: Some("t".into()),
            base_frequency: 60.0,
            buses: vec![bus("sb", &["1", "2", "3"], &[])],
            sources: vec![vs],
            loads: vec![load],
            ..DistNetwork::default()
        };
        let first = write_dss(&net);
        let hits = |warnings: &[String], name: &str| {
            warnings
                .iter()
                .any(|w| w.contains(name) && w.contains("materializes a grounded neutral"))
        };
        assert!(
            hits(&first.warnings, "vsource source"),
            "{:?}",
            first.warnings
        );
        assert!(hits(&first.warnings, "load ld"), "{:?}", first.warnings);
        let second = write_dss(&parse_dss_str(&first.text));
        assert_ne!(first.text, second.text);
        assert!(!hits(&second.warnings, "vsource"), "{:?}", second.warnings);
        assert!(!hits(&second.warnings, "load"), "{:?}", second.warnings);
        let third_write = write_dss(&parse_dss_str(&second.text));
        assert_eq!(second.text, third_write.text);
    }

    #[test]
    fn generator_phases_and_conn_match_the_load_rules() {
        let (b, vs) = three_phase_source(2400.0);
        let g = DistGenerator {
            name: "g1".into(),
            bus: "sb".into(),
            terminal_map: strings(&["1", "2", "3"]),
            configuration: Configuration::Delta,
            p_nom: vec![1e3; 3],
            q_nom: vec![0.0; 3],
            p_min: None,
            p_max: None,
            q_min: None,
            q_max: None,
            cost: None,
            extras: Extras::from([
                ("kv".to_string(), serde_json::json!("4.16")),
                ("phases".to_string(), serde_json::json!("2")),
            ]),
        };
        let net = DistNetwork {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            generators: vec![g],
            ..DistNetwork::default()
        };
        let out = write_dss(&net);
        let line = out
            .text
            .lines()
            .find(|l| l.contains("Generator.g1"))
            .unwrap();
        assert!(line.contains("phases=2 conn=delta"), "{line}");
        assert_eq!(line.matches("phases=").count(), 1, "{line}");
    }
}

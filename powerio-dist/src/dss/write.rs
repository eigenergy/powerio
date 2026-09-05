//! [`MulticonductorNetwork`] into OpenDSS `.dss` text.
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

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::convert::{TextEmission, TextSidecar};
use crate::diagnostics::codes as C;
use crate::model::{
    ActivePowerReference, ConductorMatrix, Configuration, ControlVoltageReference, DistBus,
    DistControlProfile, DistIbr, DistLineCode, DistLoad, DistLoadVoltageModel, DistTransformer,
    DistWinding, DistWindingConn, Extras, IbrPrimeMover, IbrTopology, IbrVoltageAggregation,
    MulticonductorNetwork, ReactivePowerReference, VoltVarControl, VoltWattControl,
};

use super::read::delta_edges;
use super::{lex, prop};

/// Options for canonical OpenDSS output.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct DssEmitOptions {
    /// Default voltage validity band emitted on loads that do not already
    /// carry `vminpu` / `vmaxpu` extras.
    pub default_load_voltage_bounds: Option<DssLoadVoltageBounds>,
    /// Relative companion file named by the emitted `Buscoords` command.
    /// `None` drops typed bus locations with a warning.
    pub buscoords_filename: Option<String>,
}

impl Default for DssEmitOptions {
    fn default() -> Self {
        Self {
            default_load_voltage_bounds: Some(DssLoadVoltageBounds::default()),
            buscoords_filename: Some("buscoords.csv".to_owned()),
        }
    }
}

/// OpenDSS per unit load voltage validity band.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct DssLoadVoltageBounds {
    pub vminpu: f64,
    pub vmaxpu: f64,
}

impl Default for DssLoadVoltageBounds {
    fn default() -> Self {
        Self {
            vminpu: 0.0,
            vmaxpu: 2.0,
        }
    }
}

/// Emits canonical `.dss` text from the model.
#[cfg(test)]
pub(crate) fn emit_dss_text(net: &MulticonductorNetwork) -> TextEmission {
    emit_dss_text_with_options(net, &DssEmitOptions::default())
}

/// Emits canonical `.dss` text from the model with explicit options.
pub(crate) fn emit_dss_text_with_options(
    net: &MulticonductorNetwork,
    options: &DssEmitOptions,
) -> TextEmission {
    let mut w = DssWriter {
        out: String::new(),
        sidecars: Vec::new(),
        warnings: crate::diagnostics::Diagnostics::new(),
        options: options.clone(),
        grounded: net
            .buses()
            .iter()
            .map(|b| (b.id.to_ascii_lowercase(), b.grounded.clone()))
            .collect(),
        terminals: net
            .buses()
            .iter()
            .map(|b| (b.id.to_ascii_lowercase(), b.terminals.clone()))
            .collect(),
        kv_estimate: estimate_bus_kv(net),
    };
    w.network(net);
    TextEmission::new(w.out, w.sidecars, w.warnings)
}

struct DssWriter {
    out: String,
    sidecars: Vec<TextSidecar>,
    warnings: crate::diagnostics::Diagnostics,
    options: DssEmitOptions,
    /// Bus id (lowercase) → perfectly grounded terminal names.
    grounded: BTreeMap<String, Vec<String>>,
    /// Bus id (lowercase) → ordered terminal names.
    terminals: BTreeMap<String, Vec<String>>,
    /// Bus id (lowercase) → phase to neutral voltage estimate, volts.
    kv_estimate: BTreeMap<String, f64>,
}

#[derive(Clone, Copy)]
struct ElementKv<'a> {
    bus: &'a str,
    phases: usize,
    configuration: Configuration,
    name: &'a str,
    class: &'a str,
    typed_kv: Option<f64>,
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
fn estimate_bus_kv(net: &MulticonductorNetwork) -> BTreeMap<String, f64> {
    let mut kv: BTreeMap<String, f64> = BTreeMap::new();
    for vs in net.sources() {
        let phases = source_phases(net, vs);
        let basekv = extras_f64(&vs.extras, "basekv").unwrap_or_else(|| source_basekv(vs, phases));
        let pu = extras_f64(&vs.extras, "pu").unwrap_or(1.0);
        let vln = basekv * 1e3 * pu / source_chord(phases);
        if vln > 0.0 {
            kv.insert(vs.bus.to_ascii_lowercase(), vln);
        }
    }
    // Per bus grounded terminal sets, to tell a line to neutral winding (a
    // terminal tied to ground in its map) from a line to line one. Grounding
    // and the terminal map both survive a BMOPF round trip, the wye/delta
    // label does not, so this is what the transformer ratio keys on below.
    let grounded: BTreeMap<String, &Vec<String>> = net
        .buses()
        .iter()
        .map(|b| (b.id.to_ascii_lowercase(), &b.grounded))
        .collect();
    for _ in 0..net.buses().len() {
        let mut changed = false;
        for l in net.lines() {
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
        for s in net.switches() {
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
        for t in net.transformers() {
            // Propagate by winding voltage ratio from any known winding bus.
            // The bus map holds phase to neutral voltages, so each winding's
            // v_ref is first reduced to that base. A winding's rating is the
            // voltage across its two terminals: line to line when both are
            // phases (a polyphase winding, or a single phase delta leg), line
            // to neutral when one terminal is the bus's grounded neutral.
            // Matched windings (wye-wye, three phase wye-delta) cancel the
            // factor; only a mixed open delta leg (single phase wye to delta)
            // shifts, where the old raw ratio was a sqrt(3) off.
            let pn = |w: &DistWinding| {
                let v = (w.v_ref / 1e3) * 1e3;
                if winding_is_line_to_neutral(t.phases, w, |b| {
                    grounded.get(b).map(|g| g.as_slice())
                }) {
                    v
                } else {
                    v / 3f64.sqrt()
                }
            };
            let known: Option<(usize, f64)> = t
                .windings
                .iter()
                .enumerate()
                .find_map(|(i, w)| kv.get(&w.bus.to_ascii_lowercase()).map(|v| (i, *v)));
            if let Some((i, v_known)) = known {
                let pn_known = pn(&t.windings[i]);
                if pn_known > 0.0 {
                    for (j, w) in t.windings.iter().enumerate() {
                        if j != i && !kv.contains_key(&w.bus.to_ascii_lowercase()) {
                            kv.insert(w.bus.to_ascii_lowercase(), v_known * pn(w) / pn_known);
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

/// A float in the shortest form Rust round trips. Negative zero canonicalizes
/// to `0` so a `-x/denom` that lands on `-0.0` does not emit the literal `-0`.
/// Whether a winding's voltage sits line to neutral rather than line to line:
/// a single phase transformer whose winding lands on a grounded terminal of
/// its bus. Both the bus voltage estimate and the `kv=` token derived from it
/// read this rule, and they have to read the same one — a sqrt(3) disagreement
/// between them emits a wrong `kv` with nothing to flag it.
fn winding_is_line_to_neutral<'g>(
    phases: usize,
    w: &DistWinding,
    grounded: impl Fn(&str) -> Option<&'g [String]>,
) -> bool {
    phases < 2
        && grounded(&w.bus.to_ascii_lowercase())
            .is_some_and(|g| w.terminal_map.iter().any(|tm| g.contains(tm)))
}

/// Whether a value states a usable magnitude: a rating, a voltage, or an
/// ampacity a deck can carry. OpenDSS has no token for a nonfinite number, and
/// a zero or negative one is not a nameplate. Every recovery differs — omit the
/// property, derive from the bus estimate, drop the object — so this is the
/// shared question, not the shared answer.
fn is_positive_finite(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// The conductor count a dss element declares for `phases` on `conn`. A three
/// phase delta has no neutral conductor; every other connection carries one.
fn nconds_for(conn: &str, phases: usize) -> usize {
    if conn == "delta" && phases == 3 {
        phases
    } else {
        phases + 1
    }
}

/// Drop the extras the emitted record already states in its own tokens, so
/// `extras_tail` cannot write a second, stale copy of one.
fn strip_emitted_extras(extras: &mut Extras, keys: &[&str]) {
    for key in keys {
        extras.remove(*key);
    }
}

fn num(v: f64) -> String {
    let v = if v == 0.0 { 0.0 } else { v };
    format!("{v}")
}

/// Write one per-winding transformer property. The inline `(...)` form needs
/// a token in every slot. A missing value thus moves the property to the
/// per-winding `~ wdg=` edits, which can omit a winding.
fn winding_array(
    head: &mut String,
    edits: &mut [String],
    array_key: &str,
    scalar_key: &str,
    values: &[Option<f64>],
) {
    if values.iter().all(Option::is_some) {
        let toks: Vec<String> = values.iter().map(|v| num(v.unwrap_or(0.0))).collect();
        let _ = write!(head, " {array_key}=({})", toks.join(", "));
    } else {
        for (edit, v) in edits.iter_mut().zip(values) {
            if let Some(v) = v {
                let _ = write!(edit, " {scalar_key}={}", num(*v));
            }
        }
    }
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
        // A stashed `inf`/`NaN` token parses to a non-finite f64; reject it so
        // it never reaches `num()` and emits a literal `inf`/`NaN` DSS token.
        .filter(|f| f.is_finite())
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

fn zipv_cutoff(value: Option<&serde_json::Value>) -> Option<f64> {
    let text = value?.as_str()?;
    lex::Value::new(text)
        .to_vector(None)
        .ok()
        .and_then(|v| v.get(6).copied())
        .filter(|v| v.is_finite())
}

/// Whether the dss tokenizer would split this name: its delimiters, quote
/// pair characters, comment openers, and (in bus ids) the node dot.
fn name_breaks_dss(name: &str, is_bus_id: bool) -> bool {
    name.contains("//")
        || name.chars().any(|c| {
            // A line terminator does not shift a token, it ends the command
            // and makes the rest of the name parse as a new dss object.
            matches!(
                c,
                ' ' | '\t'
                    | '\n'
                    | '\r'
                    | ','
                    | '='
                    | '!'
                    | '"'
                    | '\''
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
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
    // An empty value is never bare representable: `key=` makes the lexer
    // eat the next token as the value. `()` strips back to the empty string.
    if value.is_empty() {
        return ("()".to_string(), true);
    }
    let mut scan = lex::Scanner::new(value, None);
    let bare = scan.next_param().is_some_and(|p| {
        p.name.is_none() && !p.value.quoted && p.value.text == value && scan.next_param().is_none()
    });
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
fn source_phases(net: &MulticonductorNetwork, vs: &crate::model::VoltageSource) -> usize {
    if let Some(p) = extras_usize(&vs.extras, "phases") {
        return p.max(1);
    }
    let energized = vs.v_magnitude.iter().filter(|&&v| v > 0.0).count();
    if energized > 0
        && vs.v_magnitude.len() == vs.terminal_map.len()
        && energized + 1 == vs.v_magnitude.len()
        && vs.v_magnitude.last().is_some_and(|&v| v == 0.0)
    {
        return energized;
    }
    let grounded = net
        .buses()
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
    fn warn(&mut self, info: &'static crate::diagnostics::DiagnosticInfo, msg: impl Into<String>) {
        self.warnings.push(info, msg);
    }

    /// The engine's bus fill rule gives every conductor the dot list does
    /// not cover a default — nodes 1..=phases for the phase conductors,
    /// ground for the rest — so a map shorter than the class's conductor
    /// count comes back from a reparse one grounded neutral longer. The
    /// first write of such a model is not a fixed point; the second is.
    /// A map longer than the count is the more serious direction: dss reads
    /// the node list positionally and drops what the record cannot address.
    fn warn_map_arity(&mut self, class: &str, name: &str, map_len: usize, nconds: usize) {
        if map_len < nconds {
            self.warn(
                &C::EMIT_DSS_VALUE_COLLAPSED,
                format!(
                    "{class} {name}: terminal map lists {map_len} of {nconds} conductors; \
                 dss materializes a grounded neutral terminal and the reparsed model \
                 gains one"
                ),
            );
        } else if map_len > nconds {
            self.warn(
                &C::EMIT_DSS_VALUE_COLLAPSED,
                format!(
                    "{class} {name}: terminal map lists {map_len} conductors but the record \
                 addresses {nconds}; dss discards the last {} and the model loses them",
                    map_len - nconds
                ),
            );
        }
    }

    /// The position of the bus's grounded terminal in `map`, when the bus
    /// grounds exactly one terminal the map lists. dss reads a node list
    /// positionally, so this conductor belongs last.
    fn return_terminal_index(&self, bus: &str, map: &[String]) -> Option<usize> {
        let grounded = self.grounded.get(&bus.to_ascii_lowercase())?;
        let mut found = map
            .iter()
            .enumerate()
            .filter(|(_, t)| grounded.contains(*t));
        let (idx, _) = found.next()?;
        found.next().is_none().then_some(idx)
    }

    /// The conductor index of a line or switch's unique grounded return when
    /// it sits before the end and the endpoints agree on it (a conductor is
    /// one wire, so both maps and every conductor indexed matrix move by the
    /// same permutation). `None` when no reorder applies or the endpoints
    /// disagree — the plain order warning stands for those.
    fn endpoint_return_index(
        &self,
        bus_from: &str,
        map_from: &[String],
        bus_to: &str,
        map_to: &[String],
    ) -> Option<usize> {
        let n = map_from.len();
        if map_to.len() != n {
            return None;
        }
        let from = self.return_terminal_index(bus_from, map_from);
        let to = self.return_terminal_index(bus_to, map_to);
        let k = match (from, to) {
            (Some(a), Some(b)) if a == b => a,
            (Some(a), None) => a,
            (None, Some(b)) => b,
            _ => return None,
        };
        (k + 1 != n).then_some(k)
    }

    /// The node list with its unique grounded return moved last, when it
    /// sits elsewhere: dss reads a node list positionally with the return
    /// last, and the classes that call this carry no per conductor data into
    /// the record, so the reorder renames nothing. The reorder is declared.
    /// Lines and switches must not call this — they index impedance matrices
    /// by these maps, and use the paired permutation instead.
    fn return_last(&mut self, class: &str, name: &str, bus: &str, map: &[String]) -> Vec<String> {
        let Some(index) = self.return_terminal_index(bus, map) else {
            return map.to_vec();
        };
        if index + 1 == map.len() {
            return map.to_vec();
        }
        let mut reordered = map.to_vec();
        let terminal = reordered.remove(index);
        self.warn(
            &C::EMIT_DSS_VALUE_SUBSTITUTED,
            format!(
                "{class} {name} on bus {bus}: grounded return `{terminal}` moved last in \
                 the node list; dss reads the list positionally and this record carries \
                 no per conductor data to keep in step"
            ),
        );
        reordered.push(terminal);
        reordered
    }

    /// Report a positional node list whose unique grounded return is not
    /// last. The diagnostic is intentionally separate from [`Self::bus_ref`]:
    /// lines and switches index their impedance matrices by these maps, so a
    /// map-only repair would silently relabel their electrical data.
    fn warn_terminal_order(
        &mut self,
        class: &str,
        name: &str,
        bus: &str,
        endpoint: Option<&str>,
        map: &[String],
    ) {
        let Some(index) = self.return_terminal_index(bus, map) else {
            return;
        };
        if index + 1 == map.len() {
            return;
        }

        let terminal = &map[index];
        let position = index + 1;
        let endpoint_text = endpoint.map_or(String::new(), |value| format!(" {value}"));
        let mut details = serde_json::Map::new();
        details.insert("class".into(), serde_json::json!(class));
        details.insert("element_name".into(), serde_json::json!(name));
        details.insert("bus".into(), serde_json::json!(bus));
        if let Some(endpoint) = endpoint {
            details.insert("endpoint".into(), serde_json::json!(endpoint));
        }
        details.insert("grounded_terminal".into(), serde_json::json!(terminal));
        details.insert("position".into(), serde_json::json!(position));
        details.insert("terminal_count".into(), serde_json::json!(map.len()));

        let mut diagnostic = crate::diagnostics::Diagnostic::of(
            &C::EMIT_DSS_TERMINAL_ORDER_UNREPRESENTABLE,
            format!(
                "{class} {name}{endpoint_text} on bus {bus}: grounded terminal `{terminal}` \
                 is at 1-based position {position} of {}; dss requires the grounded return \
                 last",
                map.len()
            ),
        )
        .with_details(details)
        .expect("writer-built details stay within the record bounds");
        crate::diagnostics::attach_target(&mut diagnostic, format!("{class} {name}"));
        self.warnings.record(diagnostic);
    }

    /// A numeric source extra. A present token that does not parse warns;
    /// the derived value substitutes and the extra is consumed either way.
    fn source_extra_f64(&mut self, vs: &crate::model::VoltageSource, key: &str) -> Option<f64> {
        let v = vs.extras.get(key)?;
        let parsed = v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        if parsed.is_none() {
            self.warn(
                &C::EMIT_DSS_VALUE_DEFAULTED,
                format!(
                    "vsource {}: {key} extra `{v}` does not parse as a number; \
                 using the derived value",
                    vs.name
                ),
            );
        }
        parsed
    }

    fn line_out(&mut self, s: &str) {
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn check_name(&mut self, class: &str, name: &str) {
        if name_breaks_dss(name, false) {
            self.warn(
                &C::EMIT_DSS_VALUE_SUBSTITUTED,
                format!(
                    "{class} `{name}`: name contains characters dss cannot represent; \
                 output will not reparse identically"
                ),
            );
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
            self.warn(
                &C::EMIT_DSS_VALUE_SUBSTITUTED,
                format!(
                    "bus `{bus}`: id contains characters dss cannot represent; \
                 output will not reparse identically"
                ),
            );
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
                    self.warn(
                        &C::EMIT_DSS_VALUE_SUBSTITUTED,
                        format!(
                            "bus {bus}: terminal `{t}` is not a dss node number; \
                         emitted as node {pos}, its position on the bus"
                        ),
                    );
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
                        self.warn(&C::EMIT_DSS_EXTRAS_DROPPED, format!(
                            "{class} {name}: extra `{key}` value `{text}` contains every \
                             dss quote closer and splits when scanned bare; emitted as \
                             written and a reparse will not see the same value"
                        ));
                    }
                    let _ = write!(tail, " {key}={out}");
                }
                _ => self.warn(&C::EMIT_DSS_VALUE_SUBSTITUTED, format!(
                    "{class} {name}: extra `{key}` is not a dss property; dropped from the output"
                )),
            }
        }
        tail
    }

    /// Lower triangle matrix text. Rows shorter than the triangle pad
    /// with 0 instead of panicking, and the padding is reported.
    fn matrix_arg(&mut self, m: &ConductorMatrix, what: &str) -> String {
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
            self.warn(
                &C::EMIT_DSS_VALUE_DEFAULTED,
                format!(
                    "{what}: matrix rows are shorter than the lower triangle; \
                 missing entries emitted as 0"
                ),
            );
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
            self.warn(
                &C::EMIT_DSS_EXTRAS_DROPPED,
                format!(
                    "{what}: series impedance extras unusable ({}, {}); left in extras",
                    state(r_key, r.is_some()),
                    state(x_key, x.is_some()),
                ),
            );
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
                    self.warn(
                        &C::EMIT_DSS_VALUE_DEFAULTED,
                        format!(
                            "{class} {name}: a delta terminal map with 3 conductors is 2 or 3 \
                         phase and no phases record disambiguates; emitted phases=3"
                        ),
                    );
                    3
                }
                n => {
                    self.warn(
                        &C::EMIT_DSS_VALUE_DEFAULTED,
                        format!(
                            "{class} {name}: a delta terminal map with {n} conductors has no \
                         dss phases mapping; emitted phases={}",
                            n.max(1)
                        ),
                    );
                    n.max(1)
                }
            },
            Configuration::Wye => terminal_map.len().saturating_sub(1).max(1),
            _ => 1,
        }
    }

    fn network(&mut self, net: &MulticonductorNetwork) {
        self.line_out("Clear");
        self.line_out(&format!(
            "Set DefaultBaseFrequency={}",
            num(net.base_frequency())
        ));
        self.out.push('\n');

        self.buscoords(net);
        self.sources(net);
        self.line_codes(net);
        self.lines(net);
        self.switches(net);
        self.transformers(net);
        self.loads(net);
        self.shunts(net);
        self.capacitors(net);
        self.generators(net);
        self.ibrs(net);

        for u in net.untyped_objects() {
            self.warn(
                &C::EMIT_DSS_RECORD_DROPPED,
                format!(
                    "{} {}: untyped object is not regenerated in canonical dss output",
                    u.class, u.name
                ),
            );
        }
        for b in net.buses() {
            self.bus_extras(b);
        }

        self.out.push('\n');
        // Source options re-emit in stored order, except the keys this
        // writer derives itself (the DefaultBaseFrequency header, the
        // VoltageBases tail). Commands do not re-emit: their position in
        // the script matters and the canonical element order does not
        // preserve it, so each drop is reported instead.
        for (key, value) in net.options() {
            if key.is_empty() {
                self.warn(
                    &C::EMIT_DSS_VALUE_DEFAULTED,
                    format!(
                        "option `{value}` has no name; not regenerated in canonical dss output"
                    ),
                );
                continue;
            }
            // The engine resolves Set names by first match in option table
            // order (Command.cpp Getcommand → HashList FindAbbrev). Every
            // prefix of "voltagebases" binds Voltagebases (it precedes the
            // other v options), but prefixes of "defaultbasefrequency"
            // shorter than "defaultb" bind DefaultDaily, so the frequency
            // skip is bounded at the engine's unique resolution point.
            // Calcvoltagebases is a command, never a Set option, so it does
            // not belong here.
            let key_lc = key.to_ascii_lowercase();
            if "voltagebases".starts_with(&key_lc)
                || (key_lc.len() >= "defaultb".len() && "defaultbasefrequency".starts_with(&key_lc))
            {
                continue;
            }
            let (text, representable) = dss_value_out(value);
            if !representable {
                self.warn(
                    &C::EMIT_DSS_VALUE_SUBSTITUTED,
                    format!(
                        "option `{key}`: value `{value}` contains every dss quote closer \
                     and splits when scanned bare; emitted as written and a reparse \
                     will not see the same value"
                    ),
                );
            }
            self.line_out(&format!("Set {key}={text}"));
        }
        for (verb, args) in net.commands() {
            if verb.eq_ignore_ascii_case("calcvoltagebases") || verb.eq_ignore_ascii_case("solve") {
                continue; // the tail emits these
            }
            let shown = if args.is_empty() {
                verb.clone()
            } else {
                format!("{verb} {args}")
            };
            self.warn(
                &C::EMIT_DSS_RECORD_DROPPED,
                format!("command `{shown}` is not regenerated in canonical dss output"),
            );
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
                continue; // legacy coordinate extras are superseded by `location`
            }
            self.warnings.push(
                &C::EMIT_DSS_EXTRAS_DROPPED,
                format!(
                    "bus {}: extra `{key}` is not regenerated in canonical dss output",
                    b.id
                ),
            );
        }
        for (field, present) in [
            ("v_min", b.v_min.is_some() || b.v_min_phase.is_some()),
            ("v_max", b.v_max.is_some() || b.v_max_phase.is_some()),
            ("vpn_min", b.vpn_min.is_some()),
            ("vpn_max", b.vpn_max.is_some()),
            ("vpp_min", b.vpp_min.is_some()),
            ("vpp_max", b.vpp_max.is_some()),
            ("vpos_min", b.vpos_min.is_some()),
            ("vpos_max", b.vpos_max.is_some()),
            ("vneg_max", b.vneg_max.is_some()),
            ("vzero_max", b.vzero_max.is_some()),
            ("vn_max", b.vn_max.is_some()),
        ] {
            if present {
                self.warnings.push(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!(
                        "bus {}: `{field}` voltage bounds have no dss expression; dropped",
                        b.id
                    ),
                );
            }
        }
    }

    fn buscoords(&mut self, net: &MulticonductorNetwork) {
        let rows: Vec<(&DistBus, crate::geo::DistLocation)> = net
            .buses()
            .iter()
            .filter_map(|b| b.location.map(|location| (b, location)))
            .collect();
        if rows.is_empty() {
            return;
        }
        let Some(path) = self.options.buscoords_filename.clone() else {
            self.warn(
                &C::EMIT_DSS_FIELD_DROPPED,
                "typed bus locations have no OpenDSS buscoords filename; dropped",
            );
            return;
        };
        if path.is_empty() {
            self.warn(
                &C::EMIT_DSS_FIELD_DROPPED,
                "typed bus locations have an empty OpenDSS buscoords filename; dropped",
            );
            return;
        }
        let (path_out, path_representable) = dss_value_out(&path);
        if !path_representable {
            self.warn(&C::EMIT_DSS_VALUE_SUBSTITUTED, format!(
                "buscoords filename `{path}` contains every dss quote closer and splits when scanned bare; emitted as written and a reparse will not see the same value"
            ));
        }

        let mut text = String::new();
        for (bus, location) in rows {
            if !location.x.is_finite() || !location.y.is_finite() {
                self.warn(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!(
                        "bus {}: nonfinite location is not emitted to OpenDSS buscoords",
                        bus.id
                    ),
                );
                continue;
            }
            let (bus_out, bus_representable) = dss_value_out(&bus.id);
            if !bus_representable {
                self.warn(&C::EMIT_DSS_FIELD_DROPPED, format!(
                    "bus {}: id contains every dss quote closer and splits in buscoords; coordinates dropped",
                    bus.id
                ));
                continue;
            }
            let _ = writeln!(text, "{bus_out},{},{}", num(location.x), num(location.y));
        }
        if text.is_empty() {
            return;
        }
        self.line_out(&format!("Buscoords {path_out}"));
        self.sidecars.push(TextSidecar { path, text });
    }

    fn sources(&mut self, net: &MulticonductorNetwork) {
        let mut order: Vec<usize> = (0..net.sources().len()).collect();
        if let Some(source_idx) = net
            .sources()
            .iter()
            .position(|vs| vs.name.eq_ignore_ascii_case("source"))
        {
            order.swap(0, source_idx);
        }
        for (i, source_idx) in order.into_iter().enumerate() {
            let vs = &net.sources()[source_idx];
            let phases = source_phases(net, vs);
            let energized = vs.v_magnitude.iter().filter(|&&v| v > 0.0).count();
            if energized > 0 && energized != phases {
                self.warn(
                    &C::EMIT_DSS_VALUE_DEFAULTED,
                    format!(
                        "vsource {}: emitted phases={phases} but {energized} v_magnitude \
                     entries are positive; a reparse energizes all {phases}",
                        vs.name
                    ),
                );
            }
            self.warn_map_arity("vsource", &vs.name, vs.terminal_map.len(), phases + 1);
            let basekv = self
                .source_extra_f64(vs, "basekv")
                .unwrap_or_else(|| source_basekv(vs, phases));
            let pu = self.source_extra_f64(vs, "pu").unwrap_or(1.0);
            let angle = self
                .source_extra_f64(vs, "angle")
                .unwrap_or_else(|| vs.v_angle.first().copied().unwrap_or(0.0).to_degrees());
            let head = if i == 0 {
                let name = net.name().clone().unwrap_or_else(|| "converted".into());
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

    fn line_codes(&mut self, net: &MulticonductorNetwork) {
        let omega_nf = std::f64::consts::TAU * net.base_frequency() * 1e-9;
        for c in net.line_codes() {
            self.emit_linecode(c, omega_nf);
        }
        // #307: a line whose unique grounded return is not the last conductor
        // reorders its node lists; the matrices its linecode carries are
        // indexed by that conductor order, so a permuted copy of the linecode
        // is emitted and the line references it — the map and the matrices
        // move together, and other lines keep the original.
        let mut permuted: std::collections::HashSet<(String, usize)> =
            std::collections::HashSet::new();
        for l in net.lines() {
            let Some(k) = self.endpoint_return_index(
                &l.bus_from,
                &l.terminal_map_from,
                &l.bus_to,
                &l.terminal_map_to,
            ) else {
                continue;
            };
            let Some(code) = net
                .line_codes()
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&l.linecode))
            else {
                continue;
            };
            if code.n_conductors != l.terminal_map_from.len()
                || !permuted.insert((code.name.to_ascii_lowercase(), k))
            {
                continue;
            }
            let perm = return_permutation(k, code.n_conductors);
            let mut clone = code.clone();
            clone.name = format!("{}_ret{k}", code.name);
            clone.r_series = permute_symmetric(&code.r_series, &perm);
            clone.x_series = permute_symmetric(&code.x_series, &perm);
            clone.g_from = permute_symmetric(&code.g_from, &perm);
            clone.b_from = permute_symmetric(&code.b_from, &perm);
            clone.g_to = permute_symmetric(&code.g_to, &perm);
            clone.b_to = permute_symmetric(&code.b_to, &perm);
            clone.i_max = code.i_max.as_ref().map(|v| permute_padded(v, &perm));
            clone.s_max = code.s_max.as_ref().map(|v| permute_padded(v, &perm));
            self.emit_linecode(&clone, omega_nf);
        }
        self.out.push('\n');
    }

    fn emit_linecode(&mut self, c: &DistLineCode, omega_nf: f64) {
        {
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
            let c_nf: ConductorMatrix = c
                .b_from
                .iter()
                .map(|row| row.iter().map(|b| 2.0 * b / omega_nf).collect())
                .collect();
            let cm = self.matrix_arg(&c_nf, &what);
            let _ = write!(s, " cmatrix={cm}");
            match c.i_max.as_deref() {
                Some([amps, ..]) if amps.is_finite() => {
                    let _ = write!(s, " emergamps={}", num(*amps));
                }
                Some([_, ..]) => self.warn(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!(
                        "linecode {}: first i_max entry is nonfinite (an unbounded \
                     conductor); emergamps not emitted",
                        c.name
                    ),
                ),
                Some([]) => self.warn(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!("linecode {}: i_max is empty; emergamps not emitted", c.name),
                ),
                None => {}
            }
            if !c.g_from.iter().flatten().all(|&g| g == 0.0) {
                self.warn(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!(
                        "linecode {}: shunt conductance has no dss linecode field; dropped",
                        c.name
                    ),
                );
            }
            if c.source.is_some() {
                self.warn(
                    &C::EMIT_DSS_EXTRAS_DROPPED,
                    format!(
                        "linecode {}: matrix provenance `source` has no dss field; dropped",
                        c.name
                    ),
                );
            }
            let mut extras = c.extras.clone();
            extras.remove("units"); // canonical output is in meters
            s.push_str(&self.extras_tail("linecode", &c.name, &extras));
            self.line_out(&s);
        }
    }

    // One block per record family; splitting the reorder decision from the
    // emission would thread five locals through helpers.
    #[allow(clippy::too_many_lines)]
    fn lines(&mut self, net: &MulticonductorNetwork) {
        for l in net.lines() {
            self.check_name("line", &l.name);
            // #307: with an agreed return conductor before the end and a
            // matching permuted linecode emitted, the node lists and the
            // matrices move together; otherwise the order warning stands.
            let reorder = self
                .endpoint_return_index(
                    &l.bus_from,
                    &l.terminal_map_from,
                    &l.bus_to,
                    &l.terminal_map_to,
                )
                .filter(|_| {
                    net.line_codes().iter().any(|c| {
                        c.name.eq_ignore_ascii_case(&l.linecode)
                            && c.n_conductors == l.terminal_map_from.len()
                    })
                });
            let (map_from, map_to, code_name);
            if let Some(k) = reorder {
                let perm = return_permutation(k, l.terminal_map_from.len());
                map_from = permute_names(&l.terminal_map_from, &perm);
                map_to = permute_names(&l.terminal_map_to, &perm);
                code_name = format!("{}_ret{k}", l.linecode);
                self.warn(
                    &C::EMIT_DSS_VALUE_SUBSTITUTED,
                    format!(
                        "line {}: grounded return moved last in both node lists; \
                         linecode `{code_name}` carries the matrices permuted in step",
                        l.name
                    ),
                );
            } else {
                self.warn_terminal_order(
                    "line",
                    &l.name,
                    &l.bus_from,
                    Some("bus1"),
                    &l.terminal_map_from,
                );
                self.warn_terminal_order(
                    "line",
                    &l.name,
                    &l.bus_to,
                    Some("bus2"),
                    &l.terminal_map_to,
                );
                map_from = l.terminal_map_from.clone();
                map_to = l.terminal_map_to.clone();
                code_name = l.linecode.clone();
            }
            let phases = l.terminal_map_from.len();
            let mut s = format!(
                "New Line.{} bus1={} bus2={} phases={phases} linecode={} length={} units=m",
                l.name,
                self.bus_ref(&l.bus_from, &map_from),
                self.bus_ref(&l.bus_to, &map_to),
                code_name,
                self.checked_num(l.length, 1.0, &format!("line {}: length", l.name)),
            );
            let mut extras = l.extras.clone();
            extras.remove("units"); // canonical output is in meters
            let line_i_max = match reorder {
                Some(k) => l
                    .i_max
                    .as_ref()
                    .map(|v| permute_padded(v, &return_permutation(k, l.terminal_map_from.len()))),
                None => l.i_max.clone(),
            };
            // `i_max` maps to `emergamps`, as it does on a linecode. The
            // typed field wins over a token kept in extras.
            match line_i_max.as_deref() {
                Some([amps, rest @ ..]) if is_positive_finite(*amps) => {
                    extras.remove("emergamps");
                    let _ = write!(s, " emergamps={}", num(*amps));
                    // The dss Line has one emergamps for all phases. Compare
                    // exactly: any difference makes the token wrong for a phase.
                    #[allow(clippy::float_cmp)]
                    let uneven = rest.iter().any(|a| *a != *amps);
                    if uneven {
                        self.warn(
                            &C::EMIT_DSS_VALUE_COLLAPSED,
                            format!(
                                "line {}: i_max is not equal on all phases; emergamps \
                             holds the first phase only",
                                l.name
                            ),
                        );
                    }
                }
                Some([_, ..]) => self.warn(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!(
                        "line {}: first i_max entry is nonfinite (an unbounded \
                     conductor); emergamps not emitted",
                        l.name
                    ),
                ),
                Some([]) => self.warn(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!("line {}: i_max is empty; emergamps not emitted", l.name),
                ),
                None => {}
            }
            if l.s_max.is_some() {
                // dss Line carries current ratings only; an apparent power
                // limit is a different quantity and is not folded into
                // emergamps. The typed field drops, declared.
                self.warn(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!(
                        "line {}: `s_max` is an apparent power limit and dss Line \
                         states current ratings only; dropped",
                        l.name
                    ),
                );
            }
            s.push_str(&self.extras_tail("line", &l.name, &extras));
            self.line_out(&s);
        }
        self.out.push('\n');
    }

    fn switches(&mut self, net: &MulticonductorNetwork) {
        for sw in net.switches() {
            self.check_name("line", &sw.name);
            // #307: a switch is ideal — no conductor indexed matrices — so an
            // agreed return conductor reorders both node lists together.
            let reorder = self.endpoint_return_index(
                &sw.bus_from,
                &sw.terminal_map_from,
                &sw.bus_to,
                &sw.terminal_map_to,
            );
            let (map_from, map_to);
            if let Some(k) = reorder {
                let perm = return_permutation(k, sw.terminal_map_from.len());
                map_from = permute_names(&sw.terminal_map_from, &perm);
                map_to = permute_names(&sw.terminal_map_to, &perm);
                self.warn(
                    &C::EMIT_DSS_VALUE_SUBSTITUTED,
                    format!(
                        "switch {}: grounded return moved last in both node lists; \
                         the switch carries no per conductor data to keep in step",
                        sw.name
                    ),
                );
            } else {
                self.warn_terminal_order(
                    "switch",
                    &sw.name,
                    &sw.bus_from,
                    Some("bus1"),
                    &sw.terminal_map_from,
                );
                self.warn_terminal_order(
                    "switch",
                    &sw.name,
                    &sw.bus_to,
                    Some("bus2"),
                    &sw.terminal_map_to,
                );
                map_from = sw.terminal_map_from.clone();
                map_to = sw.terminal_map_to.clone();
            }
            let phases = sw.terminal_map_from.len();
            let mut s = format!(
                "New Line.{} bus1={} bus2={} phases={phases} switch=y",
                sw.name,
                self.bus_ref(&sw.bus_from, &map_from),
                self.bus_ref(&sw.bus_to, &map_to),
            );
            match sw.i_max.as_deref() {
                Some([amps, ..]) if amps.is_finite() => {
                    let _ = write!(s, " emergamps={}", num(*amps));
                }
                Some([_, ..]) => self.warn(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!(
                        "line {}: first i_max entry is nonfinite (an unbounded \
                     conductor); emergamps not emitted",
                        sw.name
                    ),
                ),
                Some([]) => self.warn(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!("line {}: i_max is empty; emergamps not emitted", sw.name),
                ),
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

    fn transformers(&mut self, net: &MulticonductorNetwork) {
        for t in net.transformers() {
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
                    DistWindingConn::Wye => "wye",
                    DistWindingConn::Delta => "delta",
                })
                .collect();
            let kvs: Vec<Option<f64>> = t
                .windings
                .iter()
                .enumerate()
                .map(|(idx, w)| self.winding_kv(t, idx, w))
                .collect();
            let kvas: Vec<Option<f64>> = t
                .windings
                .iter()
                .enumerate()
                .map(|(idx, w)| {
                    if is_positive_finite(w.s_rating) {
                        Some(w.s_rating / 1e3)
                    } else {
                        self.warn(
                            &C::EMIT_DSS_VALUE_DEFAULTED,
                            format!(
                                "transformer {}: winding {} has no usable rating; kva not \
                             emitted (the OpenDSS default applies)",
                                t.name,
                                idx + 1
                            ),
                        );
                        None
                    }
                })
                .collect();
            let rs: Vec<String> = t
                .windings
                .iter()
                .enumerate()
                .map(|(idx, w)| {
                    let what = format!("transformer {}: winding {} %r", t.name, idx + 1);
                    self.checked_num(w.r_pct, 0.0, &what)
                })
                .collect();
            let taps: Vec<String> = t.windings.iter().map(|w| num(w.tap)).collect();
            let mut s = format!(
                "New Transformer.{} phases={} windings={nw} buses=({}) conns=({})",
                t.name,
                t.phases,
                buses.join(", "),
                conns.join(", "),
            );
            let mut edits: Vec<String> = vec![String::new(); nw];
            winding_array(&mut s, &mut edits, "kvs", "kv", &kvs);
            winding_array(&mut s, &mut edits, "kvas", "kva", &kvas);
            let _ = write!(s, " %Rs=({}) taps=({})", rs.join(", "), taps.join(", "));
            if let Some(xhl) = t.xsc_pct.first() {
                let _ = write!(s, " xhl={}", num(*xhl));
                if t.xsc_pct.len() >= 3 {
                    let xlt = self.star_xlt(t);
                    let _ = write!(s, " xht={} xlt={}", num(t.xsc_pct[1]), num(xlt));
                }
            } else {
                self.warn(
                    &C::EMIT_DSS_VALUE_DEFAULTED,
                    format!("transformer {}: xsc_pct is empty; emitted xhl=0", t.name),
                );
                s.push_str(" xhl=0");
            }
            let mut extras = t.extras.clone();
            if let Some((loss, imag)) = crate::model::transformer_no_load_percentages(t) {
                extras.remove("no_load_shunt");
                extras.insert("%noloadloss".into(), serde_json::json!(loss));
                extras.insert("%imag".into(), serde_json::json!(imag));
            }
            s.push_str(&self.extras_tail("transformer", &t.name, &extras));
            self.line_out(&s);
            for (idx, w) in t.windings.iter().enumerate() {
                if let Some(r) = w.r_neutral {
                    let _ = write!(edits[idx], " rneut={}", num(r));
                }
                if let Some(x) = w.x_neutral {
                    let _ = write!(edits[idx], " xneut={}", num(x));
                }
            }
            for (idx, edit) in edits.iter().enumerate() {
                if !edit.is_empty() {
                    self.line_out(&format!("~ wdg={}{edit}", idx + 1));
                }
            }
        }
        self.out.push('\n');
    }

    /// The `xlt=` value for a three winding record. dss cannot solve a star
    /// whose third arm is zero: the two secondary legs collapse to about half
    /// voltage and read unequal under balanced load, and the solution
    /// converges without an error. A source that lumps the whole leakage on
    /// the primary arm states exactly that, so the split from the OpenDSS
    /// center tap example, `xlt = 2/3 xhl` at `xhl = xht`, substitutes.
    fn star_xlt(&mut self, t: &DistTransformer) -> f64 {
        let (xhl, xht, xlt) = (t.xsc_pct[0], t.xsc_pct[1], t.xsc_pct[2]);
        if xlt > 0.0 && xlt.is_finite() {
            return xlt;
        }
        #[allow(clippy::float_cmp)]
        let lumped_on_primary = xhl == xht && xhl > 0.0 && xhl.is_finite();
        if !lumped_on_primary {
            self.warn(
                &C::EMIT_DSS_VALUE_SUBSTITUTED,
                format!(
                    "transformer {}: xlt={} is not a reactance dss can solve, and the \
                 other two arms do not determine a replacement; emitted as stated",
                    t.name,
                    num(xlt)
                ),
            );
            return xlt;
        }
        let repaired = 2.0 / 3.0 * xhl;
        self.warn(
            &C::EMIT_DSS_VALUE_COLLAPSED,
            format!(
                "transformer {}: the source puts the whole leakage on the primary arm, \
             leaving xlt={}; dss solves that star as a collapsed secondary, so \
             xlt={} went out instead, holding xhl={}",
                t.name,
                num(xlt),
                num(repaired),
                num(xhl)
            ),
        );
        repaired
    }

    /// The winding `kv=` value in kV, or `None` if no value is available.
    /// A BMOPF transformer without `v_nom_from`/`v_nom_to` reads as
    /// `v_ref = NaN`, and OpenDSS refuses a deck that holds a `NaN` token.
    /// The fallback is the bus voltage estimate, scaled to the voltage across
    /// the two winding terminals: line to neutral for a single phase winding
    /// on a grounded terminal, line to line in all other cases.
    fn winding_kv(
        &mut self,
        t: &crate::model::DistTransformer,
        idx: usize,
        w: &DistWinding,
    ) -> Option<f64> {
        if is_positive_finite(w.v_ref) {
            return Some(w.v_ref / 1e3);
        }
        let bus = w.bus.to_ascii_lowercase();
        let scale =
            if winding_is_line_to_neutral(t.phases, w, |b| self.grounded.get(b).map(Vec::as_slice))
            {
                1.0
            } else {
                3f64.sqrt()
            };
        let Some(v_pn) = self.kv_estimate.get(&bus).copied() else {
            self.warn(
                &C::EMIT_DSS_VALUE_DEFAULTED,
                format!(
                    "transformer {}: winding {} has no rated voltage and bus `{}` has \
                 no voltage estimate; kv not emitted (the OpenDSS default applies)",
                    t.name,
                    idx + 1,
                    w.bus
                ),
            );
            return None;
        };
        let kv = v_pn * scale / 1e3;
        self.warn(
            &C::EMIT_DSS_FIELD_DROPPED,
            format!(
                "transformer {}: winding {} has no rated voltage; kv={} derived \
             from the bus `{}` voltage estimate",
                t.name,
                idx + 1,
                num(kv),
                w.bus
            ),
        );
        Some(kv)
    }

    /// The `Load` objects one [`DistLoad`] emits as: itself, or one per phase
    /// when its phases carry different power (#266).
    ///
    /// An OpenDSS `Load` divides its `kw`/`kvar` evenly across its phases, so a
    /// load whose `p_nom`/`q_nom` differ per phase has no single object
    /// expression. Emitting one balanced `Load` keeps the total and loses the
    /// profile; one single phase `Load` per terminal keeps both. A delta load's
    /// phases sit across terminal pairs rather than on one terminal each, so
    /// the same split needs branch geometry: it keeps the balanced form and
    /// says what was lost.
    fn load_parts<'l>(&mut self, l: &'l DistLoad) -> Vec<Cow<'l, DistLoad>> {
        let n = l.p_nom.len();
        // Exact comparison: any difference at all makes one balanced object the
        // wrong statement, and a tolerance here would decide how much
        // imbalance is allowed to vanish.
        #[allow(clippy::float_cmp)]
        let uniform = |xs: &[f64]| xs.iter().all(|x| *x == xs[0]);
        let stated_per_phase = n >= 2 && l.q_nom.len() == n;
        let unbalanced = stated_per_phase && !(uniform(&l.p_nom) && uniform(&l.q_nom));
        // dss reads the node list positionally: phase conductors first, the
        // return last. A center tapped service maps as `[p1, n, p2]`, so one
        // record over that map names a different node pair than the load sits
        // on however its power divides.
        let return_index = self.return_terminal_index(&l.bus, &l.terminal_map);
        let misordered = return_index.is_some_and(|i| i + 1 != l.terminal_map.len());
        if !unbalanced && !misordered {
            return vec![Cow::Borrowed(l)];
        }
        if l.configuration == Configuration::Delta {
            self.warn(
                &C::EMIT_DSS_VALUE_COLLAPSED,
                format!(
                    "load {}: per phase power on a delta load has no dss expression; \
                 emitted one balanced Load carrying the total",
                    l.name
                ),
            );
            return vec![Cow::Borrowed(l)];
        }
        // Without a grounded terminal the map states the return last, the
        // shape the reader writes for a wye element.
        let (hot_indices, return_terminal) = match return_index {
            Some(i) => (
                (0..l.terminal_map.len()).filter(|j| *j != i).collect(),
                Some(l.terminal_map[i].clone()),
            ),
            None if l.terminal_map.len() > n => ((0..n).collect(), Some(l.terminal_map[n].clone())),
            None => ((0..l.terminal_map.len()).collect::<Vec<_>>(), None),
        };
        if !stated_per_phase || hot_indices.len() != n {
            self.warn(
                &C::EMIT_DSS_VALUE_COLLAPSED,
                format!(
                    "load {}: {} over a terminal map with {} phase conductors; \
                 emitted one Load carrying the total",
                    l.name,
                    if stated_per_phase {
                        format!("per phase power over {n} phases")
                    } else {
                        "one power value".to_string()
                    },
                    hot_indices.len()
                ),
            );
            return vec![Cow::Borrowed(l)];
        }
        hot_indices
            .into_iter()
            .enumerate()
            .map(|(i, hot)| {
                let mut part = l.clone();
                part.name = format!("{}_{}", l.name, l.terminal_map[hot]);
                part.terminal_map = match &return_terminal {
                    Some(r) => vec![l.terminal_map[hot].clone(), r.clone()],
                    None => vec![l.terminal_map[hot].clone()],
                };
                part.configuration = Configuration::Wye;
                part.p_nom = vec![l.p_nom[i]];
                part.q_nom = vec![l.q_nom[i]];
                // The whole-load spellings do not survive the split: `phases`
                // and `conn` describe the bank, `kv` its line to line voltage,
                // and `pf` would re-derive a shared reactive ratio over power
                // this part states outright.
                for key in ["phases", "conn", "kv", "pf"] {
                    part.extras.remove(key);
                }
                Cow::Owned(part)
            })
            .collect()
    }

    fn loads(&mut self, net: &MulticonductorNetwork) {
        for load in net.loads() {
            for part in self.load_parts(load) {
                self.write_load(&part);
            }
        }
        self.out.push('\n');
    }

    /// One `New Load.<name>` record. A [`DistLoad`] emits one of these, or one
    /// per phase when [`Self::load_parts`] split it.
    fn write_load(&mut self, l: &DistLoad) {
        self.check_name("load", &l.name);
        let phases =
            self.element_phases(&l.extras, &l.terminal_map, l.configuration, "load", &l.name);
        let conn = self.element_conn(&l.extras, l.configuration, &l.bus, &l.terminal_map);
        // The reader's nconds: a 3 phase delta has no neutral conductor,
        // every other connection carries phases + 1.
        let nconds = nconds_for(conn, phases);
        self.warn_map_arity("load", &l.name, l.terminal_map.len(), nconds);
        let kw: f64 = l.p_nom.iter().sum::<f64>() / 1e3;
        let kvar: f64 = l.q_nom.iter().sum::<f64>() / 1e3;
        let typed_kv = self.load_nominal_kv(&l.voltage_model, phases, l.configuration, &l.name);
        let kv = self.element_kv(
            &l.extras,
            ElementKv {
                bus: &l.bus,
                phases,
                configuration: l.configuration,
                name: &l.name,
                class: "load",
                typed_kv,
            },
        );
        let mut extras = l.extras.clone();
        strip_emitted_extras(&mut extras, &["kv", "phases", "conn"]);
        let retained_model = extras.remove("model");
        let retained_zipv = extras.remove("zipv");
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
        match &l.voltage_model {
            DistLoadVoltageModel::ConstantPower { .. } => {
                if let Some(model) = retained_model {
                    extras.insert("model".into(), model);
                }
            }
            DistLoadVoltageModel::ConstantImpedance { .. } => {
                s.push_str(" model=2");
            }
            DistLoadVoltageModel::ConstantCurrent { .. } => {
                s.push_str(" model=5");
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
                s.push_str(" model=8");
                if let (Some(az), Some(ai), Some(ap), Some(bz), Some(bi), Some(bp)) = (
                    alpha_z.first(),
                    alpha_i.first(),
                    alpha_p.first(),
                    beta_z.first(),
                    beta_i.first(),
                    beta_p.first(),
                ) {
                    let cutoff = zipv_cutoff(retained_zipv.as_ref()).unwrap_or(0.0);
                    let _ = write!(
                        s,
                        " zipv=({}, {}, {}, {}, {}, {}, {})",
                        num(*az),
                        num(*ai),
                        num(*ap),
                        num(*bz),
                        num(*bi),
                        num(*bp),
                        num(cutoff)
                    );
                }
            }
            DistLoadVoltageModel::Exponential { .. } => {
                self.warn(&C::EMIT_DSS_VALUE_SUBSTITUTED, format!(
                    "load {}: exponential voltage model has no OpenDSS load model code; emitted constant power",
                    l.name
                ));
            }
        }
        self.add_default_load_voltage_bounds(&mut extras);
        s.push_str(&self.extras_tail("load", &l.name, &extras));
        self.line_out(&s);
    }

    fn add_default_load_voltage_bounds(&self, extras: &mut Extras) {
        if let Some(bounds) = self.options.default_load_voltage_bounds {
            extras
                .entry("vminpu".into())
                .or_insert_with(|| bounds.vminpu.into());
            extras
                .entry("vmaxpu".into())
                .or_insert_with(|| bounds.vmaxpu.into());
        }
    }

    /// `kv` for a load or capacitor: the recorded value when the source
    /// carried one, otherwise the propagated bus estimate.
    /// [`num`] for a value a payload can spell as `null`. OpenDSS has no token
    /// for a nonfinite number — `NaN` and `inf` in a deck are a parse failure
    /// downstream, not a value — so an unusable one is reported and replaced
    /// with the neutral value, as the BMOPF writer does (#288).
    fn checked_num(&mut self, v: f64, fallback: f64, what: &str) -> String {
        if v.is_finite() {
            return num(v);
        }
        self.warn(
            &C::EMIT_DSS_VALUE_SUBSTITUTED,
            format!("{what}: {v} has no dss spelling; emitted {}", num(fallback)),
        );
        num(fallback)
    }

    fn element_kv(&mut self, extras: &Extras, ctx: ElementKv<'_>) -> f64 {
        if let Some(v) = extras.get("kv") {
            match v
                .as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            {
                Some(kv) => return kv,
                None => self.warn(
                    &C::EMIT_DSS_VALUE_DEFAULTED,
                    format!(
                        "{} {}: kv extra `{v}` does not parse as a number; \
                     using the bus voltage estimate",
                        ctx.class, ctx.name
                    ),
                ),
            }
        }
        if let Some(kv) = ctx.typed_kv {
            return kv;
        }
        if let Some(vln) = self.kv_estimate.get(&ctx.bus.to_ascii_lowercase()).copied() {
            // OpenDSS convention: line to line for 2 and 3 phase, line to
            // neutral for single phase.
            let v = if ctx.phases >= 2 || ctx.configuration == Configuration::Delta {
                vln * 3f64.sqrt()
            } else {
                vln
            };
            v / 1e3
        } else {
            self.warn(
                &C::EMIT_DSS_VALUE_DEFAULTED,
                format!(
                    "{} {}: no kv in the source and no bus voltage estimate; \
                 emitted 12.47",
                    ctx.class, ctx.name
                ),
            );
            12.47
        }
    }

    fn load_nominal_kv(
        &mut self,
        model: &DistLoadVoltageModel,
        phases: usize,
        configuration: Configuration,
        name: &str,
    ) -> Option<f64> {
        let v_nom = model.v_nom();
        let v_phase = v_nom.first().copied().filter(|v| is_positive_finite(*v))?;
        if v_nom
            .iter()
            .any(|v| (*v - v_phase).abs() > 1e-9 * v.abs().max(v_phase.abs()).max(1.0))
        {
            self.warn(&C::EMIT_DSS_VALUE_COLLAPSED, format!(
                "load {name}: nonuniform nominal voltage array has no OpenDSS scalar kv; emitted the first value"
            ));
        }
        let v = if phases >= 2 && configuration == Configuration::Wye {
            v_phase * 3f64.sqrt()
        } else {
            v_phase
        };
        Some(v / 1e3)
    }

    /// Emitted `conn=`: delta for typed delta, for a stashed DSS delta token,
    /// and for a single phase two terminal map that does not include a grounded
    /// return conductor.
    fn element_conn(
        &self,
        extras: &Extras,
        configuration: Configuration,
        bus: &str,
        terminal_map: &[String],
    ) -> &'static str {
        let stash_delta = extras
            .get("conn")
            .and_then(|v| v.as_str())
            .is_some_and(|t| {
                t.to_ascii_lowercase().starts_with('d') || t.eq_ignore_ascii_case("ll")
            });
        let has_grounded_return = self
            .grounded
            .get(&bus.to_ascii_lowercase())
            .is_some_and(|g| terminal_map.iter().any(|t| g.contains(t)));
        match configuration {
            Configuration::Delta => "delta",
            Configuration::SinglePhase
                if stash_delta || (terminal_map.len() == 2 && !has_grounded_return) =>
            {
                "delta"
            }
            _ => "wye",
        }
    }

    fn write_impedance_shunt(&mut self, sh: &crate::model::DistShunt, phases: usize) {
        self.check_name("reactor", &sh.name);
        let Some((conductance, susceptance)) = first_diag_admittance(&sh.g, &sh.b, phases) else {
            self.warn(&C::EMIT_DSS_RECORD_DROPPED, format!(
                "shunt {}: conductance matrix has no diagonal admittance; dropped from the output",
                sh.name
            ));
            return;
        };
        if has_off_diagonal(&sh.g) || has_off_diagonal(&sh.b) {
            self.warn(
                &C::EMIT_DSS_VALUE_COLLAPSED,
                format!(
                    "shunt {}: off diagonal admittance has no scalar reactor expression; \
                 only the first diagonal admittance is regenerated",
                    sh.name
                ),
            );
        }
        if !uniform_diag_admittance(&sh.g, &sh.b, phases, conductance, susceptance) {
            self.warn(
                &C::EMIT_DSS_VALUE_COLLAPSED,
                format!(
                    "shunt {}: diagonal admittances differ; only the first diagonal \
                 admittance is regenerated",
                    sh.name
                ),
            );
        }
        let denom = conductance * conductance + susceptance * susceptance;
        if !denom.is_finite() || denom <= 0.0 {
            self.warn(
                &C::EMIT_DSS_RECORD_DROPPED,
                format!(
                    "shunt {}: invalid grounding admittance; dropped from the output",
                    sh.name
                ),
            );
            return;
        }
        let resistance = conductance / denom;
        let reactance = -susceptance / denom;
        let mut extras = sh.extras.clone();
        strip_shunt_extras(&mut extras);
        let ground = vec!["0".to_string(); phases.max(1)];
        let mut line = format!(
            "New Reactor.{} bus1={} bus2={} phases={} r={} x={}",
            sh.name,
            self.bus_ref(&sh.bus, &sh.terminal_map),
            self.bus_ref(&sh.bus, &ground),
            phases.max(1),
            num(resistance),
            num(reactance),
        );
        line.push_str(&self.extras_tail("reactor", &sh.name, &extras));
        self.line_out(&line);
    }

    fn shunt_phases(
        &mut self,
        sh: &crate::model::DistShunt,
        conn_delta: bool,
        inferred_phases: usize,
    ) -> usize {
        if let Some(p) = extras_usize(&sh.extras, "phases") {
            p.max(1)
        } else if conn_delta {
            self.element_phases(
                &sh.extras,
                &sh.terminal_map,
                Configuration::Delta,
                "shunt",
                &sh.name,
            )
        } else {
            inferred_phases
        }
    }

    fn write_kvar_shunt(&mut self, sh: &crate::model::DistShunt, phases: usize, conn_delta: bool) {
        // Scan every diagonal conductor, not just the first `phases` of them: a
        // delta bank's conductor count exceeds its stashed `phases`, and a
        // sign-flipped diagonal past that bound must still set the class.
        let (b_max, b_min) = (0..sh.b.len())
            .map(|idx| diag_at(&sh.b, idx))
            .fold((0.0_f64, 0.0_f64), |(mx, mn), v| (mx.max(v), mn.min(v)));
        let (class, b_phase) = if b_max > 0.0 {
            ("capacitor", b_max)
        } else if b_min < 0.0 {
            ("reactor", b_min)
        } else {
            self.warn(
                &C::EMIT_DSS_RECORD_DROPPED,
                format!(
                    "shunt {}: no nonzero susceptance; dropped from the output",
                    sh.name
                ),
            );
            return;
        };
        if b_max > 0.0 && b_min < 0.0 {
            self.warn(
                &C::EMIT_DSS_FIELD_DROPPED,
                format!(
                    "shunt {}: diagonal mixes capacitive and inductive phases; only the \
                 {class} phases are regenerated",
                    sh.name
                ),
            );
        }
        self.check_name(class, &sh.name);
        let off_diag = has_off_diagonal(&sh.b);
        if off_diag && !conn_delta {
            self.warn(
                &C::EMIT_DSS_VALUE_COLLAPSED,
                format!(
                    "shunt {}: off diagonal susceptance has no {class} expression; \
                 only the diagonal is regenerated",
                    sh.name
                ),
            );
        }
        let edges = if conn_delta {
            delta_edges(sh.terminal_map.len(), phases)
        } else {
            Vec::new()
        };
        if conn_delta && edges.is_empty() {
            self.warn(&C::EMIT_DSS_RECORD_DROPPED, format!(
                "shunt {}: delta terminal map has no branch expression; dropped from the output",
                sh.name
            ));
            return;
        }
        if conn_delta && delta_branch_susceptance(&sh.b, &edges, sh.terminal_map.len()).is_none() {
            self.warn(
                &C::EMIT_DSS_VALUE_COLLAPSED,
                format!(
                    "shunt {}: delta susceptance matrix has no scalar {class} expression; \
                 only the average branch susceptance is regenerated",
                    sh.name
                ),
            );
        }
        let configuration = if conn_delta {
            Configuration::Delta
        } else {
            Configuration::Wye
        };
        let kv = self.element_kv(
            &sh.extras,
            ElementKv {
                bus: &sh.bus,
                phases,
                configuration,
                name: &sh.name,
                class,
                typed_kv: None,
            },
        );
        let kvar = extras_f64(&sh.extras, "kvar")
            .unwrap_or_else(|| shunt_kvar(sh, phases, conn_delta, &edges, b_phase, kv));
        let mut extras = sh.extras.clone();
        strip_shunt_extras(&mut extras);
        let conn = if conn_delta { "delta" } else { "wye" };
        let decl = if class == "reactor" {
            "Reactor"
        } else {
            "Capacitor"
        };
        let mut line = format!(
            "New {decl}.{} bus1={} phases={phases} conn={conn} kv={} kvar={}",
            sh.name,
            self.bus_ref(&sh.bus, &sh.terminal_map),
            num(kv),
            num(kvar),
        );
        line.push_str(&self.extras_tail(class, &sh.name, &extras));
        self.line_out(&line);
    }

    /// Typed BMOPF capacitor banks (#266). A bank states its rating and its
    /// nameplate voltage, which is what an OpenDSS `Capacitor` takes, so the
    /// conversion is a unit change and the terminal spelling: `v_nom` is line
    /// to line for the three phase configurations and across the terminals for
    /// `SINGLE_PHASE`, which is the `kv` convention the reader applies coming
    /// back the other way.
    ///
    /// The untyped [`DistShunt`](crate::model::DistShunt) B matrix keeps its
    /// own path ([`Self::write_kvar_shunt`]): it carries phase geometry a
    /// scalar rating cannot state.
    fn capacitors(&mut self, net: &MulticonductorNetwork) {
        for c in net.capacitors() {
            if !is_positive_finite(c.q_rated) {
                self.warn(
                    &C::EMIT_DSS_VALUE_SUBSTITUTED,
                    format!(
                        "capacitor {}: rating {} is not a positive number; dropped from the output",
                        c.name, c.q_rated
                    ),
                );
                continue;
            }
            self.check_name("capacitor", &c.name);
            let node_map = self.return_last("capacitor", &c.name, &c.bus, &c.terminal_map);
            let phases = self.element_phases(
                &c.extras,
                &c.terminal_map,
                c.configuration,
                "capacitor",
                &c.name,
            );
            let conn = self.element_conn(&c.extras, c.configuration, &c.bus, &c.terminal_map);
            let nconds = nconds_for(conn, phases);
            self.warn_map_arity("capacitor", &c.name, c.terminal_map.len(), nconds);
            let typed_kv = is_positive_finite(c.v_nom).then(|| c.v_nom / 1e3);
            if typed_kv.is_none() {
                self.warn(
                    &C::EMIT_DSS_VALUE_SUBSTITUTED,
                    format!(
                        "capacitor {}: nominal voltage {} is not a positive number; \
                     using the bus voltage estimate",
                        c.name, c.v_nom
                    ),
                );
            }
            let kv = self.element_kv(
                &c.extras,
                ElementKv {
                    bus: &c.bus,
                    phases,
                    configuration: c.configuration,
                    name: &c.name,
                    class: "capacitor",
                    typed_kv,
                },
            );
            let mut extras = c.extras.clone();
            strip_emitted_extras(&mut extras, &["kv", "phases", "conn", "kvar"]);
            let mut line = format!(
                "New Capacitor.{} bus1={} phases={phases} conn={conn} kv={} kvar={}",
                c.name,
                self.bus_ref(&c.bus, &node_map),
                num(kv),
                num(c.q_rated / 1e3),
            );
            line.push_str(&self.extras_tail("capacitor", &c.name, &extras));
            self.line_out(&line);
        }
    }

    fn shunts(&mut self, net: &MulticonductorNetwork) {
        for sh in net.shunts() {
            let stashed_delta = shunt_stashed_delta(sh);
            let inferred_phases =
                extras_usize(&sh.extras, "phases").unwrap_or_else(|| sh.terminal_map.len().max(1));
            let conn_delta = stashed_delta
                || looks_like_delta_shunt(&sh.b, sh.terminal_map.len(), inferred_phases);
            let phases = self.shunt_phases(sh, conn_delta, inferred_phases);
            if has_nonzero(&sh.g) {
                self.write_impedance_shunt(sh, phases);
            } else {
                self.write_kvar_shunt(sh, phases, conn_delta);
            }
        }
        self.out.push('\n');
    }

    fn generators(&mut self, net: &MulticonductorNetwork) {
        for g in net.generators() {
            self.check_name("generator", &g.name);
            let node_map = self.return_last("generator", &g.name, &g.bus, &g.terminal_map);
            let phases = extras_usize(&g.extras, "phases")
                .or_else(|| {
                    let count = [
                        Some(g.p_nom.as_slice()),
                        g.p_min.as_deref(),
                        g.p_max.as_deref(),
                        g.q_min.as_deref(),
                        g.q_max.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .map(<[f64]>::len)
                    .max()
                    .unwrap_or(0);
                    (count > 0).then_some(count)
                })
                .unwrap_or_else(|| {
                    self.element_phases(
                        &g.extras,
                        &g.terminal_map,
                        g.configuration,
                        "generator",
                        &g.name,
                    )
                })
                .max(1);
            let conn = self.element_conn(&g.extras, g.configuration, &g.bus, &g.terminal_map);
            let nconds = nconds_for(conn, phases);
            self.warn_map_arity("generator", &g.name, g.terminal_map.len(), nconds);
            let kw: f64 = g.p_nom.iter().sum::<f64>() / 1e3;
            let kvar: f64 = g.q_nom.iter().sum::<f64>() / 1e3;
            let kv = self.element_kv(
                &g.extras,
                ElementKv {
                    bus: &g.bus,
                    phases,
                    configuration: g.configuration,
                    name: &g.name,
                    class: "generator",
                    typed_kv: None,
                },
            );
            let mut s = format!(
                "New Generator.{} bus1={} phases={phases} conn={conn} kv={} kw={} kvar={}",
                g.name,
                self.bus_ref(&g.bus, &node_map),
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
                self.warn(
                    &C::EMIT_DSS_FIELD_DROPPED,
                    format!(
                        "generator {}: generation cost has no dss field; dropped",
                        g.name
                    ),
                );
            }
            // Rating fields await the kVA mapping decision (#266); dropping
            // them stays loud in the meantime.
            for (key, present) in [("s_max", g.s_max.is_some()), ("i_max", g.i_max.is_some())] {
                if present {
                    self.warn(
                        &C::EMIT_DSS_FIELD_DROPPED,
                        format!(
                            "generator {}: `{key}` has no dss Generator field mapping yet; dropped",
                            g.name
                        ),
                    );
                }
            }
            let mut extras = g.extras.clone();
            strip_emitted_extras(&mut extras, &["kv", "phases", "conn"]);
            s.push_str(&self.extras_tail("generator", &g.name, &extras));
            self.line_out(&s);
        }
    }

    fn ibrs(&mut self, net: &MulticonductorNetwork) {
        for ibr in net.ibrs() {
            self.check_name("pvsystem", &ibr.name);
            if ibr_is_fixed_dispatch(ibr) {
                self.write_fixed_ibr_generator(ibr);
            } else {
                self.write_pvsystem(ibr, net);
            }
        }
        for ibr in net.ibrs() {
            if !ibr_is_fixed_dispatch(ibr) {
                self.write_ibr_controls(ibr, net);
            }
        }
        if !net.ibrs().is_empty() {
            self.out.push('\n');
        }
    }

    fn write_fixed_ibr_generator(&mut self, ibr: &DistIbr) {
        let node_map = self.return_last("ibr", &ibr.name, &ibr.bus, &ibr.terminal_map);
        let phases = ibr_phases(ibr);
        let configuration = ibr_configuration(ibr);
        let conn = self.element_conn(&ibr.extras, configuration, &ibr.bus, &ibr.terminal_map);
        let kv = self.ibr_kv(ibr, phases, configuration, "generator");
        let kw = ibr
            .p_min
            .as_ref()
            .map_or(0.0, |p| p.iter().sum::<f64>() / 1e3);
        let kvar = ibr
            .q_min
            .as_ref()
            .map_or(0.0, |q| q.iter().sum::<f64>() / 1e3);
        let mut line = format!(
            "New Generator.{} bus1={} phases={phases} conn={conn} kv={} kw={} kvar={} model=1 vminpu=0 vmaxpu=2",
            ibr.name,
            self.bus_ref(&ibr.bus, &node_map),
            num(kv),
            num(kw),
            num(kvar),
        );
        if let Some(q) = &ibr.q_max {
            let _ = write!(line, " maxkvar={}", num(q.iter().sum::<f64>() / 1e3));
        }
        if let Some(q) = &ibr.q_min {
            let _ = write!(line, " minkvar={}", num(q.iter().sum::<f64>() / 1e3));
        }
        self.warn_ibr_dss_drops(ibr);
        self.line_out(&line);
    }

    fn write_pvsystem(&mut self, ibr: &DistIbr, net: &MulticonductorNetwork) {
        let node_map = self.return_last("ibr", &ibr.name, &ibr.bus, &ibr.terminal_map);
        let phases = ibr_phases(ibr);
        let configuration = ibr_configuration(ibr);
        let conn = self.element_conn(&ibr.extras, configuration, &ibr.bus, &ibr.terminal_map);
        let kv = self.ibr_kv(ibr, phases, configuration, "pvsystem");
        let kva = ibr.s_max.iter().sum::<f64>() / 1e3;
        let pmpp = ibr
            .p_avail
            .or_else(|| ibr.p_max.as_ref().map(|p| p.iter().sum()))
            .unwrap_or(0.0)
            / 1e3;
        let mut line = format!(
            "New PVSystem.{} bus1={} phases={phases} conn={conn} kv={} kVA={} Pmpp={} irradiance=1 %Pmpp=100 WattPriority=No VarFollowInverter=Yes",
            ibr.name,
            self.bus_ref(&ibr.bus, &node_map),
            num(kv),
            num(kva),
            num(pmpp),
        );
        if let Some(q) = &ibr.q_max {
            let _ = write!(line, " kvarMax={}", num(q.iter().sum::<f64>() / 1e3));
        }
        if let Some(q) = &ibr.q_min {
            let _ = write!(
                line,
                " kvarMaxAbs={}",
                num(q.iter().map(|v| v.abs()).sum::<f64>() / 1e3)
            );
        }
        if let Some(profile) = ibr_profile(ibr, net) {
            if let Some(pf) = &profile.power_factor {
                let _ = write!(line, " pf={}", num(pf.pf));
            }
            if let Some(vv) = &profile.volt_var {
                if let Some(v) = vv.p_min_for_q {
                    let _ = write!(line, " %PminNoVars={}", num(v));
                }
                if let Some(v) = vv.p_min_for_q_max {
                    let _ = write!(line, " %PminkvarMax={}", num(v));
                }
            }
        }
        self.warn_ibr_dss_drops(ibr);
        self.line_out(&line);
    }

    fn write_ibr_controls(&mut self, ibr: &DistIbr, net: &MulticonductorNetwork) {
        let Some(profile) = ibr_profile(ibr, net) else {
            if let Some(name) = &ibr.control_profile {
                self.warn(
                    &C::EMIT_DSS_RECORD_DROPPED,
                    format!(
                        "ibr {}: control_profile `{name}` not found; no InvControl emitted",
                        ibr.name
                    ),
                );
            }
            return;
        };
        let phases = ibr_phases(ibr);
        let configuration = ibr_configuration(ibr);
        let kv = self.ibr_kv(ibr, phases, configuration, "pvsystem");
        let base_v = if phases >= 2 && configuration != Configuration::Delta {
            kv * 1e3 / 3f64.sqrt()
        } else {
            kv * 1e3
        };
        let mut curves = Vec::new();
        let mut has_vv = false;
        let mut has_vw = false;
        if let Some(vv) = &profile.volt_var
            && let Some(curve) = self.volt_var_curve(ibr, vv, base_v)
        {
            curves.push(curve);
            has_vv = true;
        }
        if let Some(vw) = &profile.volt_watt
            && let Some(curve) = self.volt_watt_curve(ibr, vw, base_v)
        {
            curves.push(curve);
            has_vw = true;
        }
        for line in &curves {
            self.line_out(line);
        }
        if !has_vv && !has_vw {
            return;
        }
        let mon = self.control_mon_voltage(ibr, profile);
        let inv_name = format!("ivc_{}", ibr.name);
        self.check_name("invcontrol", &inv_name);
        let mut line = format!(
            "New InvControl.{inv_name} DERList=[PVSystem.{}] voltage_curvex_ref=rated monVoltageCalc={mon}",
            ibr.name
        );
        match (has_vv, has_vw) {
            (true, true) => {
                let _ = write!(
                    line,
                    " CombiMode=VV_VW vvc_curve1=vv_{} voltwatt_curve=vw_{}",
                    ibr.name, ibr.name
                );
                if let Some(vv) = &profile.volt_var {
                    let _ = write!(
                        line,
                        " RefReactivePower={}",
                        reactive_reference(vv.q_ref.unwrap_or(ReactivePowerReference::VarMax))
                    );
                }
                if let Some(vw) = &profile.volt_watt {
                    let _ = write!(
                        line,
                        " VoltwattYAxis={}",
                        active_reference(vw.p_ref.unwrap_or(ActivePowerReference::SMax))
                    );
                }
            }
            (true, false) => {
                line.push_str(" mode=VOLTVAR");
                let _ = write!(line, " vvc_curve1=vv_{}", ibr.name);
                if let Some(vv) = &profile.volt_var {
                    let _ = write!(
                        line,
                        " RefReactivePower={}",
                        reactive_reference(vv.q_ref.unwrap_or(ReactivePowerReference::VarMax))
                    );
                }
            }
            (false, true) => {
                line.push_str(" mode=VOLTWATT");
                let _ = write!(line, " voltwatt_curve=vw_{}", ibr.name);
                if let Some(vw) = &profile.volt_watt {
                    let _ = write!(
                        line,
                        " VoltwattYAxis={}",
                        active_reference(vw.p_ref.unwrap_or(ActivePowerReference::SMax))
                    );
                }
            }
            (false, false) => {}
        }
        self.line_out(&line);
    }

    fn volt_var_curve(
        &mut self,
        ibr: &DistIbr,
        vv: &VoltVarControl,
        base_v: f64,
    ) -> Option<String> {
        self.check_control_reference(ibr, vv.voltage_reference)?;
        if !matches!(
            vv.q_unit,
            None | Some(crate::model::ReactivePowerUnit::VaFraction)
        ) {
            self.warn(
                &C::EMIT_DSS_FIELD_DROPPED,
                format!(
                    "ibr {}: volt_var q_unit is absolute VAR; DSS export only maps VA_FRACTION",
                    ibr.name
                ),
            );
            return None;
        }
        if vv.breakpoints.len() < 4 || vv.q_limits.len() < 2 || base_v <= 0.0 {
            self.warn(
                &C::EMIT_DSS_RECORD_DROPPED,
                format!(
                    "ibr {}: volt_var profile is incomplete; no XYcurve emitted",
                    ibr.name
                ),
            );
            return None;
        }
        let xs: Vec<String> = vv
            .breakpoints
            .iter()
            .take(4)
            .map(|v| num(v / base_v))
            .collect();
        let ys = [num(vv.q_limits[1]), num(0.0), num(0.0), num(vv.q_limits[0])];
        Some(format!(
            "New XYcurve.vv_{} npts=4 Xarray=[{}] Yarray=[{}]",
            ibr.name,
            xs.join(" "),
            ys.join(" ")
        ))
    }

    fn volt_watt_curve(
        &mut self,
        ibr: &DistIbr,
        vw: &VoltWattControl,
        base_v: f64,
    ) -> Option<String> {
        self.check_control_reference(ibr, vw.voltage_reference)?;
        if !matches!(
            vw.p_unit,
            None | Some(crate::model::ActivePowerUnit::VaFraction)
        ) {
            self.warn(
                &C::EMIT_DSS_FIELD_DROPPED,
                format!(
                    "ibr {}: volt_watt p_unit is absolute W; DSS export only maps VA_FRACTION",
                    ibr.name
                ),
            );
            return None;
        }
        if vw.breakpoints.len() < 2 || vw.p_limits.len() < 2 || base_v <= 0.0 {
            self.warn(
                &C::EMIT_DSS_RECORD_DROPPED,
                format!(
                    "ibr {}: volt_watt profile is incomplete; no XYcurve emitted",
                    ibr.name
                ),
            );
            return None;
        }
        let xs: Vec<String> = vw
            .breakpoints
            .iter()
            .take(2)
            .map(|v| num(v / base_v))
            .collect();
        let ys = [num(vw.p_limits[1]), num(vw.p_limits[0])];
        Some(format!(
            "New XYcurve.vw_{} npts=2 Xarray=[{}] Yarray=[{}]",
            ibr.name,
            xs.join(" "),
            ys.join(" ")
        ))
    }

    fn check_control_reference(
        &mut self,
        ibr: &DistIbr,
        reference: Option<ControlVoltageReference>,
    ) -> Option<()> {
        match reference.unwrap_or(ControlVoltageReference::PnPerPhase) {
            ControlVoltageReference::PgPerPhase | ControlVoltageReference::PgAveraged => Some(()),
            ControlVoltageReference::PnPerPhase | ControlVoltageReference::PnAveraged => {
                self.warn(&C::EMIT_DSS_VALUE_SUBSTITUTED, format!(
                    "ibr {}: PN voltage control is approximated by OpenDSS phase-to-ground InvControl",
                    ibr.name
                ));
                Some(())
            }
            ControlVoltageReference::PpPerPhase | ControlVoltageReference::PpAveraged => {
                self.warn(&C::EMIT_DSS_RECORD_DROPPED, format!(
                    "ibr {}: PP voltage control is not representable by OpenDSS InvControl; skipped",
                    ibr.name
                ));
                None
            }
        }
    }

    fn control_mon_voltage(&mut self, ibr: &DistIbr, profile: &DistControlProfile) -> &'static str {
        let reference = profile
            .volt_var
            .as_ref()
            .and_then(|vv| vv.voltage_reference)
            .or_else(|| {
                profile
                    .volt_watt
                    .as_ref()
                    .and_then(|vw| vw.voltage_reference)
            })
            .unwrap_or(ControlVoltageReference::PnPerPhase);
        let averaged = matches!(
            ibr.voltage_aggregation,
            Some(IbrVoltageAggregation::Average)
        ) || matches!(
            reference,
            ControlVoltageReference::PgAveraged
                | ControlVoltageReference::PnAveraged
                | ControlVoltageReference::PpAveraged
        );
        if !averaged && ibr_phases(ibr) > 1 {
            self.warn(
                &C::EMIT_DSS_FIELD_DROPPED,
                format!(
                    "ibr {}: per phase InvControl needs split PVSystems; emitted AVG monitor",
                    ibr.name
                ),
            );
        }
        "AVG"
    }

    fn ibr_kv(
        &mut self,
        ibr: &DistIbr,
        phases: usize,
        configuration: Configuration,
        class: &'static str,
    ) -> f64 {
        self.element_kv(
            &ibr.extras,
            ElementKv {
                bus: &ibr.bus,
                phases,
                configuration,
                name: &ibr.name,
                class,
                typed_kv: None,
            },
        )
    }

    fn warn_ibr_dss_drops(&mut self, ibr: &DistIbr) {
        for key in ibr.extras.keys() {
            if matches!(key.as_str(), "kv" | "phases") {
                continue;
            }
            self.warn(
                &C::EMIT_DSS_FIELD_DROPPED,
                format!(
                    "ibr {}: `{key}` has no OpenDSS export mapping; dropped",
                    ibr.name
                ),
            );
        }
        if ibr.i_max.is_some() {
            self.warn(
                &C::EMIT_DSS_FIELD_DROPPED,
                format!(
                    "ibr {}: i_max has no OpenDSS PVSystem current limit field; dropped",
                    ibr.name
                ),
            );
        }
        if !matches!(ibr.prime_mover, IbrPrimeMover::Pv | IbrPrimeMover::Generic) {
            self.warn(&C::EMIT_DSS_FIELD_DROPPED, format!(
                "ibr {}: prime_mover {:?} has no dedicated OpenDSS export path; emitted with the generic inverter mapping",
                ibr.name, ibr.prime_mover
            ));
        }
    }
}

/// Drop the shunt keys the writer regenerates from the typed model so a stale
/// copy is not re-emitted in the extras tail.
fn strip_shunt_extras(extras: &mut Extras) {
    for key in ["kv", "kvar", "phases", "conn"] {
        extras.remove(key);
    }
}

fn ibr_is_fixed_dispatch(ibr: &DistIbr) -> bool {
    ibr.control_profile.is_none()
        && matches!((&ibr.p_min, &ibr.p_max), (Some(a), Some(b)) if a == b)
        && matches!((&ibr.q_min, &ibr.q_max), (Some(a), Some(b)) if a == b)
}

fn ibr_profile<'a>(
    ibr: &DistIbr,
    net: &'a MulticonductorNetwork,
) -> Option<&'a DistControlProfile> {
    let name = ibr.control_profile.as_ref()?;
    net.control_profiles()
        .iter()
        .find(|profile| profile.name.eq_ignore_ascii_case(name))
}

fn ibr_phases(ibr: &DistIbr) -> usize {
    match ibr.topology {
        IbrTopology::SinglePhase => 1,
        IbrTopology::ThreeLeg | IbrTopology::FourLeg => 3,
    }
}

fn ibr_configuration(ibr: &DistIbr) -> Configuration {
    match ibr.topology {
        IbrTopology::SinglePhase => Configuration::SinglePhase,
        IbrTopology::ThreeLeg => Configuration::Delta,
        IbrTopology::FourLeg => Configuration::Wye,
    }
}

fn reactive_reference(reference: ReactivePowerReference) -> &'static str {
    match reference {
        ReactivePowerReference::VarMax => "VARMAX",
        ReactivePowerReference::VarAvailable => "VARAVAL_WATTS",
    }
}

fn active_reference(reference: ActivePowerReference) -> &'static str {
    match reference {
        ActivePowerReference::SMax => "KVARATINGPU",
        ActivePowerReference::PAvailable => "PAVAILABLEPU",
        ActivePowerReference::PMax => "PMPPPU",
    }
}

fn has_nonzero(m: &ConductorMatrix) -> bool {
    m.iter().flatten().any(|&v| v != 0.0)
}

fn has_off_diagonal(m: &ConductorMatrix) -> bool {
    m.iter()
        .enumerate()
        .any(|(i, row)| row.iter().enumerate().any(|(j, &v)| i != j && v != 0.0))
}

/// The permutation that moves conductor `k` last.
fn return_permutation(k: usize, n: usize) -> Vec<usize> {
    (0..n).filter(|&i| i != k).chain([k]).collect()
}

/// Symmetric densified read of a possibly lower triangular matrix, permuted:
/// entry `(i, j)` of the result is the stated `(perm[i], perm[j])` value,
/// read from either triangle.
fn permute_symmetric(m: &ConductorMatrix, perm: &[usize]) -> ConductorMatrix {
    let at = |i: usize, j: usize| {
        m.get(i)
            .and_then(|row| row.get(j))
            .copied()
            .or_else(|| m.get(j).and_then(|row| row.get(i)).copied())
            .unwrap_or(0.0)
    };
    perm.iter()
        .map(|&i| perm.iter().map(|&j| at(i, j)).collect())
        .collect()
}

/// A terminal name list permuted.
fn permute_names(map: &[String], perm: &[usize]) -> Vec<String> {
    perm.iter().map(|&i| map[i].clone()).collect()
}

/// A per conductor vector permuted, short vectors padded as unbounded.
fn permute_padded(v: &[f64], perm: &[usize]) -> Vec<f64> {
    perm.iter()
        .map(|&i| v.get(i).copied().unwrap_or(f64::INFINITY))
        .collect()
}

fn diag_at(m: &ConductorMatrix, i: usize) -> f64 {
    m.get(i).and_then(|row| row.get(i)).copied().unwrap_or(0.0)
}

fn matrix_scale(m: &ConductorMatrix) -> f64 {
    m.iter().flatten().fold(0.0_f64, |acc, &v| acc.max(v.abs()))
}

fn close(a: f64, b: f64, scale: f64) -> bool {
    (a - b).abs() <= 1e-12_f64.max(scale * 1e-9)
}

fn first_diag_admittance(
    g: &ConductorMatrix,
    b: &ConductorMatrix,
    phases: usize,
) -> Option<(f64, f64)> {
    (0..phases.max(1)).find_map(|i| {
        let gi = diag_at(g, i);
        let bi = diag_at(b, i);
        (gi != 0.0 || bi != 0.0).then_some((gi, bi))
    })
}

fn uniform_diag_admittance(
    g: &ConductorMatrix,
    b: &ConductorMatrix,
    phases: usize,
    g0: f64,
    b0: f64,
) -> bool {
    let scale = matrix_scale(g)
        .max(matrix_scale(b))
        .max(g0.abs())
        .max(b0.abs());
    (0..phases.max(1)).all(|i| close(diag_at(g, i), g0, scale) && close(diag_at(b, i), b0, scale))
}

fn shunt_stashed_delta(sh: &crate::model::DistShunt) -> bool {
    sh.extras
        .get("conn")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t.to_ascii_lowercase().starts_with('d') || t.eq_ignore_ascii_case("ll"))
}

fn mat_at(m: &ConductorMatrix, i: usize, j: usize) -> f64 {
    m.get(i).and_then(|row| row.get(j)).copied().unwrap_or(0.0)
}

fn looks_like_delta_shunt(b: &ConductorMatrix, terminals: usize, phases: usize) -> bool {
    if terminals < 2 || !has_off_diagonal(b) {
        return false;
    }
    let edges = delta_edges(terminals, phases);
    delta_branch_susceptance(b, &edges, terminals).is_some()
}

fn delta_branch_abs(b: &ConductorMatrix, edges: &[(usize, usize)]) -> Option<f64> {
    if edges.is_empty() {
        return None;
    }
    // Average over every edge (a missing entry contributes 0), so the divisor
    // matches the `edges.len()` that `shunt_kvar` multiplies back in; counting
    // only present entries would over-scale the regenerated kvar on a ragged
    // matrix.
    let total: f64 = edges
        .iter()
        .map(|&(i, j)| {
            b.get(i)
                .and_then(|row| row.get(j))
                .copied()
                .unwrap_or(0.0)
                .abs()
        })
        .sum();
    Some(total / edges.len() as f64)
}

fn delta_branch_susceptance(
    b: &ConductorMatrix,
    edges: &[(usize, usize)],
    terminals: usize,
) -> Option<f64> {
    if terminals < 2 || edges.is_empty() {
        return None;
    }
    let scale = matrix_scale(b);
    if scale == 0.0 {
        return None;
    }
    let first = edges[0];
    let branch = -mat_at(b, first.0, first.1);
    if branch == 0.0 {
        return None;
    }
    let scale = scale.max(branch.abs());
    for (i, row) in b.iter().enumerate() {
        for (j, &value) in row.iter().enumerate() {
            if (i >= terminals || j >= terminals) && !close(value, 0.0, scale) {
                return None;
            }
        }
    }
    for i in 0..terminals {
        let incident = edges
            .iter()
            .filter(|&&(from, to)| from == i || to == i)
            .count() as f64;
        for j in 0..terminals {
            let linked = edges
                .iter()
                .any(|&(from, to)| (from == i && to == j) || (from == j && to == i));
            let expected = if i == j {
                incident * branch
            } else if linked {
                -branch
            } else {
                0.0
            };
            if !close(mat_at(b, i, j), expected, scale) {
                return None;
            }
        }
    }
    Some(branch)
}

fn shunt_kvar(
    sh: &crate::model::DistShunt,
    phases: usize,
    conn_delta: bool,
    edges: &[(usize, usize)],
    b_phase: f64,
    kv: f64,
) -> f64 {
    if conn_delta {
        let b_branch = delta_branch_abs(&sh.b, edges).unwrap_or(b_phase.abs());
        b_branch * (kv * 1e3) * (kv * 1e3) * edges.len() as f64 / 1e3
    } else {
        let v_phase = if matches!(phases, 2 | 3) {
            kv * 1e3 / 3f64.sqrt()
        } else {
            kv * 1e3
        };
        b_phase.abs() * v_phase * v_phase * phases as f64 / 1e3
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MulticonductorNetworkTables;
    use crate::model::{
        ControlVoltageReference, DistControlProfile, DistGenerator, DistIbr, DistLine,
        DistLineCode, DistLoad, DistShunt, DistSwitch, DistTransformer, DistWinding, IbrPrimeMover,
        IbrTopology, ReactivePowerReference, ReactivePowerUnit, VoltVarControl, VoltageSource,
    };
    use crate::testkit::parse_dss_str;

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

    /// A source bus and a secondary spelled the way a center tapped service
    /// is: two hot terminals with the grounded return between them.
    fn center_tap_service(vln: f64) -> (DistBus, VoltageSource, DistBus) {
        let (mut source, vs) = three_phase_source(vln);
        source.id = "sb".into();
        (source, vs, bus("lv", &["p1", "n", "p2"], &["n"]))
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
            voltage_model: DistLoadVoltageModel::ConstantPower { v_nom: Vec::new() },
            extras: Extras::from([("kv".to_string(), serde_json::json!("0.4"))]),
        }
    }

    fn terminal_order_network(map: &[&str]) -> MulticonductorNetwork {
        let (source_bus, source) = three_phase_source(240.0);
        let element_map = strings(map);

        let capacitor = crate::model::DistCapacitor::new(
            "cap",
            "lv",
            element_map.clone(),
            Configuration::SinglePhase,
            1_000.0,
            240.0,
        );
        let mut generator = DistGenerator::new(
            "gen",
            "lv",
            element_map.clone(),
            Configuration::SinglePhase,
            vec![1_000.0],
            vec![0.0],
        );
        generator
            .extras
            .insert("kv".into(), serde_json::json!(0.24));

        let mut fixed_ibr = DistIbr::new(
            "fixed_ibr",
            "lv",
            element_map.clone(),
            IbrTopology::SinglePhase,
            IbrPrimeMover::Pv,
            vec![1_000.0],
        );
        fixed_ibr.p_min = Some(vec![1_000.0]);
        fixed_ibr.p_max = Some(vec![1_000.0]);
        fixed_ibr.q_min = Some(vec![0.0]);
        fixed_ibr.q_max = Some(vec![0.0]);
        fixed_ibr
            .extras
            .insert("kv".into(), serde_json::json!(0.24));

        let mut pv_ibr = DistIbr::new(
            "pv_ibr",
            "lv",
            element_map.clone(),
            IbrTopology::SinglePhase,
            IbrPrimeMover::Pv,
            vec![1_000.0],
        );
        pv_ibr.p_avail = Some(1_000.0);
        pv_ibr.extras.insert("kv".into(), serde_json::json!(0.24));

        MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            name: Some("terminal_order".into()),
            base_frequency: 60.0,
            buses: vec![source_bus, bus("lv", map, &["n"])],
            sources: vec![source],
            lines: vec![DistLine::new(
                "l1",
                "lv",
                "lv",
                element_map.clone(),
                element_map.clone(),
                "lc",
                1.0,
            )],
            switches: vec![DistSwitch::new(
                "sw1",
                "lv",
                "lv",
                element_map.clone(),
                element_map,
                false,
            )],
            generators: vec![generator],
            ibrs: vec![fixed_ibr, pv_ibr],
            capacitors: vec![capacitor],
            ..MulticonductorNetworkTables::default()
        })
    }

    fn terminal_order_diagnostics(
        out: &crate::convert::TextEmission,
    ) -> Vec<&crate::diagnostics::Diagnostic> {
        out.diagnostics
            .iter()
            .filter(|d| d.code() == C::EMIT_DSS_TERMINAL_ORDER_UNREPRESENTABLE.code)
            .collect()
    }

    fn roundtrip(net: &MulticonductorNetwork) -> (String, String) {
        let first = emit_dss_text(net);
        let second = emit_dss_text(&parse_dss_str(&first.text));
        (first.text, second.text)
    }

    #[test]
    fn constant_power_loads_get_wide_voltage_bounds_by_default() {
        let (b, vs) = three_phase_source(2400.0);
        let load = load_on("sb", &["1"], Configuration::Wye);
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out.text.lines().find(|l| l.contains("Load.ld")).unwrap();
        assert!(line.contains("vminpu=0"), "{line}");
        assert!(line.contains("vmaxpu=2"), "{line}");
    }

    #[test]
    fn explicit_load_voltage_bounds_are_preserved() {
        let (b, vs) = three_phase_source(2400.0);
        let mut load = load_on("sb", &["1"], Configuration::Wye);
        load.extras.insert("vminpu".into(), serde_json::json!(0.8));
        load.extras.insert("vmaxpu".into(), serde_json::json!(1.2));
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out.text.lines().find(|l| l.contains("Load.ld")).unwrap();
        assert!(line.contains("vminpu=0.8"), "{line}");
        assert!(line.contains("vmaxpu=1.2"), "{line}");
    }

    #[test]
    fn default_load_voltage_bounds_can_be_disabled() {
        let (b, vs) = three_phase_source(2400.0);
        let load = load_on("sb", &["1"], Configuration::Wye);
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..MulticonductorNetworkTables::default()
        });
        let options = DssEmitOptions {
            default_load_voltage_bounds: None,
            ..DssEmitOptions::default()
        };
        let out = emit_dss_text_with_options(&net, &options);
        let line = out.text.lines().find(|l| l.contains("Load.ld")).unwrap();
        assert!(!line.contains("vminpu="), "{line}");
        assert!(!line.contains("vmaxpu="), "{line}");
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
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            name: Some("t".into()),
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ..MulticonductorNetworkTables::default()
        });
        let (first, second) = roundtrip(&net);
        assert!(first.contains("Set VoltageBases="), "{first}");
        assert_eq!(first, second);
    }

    #[test]
    fn load_phases_prefer_the_reader_stash() {
        let (b, vs) = three_phase_source(2400.0);
        let mut load = load_on("sb", &["1", "2", "3"], Configuration::Delta);
        load.extras.insert("phases".into(), serde_json::json!("2"));
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out.text.lines().find(|l| l.contains("Load.ld")).unwrap();
        assert!(line.contains("phases=2 conn=delta"), "{line}");
        // The stash must not double emit through the extras tail.
        assert_eq!(line.matches("phases=").count(), 1, "{line}");
        assert!(
            !out.render_diagnostics()
                .iter()
                .any(|w| w.contains("2 or 3 phase"))
        );
    }

    #[test]
    fn ambiguous_delta_keeps_three_phases_loudly() {
        let (b, vs) = three_phase_source(2400.0);
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load_on("sb", &["1", "2", "3"], Configuration::Delta)],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out.text.lines().find(|l| l.contains("Load.ld")).unwrap();
        assert!(line.contains("phases=3 conn=delta"), "{line}");
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("2 or 3 phase")),
            "{:?}",
            out.render_diagnostics()
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
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![two_wire, stashed],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
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
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            name: Some("my circuit".into()),
            base_frequency: 60.0,
            buses: vec![b, bus("a=b", &["1"], &[])],
            sources: vec![vs],
            loads: vec![load],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let hits = |needle: &str| {
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains(needle) && w.contains("cannot represent"))
        };
        assert!(hits("load 1"), "{:?}", out.render_diagnostics());
        assert!(hits("my circuit"), "{:?}", out.render_diagnostics());
        // The bad bus id warns at its bus_ref emission site.
        let mut net2 = net.clone();
        net2.lines_mut().push(DistLine {
            name: "l1".into(),
            bus_from: "sb".into(),
            bus_to: "a=b".into(),
            terminal_map_from: strings(&["1"]),
            terminal_map_to: strings(&["1"]),
            linecode: "lc".into(),
            length: 1.0,
            route: None,
            i_max: None,
            s_max: None,
            extras: Extras::new(),
        });
        let out2 = emit_dss_text(&net2);
        assert!(
            out2.render_diagnostics()
                .iter()
                .any(|w| w.contains("a=b") && w.contains("cannot represent")),
            "{:?}",
            out2.render_diagnostics()
        );
    }

    #[test]
    fn unequal_per_phase_i_max_warns_that_emergamps_holds_one_phase() {
        let (b, vs) = three_phase_source(2400.0);
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b, bus("b2", &["1", "2", "3"], &[])],
            sources: vec![vs],
            lines: vec![DistLine {
                name: "l1".into(),
                bus_from: "sb".into(),
                bus_to: "b2".into(),
                terminal_map_from: strings(&["1", "2", "3"]),
                terminal_map_to: strings(&["1", "2", "3"]),
                linecode: "lc".into(),
                length: 1.0,
                route: None,
                i_max: Some(vec![400.0, 300.0, 200.0]),
                s_max: None,
                extras: Extras::new(),
            }],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out.text.lines().find(|l| l.contains("Line.l1 ")).unwrap();
        assert!(line.contains("emergamps=400"), "{line}");
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("line l1") && w.contains("not equal on all phases")),
            "{:?}",
            out.render_diagnostics()
        );
    }

    #[test]
    fn line_level_i_max_emits_emergamps_and_s_max_drops_with_a_warning() {
        let (b, vs) = three_phase_source(2400.0);
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b, bus("b2", &["1"], &[])],
            sources: vec![vs],
            lines: vec![DistLine {
                name: "l1".into(),
                bus_from: "sb".into(),
                bus_to: "b2".into(),
                terminal_map_from: strings(&["1"]),
                terminal_map_to: strings(&["1"]),
                linecode: "lc".into(),
                length: 1.0,
                route: None,
                i_max: Some(vec![400.0]),
                s_max: Some(vec![600.0]),
                extras: Extras::new(),
            }],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out.text.lines().find(|l| l.contains("Line.l1 ")).unwrap();
        assert!(line.contains("emergamps=400"), "{line}");
        assert!(
            !out.render_diagnostics()
                .iter()
                .any(|w| w.contains("line l1") && w.contains("i_max")),
            "{:?}",
            out.render_diagnostics()
        );
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("line l1") && w.contains("s_max") && w.contains("dropped")),
            "{:?}",
            out.render_diagnostics()
        );
    }

    #[test]
    fn line_level_emergamps_round_trips_as_i_max() {
        let src = "Clear\n\
                   New Circuit.c1 basekv=12.47 pu=1 angle=0 phases=3 bus1=sb.1.2.3\n\
                   New Linecode.lc nphases=3 r1=0.1 x1=0.2 emergamps=600\n\
                   New Line.l1 bus1=sb.1.2.3 bus2=b2.1.2.3 phases=3 linecode=lc \
                   length=10 units=m emergamps=250\n\
                   New Line.l2 bus1=b2.1.2.3 bus2=b3.1.2.3 phases=3 linecode=lc \
                   length=10 units=m\n";
        let net = parse_dss_str(src);
        let l1 = net.lines().iter().find(|l| l.name == "l1").unwrap();
        assert_eq!(l1.i_max.as_deref(), Some(&[250.0, 250.0, 250.0][..]));
        assert!(!l1.extras.contains_key("emergamps"), "{:?}", l1.extras);
        // A line without its own rating defers to the linecode.
        let l2 = net.lines().iter().find(|l| l.name == "l2").unwrap();
        assert_eq!(l2.i_max, None);

        let (first, second) = roundtrip(&net);
        let line = first.lines().find(|l| l.contains("Line.l1 ")).unwrap();
        assert!(line.contains("emergamps=250"), "{line}");
        assert_eq!(line.matches("emergamps=").count(), 1, "{line}");
        let line2 = first.lines().find(|l| l.contains("Line.l2 ")).unwrap();
        assert!(!line2.contains("emergamps="), "{line2}");
        assert_eq!(first, second);
    }

    #[test]
    fn unparsable_line_emergamps_stays_in_extras_for_the_echo() {
        let src = "Clear\n\
                   New Circuit.c1 basekv=12.47 pu=1 angle=0 phases=3 bus1=sb.1.2.3\n\
                   New Linecode.lc nphases=3 r1=0.1 x1=0.2\n\
                   New Line.l1 bus1=sb.1.2.3 bus2=b2.1.2.3 phases=3 linecode=lc \
                   length=10 units=m emergamps=@amps\n";
        let net = parse_dss_str(src);
        let l1 = net.lines().iter().find(|l| l.name == "l1").unwrap();
        assert_eq!(l1.i_max, None);
        assert_eq!(
            l1.extras.get("emergamps").and_then(|v| v.as_str()),
            Some("@amps")
        );
        let (first, second) = roundtrip(&net);
        let line = first.lines().find(|l| l.contains("Line.l1 ")).unwrap();
        assert!(line.contains("emergamps=@amps"), "{line}");
        assert_eq!(first, second);
    }

    #[test]
    fn unparseable_kv_extra_warns_instead_of_silently_substituting() {
        let (b, vs) = three_phase_source(2400.0);
        let mut load = load_on("sb", &["1", "2", "3", "4"], Configuration::Wye);
        load.extras.insert("kv".into(), serde_json::json!("@kv"));
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("@kv") && w.contains("does not parse")),
            "{:?}",
            out.render_diagnostics()
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
        let out = emit_dss_text(&parse_dss_str(src));
        assert!(out.text.contains("Set mode=snapshot"), "{}", out.text);
        assert!(out.text.contains("Set controlmode=OFF"), "{}", out.text);
        // The writer derives these; the stored options must not double them.
        assert_eq!(out.text.matches("Set VoltageBases").count(), 1);
        assert_eq!(out.text.matches("Calcvoltagebases").count(), 1);
        assert_eq!(out.text.matches("DefaultBaseFrequency").count(), 1);
        assert!(!out.text.to_lowercase().contains("disable"));
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("disable Line.l1") && w.contains("not regenerated")),
            "{:?}",
            out.render_diagnostics()
        );
        // Solve and Calcvoltagebases re-derive; no warning claims they drop.
        assert!(
            !out.render_diagnostics()
                .iter()
                .any(|w| w.contains("`solve`"))
        );
        let again = emit_dss_text(&parse_dss_str(&out.text));
        assert_eq!(out.text, again.text);
    }

    #[test]
    fn non_numeric_terminal_positionalizes() {
        let mut load = load_on("b1", &["a", "n"], Configuration::Wye);
        load.extras.insert("kv".into(), serde_json::json!("0.23"));
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![bus("b1", &["a", "n"], &["n"])],
            loads: vec![load],
            ..MulticonductorNetworkTables::default()
        });
        let (first, second) = roundtrip(&net);
        let line = first.lines().find(|l| l.contains("Load.ld")).unwrap();
        assert!(line.contains("bus1=b1.1.0"), "{line}");
        let out = emit_dss_text(&net);
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("`a`") && w.contains("position")),
            "{:?}",
            out.render_diagnostics()
        );
        assert_eq!(first, second);
    }

    #[test]
    fn half_present_thevenin_pair_stays_and_warns() {
        let (b, mut vs) = three_phase_source(2400.0);
        vs.extras
            .insert("rs".into(), serde_json::json!([[1.0, 0.1], [0.1, 1.0]]));
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        assert!(!out.text.contains("z1="), "{}", out.text);
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("`xs` is missing")),
            "{:?}",
            out.render_diagnostics()
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
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b, bus("b2", &["1", "2", "3"], &[])],
            sources: vec![vs],
            switches: vec![sw],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        assert!(!out.text.contains("r0="), "{}", out.text);
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("pmd_rs") && w.contains("not a numeric matrix")),
            "{:?}",
            out.render_diagnostics()
        );
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("i_max is empty")),
            "{:?}",
            out.render_diagnostics()
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
            source: None,
            extras: Extras::new(),
        };
        let t = DistTransformer {
            name: "t1".into(),
            windings: vec![
                DistWinding {
                    bus: "sb".into(),
                    terminal_map: strings(&["1", "2"]),
                    conn: DistWindingConn::Wye,
                    v_ref: 2400.0,
                    s_rating: 25e3,
                    r_pct: 0.5,
                    tap: 1.0,
                    r_neutral: None,
                    x_neutral: None,
                },
                DistWinding {
                    bus: "b2".into(),
                    terminal_map: strings(&["1", "2"]),
                    conn: DistWindingConn::Wye,
                    v_ref: 240.0,
                    s_rating: 25e3,
                    r_pct: 0.5,
                    tap: 1.0,
                    r_neutral: None,
                    x_neutral: None,
                },
            ],
            xsc_pct: Vec::new(),
            phases: 1,
            extras: Extras::new(),
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b, bus("b2", &["1", "2"], &[])],
            sources: vec![vs],
            line_codes: vec![lc],
            transformers: vec![t],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net); // must not panic
        assert!(out.text.contains("rmatrix=(1 | 0.5 0)"), "{}", out.text);
        assert!(out.text.contains("xhl=0"), "{}", out.text);
        let has = |needle: &str| out.render_diagnostics().iter().any(|w| w.contains(needle));
        assert!(
            has("shorter than the lower triangle"),
            "{:?}",
            out.render_diagnostics()
        );
        assert!(has("xsc_pct is empty"), "{:?}", out.render_diagnostics());
        assert!(has("i_max is empty"), "{:?}", out.render_diagnostics());
    }

    #[test]
    fn a_rated_capacitor_bank_writes_as_a_dss_capacitor() {
        // #266 item 1: `q_rated` at `v_nom` is what an OpenDSS Capacitor takes,
        // so the conversion is a unit change and the terminal spelling. The
        // bank used to be dropped with a warning.
        let (b, vs) = three_phase_source(2400.0);
        let cap = crate::model::DistCapacitor::new(
            "c1",
            "sb",
            strings(&["1", "2", "3", "n"]),
            Configuration::Wye,
            600e3,
            4160.0,
        );
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            capacitors: vec![cap],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out
            .text
            .lines()
            .find(|l| l.contains("Capacitor.c1"))
            .unwrap_or_else(|| panic!("no capacitor emitted: {}", out.text));
        assert!(line.contains("kvar=600"), "{line}");
        assert!(line.contains("kv=4.16"), "{line}");
        assert!(line.contains("phases=3"), "{line}");
        assert!(
            !out.render_diagnostics()
                .iter()
                .any(|w| w.contains("dropped")),
            "{:?}",
            out.render_diagnostics()
        );

        // And it comes back: the reader lowers a dss Capacitor to a shunt B
        // matrix, so the bank survives as susceptance carrying the same vars.
        let back = parse_dss_str(&out.text);
        assert_eq!(back.shunts().len(), 1, "{}", out.text);
    }

    #[test]
    fn an_unbalanced_load_splits_into_one_load_per_phase() {
        // #266 item 2: a dss Load divides kw evenly across its phases, so one
        // balanced object keeps the total and loses the profile. Splitting
        // keeps both, and a balanced load still emits as one object.
        let (b, vs) = three_phase_source(2400.0);
        let mut unbalanced = DistLoad::new(
            "l1",
            "sb",
            strings(&["1", "2", "3", "n"]),
            Configuration::Wye,
            vec![1e3, 2e3, 3e3],
            vec![100.0, 200.0, 300.0],
        );
        unbalanced.extras.insert("kv".into(), 4.16.into());
        let balanced = DistLoad::new(
            "l2",
            "sb",
            strings(&["1", "2", "3", "n"]),
            Configuration::Wye,
            vec![1e3, 1e3, 1e3],
            vec![100.0, 100.0, 100.0],
        );
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![unbalanced, balanced],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let loads: Vec<&str> = out
            .text
            .lines()
            .filter(|l| l.contains("New Load."))
            .collect();
        assert_eq!(loads.len(), 4, "{}", out.text);
        for (name, kw, kvar) in [
            ("l1_1", "1", "0.1"),
            ("l1_2", "2", "0.2"),
            ("l1_3", "3", "0.3"),
        ] {
            let line = loads
                .iter()
                .find(|l| l.contains(&format!("New Load.{name} ")))
                .unwrap_or_else(|| panic!("no {name}: {}", out.text));
            assert!(line.contains(&format!("kw={kw} ")), "{line}");
            assert!(line.contains(&format!("kvar={kvar}")), "{line}");
            assert!(line.contains("phases=1"), "{line}");
            // The whole-bank kv extra does not carry to a single phase part.
            assert!(!line.contains("kv=4.16"), "{line}");
        }
        assert_eq!(
            loads.iter().filter(|l| l.contains("New Load.l2 ")).count(),
            1,
            "a balanced load stays one object: {}",
            out.text
        );
    }

    #[test]
    fn a_center_tap_load_splits_onto_its_two_legs() {
        // PowerIO.jl#79. A center tapped service maps as `[p1, n, p2]`, and
        // dss reads a node list positionally, so one record over that map
        // states the wrong node pair and drops the conductors it cannot
        // address. The powers are equal, so the imbalance split never fires.
        let (b, vs, lv) = center_tap_service(11000.0);
        let l = DistLoad {
            name: "ld".into(),
            bus: "lv".into(),
            terminal_map: strings(&["p1", "n", "p2"]),
            configuration: Configuration::Wye,
            p_nom: vec![1304.0, 1304.0],
            q_nom: vec![978.0, 978.0],
            voltage_model: DistLoadVoltageModel::ConstantImpedance { v_nom: Vec::new() },
            extras: Extras::new(),
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b, lv],
            sources: vec![vs],
            loads: vec![l],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let loads: Vec<&str> = out
            .text
            .lines()
            .filter(|l| l.contains("New Load."))
            .collect();
        assert_eq!(loads.len(), 2, "{}", out.text);
        // Each leg carries half the power over its own hot node and the
        // grounded return, which dss spells as node 0.
        for (name, node) in [("ld_p1", "lv.1.0"), ("ld_p2", "lv.3.0")] {
            let line = loads
                .iter()
                .find(|l| l.contains(&format!("New Load.{name} ")))
                .unwrap_or_else(|| panic!("no {name}: {}", out.text));
            assert!(line.contains(&format!("bus1={node} ")), "{line}");
            assert!(line.contains("phases=1"), "{line}");
            assert!(line.contains("kw=1.304"), "{line}");
            assert!(line.contains("kvar=0.978"), "{line}");
        }
    }

    #[test]
    fn misordered_grounded_returns_reorder_when_nothing_is_indexed() {
        let out = emit_dss_text(&terminal_order_network(&["p1", "n", "p2"]));
        let diagnostics = terminal_order_diagnostics(&out);

        // The four element classes and the switch carry no conductor indexed
        // data into their records, so their node lists reorder return-last
        // with a declared substitution. The line references linecode `lc`,
        // which this network does not define, so its matrices cannot be
        // permuted in step and the order error stands for both endpoints.
        assert_eq!(diagnostics.len(), 2, "{:?}", out.diagnostics);
        for diagnostic in &diagnostics {
            assert_eq!(diagnostic.details()["class"], "line");
            assert_eq!(diagnostic.details()["element_name"], "l1");
        }
        let reordered = out
            .diagnostics
            .iter()
            .filter(|d| d.message().contains("moved last"))
            .count();
        assert_eq!(reordered, 5, "{:?}", out.render_diagnostics());

        for expected in [
            "New Capacitor.cap bus1=lv.1.3.0 ",
            "New Generator.gen bus1=lv.1.3.0 ",
            "New Generator.fixed_ibr bus1=lv.1.3.0 ",
            "New PVSystem.pv_ibr bus1=lv.1.3.0 ",
            "New Line.sw1 bus1=lv.1.3.0 bus2=lv.1.3.0 phases=3 switch=y",
            "New Line.l1 bus1=lv.1.0.3 bus2=lv.1.0.3 phases=3 linecode=lc ",
        ] {
            assert!(
                out.text.contains(expected),
                "missing {expected:?}: {}",
                out.text
            );
        }
    }

    #[test]
    fn a_line_with_its_linecode_permutes_the_matrices_in_step() {
        let mut net = terminal_order_network(&["p1", "n", "p2"]);
        net.line_codes_mut().push(DistLineCode::new(
            "lc",
            vec![vec![1.0], vec![0.2, 2.0], vec![0.3, 0.4, 3.0]],
            vec![vec![10.0], vec![0.0, 20.0], vec![0.0, 0.0, 30.0]],
        ));
        let out = emit_dss_text(&net);

        assert_eq!(
            terminal_order_diagnostics(&out).len(),
            0,
            "{:?}",
            out.render_diagnostics()
        );
        // The line references the permuted copy and both node lists move the
        // return last.
        assert!(
            out.text
                .contains("New Line.l1 bus1=lv.1.3.0 bus2=lv.1.3.0 phases=3 linecode=lc_ret1 "),
            "{}",
            out.text
        );
        // Conductor order [p1, p2, n]: the permuted rmatrix rows and columns
        // move together, so row 2 is the old conductor 3 and the old mutual
        // 0.4 sits at (3, 2).
        assert!(
            out.text
                .contains("New Linecode.lc_ret1 nphases=3 units=m rmatrix=(1 | 0.3 3 | 0.2 0.4 2)"),
            "{}",
            out.text
        );
        // The original stays for any line that keeps its order.
        assert!(
            out.text
                .contains("New Linecode.lc nphases=3 units=m rmatrix=(1 | 0.2 2 | 0.3 0.4 3)"),
            "{}",
            out.text
        );
    }

    #[test]
    fn disagreeing_endpoint_returns_keep_the_order_error() {
        let mut net = terminal_order_network(&["p1", "n", "p2"]);
        // The same wire cannot hold the return at conductor 2 on one end and
        // conductor 1 on the other; no permutation is sound, so the order
        // errors stand for the line and the switch.
        net.lines_mut()[0].terminal_map_to = strings(&["n", "p1", "p2"]);
        net.switches_mut()[0].terminal_map_to = strings(&["n", "p1", "p2"]);
        let out = emit_dss_text(&net);
        let diagnostics = terminal_order_diagnostics(&out);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.details()["class"] == "switch" && d.details()["endpoint"] == "bus2"),
            "{:?}",
            out.render_diagnostics()
        );
        assert!(
            out.text.contains("New Line.sw1 bus1=lv.1.0.3 "),
            "{}",
            out.text
        );
    }

    #[test]
    fn grounded_return_last_needs_no_terminal_order_diagnostic() {
        let out = emit_dss_text(&terminal_order_network(&["p1", "p2", "n"]));
        assert!(
            terminal_order_diagnostics(&out).is_empty(),
            "{:?}",
            out.diagnostics
        );
        assert!(out.text.contains("bus1=lv.1.2.0"), "{}", out.text);
    }

    #[test]
    fn an_unbalanced_center_tap_load_keeps_each_leg_with_its_own_power() {
        // The balanced case cannot catch a swap. With the return conductor mid
        // map, taking the last terminal as the return pairs leg 1 with the
        // neutral and puts the second leg across both hots.
        let (b, vs, lv) = center_tap_service(11000.0);
        let l = DistLoad::new(
            "ld",
            "lv",
            strings(&["p1", "n", "p2"]),
            Configuration::Wye,
            vec![1000.0, 2000.0],
            vec![100.0, 200.0],
        );
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b, lv],
            sources: vec![vs],
            loads: vec![l],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let loads: Vec<&str> = out
            .text
            .lines()
            .filter(|l| l.contains("New Load."))
            .collect();
        assert_eq!(loads.len(), 2, "{}", out.text);
        for (name, node, kw, kvar) in [
            ("ld_p1", "lv.1.0", "1", "0.1"),
            ("ld_p2", "lv.3.0", "2", "0.2"),
        ] {
            let line = loads
                .iter()
                .find(|l| l.contains(&format!("New Load.{name} ")))
                .unwrap_or_else(|| panic!("no {name}: {}", out.text));
            assert!(line.contains(&format!("bus1={node} ")), "{line}");
            assert!(line.contains(&format!("kw={kw} ")), "{line}");
            assert!(line.contains(&format!("kvar={kvar}")), "{line}");
        }
        // No part lands on the neutral terminal or spans the two hot legs.
        assert!(!out.text.contains("New Load.ld_n "), "{}", out.text);
    }

    #[test]
    fn a_map_longer_than_the_record_says_what_dss_drops() {
        // The mirror of the short map warning. One power value over three
        // conductors cannot split, so the arity is all the writer can report.
        let (b, vs, lv) = center_tap_service(11000.0);
        let mut l = DistLoad::new(
            "ld",
            "lv",
            strings(&["p1", "n", "p2"]),
            Configuration::Wye,
            vec![2608.0],
            vec![1956.0],
        );
        l.extras.insert("phases".into(), 1.into());
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b, lv],
            sources: vec![vs],
            loads: vec![l],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        assert_eq!(
            out.text.lines().filter(|l| l.contains("New Load.")).count(),
            1,
            "{}",
            out.text
        );
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("addresses 2") && w.contains("loses them")),
            "{:?}",
            out.render_diagnostics()
        );
    }

    #[test]
    fn a_star_with_no_secondary_leakage_goes_out_solvable() {
        // PowerIO.jl#79 bug 3. BMOPF puts the whole leakage on the primary
        // arm, so the star back solves to xlt=0, which dss converges on with
        // the secondary legs collapsed to about half voltage.
        let (b, vs, lv) = center_tap_service(11000.0);
        let winding = |bus: &str, map: &[&str], v: f64| DistWinding {
            bus: bus.into(),
            terminal_map: strings(map),
            conn: DistWindingConn::Wye,
            v_ref: v,
            s_rating: 25e3,
            r_pct: 0.5,
            tap: 1.0,
            r_neutral: None,
            x_neutral: None,
        };
        let t = DistTransformer {
            name: "tx".into(),
            phases: 1,
            windings: vec![
                winding("sb", &["1", "4"], 11000.0),
                winding("lv", &["p1", "n"], 240.0),
                winding("lv", &["n", "p2"], 240.0),
            ],
            xsc_pct: vec![2.5, 2.5, 0.0],
            extras: Extras::new(),
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b, lv],
            sources: vec![vs],
            transformers: vec![t],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out
            .text
            .lines()
            .find(|l| l.contains("New Transformer.tx"))
            .unwrap_or_else(|| panic!("no transformer: {}", out.text));
        assert!(line.contains("xhl=2.5 xht=2.5 xlt=1.666"), "{line}");
        // The reversed third winding is the dss center tap spelling, not a
        // node order fault: the two halves are series additive.
        assert!(line.contains("buses=(sb.1.0, lv.1.0, lv.0.3)"), "{line}");
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("collapsed secondary")),
            "{:?}",
            out.render_diagnostics()
        );
    }

    #[test]
    fn a_delta_load_with_per_phase_power_stays_balanced_and_says_so() {
        // A delta load's phases sit across terminal pairs, so the split would
        // need branch geometry it does not have.
        let (b, vs) = three_phase_source(2400.0);
        let l = DistLoad::new(
            "d1",
            "sb",
            strings(&["1", "2", "3"]),
            Configuration::Delta,
            vec![1e3, 2e3, 3e3],
            vec![0.0, 0.0, 0.0],
        );
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![l],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        assert_eq!(
            out.text.lines().filter(|l| l.contains("New Load.")).count(),
            1,
            "{}",
            out.text
        );
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("per phase power on a delta load")),
            "{:?}",
            out.render_diagnostics()
        );
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
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            shunts: vec![sh],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
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
    fn inductive_shunt_regenerates_as_a_reactor() {
        // A negative diagonal susceptance is the grounding-reactor sign; it
        // must emit `New Reactor`, not a capacitor, with the positive kvar
        // rating recovered from |b| v^2.
        let (b, vs) = three_phase_source(2400.0);
        let b_phase = -1e-3;
        let sh = DistShunt {
            name: "rx".into(),
            bus: "sb".into(),
            terminal_map: strings(&["1", "2", "3"]),
            g: vec![vec![0.0; 3]; 3],
            b: vec![
                vec![b_phase, 0.0, 0.0],
                vec![0.0, b_phase, 0.0],
                vec![0.0, 0.0, b_phase],
            ],
            extras: Extras::new(),
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            shunts: vec![sh],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out
            .text
            .lines()
            .find(|l| l.contains("Reactor.rx"))
            .unwrap_or_else(|| panic!("no reactor emitted in:\n{}", out.text));
        assert!(!out.text.contains("Capacitor.rx"), "{}", out.text);
        let kv = 2400.0 * 3f64.sqrt() / 1e3;
        let v_phase = kv * 1e3 / 3f64.sqrt();
        let expected = b_phase.abs() * v_phase * v_phase * 3.0 / 1e3;
        assert!(line.contains(&format!("kvar={}", num(expected))), "{line}");
    }

    #[test]
    fn conductive_shunt_regenerates_as_grounding_reactor() {
        let (_, vs) = three_phase_source(2400.0);
        let b = bus("sb", &["1", "2", "3", "4"], &[]);
        let sh = DistShunt {
            name: "gnd".into(),
            bus: "sb".into(),
            terminal_map: strings(&["4"]),
            g: vec![vec![1.0 / 0.3]],
            b: vec![vec![0.0]],
            extras: Extras::new(),
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            shunts: vec![sh],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out
            .text
            .lines()
            .find(|l| l.contains("Reactor.gnd"))
            .unwrap_or_else(|| panic!("no reactor emitted in:\n{}", out.text));
        assert!(line.contains("bus1=sb.4"), "{line}");
        assert!(line.contains("bus2=sb.0"), "{line}");
        assert!(line.contains("phases=1"), "{line}");
        assert!(line.contains("r=0.3"), "{line}");
        assert!(line.contains("x=0"), "{line}");
        assert!(
            !line.contains("x=-0"),
            "negative zero must canonicalize: {line}"
        );
    }

    #[test]
    fn delta_shunt_regenerates_conn_delta() {
        let (b, vs) = three_phase_source(2400.0);
        let b_branch = 2e-4;
        let bmat = vec![
            vec![2.0 * b_branch, -b_branch, -b_branch],
            vec![-b_branch, 2.0 * b_branch, -b_branch],
            vec![-b_branch, -b_branch, 2.0 * b_branch],
        ];
        let mut extras = Extras::new();
        extras.insert("conn".into(), serde_json::json!("delta"));
        extras.insert("phases".into(), serde_json::json!("3"));
        let sh = DistShunt {
            name: "capd".into(),
            bus: "sb".into(),
            terminal_map: strings(&["1", "2", "3"]),
            g: vec![vec![0.0; 3]; 3],
            b: bmat,
            extras,
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            shunts: vec![sh],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out
            .text
            .lines()
            .find(|l| l.contains("Capacitor.capd"))
            .unwrap_or_else(|| panic!("no capacitor emitted in:\n{}", out.text));
        assert!(line.contains("phases=3 conn=delta"), "{line}");
        assert!(
            !out.render_diagnostics()
                .iter()
                .any(|w| w.contains("off diagonal")),
            "{:?}",
            out.render_diagnostics()
        );
    }

    #[test]
    fn non_scalar_delta_matrix_is_not_inferred_silently() {
        let (b, vs) = three_phase_source(2400.0);
        let bmat = vec![
            vec![0.003, -0.001, -0.002],
            vec![-0.001, 0.003, -0.002],
            vec![-0.002, -0.002, 0.004],
        ];
        let sh = DistShunt {
            name: "capx".into(),
            bus: "sb".into(),
            terminal_map: strings(&["1", "2", "3"]),
            g: vec![vec![0.0; 3]; 3],
            b: bmat,
            extras: Extras::new(),
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            shunts: vec![sh],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out
            .text
            .lines()
            .find(|l| l.contains("Capacitor.capx"))
            .unwrap_or_else(|| panic!("no capacitor emitted in:\n{}", out.text));
        assert!(line.contains("conn=wye"), "{line}");
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("off diagonal")),
            "{:?}",
            out.render_diagnostics()
        );
    }

    #[test]
    fn stashed_delta_matrix_warns_when_scalar_emission_is_lossy() {
        let (b, vs) = three_phase_source(2400.0);
        let bmat = vec![
            vec![0.003, -0.001, -0.002],
            vec![-0.001, 0.003, -0.002],
            vec![-0.002, -0.002, 0.004],
        ];
        let mut extras = Extras::new();
        extras.insert("conn".into(), serde_json::json!("delta"));
        extras.insert("phases".into(), serde_json::json!("3"));
        let sh = DistShunt {
            name: "capx".into(),
            bus: "sb".into(),
            terminal_map: strings(&["1", "2", "3"]),
            g: vec![vec![0.0; 3]; 3],
            b: bmat,
            extras,
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            shunts: vec![sh],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out
            .text
            .lines()
            .find(|l| l.contains("Capacitor.capx"))
            .unwrap_or_else(|| panic!("no capacitor emitted in:\n{}", out.text));
        assert!(line.contains("conn=delta"), "{line}");
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("no scalar capacitor expression")),
            "{:?}",
            out.render_diagnostics()
        );
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
        let first = emit_dss_text(&net);
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
                .render_diagnostics()
                .iter()
                .any(|w| w.contains("emitted as written")),
            "{:?}",
            first.render_diagnostics()
        );
        // The reader strips the wrapper back off...
        let reparsed = parse_dss_str(&first.text);
        let opt = |k: &str| {
            reparsed
                .options()
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(opt("foo"), Some("a!b"));
        assert_eq!(opt("bar"), Some("(abc"));
        assert_eq!(opt("baz"), Some("x ] y"));
        assert_eq!(opt("qux"), Some("a ) b"));
        // ...and the second write picks the same wrapper from the bare value.
        let second = emit_dss_text(&reparsed);
        assert_eq!(first.text, second.text);
    }

    #[test]
    fn extras_tail_values_wrap_like_options() {
        let (b, vs) = three_phase_source(2400.0);
        let mut load = load_on("sb", &["1", "2", "3", "4"], Configuration::Wye);
        load.extras
            .insert("daily".into(), serde_json::json!("a ) b"));
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..MulticonductorNetworkTables::default()
        });
        let (first, second) = roundtrip(&net);
        // A paren wrapper would close at the `)` and land `b)` on the next
        // positional property (duty); brackets survive.
        assert!(first.contains("daily=[a ) b]"), "{first}");
        assert_eq!(first, second);
        let back = parse_dss_str(&first);
        assert_eq!(
            back.loads()[0]
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
        let mut net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            loads: vec![load],
            ..MulticonductorNetworkTables::default()
        });
        net.options_mut().push(("foo".into(), bad.into()));
        let out = emit_dss_text(&net);
        assert!(out.text.contains(&format!("Set foo={bad}")), "{}", out.text);
        assert!(out.text.contains(&format!("daily={bad}")), "{}", out.text);
        let warned = |needle: &str| {
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains(needle) && w.contains("emitted as written"))
        };
        assert!(warned("option `foo`"), "{:?}", out.render_diagnostics());
        assert!(warned("`daily`"), "{:?}", out.render_diagnostics());
    }

    #[test]
    fn empty_extras_values_wrap_instead_of_eating_the_next_token() {
        let dss = "clear\nnew circuit.c basekv=12.47 bus1=sb\n\
                   new load.ld bus1=sb.1 phases=1 kv=7.2 kw=10 daily=() duty=sh\nsolve\n";
        let net = parse_dss_str(dss);
        let load = &net.loads()[0];
        assert_eq!(load.extras.get("daily").and_then(|v| v.as_str()), Some(""));
        let w1 = emit_dss_text(&net).text;
        let again = parse_dss_str(&w1);
        let load2 = &again.loads()[0];
        assert_eq!(load2.extras.get("daily").and_then(|v| v.as_str()), Some(""));
        assert_eq!(
            load2.extras.get("duty").and_then(|v| v.as_str()),
            Some("sh")
        );
        assert_eq!(w1, emit_dss_text(&again).text);
    }

    #[test]
    fn sub_unique_option_prefixes_re_emit_instead_of_vanishing() {
        // "ca" is CapkVAR and "default" is DefaultDaily in the engine's
        // option table; neither may be skipped as a derived key, and
        // `Set default=2.5` must not change the base frequency.
        let dss = "clear\nnew circuit.c basekv=12.47 bus1=sb\n\
                   Set ca=600\nSet default=2.5\nsolve\n";
        let net = parse_dss_str(dss);
        assert!((net.base_frequency() - 60.0).abs() < 1e-12);
        let out = emit_dss_text(&net).text;
        assert!(out.contains("Set ca=600"), "{out}");
        assert!(out.contains("Set default=2.5"), "{out}");
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
        assert!((net.base_frequency() - 50.0).abs() < 1e-12);
        let out = emit_dss_text(&net);
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
        let second = emit_dss_text(&parse_dss_str(&out.text));
        assert_eq!(out.text, second.text);
    }

    #[test]
    fn non_numeric_source_extras_warn_before_falling_back() {
        let (b, mut vs) = three_phase_source(2400.0);
        vs.extras
            .insert("basekv".into(), serde_json::json!("@base"));
        vs.extras.insert("pu".into(), serde_json::json!("unity"));
        vs.extras.insert("angle".into(), serde_json::json!([0.0]));
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        for key in ["basekv", "pu", "angle"] {
            assert!(
                out.render_diagnostics()
                    .iter()
                    .any(|w| w.contains(&format!("{key} extra")) && w.contains("does not parse")),
                "{key}: {:?}",
                out.render_diagnostics()
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
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            name: Some("t".into()),
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ..MulticonductorNetworkTables::default()
        });
        let (first, second) = roundtrip(&net);
        let line = first.lines().find(|l| l.contains("Circuit.")).unwrap();
        // phases=2 against the 4 node dot list would drop a node on reparse.
        assert!(line.contains("phases=3"), "{line}");
        assert!(line.contains("bus1=sb.1.2.3.0"), "{line}");
        assert_eq!(first, second);
        let out = emit_dss_text(&net);
        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("phases=3") && w.contains("positive")),
            "{:?}",
            out.render_diagnostics()
        );
    }

    #[test]
    fn multiple_sources_keep_named_vsource_when_source_exists() {
        let third = 2.0 * std::f64::consts::FRAC_PI_3;
        let source = VoltageSource {
            name: "source".into(),
            bus: "Bx".into(),
            terminal_map: strings(&["1", "2", "3", "4"]),
            v_magnitude: vec![20_000.0, 20_000.0, 20_000.0, 0.0],
            v_angle: vec![0.0, -third, third, 0.0],
            extras: Extras::new(),
        };
        let wind = VoltageSource {
            name: "WindGen1".into(),
            bus: "Bg".into(),
            terminal_map: strings(&["1", "2", "3", "4"]),
            v_magnitude: vec![400.0, 400.0, 400.0, 0.0],
            v_angle: vec![
                -std::f64::consts::FRAC_PI_3,
                std::f64::consts::PI,
                third / 2.0,
                0.0,
            ],
            extras: Extras::new(),
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            name: Some("dg".into()),
            base_frequency: 60.0,
            buses: vec![
                bus("Bg", &["1", "2", "3", "4"], &["4"]),
                bus("Bx", &["1", "2", "3", "4"], &["4"]),
            ],
            sources: vec![wind, source],
            ..MulticonductorNetworkTables::default()
        });

        let out = emit_dss_text(&net).text;
        let circuit = out.lines().find(|l| l.starts_with("New Circuit")).unwrap();
        assert!(circuit.contains("bus1=Bx.1.2.3.0"), "{circuit}");
        assert!(
            out.lines()
                .any(|l| l.starts_with("New Vsource.WindGen1") && l.contains("bus1=Bg.1.2.3.0")),
            "{out}"
        );
        let reparsed = parse_dss_str(&out);
        assert!(
            reparsed
                .sources()
                .iter()
                .any(|vs| vs.name.eq_ignore_ascii_case("WindGen1")),
            "{:?}",
            reparsed.sources()
        );
    }

    #[test]
    fn source_phases_stash_wins_and_does_not_double_emit() {
        let (b, mut vs) = three_phase_source(2400.0);
        vs.extras.insert("phases".into(), serde_json::json!("3"));
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
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
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            name: Some("t".into()),
            base_frequency: 60.0,
            buses: vec![bus("sb", &["1", "2", "3"], &[])],
            sources: vec![vs],
            loads: vec![load],
            ..MulticonductorNetworkTables::default()
        });
        let first = emit_dss_text(&net);
        let hits = |warnings: &[String], name: &str| {
            warnings
                .iter()
                .any(|w| w.contains(name) && w.contains("materializes a grounded neutral"))
        };
        assert!(
            hits(&first.render_diagnostics(), "vsource source"),
            "{:?}",
            first.render_diagnostics()
        );
        assert!(
            hits(&first.render_diagnostics(), "load ld"),
            "{:?}",
            first.render_diagnostics()
        );
        let second = emit_dss_text(&parse_dss_str(&first.text));
        assert_ne!(first.text, second.text);
        assert!(
            !hits(&second.render_diagnostics(), "vsource"),
            "{:?}",
            second.render_diagnostics()
        );
        assert!(
            !hits(&second.render_diagnostics(), "load"),
            "{:?}",
            second.render_diagnostics()
        );
        let third_write = emit_dss_text(&parse_dss_str(&second.text));
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
            s_max: None,
            i_max: None,
            extras: Extras::from([
                ("kv".to_string(), serde_json::json!("4.16")),
                ("phases".to_string(), serde_json::json!("2")),
            ]),
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            generators: vec![g],
            ..MulticonductorNetworkTables::default()
        });
        let out = emit_dss_text(&net);
        let line = out
            .text
            .lines()
            .find(|l| l.contains("Generator.g1"))
            .unwrap();
        assert!(line.contains("phases=2 conn=delta"), "{line}");
        assert_eq!(line.matches("phases=").count(), 1, "{line}");
    }

    #[test]
    fn fixed_dispatch_ibr_exports_as_generator_model_one() {
        let (b, vs) = three_phase_source(240.0);
        let ibr = DistIbr {
            name: "pv".into(),
            bus: "sb".into(),
            terminal_map: strings(&["1", "2", "3", "4"]),
            topology: IbrTopology::FourLeg,
            prime_mover: IbrPrimeMover::Pv,
            s_max: vec![10_000.0; 3],
            i_max: None,
            p_avail: Some(24_000.0),
            p_min: Some(vec![8_000.0; 3]),
            p_max: Some(vec![8_000.0; 3]),
            q_min: Some(vec![0.0; 3]),
            q_max: Some(vec![0.0; 3]),
            control_profile: None,
            voltage_aggregation: None,
            extras: Extras::from([("kv".to_string(), serde_json::json!("0.416"))]),
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            name: Some("fixed".into()),
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ibrs: vec![ibr],
            ..MulticonductorNetworkTables::default()
        });

        let out = emit_dss_text(&net);

        assert!(
            out.render_diagnostics().is_empty(),
            "{:?}",
            out.render_diagnostics()
        );
        let line = out
            .text
            .lines()
            .find(|l| l.starts_with("New Generator.pv"))
            .unwrap();
        assert!(line.contains("model=1 vminpu=0 vmaxpu=2"), "{line}");
        assert!(line.contains("kw=24"), "{line}");
        assert!(!out.text.contains("PVSystem.pv"), "{}", out.text);
    }

    #[test]
    fn volt_var_ibr_exports_pvsystem_xycurve_and_invcontrol() {
        let (b, vs) = three_phase_source(240.0);
        let base_v = 416.0 / 3f64.sqrt();
        let ibr = DistIbr {
            name: "pv".into(),
            bus: "sb".into(),
            terminal_map: strings(&["1", "2", "3", "4"]),
            topology: IbrTopology::FourLeg,
            prime_mover: IbrPrimeMover::Pv,
            s_max: vec![10_000.0; 3],
            i_max: None,
            p_avail: Some(24_000.0),
            p_min: Some(vec![0.0; 3]),
            p_max: Some(vec![8_000.0; 3]),
            q_min: Some(vec![-4_000.0; 3]),
            q_max: Some(vec![4_000.0; 3]),
            control_profile: Some("cp".into()),
            voltage_aggregation: None,
            extras: Extras::from([("kv".to_string(), serde_json::json!("0.416"))]),
        };
        let profile = DistControlProfile {
            name: "cp".into(),
            power_factor: None,
            volt_var: Some(VoltVarControl {
                voltage_reference: Some(ControlVoltageReference::PgAveraged),
                breakpoints: [0.92, 0.98, 1.02, 1.08]
                    .into_iter()
                    .map(|v| v * base_v)
                    .collect(),
                q_limits: vec![-0.44, 0.44],
                q_unit: Some(ReactivePowerUnit::VaFraction),
                q_ref: Some(ReactivePowerReference::VarMax),
                p_min_for_q: Some(10.0),
                p_min_for_q_max: Some(50.0),
            }),
            volt_watt: None,
            extras: Extras::new(),
        };
        let net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            name: Some("controlled".into()),
            base_frequency: 60.0,
            buses: vec![b],
            sources: vec![vs],
            ibrs: vec![ibr],
            control_profiles: vec![profile],
            ..MulticonductorNetworkTables::default()
        });

        let out = emit_dss_text(&net);

        assert!(
            out.render_diagnostics().is_empty(),
            "{:?}",
            out.render_diagnostics()
        );
        let pv = out
            .text
            .lines()
            .find(|l| l.starts_with("New PVSystem.pv"))
            .unwrap();
        assert!(pv.contains("WattPriority=No VarFollowInverter=Yes"), "{pv}");
        assert!(pv.contains("kvarMax=12"), "{pv}");
        assert!(pv.contains("kvarMaxAbs=12"), "{pv}");
        assert!(pv.contains("%PminNoVars=10"), "{pv}");
        assert!(pv.contains("%PminkvarMax=50"), "{pv}");

        let curve = out
            .text
            .lines()
            .find(|l| l.starts_with("New XYcurve.vv_pv"))
            .unwrap();
        assert!(curve.contains("Xarray=[0.92 0.98 1.02 1.08]"), "{curve}");
        assert!(curve.contains("Yarray=[0.44 0 0 -0.44]"), "{curve}");

        let inv = out
            .text
            .lines()
            .find(|l| l.starts_with("New InvControl.ivc_pv"))
            .unwrap();
        assert!(inv.contains("mode=VOLTVAR"), "{inv}");
        assert!(inv.contains("vvc_curve1=vv_pv"), "{inv}");
        assert!(inv.contains("RefReactivePower=VARMAX"), "{inv}");
        assert!(inv.contains("monVoltageCalc=AVG"), "{inv}");
    }
}

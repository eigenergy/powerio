//! BMOPF JSON into the canonical [`DistNetwork`].
//!
//! The format is fully explicit, so the reader materializes nothing and
//! `defaulted` stays empty. Reading is liberal where writing is strict:
//! fields outside the schema land in the element's `extras` with a warning
//! instead of failing the parse. Transformer subtypes become windings; the
//! subtype rides in the transformer's extras (`bmopf_subtype`) so writing
//! back reproduces the same grouping for shapes the windings alone do not
//! pin down (center tap reads as two windings).

use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::model::{
    Configuration, DistBus, DistGenerator, DistLine, DistLineCode, DistLoad, DistLoadVoltageModel,
    DistNetwork, DistShunt, DistSourceFormat, DistSwitch, DistTransformer, Extras, Mat,
    UntypedObject, VoltageSource, Winding, WindingConn, n_winding_impedance_base,
    n_winding_phase_count, pair_keys,
};

pub fn parse_bmopf_file(path: impl AsRef<Path>) -> Result<DistNetwork> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;
    parse_bmopf_str(&text)
}

pub fn parse_bmopf_str(text: &str) -> Result<DistNetwork> {
    let doc: Value = serde_json::from_str(text).map_err(|e| Error::Json {
        format: "BMOPF",
        message: e.to_string(),
    })?;
    let Value::Object(doc) = doc else {
        return Err(Error::Json {
            format: "BMOPF",
            message: "top level is not an object".into(),
        });
    };
    let mut net = DistNetwork {
        source: Some(Arc::new(text.to_string())),
        source_format: Some(DistSourceFormat::BmopfJson),
        base_frequency: 60.0,
        ..DistNetwork::default()
    };
    let mut rd = Reader { net: &mut net };
    rd.document(&doc);
    Ok(net)
}

struct Reader<'a> {
    net: &'a mut DistNetwork,
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

fn floats(v: Option<&Value>) -> Option<Vec<f64>> {
    v?.as_array().map(|a| a.iter().map(f).collect())
}

fn first_float(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Array(a) => a.first().map(f),
        v => Some(f(v)),
    }
}

/// Like [`first_float`], but the field is per-phase-terminal in the schema
/// while powerio's model holds one value; warn when collapsing loses a
/// genuine per-phase difference instead of dropping it silently.
fn first_float_collapsed(v: Option<&Value>, what: &str, warnings: &mut Vec<String>) -> Option<f64> {
    match v? {
        Value::Array(a) => {
            let vals: Vec<f64> = a.iter().map(f).collect();
            if vals.windows(2).any(|w| w[0].to_bits() != w[1].to_bits()) {
                warnings.push(format!(
                    "{what}: per-phase-terminal bound is non-uniform; collapsed to the first entry"
                ));
            }
            vals.first().copied()
        }
        v => Some(f(v)),
    }
}

fn value_alias<'a>(o: &'a Map<String, Value>, primary: &str, legacy: &str) -> Option<&'a Value> {
    o.get(primary).or_else(|| o.get(legacy))
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| s.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn string(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or_default().to_string()
}

/// Case insensitive on the recognized values (the dss reader's tolerance);
/// a present but unrecognized string warns and reads as WYE.
fn config(v: Option<&Value>, what: &str, warnings: &mut Vec<String>) -> Configuration {
    let Some(s) = v.and_then(Value::as_str) else {
        return Configuration::Wye;
    };
    match s.to_ascii_uppercase().as_str() {
        "WYE" => Configuration::Wye,
        "DELTA" => Configuration::Delta,
        "SINGLE_PHASE" => Configuration::SinglePhase,
        _ => {
            warnings.push(format!(
                "{what}: configuration `{s}` is not WYE, DELTA, or SINGLE_PHASE; read as WYE"
            ));
            Configuration::Wye
        }
    }
}

/// Parses the `_i_j` tail of a `prefix_i_j` matrix key (1 based). None
/// when the key is not a well formed entry for this prefix.
fn matrix_indices(key: &str, prefix: &str) -> Option<(usize, usize)> {
    let rest = key.strip_prefix(prefix)?.strip_prefix('_')?;
    let (i, j) = rest.split_once('_')?;
    let (i, j) = (i.parse::<usize>().ok()?, j.parse::<usize>().ok()?);
    (i >= 1 && j >= 1).then_some((i, j))
}

/// Collects `prefix_i_j` keys into a square matrix; `n` is the largest
/// index seen. Returns None when no key carries the prefix.
fn flat_matrix(o: &Map<String, Value>, prefix: &str) -> Option<Mat> {
    let mut entries: Vec<(usize, usize, f64)> = Vec::new();
    let mut n = 0;
    for (k, v) in o {
        let Some((i, j)) = matrix_indices(k, prefix) else {
            continue;
        };
        entries.push((i - 1, j - 1, f(v)));
        n = n.max(i).max(j);
    }
    if n == 0 {
        return None;
    }
    let mut m = vec![vec![0.0; n]; n];
    for (i, j, v) in entries {
        m[i][j] = v;
    }
    Some(m)
}

/// Grows `m` to `n` by `n`, preserving the existing entries.
fn pad_to(m: Mat, n: usize) -> Mat {
    if m.len() >= n {
        return m;
    }
    let mut out = vec![vec![0.0; n]; n];
    for (i, row) in m.into_iter().enumerate() {
        for (j, v) in row.into_iter().enumerate() {
            out[i][j] = v;
        }
    }
    out
}

/// Element fields outside `known` go to extras with a warning.
fn take_extras(
    o: &Map<String, Value>,
    known: &[&str],
    what: &str,
    warnings: &mut Vec<String>,
    matrix_prefixes: &[&str],
) -> Extras {
    let mut extras = Extras::new();
    for (k, v) in o {
        if known.contains(&k.as_str()) {
            continue;
        }
        if matrix_prefixes
            .iter()
            .any(|p| matrix_indices(k, p).is_some())
        {
            continue;
        }
        warnings.push(format!(
            "{what}: `{k}` is outside the schema; kept in extras"
        ));
        extras.insert(k.clone(), v.clone());
    }
    extras
}

impl Reader<'_> {
    fn document(&mut self, doc: &Map<String, Value>) {
        if let Some(name) = doc.get("name").and_then(Value::as_str) {
            self.net.name = Some(name.to_string());
        }
        if let Some(frequency) =
            first_float(doc.get("base_frequency")).or_else(|| first_float(doc.get("frequency")))
            && frequency.is_finite()
            && frequency > 0.0
        {
            self.net.base_frequency = frequency;
        }
        for (key, value) in doc {
            let Value::Object(items) = value else {
                continue;
            };
            match key.as_str() {
                "bus" => self.buses(items),
                "linecode" => self.linecodes(items),
                "line" => self.lines(items),
                "switch" => self.switches(items),
                "load" => self.loads(items),
                "generator" => self.generators(items),
                "shunt" => self.shunts(items),
                "voltage_source" => self.sources(items),
                "transformer" => self.transformers(items),
                // `meta` is provenance, not network data; the writer regenerates it.
                "name" | "meta" => {}
                other => {
                    self.net.warnings.push(format!(
                        "top level `{other}` is outside the schema; kept untyped"
                    ));
                    for (name, v) in items {
                        self.net.untyped.push(UntypedObject {
                            class: other.to_string(),
                            name: name.clone(),
                            props: vec![(None, v.to_string())],
                        });
                    }
                }
            }
        }
    }

    fn buses(&mut self, items: &Map<String, Value>) {
        for (id, v) in items {
            let Value::Object(o) = v else { continue };
            let known = [
                "terminal_names",
                "perfectly_grounded_terminals",
                "v_min",
                "v_max",
                "vpn_min",
                "vpn_max",
                "vpp_min",
                "vpp_max",
                "vsym_min",
                "vsym_max",
            ];
            self.net.buses.push(DistBus {
                id: id.clone(),
                terminals: strings(o.get("terminal_names")),
                grounded: strings(o.get("perfectly_grounded_terminals")),
                v_min: first_float_collapsed(
                    o.get("v_min"),
                    &format!("bus {id} v_min"),
                    &mut self.net.warnings,
                ),
                v_max: first_float_collapsed(
                    o.get("v_max"),
                    &format!("bus {id} v_max"),
                    &mut self.net.warnings,
                ),
                vpn_min: floats(o.get("vpn_min")),
                vpn_max: floats(o.get("vpn_max")),
                vpp_min: floats(o.get("vpp_min")),
                vpp_max: floats(o.get("vpp_max")),
                vsym_min: floats(o.get("vsym_min")),
                vsym_max: floats(o.get("vsym_max")),
                extras: take_extras(o, &known, &format!("bus {id}"), &mut self.net.warnings, &[]),
            });
        }
    }

    fn linecodes(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let mats = [
                flat_matrix(o, "R_series"),
                flat_matrix(o, "X_series"),
                flat_matrix(o, "G_from"),
                flat_matrix(o, "B_from"),
                flat_matrix(o, "G_to"),
                flat_matrix(o, "B_to"),
            ];
            // Conductor count is the widest matrix present; absent matrices
            // read as zero, smaller ones pad without losing entries.
            let n = mats.iter().flatten().map(Vec::len).max().unwrap_or(0);
            if mats.iter().flatten().any(|m| m.len() < n) {
                self.net.warnings.push(format!(
                    "linecode {name}: matrix sizes disagree; smaller ones padded \
                     with zeros to {n}x{n}"
                ));
            }
            let [r, x, gf, bf, gt, bt] = mats.map(|m| pad_to(m.unwrap_or_default(), n));
            let code = DistLineCode {
                name: name.clone(),
                n_conductors: n,
                r_series: r,
                x_series: x,
                g_from: gf,
                b_from: bf,
                g_to: gt,
                b_to: bt,
                i_max: floats(o.get("i_max")),
                s_max: floats(o.get("s_max")),
                extras: take_extras(
                    o,
                    &["i_max", "s_max"],
                    &format!("linecode {name}"),
                    &mut self.net.warnings,
                    &["R_series", "X_series", "G_from", "G_to", "B_from", "B_to"],
                ),
            };
            self.net.linecodes.push(code);
        }
    }

    fn lines(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let known = [
                "length",
                "linecode",
                "bus_from",
                "bus_to",
                "terminal_map_from",
                "terminal_map_to",
            ];
            self.net.lines.push(DistLine {
                name: name.clone(),
                bus_from: string(o.get("bus_from")),
                bus_to: string(o.get("bus_to")),
                terminal_map_from: strings(o.get("terminal_map_from")),
                terminal_map_to: strings(o.get("terminal_map_to")),
                linecode: string(o.get("linecode")),
                length: o.get("length").map_or(f64::NAN, f),
                extras: take_extras(
                    o,
                    &known,
                    &format!("line {name}"),
                    &mut self.net.warnings,
                    &[],
                ),
            });
        }
    }

    fn switches(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let known = [
                "bus_from",
                "bus_to",
                "terminal_map_from",
                "terminal_map_to",
                "open_switch",
                "i_max",
            ];
            self.net.switches.push(DistSwitch {
                name: name.clone(),
                bus_from: string(o.get("bus_from")),
                bus_to: string(o.get("bus_to")),
                terminal_map_from: strings(o.get("terminal_map_from")),
                terminal_map_to: strings(o.get("terminal_map_to")),
                open: o
                    .get("open_switch")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                i_max: floats(o.get("i_max")),
                extras: take_extras(
                    o,
                    &known,
                    &format!("switch {name}"),
                    &mut self.net.warnings,
                    &[],
                ),
            });
        }
    }

    fn loads(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let known = [
                "p_nom",
                "q_nom",
                "bus",
                "configuration",
                "terminal_map",
                "model",
                "v_nom",
                "alpha_z",
                "alpha_i",
                "alpha_p",
                "beta_z",
                "beta_i",
                "beta_p",
                "gamma_p",
                "gamma_q",
            ];
            let v_nom = floats(o.get("v_nom")).unwrap_or_default();
            let has_zip = [
                "alpha_z", "alpha_i", "alpha_p", "beta_z", "beta_i", "beta_p",
            ]
            .iter()
            .any(|key| o.get(*key).is_some());
            let has_exp = o.get("gamma_p").is_some() || o.get("gamma_q").is_some();
            let model = o
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("POWER")
                .to_ascii_uppercase();
            let voltage_model = if has_exp {
                DistLoadVoltageModel::Exponential {
                    v_nom,
                    gamma_p: floats(o.get("gamma_p")).unwrap_or_default(),
                    gamma_q: floats(o.get("gamma_q")).unwrap_or_default(),
                }
            } else if has_zip {
                DistLoadVoltageModel::Zip {
                    v_nom,
                    alpha_z: floats(o.get("alpha_z")).unwrap_or_default(),
                    alpha_i: floats(o.get("alpha_i")).unwrap_or_default(),
                    alpha_p: floats(o.get("alpha_p")).unwrap_or_default(),
                    beta_z: floats(o.get("beta_z")).unwrap_or_default(),
                    beta_i: floats(o.get("beta_i")).unwrap_or_default(),
                    beta_p: floats(o.get("beta_p")).unwrap_or_default(),
                }
            } else if model.contains("IMPEDANCE") {
                DistLoadVoltageModel::ConstantImpedance { v_nom }
            } else if model.contains("CURRENT") {
                DistLoadVoltageModel::ConstantCurrent { v_nom }
            } else {
                DistLoadVoltageModel::ConstantPower { v_nom }
            };
            self.net.loads.push(DistLoad {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: strings(o.get("terminal_map")),
                configuration: config(
                    o.get("configuration"),
                    &format!("load {name}"),
                    &mut self.net.warnings,
                ),
                p_nom: floats(o.get("p_nom")).unwrap_or_default(),
                q_nom: floats(o.get("q_nom")).unwrap_or_default(),
                voltage_model,
                extras: take_extras(
                    o,
                    &known,
                    &format!("load {name}"),
                    &mut self.net.warnings,
                    &[],
                ),
            });
        }
    }

    fn generators(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let known = [
                "p_min",
                "p_max",
                "q_min",
                "q_max",
                "cost",
                "bus",
                "configuration",
                "terminal_map",
            ];
            let p_min = floats(o.get("p_min"));
            let p_max = floats(o.get("p_max"));
            let q_min = floats(o.get("q_min"));
            let q_max = floats(o.get("q_max"));
            // Pinned bounds are a fixed dispatch; surface them as the
            // setpoint too so a power flow oriented target has one.
            let pinned = |lo: &Option<Vec<f64>>, hi: &Option<Vec<f64>>| match (lo, hi) {
                (Some(a), Some(b)) if a == b => a.clone(),
                _ => Vec::new(),
            };
            // Cost is a per-phase array in the schema; powerio's model holds one
            // value, so take the first entry (warning if the phases disagree). A
            // bare scalar is still accepted for documents written before v0.0.1.
            let cost = match o.get("cost") {
                Some(Value::Array(a)) => {
                    let vals: Vec<f64> = a.iter().map(f).collect();
                    // Bit comparison: detect any per-phase difference exactly
                    // (broadcast entries are bit-identical), without a float_cmp.
                    if vals.windows(2).any(|w| w[0].to_bits() != w[1].to_bits()) {
                        self.net.warnings.push(format!(
                            "generator {name}: per-phase cost is non-uniform; \
                             collapsed to the first entry"
                        ));
                    }
                    vals.first().copied()
                }
                Some(v) => Some(f(v)),
                None => None,
            };
            self.net.generators.push(DistGenerator {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: strings(o.get("terminal_map")),
                configuration: config(
                    o.get("configuration"),
                    &format!("generator {name}"),
                    &mut self.net.warnings,
                ),
                p_nom: pinned(&p_min, &p_max),
                q_nom: pinned(&q_min, &q_max),
                p_min,
                p_max,
                q_min,
                q_max,
                cost,
                extras: take_extras(
                    o,
                    &known,
                    &format!("generator {name}"),
                    &mut self.net.warnings,
                    &[],
                ),
            });
        }
    }

    fn shunts(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let g = flat_matrix(o, "G").unwrap_or_default();
            let b = flat_matrix(o, "B").unwrap_or_default();
            let n = g.len().max(b.len());
            if g.len() != b.len() {
                self.net.warnings.push(format!(
                    "shunt {name}: G is {gx}x{gx} but B is {bx}x{bx}; the smaller \
                     padded with zeros to {n}x{n}",
                    gx = g.len(),
                    bx = b.len(),
                ));
            }
            self.net.shunts.push(DistShunt {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: strings(o.get("terminal_map")),
                g: pad_to(g, n),
                b: pad_to(b, n),
                extras: take_extras(
                    o,
                    &["bus", "terminal_map"],
                    &format!("shunt {name}"),
                    &mut self.net.warnings,
                    &["G", "B"],
                ),
            });
        }
    }

    fn sources(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let known = ["v_magnitude", "v_angle", "bus", "terminal_map"];
            self.net.sources.push(VoltageSource {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: strings(o.get("terminal_map")),
                v_magnitude: floats(o.get("v_magnitude")).unwrap_or_default(),
                v_angle: floats(o.get("v_angle")).unwrap_or_default(),
                extras: take_extras(
                    o,
                    &known,
                    &format!("voltage source {name}"),
                    &mut self.net.warnings,
                    &[],
                ),
            });
        }
    }

    fn transformers(&mut self, subtypes: &Map<String, Value>) {
        for (subtype, group) in subtypes {
            let Value::Object(items) = group else {
                continue;
            };
            for (name, v) in items {
                let Value::Object(o) = v else { continue };
                match subtype.as_str() {
                    "n_winding" => {
                        let t = self.n_winding_transformer(name, o);
                        self.net.transformers.push(t);
                    }
                    "single_phase_autotransformer" | "open_delta_regulator" => {
                        self.net.warnings.push(format!(
                            "transformer {name}: subtype `{subtype}` is not typed yet; kept untyped"
                        ));
                        self.net.untyped.push(UntypedObject {
                            class: format!("transformer.{subtype}"),
                            name: name.clone(),
                            props: vec![(None, v.to_string())],
                        });
                    }
                    _ => {
                        let t = self.transformer(subtype, name, o);
                        self.net.transformers.push(t);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)] // one BMOPF transformer record maps many optional schema aliases
    fn transformer(
        &mut self,
        subtype: &str,
        name: &str,
        o: &Map<String, Value>,
    ) -> DistTransformer {
        let known = [
            "bus_from",
            "bus_to",
            "terminal_map_from",
            "terminal_map_to",
            "s_rating",
            "v_nom_from",
            "v_nom_to",
            "v_ref_from",
            "v_ref_to",
            "g_no_load",
            "b_no_load",
            "r_series",
            "x_series",
            "r_series_from",
            "r_series_to",
            "x_series_from",
            "x_series_to",
            "tap",
            "tap_min",
            "tap_max",
        ];
        if !matches!(
            subtype,
            "single_phase" | "center_tap" | "wye_delta" | "delta_wye"
        ) {
            self.net.warnings.push(format!(
                "transformer {name}: subtype `{subtype}` is outside the schema; \
                 read as a single phase pair"
            ));
        }
        let s = o.get("s_rating").map_or(f64::NAN, f);
        let v_from = value_alias(o, "v_nom_from", "v_ref_from").map_or(f64::NAN, f);
        let v_to = value_alias(o, "v_nom_to", "v_ref_to").map_or(f64::NAN, f);
        let positive = |v: f64| v.is_finite() && v > 0.0;
        if !positive(s) || !positive(v_from) || !positive(v_to) {
            self.net.warnings.push(format!(
                "transformer {name}: s_rating or v_nom missing or nonpositive; \
                 impedances read as zero"
            ));
        }
        let three_phase = matches!(subtype, "wye_delta" | "delta_wye");
        let phases = if three_phase { 3 } else { 1 };

        let pct = |x_ohm: f64, v: f64| {
            if s > 0.0 && v > 0.0 {
                x_ohm / (v * v / s) * 100.0
            } else {
                0.0
            }
        };
        let (r_from_pct, r_to_pct, xsc) = if three_phase {
            let wye_v = if subtype == "wye_delta" { v_from } else { v_to };
            // The schema puts one series impedance on the wye side; the
            // model splits resistance evenly across the windings.
            let r = pct(o.get("r_series").map_or(0.0, f), wye_v);
            let x = pct(o.get("x_series").map_or(0.0, f), wye_v);
            (r / 2.0, r / 2.0, x)
        } else {
            let r_from = pct(o.get("r_series_from").map_or(0.0, f), v_from);
            let r_to = pct(o.get("r_series_to").map_or(0.0, f), v_to);
            let x = pct(o.get("x_series_from").map_or(0.0, f), v_from)
                + pct(o.get("x_series_to").map_or(0.0, f), v_to);
            (r_from, r_to, x)
        };

        let conn = |delta: bool| {
            if delta {
                WindingConn::Delta
            } else {
                WindingConn::Wye
            }
        };
        let mut windings = vec![
            Winding {
                bus: string(o.get("bus_from")),
                terminal_map: strings(o.get("terminal_map_from")),
                conn: conn(subtype == "delta_wye"),
                v_ref: v_from,
                s_rating: s,
                r_pct: r_from_pct,
                tap: first_float(o.get("tap")).unwrap_or(1.0),
            },
            Winding {
                bus: string(o.get("bus_to")),
                terminal_map: strings(o.get("terminal_map_to")),
                conn: conn(subtype == "wye_delta"),
                v_ref: v_to,
                s_rating: s,
                r_pct: r_to_pct,
                tap: 1.0,
            },
        ];
        expand_center_tap_windings(subtype, &mut windings);
        let mut extras = take_extras(
            o,
            &known,
            &format!("transformer {name}"),
            &mut self.net.warnings,
            &[],
        );
        for key in ["tap_min", "tap_max"] {
            if let Some(v) = o.get(key) {
                extras.insert(key.into(), v.clone());
            }
        }
        for key in ["g_no_load", "b_no_load"] {
            if let Some(v) = o.get(key) {
                extras.insert(key.into(), v.clone());
            }
        }
        // Windings alone cannot tell single_phase from center_tap back
        // apart; record the subtype for the writer.
        extras.insert("bmopf_subtype".into(), subtype.into());
        DistTransformer {
            name: name.to_string(),
            windings,
            xsc_pct: vec![xsc],
            phases,
            extras,
        }
    }

    fn n_winding_transformer(&mut self, name: &str, o: &Map<String, Value>) -> DistTransformer {
        let known = ["windings", "x_sc", "s_rating", "g_no_load", "b_no_load"];
        let s = o.get("s_rating").map_or(f64::NAN, f);
        let mut windings = Vec::new();
        if let Some(items) = o.get("windings").and_then(Value::as_array) {
            for (idx, item) in items.iter().enumerate() {
                let Some(w) = item.as_object() else {
                    self.net.warnings.push(format!(
                        "transformer {name}: winding {} is not an object; skipped",
                        idx + 1
                    ));
                    continue;
                };
                let terminal_map = strings(w.get("terminal_map"));
                let bmopf_v_nom = value_alias(w, "v_nom", "v_ref").map_or(f64::NAN, f);
                let r_winding = w.get("r_winding").map_or(0.0, f);
                let connection = w
                    .get("configuration")
                    .or_else(|| w.get("connection"))
                    .and_then(Value::as_str)
                    .unwrap_or("WYE")
                    .to_ascii_uppercase();
                if !matches!(connection.as_str(), "WYE" | "DELTA") {
                    self.net.warnings.push(format!(
                        "transformer {name}: winding {} connection `{connection}` is not WYE or DELTA; read as WYE",
                        idx + 1
                    ));
                }
                let conn = if connection == "DELTA" {
                    WindingConn::Delta
                } else {
                    WindingConn::Wye
                };
                let r_pct = if let Some(base_z) =
                    n_winding_base_from_bmopf(conn, &terminal_map, bmopf_v_nom, s)
                {
                    r_winding / base_z * 100.0
                } else {
                    0.0
                };
                windings.push(Winding {
                    bus: string(w.get("bus")),
                    terminal_map: terminal_map.clone(),
                    conn,
                    v_ref: n_winding_internal_v_ref(conn, &terminal_map, bmopf_v_nom),
                    s_rating: s,
                    r_pct,
                    tap: 1.0,
                });
            }
        }
        let base_z = windings
            .first()
            .and_then(|w| n_winding_base_from_internal(w, s))
            .unwrap_or(f64::NAN);
        let mut xsc_pct = Vec::new();
        let x_sc = o.get("x_sc").and_then(Value::as_object);
        for (i, j) in pair_keys(windings.len()) {
            let key = format!("{}_{}", i + 1, j + 1);
            let x = x_sc.and_then(|m| m.get(&key)).map_or(0.0, f);
            xsc_pct.push(if base_z.is_finite() && base_z > 0.0 {
                x / base_z * 100.0
            } else {
                0.0
            });
        }
        let mut extras = take_extras(
            o,
            &known,
            &format!("transformer {name}"),
            &mut self.net.warnings,
            &[],
        );
        extras.insert("bmopf_subtype".into(), "n_winding".into());
        for key in ["g_no_load", "b_no_load"] {
            if let Some(v) = o.get(key) {
                extras.insert(key.into(), v.clone());
            }
        }
        DistTransformer {
            name: name.to_string(),
            phases: windings
                .iter()
                .map(|w| n_winding_phase_count(w.conn, &w.terminal_map))
                .max()
                .unwrap_or(1)
                .max(1),
            windings,
            xsc_pct,
            extras,
        }
    }
}

fn expand_center_tap_windings(subtype: &str, windings: &mut Vec<Winding>) {
    if subtype != "center_tap" || windings[1].terminal_map.len() < 3 {
        return;
    }
    let to = windings.pop().expect("secondary winding exists");
    let common = to.terminal_map.last().cloned().unwrap_or_default();
    let hot_a = to.terminal_map[0].clone();
    let hot_b = to.terminal_map[1].clone();
    let half = Winding {
        bus: to.bus.clone(),
        terminal_map: vec![hot_a, common.clone()],
        conn: WindingConn::Wye,
        v_ref: to.v_ref / 2.0,
        s_rating: to.s_rating,
        r_pct: to.r_pct * 2.0,
        tap: to.tap,
    };
    let other_half = Winding {
        bus: to.bus,
        terminal_map: vec![common, hot_b],
        conn: WindingConn::Wye,
        v_ref: to.v_ref / 2.0,
        s_rating: to.s_rating,
        r_pct: to.r_pct * 2.0,
        tap: to.tap,
    };
    windings.push(half);
    windings.push(other_half);
}

fn n_winding_internal_v_ref(conn: WindingConn, terminal_map: &[String], bmopf_v_nom: f64) -> f64 {
    if conn == WindingConn::Wye && n_winding_phase_count(conn, terminal_map) >= 2 {
        bmopf_v_nom * 3f64.sqrt()
    } else {
        bmopf_v_nom
    }
}

fn n_winding_bmopf_v_nom_from_internal(w: &Winding) -> f64 {
    if w.conn == WindingConn::Wye && n_winding_phase_count(w.conn, &w.terminal_map) >= 2 {
        w.v_ref / 3f64.sqrt()
    } else {
        w.v_ref
    }
}

fn n_winding_base_from_bmopf(
    conn: WindingConn,
    terminal_map: &[String],
    bmopf_v_nom: f64,
    s: f64,
) -> Option<f64> {
    n_winding_impedance_base(n_winding_phase_count(conn, terminal_map), bmopf_v_nom, s)
}

fn n_winding_base_from_internal(w: &Winding, s: f64) -> Option<f64> {
    n_winding_base_from_bmopf(
        w.conn,
        &w.terminal_map,
        n_winding_bmopf_v_nom_from_internal(w),
        s,
    )
}

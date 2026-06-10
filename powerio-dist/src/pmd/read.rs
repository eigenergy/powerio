//! PMD ENGINEERING JSON into the canonical [`DistNetwork`].
//!
//! The reader applies PMD's own import corrections: `null` becomes +Inf
//! under a `_ub`/`max` suffix, -Inf under `_lb`/`min`, NaN elsewhere, and
//! arrays of arrays rebuild as matrices with the inner arrays as columns.
//! Integer terminals become the model's string names; per unit transformer
//! impedances become the model's percent fields; kV, kW, and degrees scale
//! to volts, watts, and radians. Fields the model does not type ride in
//! `extras` so the PMD writer can reproduce them.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::model::{
    Configuration, DistBus, DistGenerator, DistLine, DistLineCode, DistLoad, DistNetwork,
    DistShunt, DistSourceFormat, DistSwitch, DistTransformer, Extras, Mat, UntypedObject,
    VoltageSource, Winding, WindingConn,
};

pub fn parse_pmd_file(path: impl AsRef<Path>) -> Result<DistNetwork> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;
    parse_pmd_str(&text)
}

pub fn parse_pmd_str(text: &str) -> Result<DistNetwork> {
    let doc: Value = serde_json::from_str(text).map_err(|e| Error::Json {
        format: "PMD",
        message: e.to_string(),
    })?;
    let Value::Object(doc) = doc else {
        return Err(Error::Json {
            format: "PMD",
            message: "top level is not an object".into(),
        });
    };
    let mut net = DistNetwork {
        source: Some(Arc::new(text.to_string())),
        source_format: Some(DistSourceFormat::PmdJson),
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

/// PMD's null restoration: the field suffix picks the value.
fn restore(key: &str, v: &Value) -> f64 {
    if v.is_null() {
        if key.ends_with("_ub") || key.ends_with("max") {
            f64::INFINITY
        } else if key.ends_with("_lb") || key.ends_with("min") {
            f64::NEG_INFINITY
        } else {
            f64::NAN
        }
    } else {
        v.as_f64().unwrap_or(f64::NAN)
    }
}

fn floats(key: &str, v: Option<&Value>) -> Option<Vec<f64>> {
    v?.as_array()
        .map(|a| a.iter().map(|x| restore(key, x)).collect())
}

/// Arrays of arrays rebuild with the inner arrays as columns (`hcat`).
fn matrix(key: &str, v: Option<&Value>) -> Option<Mat> {
    let cols = v?.as_array()?;
    let n = cols.len();
    let mut m = vec![vec![0.0; n]; n];
    for (j, col) in cols.iter().enumerate() {
        let col = col.as_array()?;
        for (i, x) in col.iter().enumerate().take(n) {
            m[i][j] = restore(key, x);
        }
    }
    Some(m)
}

fn ints_as_strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| {
                    x.as_i64().map_or_else(
                        || x.as_str().unwrap_or_default().to_string(),
                        |i| i.to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or_default().to_string()
}

/// Keeps fields outside `known` in extras verbatim (no warning: the
/// ENGINEERING model legitimately carries fields the hub does not type,
/// and the PMD writer reproduces the typed ones).
fn take_extras(o: &Map<String, Value>, known: &[&str]) -> Extras {
    o.iter()
        // The inner `name` duplicates the element's key.
        .filter(|(k, _)| !known.contains(&k.as_str()) && k.as_str() != "name")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// The model has no status field; a non ENABLED status rides in extras so
/// the PMD writer reproduces it instead of silently re-enabling.
fn stash_status(
    o: &Map<String, Value>,
    extras: &mut Extras,
    what: &str,
    warnings: &mut Vec<String>,
) {
    if let Some(s) = o.get("status").and_then(Value::as_str)
        && s != "ENABLED"
    {
        extras.insert("pmd_status".into(), Value::String(s.to_string()));
        warnings.push(format!(
            "{what}: status {s} kept in extras; other formats emit the element enabled"
        ));
    }
}

/// A linecode from an object carrying the linecode matrix fields: a
/// `linecode` entry, or a line with inline impedance (the dss2eng output
/// for rmatrix defined lines). Extras hold only the raw `b_fr`/`b_to`
/// stash; the caller merges anything else.
fn linecode_from(name: &str, o: &Map<String, Value>, base_frequency: f64) -> DistLineCode {
    let r = matrix("rs", o.get("rs")).unwrap_or_default();
    let n = r.len();
    let zero = || vec![vec![0.0; n]; n];
    // b_fr/b_to numbers are cmatrix halves in nF per meter; the model
    // holds siemens per meter.
    let omega = std::f64::consts::TAU * base_frequency * 1e-9;
    let to_b = |m: Option<Mat>| {
        m.map(|m| {
            m.iter()
                .map(|row| row.iter().map(|v| v * omega).collect())
                .collect()
        })
    };
    DistLineCode {
        name: name.to_string(),
        n_conductors: n,
        x_series: matrix("xs", o.get("xs")).unwrap_or_else(zero),
        g_from: matrix("g_fr", o.get("g_fr")).unwrap_or_else(zero),
        g_to: matrix("g_to", o.get("g_to")).unwrap_or_else(zero),
        b_from: to_b(matrix("b_fr", o.get("b_fr"))).unwrap_or_else(zero),
        b_to: to_b(matrix("b_to", o.get("b_to"))).unwrap_or_else(zero),
        r_series: r,
        i_max: floats("cm_ub", o.get("cm_ub")).filter(|v| v.iter().all(|x| x.is_finite())),
        s_max: floats("sm_ub", o.get("sm_ub")).filter(|v| v.iter().all(|x| x.is_finite())),
        extras: {
            // The raw arrays make writing back bit exact across the
            // capacitance to susceptance basis change.
            let mut extras = Extras::new();
            if let Some(b) = o.get("b_fr") {
                extras.insert("pmd_b_fr".into(), b.clone());
            }
            if let Some(b) = o.get("b_to") {
                extras.insert("pmd_b_to".into(), b.clone());
            }
            extras
        },
    }
}

/// `Winding.tap` is a scalar; the first phase tap represents each winding
/// (the raw per phase arrays ride in extras). The flag reports whether any
/// winding's phases disagree, which exact comparison detects: a copied
/// default differs by zero bits.
#[allow(clippy::float_cmp)]
fn representative_taps(tm_set: Option<&Value>) -> (Vec<f64>, bool) {
    let mut firsts = Vec::new();
    let mut differ = false;
    for w in tm_set
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let taps: Vec<f64> = w
            .as_array()
            .map(|p| p.iter().map(|v| restore("tm_set", v)).collect())
            .unwrap_or_default();
        let first = taps.first().copied().unwrap_or(1.0);
        differ |= taps.iter().any(|&t| t != first);
        firsts.push(first);
    }
    (firsts, differ)
}

struct WindingNums<'a> {
    rw: &'a [f64],
    xsc: &'a [f64],
    sm_nom: &'a [f64],
    vm_nom: &'a [f64],
    tm_set: &'a [f64],
}

/// Windings from the parallel per winding arrays; undoes the lag
/// connection's barrel roll (`polarity` -1 on a wye winding under a delta
/// primary) so the model holds the source case's order. The flag reports
/// whether any winding was unrolled.
fn build_windings(
    buses: &[String],
    configs: &[WindingConn],
    polarity: &[i64],
    o: &Map<String, Value>,
    nums: &WindingNums,
) -> (Vec<Winding>, usize, bool) {
    let _ = nums.xsc;
    let mut windings = Vec::with_capacity(buses.len());
    let mut phases = 1;
    let mut unrolled = false;
    for (w, bus) in buses.iter().enumerate() {
        let mut map = ints_as_strings(
            o.get("connections")
                .and_then(Value::as_array)
                .and_then(|a| a.get(w)),
        );
        let conn = configs.get(w).copied().unwrap_or(WindingConn::Wye);
        if polarity.get(w) == Some(&-1)
            && conn == WindingConn::Wye
            && configs.first() == Some(&WindingConn::Delta)
            && map.len() > 1
        {
            let phases_part = map.len() - 1;
            map[..phases_part].rotate_right(1);
            unrolled = true;
        }
        if conn == WindingConn::Wye {
            phases = phases.max(map.len().saturating_sub(1));
        } else {
            phases = phases.max(map.len());
        }
        windings.push(Winding {
            bus: bus.clone(),
            terminal_map: map,
            conn,
            v_ref: nums.vm_nom.get(w).copied().unwrap_or(f64::NAN) * 1e3,
            s_rating: nums.sm_nom.get(w).copied().unwrap_or(f64::NAN) * 1e3,
            r_pct: nums.rw.get(w).copied().unwrap_or(0.0) * 100.0,
            tap: nums.tm_set.get(w).copied().unwrap_or(1.0),
        });
    }
    (windings, phases, unrolled)
}

impl Reader<'_> {
    fn document(&mut self, doc: &Map<String, Value>) {
        if let Some(name) = doc.get("name").and_then(Value::as_str) {
            self.net.name = Some(name.to_string());
        }
        if let Some(settings) = doc.get("settings").and_then(Value::as_object) {
            if let Some(f) = settings.get("base_frequency").and_then(Value::as_f64) {
                self.net.base_frequency = f;
            }
            self.net
                .extras
                .insert("pmd_settings".into(), Value::Object(settings.clone()));
        }
        for key in ["data_model", "files", "conductor_ids", "per_unit"] {
            if let Some(v) = doc.get(key) {
                self.net.extras.insert(format!("pmd_{key}"), v.clone());
            }
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
                "settings" | "name" => {}
                other => {
                    self.net.warnings.push(format!(
                        "ENGINEERING `{other}` components are not typed; kept untyped"
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
            let mut extras = take_extras(
                o,
                &["terminals", "grounded", "rg", "xg", "status", "lat", "lon"],
            );
            if let Some(x) = o.get("lon") {
                extras.insert("x".into(), x.clone());
            }
            if let Some(y) = o.get("lat") {
                extras.insert("y".into(), y.clone());
            }
            let rg = floats("rg", o.get("rg")).unwrap_or_default();
            let xg = floats("xg", o.get("xg")).unwrap_or_default();
            if rg.iter().any(|&r| r != 0.0) || xg.iter().any(|&x| x != 0.0) {
                self.net.warnings.push(format!(
                    "bus {id}: nonzero grounding impedance is not typed; kept in extras"
                ));
                extras.insert("rg".into(), o.get("rg").cloned().unwrap_or(Value::Null));
                extras.insert("xg".into(), o.get("xg").cloned().unwrap_or(Value::Null));
            }
            stash_status(o, &mut extras, &format!("bus {id}"), &mut self.net.warnings);
            self.net.buses.push(DistBus {
                id: id.clone(),
                terminals: ints_as_strings(o.get("terminals")),
                grounded: ints_as_strings(o.get("grounded")),
                extras,
                ..DistBus::default()
            });
        }
    }

    fn linecodes(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let mut lc = linecode_from(name, o, self.net.base_frequency);
            let mut extras = take_extras(
                o,
                &["rs", "xs", "g_fr", "g_to", "b_fr", "b_to", "cm_ub", "sm_ub"],
            );
            extras.append(&mut lc.extras);
            lc.extras = extras;
            self.net.linecodes.push(lc);
        }
    }

    fn lines(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let mut known = vec![
                "f_bus",
                "t_bus",
                "f_connections",
                "t_connections",
                "linecode",
                "length",
                "status",
                "source_id",
            ];
            let mut linecode = string(o.get("linecode"));
            let mut extras;
            // Inline impedance (the dss2eng output for rmatrix defined
            // lines): materialize a linecode so the matrices survive, and
            // mark the line so the PMD writer re-inlines them.
            if linecode.is_empty() && o.get("rs").is_some() {
                known.extend(["rs", "xs", "g_fr", "g_to", "b_fr", "b_to", "cm_ub"]);
                extras = take_extras(o, &known);
                let mut lc_name = format!("{name}_z");
                let mut k = 2;
                while self.net.linecode(&lc_name).is_some() {
                    lc_name = format!("{name}_z{k}");
                    k += 1;
                }
                self.net
                    .linecodes
                    .push(linecode_from(&lc_name, o, self.net.base_frequency));
                self.net.warnings.push(format!(
                    "line {name}: inline impedance materialized as linecode {lc_name}; the PMD writer re-inlines it"
                ));
                extras.insert("pmd_inline".into(), Value::Bool(true));
                linecode = lc_name;
            } else {
                extras = take_extras(o, &known);
            }
            stash_status(
                o,
                &mut extras,
                &format!("line {name}"),
                &mut self.net.warnings,
            );
            self.net.lines.push(DistLine {
                name: name.clone(),
                bus_from: string(o.get("f_bus")),
                bus_to: string(o.get("t_bus")),
                terminal_map_from: ints_as_strings(o.get("f_connections")),
                terminal_map_to: ints_as_strings(o.get("t_connections")),
                linecode,
                length: o.get("length").map_or(f64::NAN, |v| restore("length", v)),
                extras,
            });
        }
    }

    fn switches(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let mut extras = take_extras(
                o,
                &[
                    "f_bus",
                    "t_bus",
                    "f_connections",
                    "t_connections",
                    "state",
                    "cm_ub",
                    "status",
                    "source_id",
                    "dispatchable",
                    "rs",
                    "xs",
                    "g_fr",
                    "g_to",
                    "b_fr",
                    "b_to",
                ],
            );
            // The series matrices ride along raw so a dss regeneration can
            // override the engine's switch dummy impedance with the real
            // one, and the PMD writer can reproduce them.
            for key in ["rs", "xs"] {
                if let Some(m) = o.get(key) {
                    extras.insert(format!("pmd_{key}"), m.clone());
                }
            }
            stash_status(
                o,
                &mut extras,
                &format!("switch {name}"),
                &mut self.net.warnings,
            );
            self.net.switches.push(DistSwitch {
                name: name.clone(),
                bus_from: string(o.get("f_bus")),
                bus_to: string(o.get("t_bus")),
                terminal_map_from: ints_as_strings(o.get("f_connections")),
                terminal_map_to: ints_as_strings(o.get("t_connections")),
                open: o.get("state").and_then(Value::as_str) == Some("OPEN"),
                i_max: floats("cm_ub", o.get("cm_ub")),
                extras,
            });
        }
    }

    fn loads(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let connections = ints_as_strings(o.get("connections"));
            let configuration = match o.get("configuration").and_then(Value::as_str) {
                Some("DELTA") if connections.len() > 2 => Configuration::Delta,
                _ if connections.len() <= 2 => Configuration::SinglePhase,
                Some("DELTA") => Configuration::Delta,
                _ => Configuration::Wye,
            };
            let scale = |key: &str| {
                floats(key, o.get(key))
                    .unwrap_or_default()
                    .iter()
                    .map(|v| v * 1e3)
                    .collect::<Vec<_>>()
            };
            let mut extras = take_extras(
                o,
                &[
                    "bus",
                    "connections",
                    "configuration",
                    "pd_nom",
                    "qd_nom",
                    "status",
                    "source_id",
                    "dispatchable",
                    "vm_nom",
                    "model",
                ],
            );
            if let Some(kv) = o.get("vm_nom") {
                extras.insert("kv".into(), kv.clone());
            }
            if let Some(model) = o.get("model").and_then(Value::as_str) {
                let dss_model = match model {
                    "IMPEDANCE" => 2,
                    "CURRENT" => 5,
                    "ZIPV" => 8,
                    _ => 1,
                };
                if dss_model != 1 {
                    extras.insert("model".into(), dss_model.into());
                }
            }
            stash_status(
                o,
                &mut extras,
                &format!("load {name}"),
                &mut self.net.warnings,
            );
            self.net.loads.push(DistLoad {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: connections,
                configuration,
                p_nom: scale("pd_nom"),
                q_nom: scale("qd_nom"),
                extras,
            });
        }
    }

    fn generators(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let scale = |key: &str| {
                floats(key, o.get(key)).map(|v| v.iter().map(|x| x * 1e3).collect::<Vec<f64>>())
            };
            let mut extras = take_extras(
                o,
                &[
                    "bus",
                    "connections",
                    "configuration",
                    "pg",
                    "qg",
                    "pg_lb",
                    "pg_ub",
                    "qg_lb",
                    "qg_ub",
                    "status",
                    "source_id",
                ],
            );
            stash_status(
                o,
                &mut extras,
                &format!("generator {name}"),
                &mut self.net.warnings,
            );
            self.net.generators.push(DistGenerator {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: ints_as_strings(o.get("connections")),
                configuration: match o.get("configuration").and_then(Value::as_str) {
                    Some("DELTA") => Configuration::Delta,
                    _ => Configuration::Wye,
                },
                p_nom: scale("pg").unwrap_or_default(),
                q_nom: scale("qg").unwrap_or_default(),
                p_min: scale("pg_lb").filter(|v| v.iter().all(|x| x.is_finite())),
                p_max: scale("pg_ub").filter(|v| v.iter().all(|x| x.is_finite())),
                q_min: scale("qg_lb").filter(|v| v.iter().all(|x| x.is_finite())),
                q_max: scale("qg_ub").filter(|v| v.iter().all(|x| x.is_finite())),
                cost: None,
                extras,
            });
        }
    }

    fn shunts(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let g = matrix("gs", o.get("gs")).unwrap_or_default();
            let b = matrix("bs", o.get("bs")).unwrap_or_default();
            let mut extras = take_extras(
                o,
                &["bus", "connections", "gs", "bs", "status", "source_id"],
            );
            stash_status(
                o,
                &mut extras,
                &format!("shunt {name}"),
                &mut self.net.warnings,
            );
            self.net.shunts.push(DistShunt {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: ints_as_strings(o.get("connections")),
                g,
                b,
                extras,
            });
        }
    }

    fn sources(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let mut extras = take_extras(
                o,
                &["bus", "connections", "vm", "va", "status", "source_id"],
            );
            stash_status(
                o,
                &mut extras,
                &format!("voltage source {name}"),
                &mut self.net.warnings,
            );
            self.net.sources.push(VoltageSource {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: ints_as_strings(o.get("connections")),
                v_magnitude: floats("vm", o.get("vm"))
                    .unwrap_or_default()
                    .iter()
                    .map(|v| v * 1e3)
                    .collect(),
                v_angle: floats("va", o.get("va"))
                    .unwrap_or_default()
                    .iter()
                    .map(|a| a.to_radians())
                    .collect(),
                extras,
            });
        }
    }

    fn transformers(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let t = self.transformer(name, o);
            self.net.transformers.push(t);
        }
    }

    /// The writer recomputes polarity from the lag convention; when the
    /// file disagrees (a euro/lead or reversed winding), the raw arrays
    /// ride in extras and the writer emits them verbatim.
    fn stash_polarity(
        &mut self,
        name: &str,
        o: &Map<String, Value>,
        windings: &[Winding],
        polarity: &[i64],
        unrolled: bool,
        extras: &mut Extras,
    ) {
        let file_polarity: Vec<i64> = (0..windings.len())
            .map(|w| polarity.get(w).copied().unwrap_or(1))
            .collect();
        if file_polarity == super::write::lag_polarity(windings) {
            return;
        }
        extras.insert(
            "pmd_polarity".into(),
            o.get("polarity")
                .cloned()
                .unwrap_or_else(|| file_polarity.clone().into()),
        );
        if unrolled && let Some(c) = o.get("connections") {
            extras.insert("pmd_connections".into(), c.clone());
        }
        self.net.warnings.push(format!(
            "transformer {name}: polarity {file_polarity:?} is not the lag convention; kept in extras (other formats assume lag)"
        ));
    }

    fn transformer(&mut self, name: &str, o: &Map<String, Value>) -> DistTransformer {
        let buses = ints_as_strings(o.get("bus"));
        let configs: Vec<WindingConn> = o
            .get("configuration")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|c| {
                        if c.as_str() == Some("DELTA") {
                            WindingConn::Delta
                        } else {
                            WindingConn::Wye
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let polarity: Vec<i64> = o
            .get("polarity")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|p| p.as_i64().unwrap_or(1)).collect())
            .unwrap_or_default();
        let rw = floats("rw", o.get("rw")).unwrap_or_default();
        let xsc = floats("xsc", o.get("xsc")).unwrap_or_default();
        let sm_nom = floats("sm_nom", o.get("sm_nom")).unwrap_or_default();
        let vm_nom = floats("vm_nom", o.get("vm_nom")).unwrap_or_default();
        let (tm_set, taps_differ) = representative_taps(o.get("tm_set"));
        if taps_differ {
            self.net.warnings.push(format!(
                "transformer {name}: per phase taps differ; the winding tap keeps the first phase (full arrays in extras)"
            ));
        }

        let (windings, phases, unrolled) = build_windings(
            &buses,
            &configs,
            &polarity,
            o,
            &WindingNums {
                rw: &rw,
                xsc: &xsc,
                sm_nom: &sm_nom,
                vm_nom: &vm_nom,
                tm_set: &tm_set,
            },
        );

        if o.get("controls").is_some() {
            self.net.warnings.push(format!(
                "transformer {name}: regulator controls are not typed; kept in extras"
            ));
        }
        let mut extras = take_extras(
            o,
            &[
                "bus",
                "connections",
                "configuration",
                "polarity",
                "rw",
                "xsc",
                "sm_nom",
                "vm_nom",
                "tm_set",
                "tm_fix",
                "tm_lb",
                "tm_ub",
                "tm_step",
                "status",
                "source_id",
                "noloadloss",
                "cmag",
                "sm_ub",
            ],
        );
        for key in ["tm_set", "tm_lb", "tm_ub", "tm_fix", "tm_step"] {
            if let Some(v) = o.get(key) {
                extras.insert(format!("pmd_{key}"), v.clone());
            }
        }
        self.stash_polarity(name, o, &windings, &polarity, unrolled, &mut extras);
        stash_status(
            o,
            &mut extras,
            &format!("transformer {name}"),
            &mut self.net.warnings,
        );
        DistTransformer {
            name: name.to_string(),
            windings,
            xsc_pct: xsc.iter().map(|x| x * 100.0).collect(),
            phases,
            extras,
        }
    }
}

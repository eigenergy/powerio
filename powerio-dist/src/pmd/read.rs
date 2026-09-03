//! PMD ENGINEERING JSON into the canonical [`MulticonductorNetwork`].
//!
//! The reader applies PMD's own import corrections: `null` becomes +Inf
//! under a `_ub`/`max` suffix, -Inf under `_lb`/`min`, NaN elsewhere, and
//! arrays of arrays rebuild as matrices with the inner arrays as columns.
//! Integer terminals become the model's string names; per unit transformer
//! impedances become the model's percent fields; kV, kW, and degrees scale
//! to volts, watts, and radians. Fields the model does not type ride in
//! `extras` so the PMD writer can reproduce them.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::diagnostics::codes as C;
use crate::error::{Error, Result};
use crate::model::{
    Configuration, DistBus, DistGenerator, DistLine, DistLineCode, DistLoad, DistLoadVoltageModel,
    DistShunt, DistSourceFormat, DistSwitch, DistTransformer, DistWinding, DistWindingConn, Extras,
    Mat, MulticonductorNetwork, MulticonductorNetworkTables, UntypedObject, VoltageSource,
};

pub(crate) fn parse_pmd_collecting(
    text: &str,
    diags: &mut crate::collect::Diagnostics,
) -> Result<MulticonductorNetwork> {
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
    // This reader types the ENGINEERING model only. The MATHEMATICAL model
    // shares the `data_model` marker but is index based and structurally
    // unrelated; interpreting its sections as ENGINEERING would silently
    // build a wrong network.
    if let Some(dm) = doc.get("data_model").and_then(Value::as_str) {
        if !dm.eq_ignore_ascii_case("ENGINEERING") {
            return Err(Error::Json {
                format: "PMD",
                message: format!(
                    "`data_model` is `{dm}`; this reader supports the ENGINEERING model \
                     (convert MATHEMATICAL output back with \
                     PowerModelsDistribution.transform_solution or export the ENGINEERING \
                     model)"
                ),
            });
        }
    }
    let mut net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
        source_format: Some(DistSourceFormat::PmdJson),
        base_frequency: 60.0,
        ..MulticonductorNetworkTables::default()
    });
    let mut rd = Reader {
        net: &mut net,
        diagnostics: crate::diagnostics::Diagnostics::new(),
    };
    rd.document(&doc);
    let found = std::mem::take(&mut rd.diagnostics);
    diags.absorb(found);
    crate::model::warn_unresolved_references(&net, diags);
    Ok(net)
}

struct Reader<'a> {
    net: &'a mut MulticonductorNetwork,
    /// The reader's findings, flushed into the network once the parse is
    /// done so a helper never borrows the network the caller is writing into.
    diagnostics: crate::diagnostics::Diagnostics,
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

/// Largest conductor count a PMD impedance/admittance matrix may declare. `n`
/// is the outer array length and sizes a dense n×n allocation, so an unbounded
/// value from a small array of empty arrays could demand gigabytes. No physical
/// conductor bundle comes near this bound; it matches the DSS reader's count
/// cap. An oversized matrix is dropped with a warning.
const MAX_MATRIX_DIM: usize = 64;

/// Arrays of arrays rebuild with the inner arrays as columns (`hcat`). A
/// column that is not an array warns and stays zero instead of dropping
/// the whole matrix silently; a field that is not an array of columns at
/// all warns and drops, because there is no shape to keep. An absent field
/// is not a defect, so it stays quiet.
fn matrix(
    key: &str,
    v: Option<&Value>,
    what: &str,
    diagnostics: &mut crate::diagnostics::Diagnostics,
) -> Option<Mat> {
    let v = v?;
    let Some(cols) = v.as_array() else {
        diagnostics.push(
            &C::READ_PMD_SOURCE_MALFORMED,
            format!("{what}: `{key}` is not an array of columns; dropped"),
        );
        return None;
    };
    let n = cols.len();
    if n > MAX_MATRIX_DIM {
        diagnostics.push(
            &C::READ_PMD_VALUE_CLAMPED,
            format!(
                "{key}: matrix dimension {n} exceeds the supported maximum of \
             {MAX_MATRIX_DIM}; dropped"
            ),
        );
        return None;
    }
    let mut m = vec![vec![0.0; n]; n];
    for (j, col) in cols.iter().enumerate() {
        let Some(col) = col.as_array() else {
            diagnostics.push(
                &C::READ_PMD_SOURCE_MALFORMED,
                format!("{what}: `{key}` column {j} is not an array; kept as zeros"),
            );
            continue;
        };
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
    diagnostics: &mut crate::diagnostics::Diagnostics,
) {
    if let Some(s) = o.get("status").and_then(Value::as_str)
        && s != "ENABLED"
    {
        extras.insert("pmd_status".into(), Value::String(s.to_string()));
        diagnostics.push(
            &C::READ_PMD_RETAINED_SOURCE_ONLY,
            format!("{what}: status {s} kept in extras; other formats emit the element enabled"),
        );
    }
}

/// A linecode from an object carrying the linecode matrix fields: a
/// `linecode` entry, or a line with inline impedance (the dss2eng output
/// for rmatrix defined lines). Extras hold only the raw `b_fr`/`b_to`
/// stash; the caller merges anything else.
fn linecode_from(
    name: &str,
    o: &Map<String, Value>,
    base_frequency: f64,
    diagnostics: &mut crate::diagnostics::Diagnostics,
) -> DistLineCode {
    let what = format!("linecode {name}");
    let mut mat = |key: &str| matrix(key, o.get(key), &what, diagnostics);
    let mats = [
        mat("rs"),
        mat("xs"),
        mat("g_fr"),
        mat("g_to"),
        mat("b_fr"),
        mat("b_to"),
    ];
    // Conductor count is the widest matrix present; absent matrices read
    // as zero, smaller ones pad without losing entries.
    let n = mats.iter().flatten().map(Vec::len).max().unwrap_or(0);
    if mats.iter().flatten().any(|m| m.len() < n) {
        diagnostics.push(
            &C::READ_PMD_VALUE_DEFAULTED,
            format!(
                "linecode {name}: matrix sizes disagree; smaller ones padded \
             with zeros to {n}x{n}"
            ),
        );
    }
    let [r, x, gf, gt, bf, bt] = mats.map(|m| pad_to(m.unwrap_or_default(), n));
    // b_fr/b_to numbers are cmatrix halves in nF per meter; the model
    // holds siemens per meter.
    let omega = std::f64::consts::TAU * base_frequency * 1e-9;
    let to_b = |m: Mat| -> Mat {
        m.into_iter()
            .map(|row| row.into_iter().map(|v| v * omega).collect())
            .collect()
    };
    DistLineCode {
        name: name.to_string(),
        n_conductors: n,
        x_series: x,
        g_from: gf,
        g_to: gt,
        b_from: to_b(bf),
        b_to: to_b(bt),
        r_series: r,
        // A null entry restores as Inf (an unbounded conductor); keeping the
        // mixed array preserves the finite bounds beside it. `None` means
        // the field was absent.
        i_max: floats("cm_ub", o.get("cm_ub")),
        s_max: floats("sm_ub", o.get("sm_ub")),
        source: None,
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

/// `DistWinding.tap` is a scalar; the first phase tap represents each winding
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
    configs: &[DistWindingConn],
    polarity: &[i64],
    o: &Map<String, Value>,
    nums: &WindingNums,
) -> (Vec<DistWinding>, usize, bool) {
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
        let conn = configs.get(w).copied().unwrap_or(DistWindingConn::Wye);
        if polarity.get(w) == Some(&-1)
            && conn == DistWindingConn::Wye
            && configs.first() == Some(&DistWindingConn::Delta)
            && map.len() > 1
        {
            let phases_part = map.len() - 1;
            map[..phases_part].rotate_right(1);
            unrolled = true;
        }
        if conn == DistWindingConn::Wye {
            phases = phases.max(map.len().saturating_sub(1));
        } else {
            phases = phases.max(map.len());
        }
        windings.push(DistWinding {
            bus: bus.clone(),
            terminal_map: map,
            conn,
            v_ref: nums.vm_nom.get(w).copied().unwrap_or(f64::NAN) * 1e3,
            s_rating: nums.sm_nom.get(w).copied().unwrap_or(f64::NAN) * 1e3,
            r_pct: nums.rw.get(w).copied().unwrap_or(0.0) * 100.0,
            tap: nums.tm_set.get(w).copied().unwrap_or(1.0),
            r_neutral: None,
            x_neutral: None,
        });
    }
    (windings, phases, unrolled)
}

/// The known sections process in a fixed order, not the document's
/// (serde_json maps iterate sorted, which puts "line" before "linecode"):
/// `lines` consults the already materialized linecodes for the inline
/// impedance `{name}_z` collision check, so "linecode" must come first.
/// The other sections do not consult each other; unknown sections follow
/// in document order.
const SECTIONS: &[&str] = &[
    "bus",
    "linecode",
    "line",
    "switch",
    "load",
    "generator",
    "shunt",
    "voltage_source",
    "transformer",
];

impl Reader<'_> {
    fn document(&mut self, doc: &Map<String, Value>) {
        if let Some(name) = doc.get("name").and_then(Value::as_str) {
            *self.net.name_mut() = Some(name.to_string());
        }
        let settings = doc.get("settings").and_then(Value::as_object);
        let stated = settings
            .and_then(|s| s.get("base_frequency"))
            .and_then(Value::as_f64);
        if let Some(f) = stated {
            *self.net.base_frequency_mut() = f;
        }
        if let Some(settings) = settings {
            self.net
                .extras_mut()
                .insert("pmd_settings".into(), Value::Object(settings.clone()));
        }
        for key in ["data_model", "files", "conductor_ids", "per_unit"] {
            if let Some(v) = doc.get(key) {
                self.net
                    .extras_mut()
                    .insert(format!("pmd_{key}"), v.clone());
            }
        }

        for &key in SECTIONS {
            let Some(Value::Object(items)) = doc.get(key) else {
                continue;
            };
            match key {
                "bus" => self.buses(items),
                "linecode" => self.linecodes(items),
                "line" => self.lines(items),
                "switch" => self.switches(items),
                "load" => self.loads(items),
                "generator" => self.generators(items),
                "shunt" => self.shunts(items),
                "voltage_source" => self.sources(items),
                "transformer" => self.transformers(items),
                _ => unreachable!(),
            }
        }
        for (key, value) in doc {
            if SECTIONS.contains(&key.as_str()) || key == "settings" || key == "name" {
                continue;
            }
            let Value::Object(items) = value else {
                continue;
            };
            self.diagnostics.push(
                &C::READ_PMD_RETAINED_SOURCE_ONLY,
                format!("ENGINEERING `{key}` components are not typed; kept untyped"),
            );
            for (name, v) in items {
                self.net.untyped_mut().push(UntypedObject {
                    class: key.clone(),
                    name: name.clone(),
                    props: vec![(None, v.to_string())],
                });
            }
        }
        if stated.is_none() {
            crate::model::warn_defaulted_frequency(
                self.net,
                "settings.base_frequency",
                &mut self.diagnostics,
            );
        }
    }

    fn buses(&mut self, items: &Map<String, Value>) {
        for (id, v) in items {
            let Value::Object(o) = v else { continue };
            let mut extras = take_extras(
                o,
                &[
                    "terminals",
                    "grounded",
                    "rg",
                    "xg",
                    "status",
                    "lat",
                    "lon",
                    "vm_lb",
                    "vm_ub",
                ],
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
                self.diagnostics.push(
                    &C::READ_PMD_RETAINED_SOURCE_ONLY,
                    format!("bus {id}: nonzero grounding impedance is not typed; kept in extras"),
                );
                extras.insert("rg".into(), o.get("rg").cloned().unwrap_or(Value::Null));
                extras.insert("xg".into(), o.get("xg").cloned().unwrap_or(Value::Null));
            }
            stash_status(o, &mut extras, &format!("bus {id}"), &mut self.diagnostics);
            // `vm_lb`/`vm_ub` are per-terminal vectors in the ENGINEERING
            // voltage unit (volts over `voltage_scale_factor`); the model's
            // scalar bound takes the uniform value, the same collapse the
            // BMOPF reader applies to its per-terminal arrays.
            let bound = |key: &str,
                         diagnostics: &mut crate::diagnostics::Diagnostics|
             -> Option<f64> {
                let vals = floats(key, o.get(key))?;
                let first = vals.first().copied()?;
                if vals.iter().any(|v| v.to_bits() != first.to_bits()) {
                    diagnostics.push(&C::READ_PMD_VALUE_COLLAPSED, format!(
                        "bus {id}: `{key}` is non-uniform across terminals; collapsed to the first entry"
                    ));
                }
                Some(first * 1e3)
            };
            let v_min = bound("vm_lb", &mut self.diagnostics);
            let v_max = bound("vm_ub", &mut self.diagnostics);
            self.net.buses_mut().push(DistBus {
                id: id.clone(),
                terminals: ints_as_strings(o.get("terminals")),
                grounded: ints_as_strings(o.get("grounded")),
                v_min,
                v_max,
                extras,
                ..DistBus::default()
            });
        }
    }

    fn linecodes(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let mut lc = linecode_from(name, o, self.net.base_frequency(), &mut self.diagnostics);
            let mut extras = take_extras(
                o,
                &["rs", "xs", "g_fr", "g_to", "b_fr", "b_to", "cm_ub", "sm_ub"],
            );
            extras.append(&mut lc.extras);
            lc.extras = extras;
            self.net.linecodes_mut().push(lc);
        }
    }

    fn lines(&mut self, items: &Map<String, Value>) {
        // Carried across the loop rather than probed with net.linecode()
        // (a linear scan) on every candidate name: a document with many
        // colliding inline-impedance lines would otherwise make the
        // collision search itself quadratic in the line count.
        let mut linecode_names: BTreeSet<String> = self
            .net
            .linecodes()
            .iter()
            .map(|c| c.name.to_ascii_lowercase())
            .collect();
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
            let mut i_max = None;
            let mut s_max = None;
            // Inline impedance (the dss2eng output for rmatrix defined
            // lines): materialize a linecode so the matrices survive, and
            // mark the line so the PMD writer re-inlines them. The inline
            // ratings live on the synthesized linecode, not the line.
            if linecode.is_empty() && o.get("rs").is_some() {
                known.extend(["rs", "xs", "g_fr", "g_to", "b_fr", "b_to", "cm_ub", "sm_ub"]);
                extras = take_extras(o, &known);
                let mut lc_name = format!("{name}_z");
                let mut k = 2;
                while linecode_names.contains(&lc_name.to_ascii_lowercase()) {
                    lc_name = format!("{name}_z{k}");
                    k += 1;
                }
                linecode_names.insert(lc_name.to_ascii_lowercase());
                let lc = linecode_from(
                    &lc_name,
                    o,
                    self.net.base_frequency(),
                    &mut self.diagnostics,
                );
                self.net.linecodes_mut().push(lc);
                self.diagnostics.push(&C::READ_PMD_VALUE_INLINED, format!(
                    "line {name}: inline impedance materialized as linecode {lc_name}; the PMD writer re-inlines it"
                ));
                extras.insert("pmd_inline".into(), Value::Bool(true));
                linecode = lc_name;
            } else {
                known.extend(["cm_ub", "sm_ub"]);
                extras = take_extras(o, &known);
                i_max = floats("cm_ub", o.get("cm_ub"));
                s_max = floats("sm_ub", o.get("sm_ub"));
            }
            stash_status(
                o,
                &mut extras,
                &format!("line {name}"),
                &mut self.diagnostics,
            );
            self.net.lines_mut().push(DistLine {
                name: name.clone(),
                bus_from: string(o.get("f_bus")),
                bus_to: string(o.get("t_bus")),
                terminal_map_from: ints_as_strings(o.get("f_connections")),
                terminal_map_to: ints_as_strings(o.get("t_connections")),
                linecode: Some(linecode),
                length: o.get("length").map_or(f64::NAN, |v| restore("length", v)),
                route: None,
                i_max,
                s_max,
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
                &mut self.diagnostics,
            );
            self.net.switches_mut().push(DistSwitch {
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
            // A scalar vm_nom rides along as the dss-style kv hint so the
            // original token replays verbatim. The array form restates the
            // typed voltage model's v_nom entry for entry — the PMD writer
            // falls back to v_nom for it, and the dss writer can only warn
            // about a token it cannot parse — so it is not copied.
            if let Some(kv) = o.get("vm_nom").filter(|v| !v.is_array()) {
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
            let v_nom: Vec<f64> = floats("vm_nom", o.get("vm_nom"))
                .or_else(|| o.get("vm_nom").map(|v| vec![restore("vm_nom", v)]))
                .unwrap_or_default()
                .iter()
                .map(|v| v * 1e3)
                .collect();
            let voltage_model = match o.get("model").and_then(Value::as_str) {
                Some("IMPEDANCE") => DistLoadVoltageModel::ConstantImpedance { v_nom },
                Some("CURRENT") => DistLoadVoltageModel::ConstantCurrent { v_nom },
                Some("ZIPV") => DistLoadVoltageModel::Zip {
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
            stash_status(
                o,
                &mut extras,
                &format!("load {name}"),
                &mut self.diagnostics,
            );
            self.net.loads_mut().push(DistLoad {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: connections,
                configuration,
                p_nom: scale("pd_nom"),
                q_nom: scale("qd_nom"),
                voltage_model,
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
                &mut self.diagnostics,
            );
            self.net.generators_mut().push(DistGenerator {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: ints_as_strings(o.get("connections")),
                configuration: match o.get("configuration").and_then(Value::as_str) {
                    Some("DELTA") => Configuration::Delta,
                    _ => Configuration::Wye,
                },
                p_nom: scale("pg").unwrap_or_default(),
                q_nom: scale("qg").unwrap_or_default(),
                // A null bound restores as +/-Inf (an unbounded phase);
                // keeping the mixed array preserves the finite bounds
                // beside it. `None` means the field was absent.
                p_min: scale("pg_lb"),
                p_max: scale("pg_ub"),
                q_min: scale("qg_lb"),
                q_max: scale("qg_ub"),
                cost: None,
                s_max: None,
                i_max: None,
                extras,
            });
        }
    }

    fn shunts(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let what = format!("shunt {name}");
            let g = matrix("gs", o.get("gs"), &what, &mut self.diagnostics).unwrap_or_default();
            let b = matrix("bs", o.get("bs"), &what, &mut self.diagnostics).unwrap_or_default();
            let mut extras = take_extras(
                o,
                &["bus", "connections", "gs", "bs", "status", "source_id"],
            );
            stash_status(
                o,
                &mut extras,
                &format!("shunt {name}"),
                &mut self.diagnostics,
            );
            self.net.shunts_mut().push(DistShunt {
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
                &mut self.diagnostics,
            );
            self.net.sources_mut().push(VoltageSource {
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
            self.net.transformers_mut().push(t);
        }
    }

    /// The writer recomputes polarity from the lag convention; when the
    /// file disagrees (a euro/lead or reversed winding), the raw arrays
    /// ride in extras and the writer emits them verbatim.
    fn stash_polarity(
        &mut self,
        name: &str,
        o: &Map<String, Value>,
        windings: &[DistWinding],
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
        self.diagnostics.push(&C::READ_PMD_RETAINED_SOURCE_ONLY, format!(
            "transformer {name}: polarity {file_polarity:?} is not the lag convention; kept in extras (other formats assume lag)"
        ));
    }

    // One block per object field; splitting it would scatter a list that
    // reads end to end.
    #[expect(clippy::too_many_lines)]
    fn transformer(&mut self, name: &str, o: &Map<String, Value>) -> DistTransformer {
        // The winding count is the `bus` array length and drives an O(n^2)
        // pair enumeration in the graph builder, so it is capped like the
        // conductor matrix dimension: a huge array cannot force quadratic work.
        let mut buses = ints_as_strings(o.get("bus"));
        if buses.len() > MAX_MATRIX_DIM {
            self.diagnostics.push(
                &C::READ_PMD_RECORD_DROPPED,
                format!(
                    "transformer {name}: winding count {} exceeds the supported maximum of \
                 {MAX_MATRIX_DIM}; extra windings dropped",
                    buses.len()
                ),
            );
            buses.truncate(MAX_MATRIX_DIM);
        }
        let configs: Vec<DistWindingConn> = o
            .get("configuration")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|c| {
                        if c.as_str() == Some("DELTA") {
                            DistWindingConn::Delta
                        } else {
                            DistWindingConn::Wye
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
            self.diagnostics.push(&C::READ_PMD_VALUE_COLLAPSED, format!(
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
            self.diagnostics.push(
                &C::READ_PMD_RETAINED_SOURCE_ONLY,
                format!("transformer {name}: regulator controls are not typed; kept in extras"),
            );
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
            &mut self.diagnostics,
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

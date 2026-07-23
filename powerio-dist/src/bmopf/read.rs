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

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::geo::{CoordinateSpace, CoordsKind, GeoMeta, Location};
use crate::model::{
    ActivePowerReference, ActivePowerUnit, Configuration, ControlVoltageReference, DistBus,
    DistCapacitor, DistControlProfile, DistGenerator, DistIbr, DistLine, DistLineCode, DistLoad,
    DistLoadVoltageModel, DistNetwork, DistShunt, DistSourceFormat, DistSwitch, DistTransformer,
    Extras, IbrPrimeMover, IbrTopology, IbrVoltageAggregation, Mat, PowerFactorControl,
    ReactivePowerReference, ReactivePowerUnit, UntypedObject, VoltVarControl, VoltWattControl,
    VoltageSource, Winding, WindingConn, n_winding_impedance_base, n_winding_phase_count,
    pair_keys,
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
    crate::model::warn_unresolved_references(&mut net);
    Ok(net)
}

struct Reader<'a> {
    net: &'a mut DistNetwork,
}

const BMOPF_DELTA_ROLLS_EXTRA: &str = "bmopf_delta_rolls";

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

fn delta_roll_value(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::as_i64)
        .filter(|roll| matches!(*roll, -1 | 1))
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

/// Folds `extras.transformer.<subtype>.<name>` fields back onto the raw
/// transformer objects (the reverse of the writer's overflow split). The
/// in-place field wins over the overlay on a key collision.
fn merge_transformer_overlay(
    subtypes: &Map<String, Value>,
    overlay: &Map<String, Value>,
) -> Map<String, Value> {
    let mut merged = subtypes.clone();
    for (subtype, names) in overlay {
        let Value::Object(names) = names else {
            continue;
        };
        let Some(Value::Object(table)) = merged.get_mut(subtype) else {
            continue;
        };
        for (name, fields) in names {
            let (Value::Object(fields), Some(Value::Object(target))) =
                (fields, table.get_mut(name))
            else {
                continue;
            };
            for (key, value) in fields {
                target.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    merged
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

fn enum_field<T: DeserializeOwned>(
    v: Option<&Value>,
    what: &str,
    warnings: &mut Vec<String>,
) -> Option<T> {
    let value = v?;
    match serde_json::from_value(value.clone()) {
        Ok(parsed) => Some(parsed),
        Err(err) => {
            warnings.push(format!("{what}: {err}; field ignored"));
            None
        }
    }
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

/// The largest conductor index a `prefix_i_j` matrix key may carry. The
/// largest index seen sizes a dense n×n allocation in [`flat_matrix`], so an
/// unbounded key (a few bytes of JSON) could demand gigabytes; no physical
/// conductor bundle comes near this bound. An oversized index makes the key
/// unrecognized, so it lands in extras with the malformed-key warning.
const MAX_MATRIX_INDEX: usize = 64;

/// Parses the `_i_j` tail of a `prefix_i_j` matrix key (1 based). None
/// when the key is not a well formed entry for this prefix.
fn matrix_indices(key: &str, prefix: &str) -> Option<(usize, usize)> {
    let rest = key.strip_prefix(prefix)?.strip_prefix('_')?;
    let (i, j) = rest.split_once('_')?;
    let (i, j) = (i.parse::<usize>().ok()?, j.parse::<usize>().ok()?);
    (i >= 1 && j >= 1 && i <= MAX_MATRIX_INDEX && j <= MAX_MATRIX_INDEX).then_some((i, j))
}

/// Collects `prefix_i_j` keys into a square matrix; `n` is the largest
/// index seen. Returns None when no key carries the prefix. A cell whose
/// transpose is not spelled is mirrored: these matrices are symmetric and
/// BMOPFTools writes one triangle only.
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
    let mut spelled = vec![vec![false; n]; n];
    for (i, j, v) in entries {
        m[i][j] = v;
        spelled[i][j] = true;
    }
    for i in 0..n {
        for j in 0..n {
            if spelled[i][j] && !spelled[j][i] {
                m[j][i] = m[i][j];
            }
        }
    }
    Some(m)
}

/// The six linecode-shaped matrices of `o`, padded square to the widest one
/// present; `ragged` reports a genuine size disagreement between them.
fn linecode_matrices(o: &Map<String, Value>) -> ([Mat; 6], usize, bool) {
    let mats = [
        flat_matrix(o, "R_series"),
        flat_matrix(o, "X_series"),
        flat_matrix(o, "G_from"),
        flat_matrix(o, "B_from"),
        flat_matrix(o, "G_to"),
        flat_matrix(o, "B_to"),
    ];
    let n = mats.iter().flatten().map(Vec::len).max().unwrap_or(0);
    let ragged = mats.iter().flatten().any(|m| m.len() < n);
    (mats.map(|m| pad_to(m.unwrap_or_default(), n)), n, ragged)
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
        // Schema 0.1.0 carries the frequency in `meta`; the top-level
        // spellings are the pre-0.1.0 vintage.
        if let Some(frequency) = first_float(
            doc.get("meta")
                .and_then(Value::as_object)
                .and_then(|m| m.get("frequency")),
        )
        .or_else(|| first_float(doc.get("base_frequency")))
        .or_else(|| first_float(doc.get("frequency")))
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
                "capacitor" => self.capacitors(items),
                "ibr" => self.ibrs(items),
                "control_profile" => self.control_profiles(items),
                "shunt" => self.shunts(items),
                "voltage_source" => self.sources(items),
                "transformer" => {
                    // The writer relocates schema-less transformer fields
                    // (taps, neutral impedance, no load admittance) to
                    // `extras.transformer.<subtype>.<name>`; fold them back
                    // onto the raw objects before parsing.
                    let overlay = doc
                        .get("extras")
                        .and_then(Value::as_object)
                        .and_then(|e| e.get("transformer"))
                        .and_then(Value::as_object)
                        .filter(|o| !o.is_empty());
                    match overlay {
                        Some(overlay) => {
                            let merged = merge_transformer_overlay(items, overlay);
                            self.transformers(&merged);
                        }
                        None => self.transformers(items),
                    }
                }
                "extras" => self.extras_block(items),
                // The phase/neutral label conventions block: no typed slot,
                // stashed whole so a round trip keeps it (the meta pattern).
                "terminal_conventions" => {
                    self.net.extras.insert(
                        "bmopf_terminal_conventions".into(),
                        Value::Object(items.clone()),
                    );
                }
                "name" => {}
                // `meta` is provenance (license, authors, generator tool),
                // with no typed slot in the model. Stash it whole, the way the
                // PMD reader stashes `pmd_settings`, so a read keeps it; the
                // BMOPF writer still regenerates its own `meta` block.
                "meta" => {
                    self.net
                        .extras
                        .insert("bmopf_meta".into(), Value::Object(items.clone()));
                }
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

    /// The top-level `extras` escape hatch (schema 0.1.0). The IBR and
    /// control profile tables that lost their top-level slots read typed from
    /// here, the transformer overflow is consumed by the transformer merge,
    /// and everything else is stashed verbatim for the writer to re-emit.
    fn extras_block(&mut self, items: &Map<String, Value>) {
        let mut stash = Map::new();
        for (key, value) in items {
            match (key.as_str(), value) {
                ("ibr", Value::Object(table)) => self.ibrs(table),
                ("control_profile", Value::Object(table)) => self.control_profiles(table),
                ("transformer", Value::Object(_)) => {}
                _ => {
                    stash.insert(key.clone(), value.clone());
                }
            }
        }
        if !stash.is_empty() {
            self.net
                .extras
                .insert("bmopf_extras".into(), Value::Object(stash));
        }
    }

    fn capacitors(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let known = ["bus", "terminal_map", "configuration", "q_rated", "v_nom"];
            for (field, value) in [("q_rated", o.get("q_rated")), ("v_nom", o.get("v_nom"))] {
                if value.is_none() {
                    self.net
                        .warnings
                        .push(format!("capacitor {name}: `{field}` missing; read as NaN"));
                }
            }
            self.net.capacitors.push(DistCapacitor {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: strings(o.get("terminal_map")),
                configuration: config(
                    o.get("configuration"),
                    &format!("capacitor {name}"),
                    &mut self.net.warnings,
                ),
                q_rated: o.get("q_rated").map_or(f64::NAN, f),
                v_nom: o.get("v_nom").map_or(f64::NAN, f),
                extras: take_extras(
                    o,
                    &known,
                    &format!("capacitor {name}"),
                    &mut self.net.warnings,
                    &[],
                ),
            });
        }
    }

    fn ibrs(&mut self, items: &Map<String, Value>) {
        const TYPED: &[&str] = &[
            "bus",
            "terminal_map",
            "topology",
            "prime_mover",
            "s_max",
            "i_max",
            "p_avail",
            "p_min",
            "p_max",
            "q_min",
            "q_max",
            "control_profile",
            "voltage_aggregation",
        ];
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let topology = enum_field::<IbrTopology>(
                o.get("topology"),
                &format!("ibr {name} topology"),
                &mut self.net.warnings,
            )
            .unwrap_or(IbrTopology::SinglePhase);
            let prime_mover = enum_field::<IbrPrimeMover>(
                o.get("prime_mover"),
                &format!("ibr {name} prime_mover"),
                &mut self.net.warnings,
            )
            .unwrap_or(IbrPrimeMover::Generic);
            let mut extras = Extras::new();
            for (key, value) in o {
                if !TYPED.contains(&key.as_str()) {
                    extras.insert(key.clone(), value.clone());
                }
            }
            self.net.ibrs.push(DistIbr {
                name: name.clone(),
                bus: string(o.get("bus")),
                terminal_map: strings(o.get("terminal_map")),
                topology,
                prime_mover,
                s_max: floats(o.get("s_max")).unwrap_or_default(),
                i_max: floats(o.get("i_max")),
                p_avail: first_float(o.get("p_avail")),
                p_min: floats(o.get("p_min")),
                p_max: floats(o.get("p_max")),
                q_min: floats(o.get("q_min")),
                q_max: floats(o.get("q_max")),
                control_profile: o
                    .get("control_profile")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                voltage_aggregation: enum_field::<IbrVoltageAggregation>(
                    o.get("voltage_aggregation"),
                    &format!("ibr {name} voltage_aggregation"),
                    &mut self.net.warnings,
                ),
                extras,
            });
        }
    }

    fn control_profiles(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let mut profile = DistControlProfile::new(name.clone());
            if let Some(Value::Object(pf)) = o.get("power_factor") {
                profile.power_factor =
                    first_float(pf.get("pf")).map(|pf| PowerFactorControl { pf });
            }
            if let Some(Value::Object(vv)) = o.get("volt_var") {
                profile.volt_var = Some(VoltVarControl {
                    voltage_reference: enum_field::<ControlVoltageReference>(
                        vv.get("voltage_reference"),
                        &format!("control_profile {name} volt_var voltage_reference"),
                        &mut self.net.warnings,
                    ),
                    breakpoints: floats(vv.get("breakpoints")).unwrap_or_default(),
                    q_limits: floats(vv.get("q_limits")).unwrap_or_default(),
                    q_unit: enum_field::<ReactivePowerUnit>(
                        vv.get("q_unit"),
                        &format!("control_profile {name} volt_var q_unit"),
                        &mut self.net.warnings,
                    ),
                    q_ref: enum_field::<ReactivePowerReference>(
                        vv.get("q_ref"),
                        &format!("control_profile {name} volt_var q_ref"),
                        &mut self.net.warnings,
                    ),
                    p_min_for_q: first_float(vv.get("p_min_for_q")),
                    p_min_for_q_max: first_float(vv.get("p_min_for_q_max")),
                });
            }
            if let Some(Value::Object(vw)) = o.get("volt_watt") {
                profile.volt_watt = Some(VoltWattControl {
                    voltage_reference: enum_field::<ControlVoltageReference>(
                        vw.get("voltage_reference"),
                        &format!("control_profile {name} volt_watt voltage_reference"),
                        &mut self.net.warnings,
                    ),
                    breakpoints: floats(vw.get("breakpoints")).unwrap_or_default(),
                    p_limits: floats(vw.get("p_limits")).unwrap_or_default(),
                    p_unit: enum_field::<ActivePowerUnit>(
                        vw.get("p_unit"),
                        &format!("control_profile {name} volt_watt p_unit"),
                        &mut self.net.warnings,
                    ),
                    p_ref: enum_field::<ActivePowerReference>(
                        vw.get("p_ref"),
                        &format!("control_profile {name} volt_watt p_ref"),
                        &mut self.net.warnings,
                    ),
                });
            }
            for (key, value) in o {
                if !matches!(key.as_str(), "power_factor" | "volt_var" | "volt_watt") {
                    profile.extras.insert(key.clone(), value.clone());
                }
            }
            self.net.control_profiles.push(profile);
        }
    }

    fn buses(&mut self, items: &Map<String, Value>) {
        for (id, v) in items {
            let Value::Object(o) = v else { continue };
            let known = [
                "terminal_names",
                "perfectly_grounded_terminals",
                "longitude",
                "latitude",
                "v_min",
                "v_max",
                "vpn_min",
                "vpn_max",
                "vpp_min",
                "vpp_max",
                "vpos_min",
                "vpos_max",
                "vneg_max",
                "vzero_max",
                "vn_max",
                "vsym_min",
                "vsym_max",
            ];
            let lon = first_float(o.get("longitude")).filter(|v| v.is_finite());
            let lat = first_float(o.get("latitude")).filter(|v| v.is_finite());
            let has_lon = o.contains_key("longitude");
            let has_lat = o.contains_key("latitude");
            let location = match (lon, lat) {
                (Some(x), Some(y)) => {
                    self.net.geo = Some(GeoMeta {
                        space: CoordinateSpace::Geographic { crs: None },
                        kind: Some(CoordsKind::Source),
                    });
                    Some(Location { x, y, kind: None })
                }
                _ if has_lon || has_lat => {
                    self.net.warnings.push(format!(
                        "bus {id}: longitude/latitude sideload is incomplete or nonfinite; kept in extras"
                    ));
                    None
                }
                _ => None,
            };
            let mut extras =
                take_extras(o, &known, &format!("bus {id}"), &mut self.net.warnings, &[]);
            if location.is_none() {
                if let Some(value) = o.get("longitude") {
                    extras.insert("longitude".into(), value.clone());
                }
                if let Some(value) = o.get("latitude") {
                    extras.insert("latitude".into(), value.clone());
                }
            }
            // Legacy (pre-0.1.0) `vsym_min`/`vsym_max` arrays carried the
            // symmetrical component bounds in zero/positive/negative order.
            // Schema 0.1.0 replaced them with named per-sequence scalars;
            // map what has a slot and warn about what does not (the negative
            // and zero sequence lower bounds are fixed at 0 in 0.1.0).
            let legacy_min = floats(o.get("vsym_min"));
            let legacy_max = floats(o.get("vsym_max"));
            if legacy_min.is_some() || legacy_max.is_some() {
                self.net.warnings.push(format!(
                    "bus {id}: legacy vsym_min/vsym_max arrays mapped to the per-sequence \
                     scalars assuming zero/positive/negative order"
                ));
            }
            let legacy_vpos_min = legacy_min.as_ref().and_then(|v| v.get(1).copied());
            let legacy_vpos_max = legacy_max.as_ref().and_then(|v| v.get(1).copied());
            let legacy_vzero_max = legacy_max.as_ref().and_then(|v| v.first().copied());
            let legacy_vneg_max = legacy_max.as_ref().and_then(|v| v.get(2).copied());
            if legacy_min
                .as_ref()
                .is_some_and(|v| [v.first(), v.get(2)].iter().flatten().any(|&&m| m != 0.0))
            {
                self.net.warnings.push(format!(
                    "bus {id}: legacy vsym_min zero/negative sequence lower bounds have no \
                     slot in schema 0.1.0 (fixed at 0); dropped"
                ));
            }
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
                vpos_min: first_float(o.get("vpos_min")).or(legacy_vpos_min),
                vpos_max: first_float(o.get("vpos_max")).or(legacy_vpos_max),
                vneg_max: first_float(o.get("vneg_max")).or(legacy_vneg_max),
                vzero_max: first_float(o.get("vzero_max")).or(legacy_vzero_max),
                vn_max: first_float(o.get("vn_max")),
                location,
                extras,
            });
        }
    }

    fn linecodes(&mut self, items: &Map<String, Value>) {
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            // Conductor count is the widest matrix present; absent matrices
            // read as zero, smaller ones pad without losing entries.
            let ([r, x, gf, bf, gt, bt], n, ragged) = linecode_matrices(o);
            if ragged {
                self.net.warnings.push(format!(
                    "linecode {name}: matrix sizes disagree; smaller ones padded \
                     with zeros to {n}x{n}"
                ));
            }
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
        let mut taken: std::collections::BTreeSet<String> =
            self.net.linecodes.iter().map(|c| c.name.clone()).collect();
        for (name, v) in items {
            let Value::Object(o) = v else { continue };
            let known = [
                "length",
                "linecode",
                "bus_from",
                "bus_to",
                "terminal_map_from",
                "terminal_map_to",
                "i_max",
                "s_max",
            ];
            // Schema 0.1.0 lines carry either a linecode + length or inline
            // impedance matrices (the linecode oneOf). Inline matrices read
            // into a synthesized single-use linecode named after the line.
            let mut linecode = string(o.get("linecode"));
            let mut length = o.get("length").map_or(f64::NAN, f);
            // The inline branch of the schema's oneOf requires R_series_1_1.
            let inline = linecode.is_empty() && o.contains_key("R_series_1_1");
            if inline {
                linecode = self.synthesized_linecode(name, o, &mut taken);
                if !length.is_finite() {
                    // Inline matrices are the line's whole impedance; the
                    // synthesized linecode is per meter at unit length.
                    length = 1.0;
                }
            } else if !length.is_finite() {
                // The schema requires `length` alongside a linecode; a missing
                // one becomes NaN in the model, which every impedance
                // computation downstream inherits, so name the gap the way the
                // transformer reader names a missing s_rating instead of
                // letting the NaN travel silently.
                self.net.warnings.push(format!(
                    "line {name}: `length` missing or non-finite; impedances derived from \
                     this line are undefined"
                ));
            }
            self.net.lines.push(DistLine {
                name: name.clone(),
                bus_from: string(o.get("bus_from")),
                bus_to: string(o.get("bus_to")),
                terminal_map_from: strings(o.get("terminal_map_from")),
                terminal_map_to: strings(o.get("terminal_map_to")),
                linecode,
                length,
                route: None,
                i_max: floats(o.get("i_max")),
                s_max: floats(o.get("s_max")),
                extras: take_extras(
                    o,
                    &known,
                    &format!("line {name}"),
                    &mut self.net.warnings,
                    if inline {
                        &["R_series", "X_series", "G_from", "G_to", "B_from", "B_to"]
                    } else {
                        &[]
                    },
                ),
            });
        }
    }

    /// Reads a line's inline impedance matrices into a linecode named after
    /// the line (suffixed if taken), returning the linecode name.
    fn synthesized_linecode(
        &mut self,
        line: &str,
        o: &Map<String, Value>,
        taken: &mut std::collections::BTreeSet<String>,
    ) -> String {
        let mut name = line.to_string();
        while taken.contains(name.as_str()) {
            name.push('_');
        }
        taken.insert(name.clone());
        self.net.warnings.push(format!(
            "line {line}: inline impedance matrices read into synthesized linecode `{name}`"
        ));
        let ([r, x, gf, bf, gt, bt], n, _) = linecode_matrices(o);
        self.net.linecodes.push(DistLineCode {
            name: name.clone(),
            n_conductors: n,
            r_series: r,
            x_series: x,
            g_from: gf,
            b_from: bf,
            g_to: gt,
            b_to: bt,
            i_max: None,
            s_max: None,
            extras: Extras::new(),
        });
        name
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
            "r_neutral_from",
            "x_neutral_from",
            "r_neutral_to",
            "x_neutral_to",
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
        let has_split_three_phase_fields = [
            "r_series_from",
            "r_series_to",
            "x_series_from",
            "x_series_to",
        ]
        .iter()
        .any(|k| o.contains_key(*k));
        let (r_from_pct, r_to_pct, xsc_pct) = if three_phase && has_split_three_phase_fields {
            let r_from = pct(o.get("r_series_from").map_or(0.0, f), v_from);
            let r_to = pct(o.get("r_series_to").map_or(0.0, f), v_to);
            let x_from = pct(o.get("x_series_from").map_or(0.0, f), v_from);
            let x_to = pct(o.get("x_series_to").map_or(0.0, f), v_to);
            (r_from, r_to, vec![x_from + x_to])
        } else if three_phase {
            let wye_v = if subtype == "wye_delta" { v_from } else { v_to };
            // The schema puts one series impedance on the wye side; the
            // model splits resistance evenly across the windings.
            let r = pct(o.get("r_series").map_or(0.0, f), wye_v);
            let x = pct(o.get("x_series").map_or(0.0, f), wye_v);
            (r / 2.0, r / 2.0, vec![x])
        } else {
            let r_from = pct(o.get("r_series_from").map_or(0.0, f), v_from);
            let r_to = pct(o.get("r_series_to").map_or(0.0, f), v_to);
            let x_from = pct(o.get("x_series_from").map_or(0.0, f), v_from);
            let x_to = pct(o.get("x_series_to").map_or(0.0, f), v_to);
            let xsc = if subtype == "center_tap" {
                vec![x_from + x_to, x_from + x_to, 2.0 * x_to]
            } else {
                vec![x_from + x_to]
            };
            (r_from, r_to, xsc)
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
                r_neutral: first_float(o.get("r_neutral_from")),
                x_neutral: first_float(o.get("x_neutral_from")),
            },
            Winding {
                bus: string(o.get("bus_to")),
                terminal_map: strings(o.get("terminal_map_to")),
                conn: conn(subtype == "wye_delta"),
                v_ref: v_to,
                s_rating: s,
                r_pct: r_to_pct,
                tap: 1.0,
                r_neutral: first_float(o.get("r_neutral_to")),
                x_neutral: first_float(o.get("x_neutral_to")),
            },
        ];
        expand_center_tap_windings(subtype, &mut windings, &self.net.buses);
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
            xsc_pct,
            phases,
            extras,
        }
    }

    /// Cap the winding list a document supplies. The short circuit pair
    /// enumeration is quadratic in the winding count, so an unbounded count
    /// would be a quadratic blowup from a linear document; no physical
    /// transformer comes near the cap.
    fn bounded_windings<'a>(&mut self, name: &str, items: &'a [Value]) -> &'a [Value] {
        const MAX_WINDINGS: usize = 64;
        if items.len() > MAX_WINDINGS {
            self.net.warnings.push(format!(
                "transformer {name}: {} windings exceed the supported maximum of \
                 {MAX_WINDINGS}; the rest are ignored",
                items.len()
            ));
        }
        &items[..items.len().min(MAX_WINDINGS)]
    }

    fn n_winding_transformer(&mut self, name: &str, o: &Map<String, Value>) -> DistTransformer {
        let known = ["windings", "x_sc", "s_rating", "g_no_load", "b_no_load"];
        let s = o.get("s_rating").map_or(f64::NAN, f);
        let mut windings = Vec::new();
        let mut delta_rolls = Map::new();
        if let Some(items) = o.get("windings").and_then(Value::as_array) {
            for (idx, item) in self.bounded_windings(name, items).iter().enumerate() {
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
                if let Some(delta_roll) = delta_roll_value(w.get("delta_roll")) {
                    delta_rolls.insert((idx + 1).to_string(), Value::from(delta_roll));
                }
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
                    r_neutral: None,
                    x_neutral: None,
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
        if !delta_rolls.is_empty() {
            extras.insert(BMOPF_DELTA_ROLLS_EXTRA.into(), Value::Object(delta_rolls));
        }
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

fn expand_center_tap_windings(subtype: &str, windings: &mut Vec<Winding>, buses: &[DistBus]) {
    if subtype != "center_tap" || windings[1].terminal_map.len() < 3 {
        return;
    }
    let to = windings.pop().expect("secondary winding exists");
    let neutral_idx = center_tap_neutral_index(&to, buses);
    let canonical = neutral_idx == 1;
    let common = to
        .terminal_map
        .get(neutral_idx)
        .cloned()
        .unwrap_or_default();
    let hots: Vec<String> = to
        .terminal_map
        .iter()
        .enumerate()
        .filter_map(|(idx, term)| (idx != neutral_idx).then_some(term.clone()))
        .collect();
    let hot_a = hots.first().cloned().unwrap_or_default();
    let hot_b = hots.get(1).cloned().unwrap_or_default();
    let v_ref = if canonical { to.v_ref } else { to.v_ref / 2.0 };
    let r_pct = if canonical { to.r_pct } else { to.r_pct * 2.0 };
    let half = Winding {
        bus: to.bus.clone(),
        terminal_map: vec![hot_a, common.clone()],
        conn: WindingConn::Wye,
        v_ref,
        s_rating: to.s_rating,
        r_pct,
        tap: to.tap,
        r_neutral: to.r_neutral,
        x_neutral: to.x_neutral,
    };
    let other_half = Winding {
        bus: to.bus,
        terminal_map: vec![common, hot_b],
        conn: WindingConn::Wye,
        v_ref,
        s_rating: to.s_rating,
        r_pct,
        tap: to.tap,
        r_neutral: None,
        x_neutral: None,
    };
    windings.push(half);
    windings.push(other_half);
}

fn center_tap_neutral_index(to: &Winding, buses: &[DistBus]) -> usize {
    if let Some(bus) = buses.iter().find(|bus| bus.id == to.bus)
        && let Some((idx, _)) = to
            .terminal_map
            .iter()
            .enumerate()
            .find(|(_, term)| bus.grounded.iter().any(|ground| ground == *term))
    {
        return idx;
    }
    to.terminal_map
        .iter()
        .position(|term| term.eq_ignore_ascii_case("n") || term == "4")
        .unwrap_or_else(|| to.terminal_map.len() - 1)
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

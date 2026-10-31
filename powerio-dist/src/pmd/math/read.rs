//! PMD MATHEMATICAL JSON into the [`MulticonductorNetwork`].
//!
//! The document is index keyed and per unit, so the reader first takes the
//! bases it states (`settings.sbase`, each bus's `vbase`) and restores SI,
//! then walks `map` to fold the lowering back: the ideal units and loss
//! branches of a transformer become one [`DistTransformer`], a switch's
//! virtual bus and branch become its series impedance stash, a voltage
//! source's virtual generator, bus, and branch become one
//! [`VoltageSource`], and a ZIP load's three parts become one
//! [`DistLoad`]. What the lowering discarded is restated and reported: a
//! transformer's winding power rating (the emergency rating stands in, and
//! the impedances are restated on it so the ohms are exact), a line's
//! length (one meter, the impedance being the total), and the neutral a
//! Kron reduced document no longer carries.
//!
//! A document with no `map` reads its real objects and keeps the virtual
//! ones untyped, since nothing says which transformer they belonged to.

use std::collections::{BTreeMap, BTreeSet};

use num_complex::Complex64;
use serde_json::{Map, Value, json};

use super::restore;
use crate::diagnostics::codes as C;
use crate::error::{Error, Result};
use crate::model::{
    ConductorMatrix, Configuration, DistBus, DistGenerator, DistIbr, DistLine, DistLineCode,
    DistLoad, DistLoadVoltageModel, DistShunt, DistSourceFormat, DistSwitch, DistTransformer,
    DistWinding, DistWindingConn, Extras, IbrPrimeMover, IbrTopology, MulticonductorNetwork,
    MulticonductorNetworkTables, UntypedObject, VoltageSource,
};

const FMT: &str = "PMD MATHEMATICAL";

/// A generator's `pmin`, `pmax`, `qmin`, and `qmax`, each when stated.
type GenBounds = (
    Option<Vec<f64>>,
    Option<Vec<f64>>,
    Option<Vec<f64>>,
    Option<Vec<f64>>,
);

/// Parse a MATHEMATICAL document, recording the reader's findings in
/// `diags`.
///
/// # Errors
/// The text is not JSON, not an object, not the MATHEMATICAL model, or a
/// multinetwork document.
pub(crate) fn parse_pmd_math_collecting(
    text: &str,
    diags: &mut crate::collect::Diagnostics,
) -> Result<MulticonductorNetwork> {
    let doc: Value = serde_json::from_str(text).map_err(|e| Error::Json {
        format: FMT,
        message: e.to_string(),
    })?;
    let Value::Object(doc) = doc else {
        return Err(Error::Json {
            format: FMT,
            message: "top level is not an object".into(),
        });
    };
    match doc.get("data_model").and_then(Value::as_str) {
        Some(model) if model.eq_ignore_ascii_case("MATHEMATICAL") => {}
        other => {
            return Err(Error::Json {
                format: FMT,
                message: format!(
                    "`data_model` is {}; this reader takes the MATHEMATICAL model, and the \
                     ENGINEERING model is the `pmd-json` format",
                    other.map_or_else(|| "absent".to_owned(), |m| format!("`{m}`"))
                ),
            });
        }
    }
    if doc.get("multinetwork").and_then(Value::as_bool) == Some(true) {
        return Err(Error::Json {
            format: FMT,
            message: "a multinetwork MATHEMATICAL document is not read; export the ENGINEERING \
                      multinetwork, which parses to a time series"
                .into(),
        });
    }
    validate_shapes(&doc)?;
    validate_bases(&doc)?;
    validate_indices(&doc)?;
    let mut reader = Reader::new(&doc);
    reader.run();
    let net = reader.net;
    diags.absorb(reader.diagnostics);
    crate::model::warn_unresolved_references(&net, diags);
    Ok(net)
}

// ---------------------------------------------------------------------------
// Document access
// ---------------------------------------------------------------------------

/// A stated base must be usable for both dimensionalization and division.
/// Missing bases retain the documented defaults; invalid stated ones must
/// not silently turn impedances into zero or change the unit system.
fn validate_bases(doc: &Map<String, Value>) -> Result<()> {
    let positive = |value: &Value, path: String| -> Result<()> {
        if value.as_f64().is_some_and(|v| v.is_finite() && v > 0.0) {
            Ok(())
        } else {
            Err(Error::Json {
                format: FMT,
                message: format!("{path} must be a finite positive number"),
            })
        }
    };
    if let Some(settings) = doc.get("settings").and_then(Value::as_object) {
        for key in [
            "sbase",
            "sbase_default",
            "voltage_scale_factor",
            "power_scale_factor",
        ] {
            if let Some(value) = settings.get(key) {
                positive(value, format!("settings.{key}"))?;
            }
        }
        if let Some(bases) = settings.get("vbases_default").and_then(Value::as_object) {
            for (key, value) in bases {
                positive(value, format!("settings.vbases_default.{key}"))?;
            }
        }
    }
    for (table, fields) in [
        ("bus", &["vbase"][..]),
        ("transformer", &["f_vbase", "t_vbase"][..]),
    ] {
        if let Some(objects) = doc.get(table).and_then(Value::as_object) {
            for (id, object) in objects {
                for key in fields {
                    if let Some(value) = object.get(*key) {
                        positive(value, format!("{table}.{id}.{key}"))?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        let f = value.as_f64()?;
        (f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f < -(i64::MIN as f64))
            .then_some(f as i64)
    })
}

/// Bus references are identities. Truncating 1.9 to 1 rewires the network.
fn validate_indices(doc: &Map<String, Value>) -> Result<()> {
    for table in [
        "bus",
        "branch",
        "switch",
        "transformer",
        "gen",
        "load",
        "shunt",
        "storage",
    ] {
        if let Some(objects) = doc.get(table).and_then(Value::as_object) {
            for (id, object) in objects {
                for key in [
                    "index",
                    "bus_i",
                    "f_bus",
                    "t_bus",
                    "gen_bus",
                    "load_bus",
                    "shunt_bus",
                    "storage_bus",
                ] {
                    if let Some(value) = object.get(key)
                        && integer(value).is_none()
                    {
                        return Err(Error::Json {
                            format: FMT,
                            message: format!(
                                "{table}.{id}.{key} must be an integer in the signed 64-bit range"
                            ),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

// Match the ENGINEERING reader's bound: conductor matrices and transformer
// loss models allocate quadratically in these dimensions.
pub(super) const MAX_MATRIX_DIM: usize = 64;

/// Check stashed matrices too: the ENGINEERING reader retains switch and
/// source matrices as raw extras, so canonical emission meets the same input.
pub(super) fn matrix_dimension(value: &Value) -> Option<usize> {
    let columns = value.as_array()?;
    let n = columns.len();
    if !(1..=MAX_MATRIX_DIM).contains(&n)
        || !columns.iter().all(|column| {
            column.as_array().is_some_and(|values| {
                values.len() == n && values.iter().all(|v| v.is_number() || v.is_null())
            })
        })
    {
        return None;
    }
    Some(n)
}

fn validate_shapes(doc: &Map<String, Value>) -> Result<()> {
    validate_phase_vectors(doc)?;
    let malformed = |message| Error::Json {
        format: FMT,
        message,
    };
    for (table, keys) in [
        (
            "branch",
            &["br_r", "br_x", "g_fr", "g_to", "b_fr", "b_to"][..],
        ),
        ("shunt", &["gs", "bs"][..]),
    ] {
        let Some(objects) = doc.get(table).and_then(Value::as_object) else {
            continue;
        };
        for (id, object) in objects {
            let mut dimension = None;
            for key in keys {
                let Some(value) = object.get(*key) else {
                    continue;
                };
                let Some(columns) = value.as_array() else {
                    return Err(malformed(format!(
                        "{table}.{id}.{key} is not an array of columns"
                    )));
                };
                let n = columns.len();
                if n == 0 || n > MAX_MATRIX_DIM {
                    return Err(malformed(format!(
                        "{table}.{id}.{key}: matrix dimension {n} is outside 1..={MAX_MATRIX_DIM}"
                    )));
                }
                if matrix_dimension(value).is_none() {
                    return Err(malformed(format!(
                        "{table}.{id}.{key}: expected a square {n} by {n} numeric matrix"
                    )));
                }
                if dimension.is_some_and(|expected| n != expected) {
                    return Err(malformed(format!(
                        "{table}.{id}.{key}: matrix dimensions disagree"
                    )));
                }
                dimension = Some(n);
            }
            if let Some(n) = dimension {
                for key in ["connections", "f_connections", "t_connections"] {
                    if let Some(connections) = object.get(key)
                        && connections.as_array().is_none_or(|a| a.len() != n)
                    {
                        return Err(malformed(format!(
                            "{table}.{id}.{key}: connection count does not match matrix dimension {n}"
                        )));
                    }
                }
            }
        }
    }
    if let Some(entries) = doc.get("map").and_then(Value::as_array) {
        for entry in entries {
            if entry.get("unmap_function").and_then(Value::as_str)
                == Some("_map_math2eng_transformer!")
                && let Some(ids) = entry.get("to").and_then(Value::as_array)
                && ids
                    .iter()
                    .filter(|id| id.as_str().is_some_and(|s| s.starts_with("transformer.")))
                    .count()
                    > MAX_MATRIX_DIM
            {
                return Err(malformed(format!(
                    "transformer map has more than {MAX_MATRIX_DIM} winding units"
                )));
            }
        }
    }
    Ok(())
}

fn validate_phase_vectors(doc: &Map<String, Value>) -> Result<()> {
    for table in [
        "bus",
        "branch",
        "switch",
        "transformer",
        "gen",
        "load",
        "shunt",
    ] {
        let Some(objects) = doc.get(table).and_then(Value::as_object) else {
            continue;
        };
        for (id, object) in objects {
            for key in [
                "terminals",
                "connections",
                "f_connections",
                "t_connections",
                "grounded",
                "pd",
                "qd",
                "pg",
                "qg",
                "pmin",
                "pmax",
                "qmin",
                "qmax",
                "vm",
                "va",
                "vmin",
                "vmax",
                "vg",
                "tm_set",
                "tm_fix",
                "tm_lb",
                "tm_ub",
                "tm_step",
            ] {
                if object
                    .get(key)
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.len() > MAX_MATRIX_DIM)
                {
                    return Err(Error::Json {
                        format: FMT,
                        message: format!(
                            "{table}.{id}.{key}: phase vector exceeds {MAX_MATRIX_DIM} entries"
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

/// A MATHEMATICAL object: its table, index key, and fields.
#[derive(Clone)]
struct Obj<'a> {
    table: &'static str,
    key: String,
    fields: &'a Map<String, Value>,
}

impl Obj<'_> {
    fn id(&self) -> String {
        format!("{}.{}", self.table, self.key)
    }

    fn str(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::as_str)
    }

    fn name(&self) -> String {
        self.str("name")
            .map_or_else(|| self.key.clone(), str::to_owned)
    }

    fn f(&self, key: &str) -> Option<f64> {
        self.fields
            .get(key)
            .map(|v| restore(key, v))
            .filter(|v| !v.is_nan())
    }

    fn int(&self, key: &str) -> Option<i64> {
        self.fields.get(key).and_then(integer)
    }

    fn index(&self, key: &str) -> Option<String> {
        self.int(key).map(|i| i.to_string())
    }

    fn floats(&self, key: &str) -> Vec<f64> {
        self.fields
            .get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|v| restore(key, v)).collect())
            .unwrap_or_default()
    }

    fn connections(&self, key: &str) -> Vec<String> {
        self.fields
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| {
                        v.as_i64().map_or_else(
                            || v.as_str().unwrap_or_default().to_owned(),
                            |i| i.to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn bools(&self, key: &str) -> Vec<bool> {
        self.fields
            .get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|v| v.as_bool().unwrap_or(false)).collect())
            .unwrap_or_default()
    }

    /// A matrix as PMD serializes it: an array of columns.
    fn matrix(&self, key: &str) -> Option<ConductorMatrix> {
        let value = self.fields.get(key)?;
        let n = matrix_dimension(value)?;
        let cols = value.as_array()?;
        let mut m = vec![vec![0.0; n]; n];
        for (j, col) in cols.iter().enumerate() {
            for (i, x) in col.as_array()?.iter().enumerate().take(n) {
                m[i][j] = restore(key, x);
            }
        }
        Some(m)
    }
}

/// One `map` entry: the ENGINEERING object and the MATHEMATICAL ids it
/// became.
#[derive(Clone, Debug)]
struct MapEntry {
    from: String,
    to: Vec<String>,
    unmap: String,
}

struct Reader<'a> {
    doc: &'a Map<String, Value>,
    net: MulticonductorNetwork,
    diagnostics: crate::diagnostics::Diagnostics,
    /// System power base in `power_scale_factor` units (kVA).
    sbase: f64,
    voltage_scale_factor: f64,
    power_scale_factor: f64,
    per_unit: bool,
    /// Bus index key to its network bus id.
    bus_name: BTreeMap<String, String>,
    /// Bus index key to its voltage base, kV.
    vbase: BTreeMap<String, f64>,
    /// MATHEMATICAL ids a map group consumed.
    consumed: BTreeSet<String>,
    map: Vec<MapEntry>,
}

fn matrix_value(m: &ConductorMatrix) -> Value {
    let n = m.len();
    Value::Array(
        (0..n)
            .map(|j| {
                Value::Array(
                    (0..n)
                        .map(|i| json!(m[i].get(j).copied().unwrap_or(0.0)))
                        .collect(),
                )
            })
            .collect(),
    )
}

fn scaled(m: &ConductorMatrix, k: f64) -> ConductorMatrix {
    m.iter()
        .map(|row| row.iter().map(|v| v * k).collect())
        .collect()
}

/// Division by the base, the exact inverse of the writer's multiplication,
/// so a value written, read, and written again does not drift.
fn divided(m: &ConductorMatrix, k: f64) -> ConductorMatrix {
    m.iter()
        .map(|row| row.iter().map(|v| v / k).collect())
        .collect()
}

/// `uniform` when every entry equals the first, bit for bit.
fn uniform(values: &[f64]) -> bool {
    values.windows(2).all(|w| w[0].to_bits() == w[1].to_bits())
}

impl<'a> Reader<'a> {
    fn new(doc: &'a Map<String, Value>) -> Self {
        let settings = doc.get("settings").and_then(Value::as_object);
        let number = |key: &str, default: f64| {
            settings
                .and_then(|s| s.get(key))
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(default)
        };
        let per_unit = doc.get("per_unit").and_then(Value::as_bool).unwrap_or(true);
        let base_frequency = number("base_frequency", crate::dss::defaults::BASE_FREQUENCY);
        let mut net = MulticonductorNetwork::from_tables(MulticonductorNetworkTables {
            source_format: Some(DistSourceFormat::PmdMathJson),
            base_frequency,
            ..MulticonductorNetworkTables::default()
        });
        if let Some(name) = doc
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty())
        {
            *net.name_mut() = Some(name.to_owned());
        }
        let map = doc
            .get("map")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let item = item.as_object()?;
                        let unmap = item.get("unmap_function")?.as_str()?.to_owned();
                        let from = item
                            .get("from")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let to = match item.get("to") {
                            Some(Value::String(s)) => vec![s.clone()],
                            Some(Value::Array(a)) => a
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect(),
                            _ => Vec::new(),
                        };
                        Some(MapEntry { from, to, unmap })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            doc,
            net,
            diagnostics: crate::diagnostics::Diagnostics::new(),
            sbase: number("sbase", number("sbase_default", 1.0)),
            voltage_scale_factor: number("voltage_scale_factor", 1000.0),
            power_scale_factor: number("power_scale_factor", 1000.0),
            per_unit,
            bus_name: BTreeMap::new(),
            vbase: BTreeMap::new(),
            consumed: BTreeSet::new(),
            map,
        }
    }

    fn warn(&mut self, info: &'static crate::diagnostics::DiagnosticInfo, msg: impl Into<String>) {
        self.diagnostics.push(info, msg);
    }

    fn table(&self, table: &'static str) -> Vec<Obj<'a>> {
        let mut objs: Vec<Obj<'a>> = self
            .doc
            .get(table)
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|t| t.iter())
            .filter_map(|(key, v)| {
                v.as_object().map(|fields| Obj {
                    table,
                    key: key.clone(),
                    fields,
                })
            })
            .collect();
        objs.sort_by_key(|o| o.key.parse::<u64>().unwrap_or(u64::MAX));
        objs
    }

    fn object(&self, id: &str) -> Option<Obj<'a>> {
        let (table, key) = id.split_once('.')?;
        let table: &'static str = match table {
            "bus" => "bus",
            "branch" => "branch",
            "switch" => "switch",
            "transformer" => "transformer",
            "gen" => "gen",
            "load" => "load",
            "shunt" => "shunt",
            "storage" => "storage",
            _ => return None,
        };
        let fields = self.doc.get(table)?.as_object()?.get(key)?.as_object()?;
        Some(Obj {
            table,
            key: key.to_owned(),
            fields,
        })
    }

    // ----- bases ------------------------------------------------------------------

    /// Volts of a per unit magnitude at a bus.
    fn volts(&self, bus_key: &str, pu: f64) -> f64 {
        if !self.per_unit {
            return pu * self.voltage_scale_factor;
        }
        pu * self.vbase.get(bus_key).copied().unwrap_or(1.0) * self.voltage_scale_factor
    }

    /// Watts of a per unit power.
    fn watts(&self, pu: f64) -> f64 {
        (if self.per_unit { pu * self.sbase } else { pu }) * self.power_scale_factor
    }

    /// The impedance base at a bus, ohms.
    fn zbase(&self, bus_key: &str) -> f64 {
        if !self.per_unit {
            return 1.0;
        }
        let vbase = self.vbase.get(bus_key).copied().unwrap_or(1.0);
        vbase * vbase / self.sbase * self.voltage_scale_factor
    }

    /// Amperes of a per unit current at a bus.
    fn amps(&self, bus_key: &str, pu: f64) -> f64 {
        if !self.per_unit {
            return pu;
        }
        pu * self.sbase / self.vbase.get(bus_key).copied().unwrap_or(1.0)
    }

    /// Give a bus the neutral terminal a Kron reduced document eliminated,
    /// grounded, since the reduction removes a perfectly grounded neutral.
    fn restore_neutral(&mut self, bus: &str) {
        let kron = self
            .doc
            .get("is_kron_reduced")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(b) = self.net.buses_mut().iter_mut().find(|b| b.id == bus) {
            if !b.terminals.iter().any(|t| t == "4") {
                b.terminals.push("4".to_owned());
            }
            if kron && !b.grounded.iter().any(|t| t == "4") {
                b.grounded.push("4".to_owned());
            }
        }
        self.net.extras_mut().insert(
            "bmopf_terminal_conventions".into(),
            json!({"neutral": ["4"]}),
        );
    }

    fn bus_id(&mut self, key: &str, what: &str) -> String {
        if let Some(name) = self.bus_name.get(key) {
            return name.clone();
        }
        self.warn(
            &C::READ_PMD_MATH_REFERENCE_DROPPED,
            format!("{what}: bus {key} is not in the document; the index is kept as the bus id"),
        );
        key.to_owned()
    }

    // ----- run --------------------------------------------------------------------

    fn run(&mut self) {
        self.extras();
        self.neutral_roles();
        self.buses();
        let map = std::mem::take(&mut self.map);
        for entry in &map {
            match entry.unmap.as_str() {
                "_map_math2eng_root!" | "_map_math2eng_bus!" => {}
                "_map_math2eng_line!" => self.map_line(entry),
                "_map_math2eng_transformer!" => self.map_transformer(entry),
                "_map_math2eng_switch!" => self.map_switch(entry),
                "_map_math2eng_shunt!" => self.map_shunt(entry),
                "_map_math2eng_load!" => self.map_load(entry),
                "_map_math2eng_generator!" => self.map_generator(entry),
                "_map_math2eng_solar!" => self.map_solar(entry),
                "_map_math2eng_voltage_source!" => self.map_voltage_source(entry),
                "_map_math2eng_storage!" => self.map_storage(entry),
                other => {
                    self.warn(
                        &C::READ_PMD_MATH_RETAINED_SOURCE_ONLY,
                        format!(
                            "map entry `{}` uses `{other}`, which this reader does not know; its objects are kept untyped",
                            entry.from
                        ),
                    );
                    for id in &entry.to {
                        self.keep_untyped(id, "mapped by an unknown function");
                    }
                }
            }
        }
        self.map = map;
        self.unmapped();
    }

    /// Top level values and the settings, whose `vbases_default` the lowering
    /// keyed by bus index; the ENGINEERING writers want bus names.
    fn extras(&mut self) {
        if let Some(Value::Object(settings)) = self.doc.get("settings") {
            let mut settings = settings.clone();
            if let Some(Value::Object(bases)) = settings.get("vbases_default").cloned() {
                let names: Map<String, Value> = bases
                    .iter()
                    .map(|(key, kv)| {
                        let name = self
                            .doc
                            .get("bus")
                            .and_then(|b| b.get(key))
                            .and_then(|b| b.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or(key)
                            .to_owned();
                        (name, kv.clone())
                    })
                    .collect();
                settings.insert("vbases_default".into(), Value::Object(names));
            }
            self.net
                .extras_mut()
                .insert("pmd_settings".into(), Value::Object(settings));
        }
        let mut top = Map::new();
        for (key, value) in self.doc {
            if matches!(
                key.as_str(),
                "bus"
                    | "branch"
                    | "switch"
                    | "transformer"
                    | "gen"
                    | "load"
                    | "shunt"
                    | "storage"
                    | "map"
                    | "settings"
                    | "name"
                    | "data_model"
                    | "bus_lookup"
            ) {
                continue;
            }
            top.insert(key.clone(), value.clone());
        }
        if !top.is_empty() {
            self.net
                .extras_mut()
                .insert("pmd_math_top_level".into(), Value::Object(top));
        }
    }

    // ----- buses ------------------------------------------------------------------

    fn neutral_roles(&mut self) {
        // PMD's grounded terminal 4 is a neutral even on a single-phase
        // bus such as [3,4]. DistBus's fallback only recognizes the complete
        // four-wire [1,2,3,4] spelling, so retain the explicit role as well.
        if self.table("bus").iter().any(|bus| {
            bus.connections("terminals")
                .iter()
                .zip(bus.bools("grounded"))
                .any(|(terminal, grounded)| terminal == "4" && grounded)
        }) {
            self.net.extras_mut().insert(
                "bmopf_terminal_conventions".into(),
                json!({"neutral": ["4"]}),
            );
        }
    }

    fn buses(&mut self) {
        // The virtual buses of transformers, switches, and sources are read
        // by their groups; the rest are the network's buses.
        let virtual_ids: BTreeSet<String> = self
            .map
            .iter()
            .filter(|e| {
                matches!(
                    e.unmap.as_str(),
                    "_map_math2eng_transformer!"
                        | "_map_math2eng_switch!"
                        | "_map_math2eng_voltage_source!"
                )
            })
            .flat_map(|e| e.to.iter().filter(|id| id.starts_with("bus.")).cloned())
            .collect();
        let names: BTreeMap<String, String> = self
            .map
            .iter()
            .filter(|e| e.unmap == "_map_math2eng_bus!")
            .filter_map(|e| {
                Some((
                    e.to.first()?.strip_prefix("bus.")?.to_owned(),
                    e.from.clone(),
                ))
            })
            .collect();
        for bus in self.table("bus") {
            if let Some(vbase) = bus.f("vbase") {
                self.vbase.insert(bus.key.clone(), vbase);
            } else if self.per_unit {
                self.warn(
                    &C::READ_PMD_MATH_VALUE_DEFAULTED,
                    format!("bus {}: no `vbase`; a base of 1 kV is assumed", bus.key),
                );
            }
            if virtual_ids.contains(&bus.id()) {
                continue;
            }
            let name = names.get(&bus.key).cloned().unwrap_or_else(|| bus.name());
            self.bus_name.insert(bus.key.clone(), name.clone());
            let terminals = bus.connections("terminals");
            let grounded_flags = bus.bools("grounded");
            let grounded: Vec<String> = terminals
                .iter()
                .zip(grounded_flags.iter().chain(std::iter::repeat(&false)))
                .filter(|(_, g)| **g)
                .map(|(t, _)| t.clone())
                .collect();
            let mut extras = Extras::new();
            if bus.int("bus_type") == Some(4) {
                extras.insert("pmd_status".into(), json!("DISABLED"));
            }
            // The document's voltage base is PMD's `calc_voltage_bases`
            // result at full precision. Kept, the MATHEMATICAL writer seeds
            // its flood from it, so a document reads and writes back to the
            // same bases instead of ones recomputed through a winding ratio
            // that per unit recovery rounded.
            if let Some(vbase) = bus.f("vbase") {
                extras.insert("pmd_vbase".into(), json!(vbase));
            }
            for key in ["vm_pair_lb", "vm_pair_ub"] {
                if bus
                    .fields
                    .get(key)
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty())
                {
                    extras.insert(format!("pmd_{key}"), bus.fields[key].clone());
                    self.warn(
                        &C::READ_PMD_MATH_RETAINED_SOURCE_ONLY,
                        format!(
                            "bus {name}: pairwise voltage bounds are not typed; kept in extras"
                        ),
                    );
                }
            }
            let mut out = DistBus {
                id: name.clone(),
                terminals: terminals.clone(),
                grounded,
                extras,
                ..DistBus::default()
            };
            let phases = out.phase_indices(self.net.extras().get("bmopf_terminal_conventions"));
            let bound = |key: &str, open: f64| -> (Option<f64>, Option<Vec<f64>>) {
                let values = bus.floats(key);
                let phase_values: Vec<f64> = phases
                    .iter()
                    .filter_map(|&i| values.get(i).copied())
                    .map(|v| self.volts(&bus.key, v))
                    .collect();
                // Open entries belong to individual conductors. Removing one
                // must not discard the finite constraints on the other phases.
                if phase_values.len() != phases.len()
                    || phase_values.is_empty()
                    || phase_values.iter().all(|v| v.to_bits() == open.to_bits())
                {
                    return (None, None);
                }
                if uniform(&phase_values) {
                    (Some(phase_values[0]), None)
                } else {
                    (None, Some(phase_values))
                }
            };
            (out.v_min, out.v_min_phase) = bound("vmin", 0.0);
            (out.v_max, out.v_max_phase) = bound("vmax", f64::INFINITY);
            self.net.buses_mut().push(out);
        }
    }

    // ----- lines ------------------------------------------------------------------

    fn map_line(&mut self, entry: &MapEntry) {
        let Some(id) = entry.to.first() else { return };
        let Some(branch) = self.object(id) else {
            self.warn(
                &C::READ_PMD_MATH_REFERENCE_DROPPED,
                format!("line {}: `{id}` is not in the document", entry.from),
            );
            return;
        };
        self.consumed.insert(id.clone());
        let name = if entry.from.is_empty() {
            branch.name()
        } else {
            entry.from.clone()
        };
        if let Some(line) = self.line_from_branch(&branch, &name, &format!("line.{name}")) {
            self.net.lines_mut().push(line);
        }
    }

    /// A branch as a line of one meter with an inline linecode carrying the
    /// total impedance, the shape the ENGINEERING reader gives an inline
    /// impedance.
    fn line_from_branch(&mut self, branch: &Obj<'_>, name: &str, what: &str) -> Option<DistLine> {
        let f_key = branch.index("f_bus")?;
        let t_key = branch.index("t_bus")?;
        let bus_from = self.bus_id(&f_key, what);
        let bus_to = self.bus_id(&t_key, what);
        let zbase = self.zbase(&f_key);
        let r = scaled(&branch.matrix("br_r").unwrap_or_default(), zbase);
        let x = scaled(&branch.matrix("br_x").unwrap_or_default(), zbase);
        let n = r.len().max(x.len());
        let admittance = |key: &str| {
            divided(
                &branch.matrix(key).unwrap_or_else(|| vec![vec![0.0; n]; n]),
                zbase,
            )
        };
        let mut code = DistLineCode::new(format!("{name}_z"), r, x);
        code.n_conductors = n;
        code.g_from = admittance("g_fr");
        code.g_to = admittance("g_to");
        code.b_from = admittance("b_fr");
        code.b_to = admittance("b_to");
        let current = branch.floats("c_rating_a");
        if current.iter().any(|v| v.is_finite()) {
            code.i_max = Some(current.iter().map(|v| self.amps(&f_key, *v)).collect());
        }
        let power = branch.floats("rate_a");
        if power.iter().any(|v| v.is_finite()) {
            code.s_max = Some(power.iter().map(|v| self.watts(*v)).collect());
        }
        self.net.line_codes_mut().push(code);
        let mut extras = Extras::new();
        extras.insert("pmd_inline".into(), json!(true));
        if branch.int("br_status") == Some(0) {
            extras.insert("pmd_status".into(), json!("DISABLED"));
        }
        for key in ["angmin", "angmax"] {
            let values = branch.floats(key);
            if !values.is_empty()
                && values.iter().any(|v| {
                    (v.abs()
                        - if self.per_unit {
                            60f64.to_radians()
                        } else {
                            60.0
                        })
                    .abs()
                        > 1e-9
                })
            {
                extras.insert(
                    format!("pmd_{key}"),
                    json!(
                        values
                            .iter()
                            .map(|v| if self.per_unit { v.to_degrees() } else { *v })
                            .collect::<Vec<_>>()
                    ),
                );
            }
        }
        self.warn(
            &C::READ_PMD_MATH_VALUE_DEFAULTED,
            format!("{what}: the MATHEMATICAL branch states a total impedance and no length; read as 1 m"),
        );
        let mut line = DistLine::new(
            name,
            bus_from,
            bus_to,
            branch.connections("f_connections"),
            branch.connections("t_connections"),
            format!("{name}_z"),
            1.0,
        );
        line.extras = extras;
        Some(line)
    }

    // ----- transformers -----------------------------------------------------------

    #[allow(clippy::too_many_lines)] // one transformer folds back in one pass: windings, loss model, tests
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)] // PMD's loss model inverted in one pass, in its own names
    fn map_transformer(&mut self, entry: &MapEntry) {
        let what = format!("transformer {}", entry.from);
        let mut units: Vec<Obj<'a>> = Vec::new();
        let mut branches: Vec<Obj<'a>> = Vec::new();
        let mut virtual_buses: BTreeSet<String> = BTreeSet::new();
        for id in &entry.to {
            let Some(obj) = self.object(id) else {
                self.warn(
                    &C::READ_PMD_MATH_REFERENCE_DROPPED,
                    format!("{what}: `{id}` is not in the document"),
                );
                continue;
            };
            self.consumed.insert(id.clone());
            match obj.table {
                "transformer" => units.push(obj),
                "branch" => branches.push(obj),
                "bus" => {
                    virtual_buses.insert(obj.key.clone());
                }
                _ => {}
            }
        }
        // Integer indices can be renumbered without changing the circuit.
        // Winding identity is recorded in PMD's virtual source/name suffix.
        let unit_prefix = format!("_virtual_transformer.{}.", entry.from);
        let source_prefix = format!("_virtual_transformer.transformer.{}.", entry.from);
        units.sort_by_key(|u| {
            u.str("source_id")
                .and_then(|s| s.strip_prefix(&source_prefix))
                .or_else(|| u.str("name").and_then(|s| s.strip_prefix(&unit_prefix)))
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| u.int("index"))
                .unwrap_or(i64::MAX)
        });
        if units.is_empty() {
            self.warn(
                &C::READ_PMD_MATH_RECORD_DROPPED,
                format!("{what}: no ideal transformer unit in its map entry; dropped"),
            );
            return;
        }
        // The two-winding `f_bus`/`t_bus` form (an ENGINEERING asymmetric
        // transformer) has one unit and no loss model.
        let direct = units.len() == 1 && branches.is_empty() && virtual_buses.is_empty();
        let mut windings = Vec::new();
        let mut internal: Vec<String> = Vec::new();
        let mut vnom_kv: Vec<f64> = Vec::new();
        let mut phases = 1usize;
        let mut tm_set_rows = Vec::new();
        let mut tm_fix_rows = Vec::new();
        let mut tm_lb_rows = Vec::new();
        let mut tm_ub_rows = Vec::new();
        let mut tm_step_rows = Vec::new();
        let mut polarity = Vec::new();
        let mut taps_differ = false;
        let mut status_disabled = false;
        let mut sm_ub_power_units = None;
        let ends: Vec<(String, String)> = if direct {
            let u = &units[0];
            vec![
                (u.index("f_bus").unwrap_or_default(), "f_connections".into()),
                (u.index("t_bus").unwrap_or_default(), "t_connections".into()),
            ]
        } else {
            units
                .iter()
                .map(|u| (u.index("f_bus").unwrap_or_default(), "f_connections".into()))
                .collect()
        };
        for (w, (bus_key, connections_key)) in ends.iter().enumerate() {
            let unit = if direct { &units[0] } else { &units[w] };
            let conn = match unit.str("configuration") {
                _ if direct && w == 1 => DistWindingConn::Wye,
                Some("DELTA") => DistWindingConn::Delta,
                _ => DistWindingConn::Wye,
            };
            // tm_nom per unit is the nominal ratio on the two buses' bases;
            // the nominal kV is that ratio times the base ratio back.
            let f_vbase = unit
                .f("f_vbase")
                .or_else(|| self.vbase.get(&unit.index("f_bus")?).copied())
                .unwrap_or(1.0);
            let t_vbase = unit
                .f("t_vbase")
                .or_else(|| self.vbase.get(&unit.index("t_bus")?).copied())
                .unwrap_or(1.0);
            let tm_nom = unit.f("tm_nom").unwrap_or(1.0);
            let kv_ll = if self.per_unit {
                tm_nom * f_vbase / t_vbase
            } else {
                tm_nom
            };
            let to_kv = if self.per_unit { t_vbase } else { 1.0 };
            let ratio_kv = if direct { kv_ll * to_kv } else { kv_ll };
            let kv = if direct && w == 1 {
                // A direct unit states a ratio, not two winding voltages.
                // Choose the to bus base and recover the from voltage on it.
                to_kv
            } else if conn == DistWindingConn::Delta {
                ratio_kv / 3f64.sqrt()
            } else {
                ratio_kv
            };
            vnom_kv.push(kv);
            let mut map = unit.connections(connections_key);
            let bus = self.bus_id(bus_key, &what);
            if conn == DistWindingConn::Wye
                && self.doc.get("is_kron_reduced").and_then(Value::as_bool) == Some(true)
                && !map.iter().any(|t| t == "4")
            {
                // Native PMD removes the first (grounded) terminal of the
                // reversed center-tap half, retaining its -1 polarity. Put
                // the neutral back on that side so typed consumers preserve
                // the split-phase voltage instead of ignoring an extras flag.
                if w == 2
                    && ends.len() == 3
                    && ends[1].0 == *bus_key
                    && map.len() == 1
                    && unit.int("polarity") == Some(-1)
                {
                    map.insert(0, "4".to_owned());
                } else {
                    map.push("4".to_owned());
                }
                self.restore_neutral(&bus);
            }
            let hot = crate::model::n_winding_phase_count(conn, &map);
            phases = phases.max(hot);
            // The direct unit's tap and polarity act once, on its from
            // winding. Applying them to both synthesized windings cancels them.
            let identity_side = direct && w == 1;
            let tm_set = if identity_side {
                vec![1.0; hot]
            } else {
                unit.floats("tm_set")
            };
            taps_differ |= !uniform(&tm_set);
            let tap_field = |key: &str, identity: Value| {
                if identity_side {
                    identity
                } else {
                    unit.fields.get(key).cloned().unwrap_or(Value::Null)
                }
            };
            tm_set_rows.push(tap_field("tm_set", json!(tm_set)));
            tm_fix_rows.push(tap_field("tm_fix", json!(vec![true; hot])));
            tm_lb_rows.push(tap_field("tm_lb", json!(vec![1.0; hot])));
            tm_ub_rows.push(tap_field("tm_ub", json!(vec![1.0; hot])));
            tm_step_rows.push(tap_field("tm_step", json!(vec![0.0; hot])));
            polarity.push(if identity_side {
                1
            } else {
                unit.int("polarity").unwrap_or(1)
            });
            status_disabled |= unit.int("status") == Some(0);
            if let Some(sm_ub) = unit.f("sm_ub") {
                sm_ub_power_units = Some(if self.per_unit {
                    sm_ub * self.sbase
                } else {
                    sm_ub
                });
            }
            internal.push(unit.index("t_bus").unwrap_or_default());
            let mut winding = DistWinding::new(bus, map, conn, kv * self.voltage_scale_factor, 0.0);
            winding.tap = tm_set.first().copied().unwrap_or(1.0);
            windings.push(winding);
        }
        if taps_differ {
            self.warn(&C::READ_PMD_MATH_VALUE_COLLAPSED, format!("{what}: per phase taps differ; the winding tap keeps the first phase (full arrays in extras)"));
        }
        // The winding power rating is not in the MATHEMATICAL model. The
        // emergency rating stands in for it, and every impedance below is
        // restated on that base, so the ohms are exact.
        let s_rating = sm_ub_power_units
            .filter(|v| v.is_finite() && *v > 0.0)
            .map_or(self.sbase * self.power_scale_factor, |kva| {
                kva * self.power_scale_factor
            });
        if sm_ub_power_units.is_some_and(|v| v.is_finite() && v > 0.0) {
            self.warn(
                &C::READ_PMD_MATH_VALUE_SUBSTITUTED,
                format!("{what}: the MATHEMATICAL model states no winding power rating; the emergency rating sm_ub is used as the rating and the impedances are restated on it"),
            );
        } else {
            self.warn(
                &C::READ_PMD_MATH_VALUE_DEFAULTED,
                format!("{what}: no positive finite winding power rating is recoverable; the system power base is used only to restate winding impedances and the original capacity bound is retained"),
            );
        }
        for w in &mut windings {
            w.s_rating = s_rating;
        }
        let nrw = windings.len();
        let mut xsc_pct = vec![0.0; crate::model::pair_keys(nrw).len()];
        let mut extras = Extras::new();
        if !direct {
            // Invert the loss model: the resistance branch of each winding is
            // the one incident to its internal bus whose far end is not
            // another winding's internal bus; the rest form the star.
            let internal_set: BTreeSet<&String> = internal.iter().collect();
            let zbase_int = internal.first().map_or(1.0, |k| self.zbase(k));
            let ratios: Vec<f64> = vnom_kv.clone();
            let mut star: Vec<String> = Vec::new();
            let mut used: BTreeSet<String> = BTreeSet::new();
            let branch_prefix = format!("_virtual_branch.transformer.{}_", entry.from);
            let native_names = branches.iter().any(|b| {
                b.name().starts_with(&branch_prefix)
                    || b.str("source_id")
                        .is_some_and(|s| s.starts_with(&branch_prefix))
            });
            for (w, t_bus) in internal.iter().enumerate() {
                // A zero resistance is merged out by PMD. The remaining
                // incident edge may then be leakage, or another winding's
                // resistance. PMD's branch identity keeps the original edge
                // number (1..=nrw are resistances) across those merges.
                let resistance_name =
                    format!("_virtual_branch.transformer.{}_{}", entry.from, w + 1);
                let found = branches.iter().find(|b| {
                    !used.contains(&b.key)
                        && if native_names {
                            b.name() == resistance_name
                                || b.str("source_id") == Some(resistance_name.as_str())
                        } else {
                            b.matrix("br_x")
                                .is_none_or(|m| m.iter().flatten().all(|v| *v == 0.0))
                                && b.matrix("br_r")
                                    .is_some_and(|m| m.iter().flatten().any(|v| *v != 0.0))
                        }
                        && (b.index("f_bus").as_ref() == Some(t_bus)
                            || b.index("t_bus").as_ref() == Some(t_bus))
                        && (native_names || {
                            let other = if b.index("f_bus").as_ref() == Some(t_bus) {
                                b.index("t_bus")
                            } else {
                                b.index("f_bus")
                            };
                            other.as_ref().is_none_or(|o| !internal_set.contains(o))
                        })
                });
                if let Some(b) = found {
                    used.insert(b.key.clone());
                    let r_int = b
                        .matrix("br_r")
                        .and_then(|m| m.first().and_then(|r| r.first().copied()))
                        .unwrap_or(0.0)
                        * zbase_int;
                    let other = if b.index("f_bus").as_ref() == Some(t_bus) {
                        b.index("t_bus")
                    } else {
                        b.index("f_bus")
                    };
                    star.push(other.unwrap_or_else(|| t_bus.clone()));
                    let z_w =
                        crate::model::n_winding_impedance_base(1, windings[w].v_ref, s_rating);
                    if let Some(z_w) = z_w {
                        windings[w].r_pct = r_int * ratios[w] * ratios[w] / z_w * 100.0;
                    }
                } else {
                    star.push(t_bus.clone());
                }
            }
            // Star branches between the star buses give the pairwise short
            // circuit impedances through the nodal admittance matrix.
            let mut zbr: BTreeMap<(usize, usize), Complex64> = BTreeMap::new();
            let mut y_sh = Complex64::default();
            for b in &branches {
                let (Some(f), Some(t)) = (b.index("f_bus"), b.index("t_bus")) else {
                    continue;
                };
                let g = b.matrix("g_fr").map_or(0.0, |m| m[0][0])
                    + b.matrix("g_to").map_or(0.0, |m| m[0][0]);
                let bb = b.matrix("b_fr").map_or(0.0, |m| m[0][0])
                    + b.matrix("b_to").map_or(0.0, |m| m[0][0]);
                y_sh += Complex64::new(g, bb) / zbase_int;
                if used.contains(&b.key) {
                    continue;
                }
                let (Some(i), Some(j)) = (
                    star.iter().position(|s| *s == f),
                    star.iter().position(|s| *s == t),
                ) else {
                    continue;
                };
                let r = b.matrix("br_r").map_or(0.0, |m| m[0][0]) * zbase_int;
                let x = b.matrix("br_x").map_or(0.0, |m| m[0][0]) * zbase_int;
                zbr.insert((i.min(j), j.max(i)), Complex64::new(r, x));
            }
            let zsc = br2sc_impedance(&zbr, nrw);
            if let Some(z1) = crate::model::n_winding_impedance_base(1, windings[0].v_ref, s_rating)
            {
                for (k, pair) in crate::model::pair_keys(nrw).iter().enumerate() {
                    if let Some(z) = zsc.get(pair) {
                        xsc_pct[k] = z.im * ratios[0] * ratios[0] / z1 * 100.0;
                    }
                }
            }
            if y_sh != Complex64::default() && s_rating > 0.0 {
                let v1 = windings[0].v_ref;
                let g_sh = y_sh.re / (ratios[0] * ratios[0]);
                let b_sh = y_sh.im / (ratios[0] * ratios[0]);
                let noloadloss = g_sh * v1 * v1 / s_rating;
                let cmag = -b_sh * v1 * v1 / s_rating;
                if noloadloss.abs() > 0.0 {
                    extras.insert("%noloadloss".into(), json!(noloadloss * 100.0));
                }
                if cmag.abs() > 0.0 {
                    extras.insert("%imag".into(), json!(cmag * 100.0));
                }
            }
        }
        for (key, rows) in [
            ("tm_set", tm_set_rows),
            ("tm_fix", tm_fix_rows),
            ("tm_lb", tm_lb_rows),
            ("tm_ub", tm_ub_rows),
            ("tm_step", tm_step_rows),
        ] {
            if rows.iter().any(|r| !r.is_null()) {
                extras.insert(format!("pmd_{key}"), Value::Array(rows));
            }
        }
        if polarity != crate::pmd::write::lag_polarity(&windings) {
            extras.insert("pmd_polarity".into(), json!(polarity));
        }
        // An absent or null sm_ub is unbounded, independently of the
        // positive computational rating used above. Both PMD writers read
        // the existing DSS-style numeric-string emergency-rating stash.
        // This existing cross-format stash is always kVA, independent of
        // the MATHEMATICAL document's configurable power scale factor.
        let kva = sm_ub_power_units.unwrap_or(f64::INFINITY) * (self.power_scale_factor / 1000.0);
        extras.insert(
            "emerghkva".into(),
            if kva.is_finite() {
                json!(kva)
            } else {
                json!("Inf")
            },
        );
        if status_disabled {
            extras.insert("pmd_status".into(), json!("DISABLED"));
        }
        let mut xf = DistTransformer::new(entry.from.clone(), windings, xsc_pct, phases.max(1));
        xf.extras = extras;
        self.net.transformers_mut().push(xf);
    }

    // ----- switches ---------------------------------------------------------------

    fn map_switch(&mut self, entry: &MapEntry) {
        let what = format!("switch {}", entry.from);
        let mut switch = None;
        let mut branch = None;
        for id in &entry.to {
            let Some(obj) = self.object(id) else {
                self.warn(
                    &C::READ_PMD_MATH_REFERENCE_DROPPED,
                    format!("{what}: `{id}` is not in the document"),
                );
                continue;
            };
            self.consumed.insert(id.clone());
            match obj.table {
                "switch" => switch = Some(obj),
                "branch" => branch = Some(obj),
                _ => {}
            }
        }
        let Some(s) = switch else {
            self.warn(
                &C::READ_PMD_MATH_RECORD_DROPPED,
                format!("{what}: no switch object in its map entry; dropped"),
            );
            return;
        };
        let f_key = s.index("f_bus").unwrap_or_default();
        // With a loss branch the switch's to bus is the virtual bus; the
        // real one is the branch's far end.
        let t_key = branch
            .as_ref()
            .and_then(|b| b.index("t_bus"))
            .or_else(|| s.index("t_bus"))
            .unwrap_or_default();
        let bus_from = self.bus_id(&f_key, &what);
        let bus_to = self.bus_id(&t_key, &what);
        let mut extras = Extras::new();
        if s.int("status") == Some(0) {
            extras.insert("pmd_status".into(), json!("DISABLED"));
        }
        if let Some(b) = &branch {
            let zbase = self.zbase(&f_key);
            for (key, out) in [("br_r", "pmd_rs"), ("br_x", "pmd_xs")] {
                if let Some(m) = b.matrix(key) {
                    extras.insert(out.into(), matrix_value(&scaled(&m, zbase)));
                }
            }
            // Siemens back to the ENGINEERING nanofarads.
            let to_nf = 1.0 / (2.0 * std::f64::consts::PI * self.net.base_frequency() * 1e-9);
            for (key, out) in [("b_fr", "pmd_b_fr"), ("b_to", "pmd_b_to")] {
                if let Some(m) = b.matrix(key) {
                    extras.insert(
                        out.into(),
                        matrix_value(&scaled(&divided(&m, zbase), to_nf)),
                    );
                }
            }
            for (key, out) in [("g_fr", "pmd_g_fr"), ("g_to", "pmd_g_to")] {
                if let Some(m) = b.matrix(key) {
                    // PMD applies the same frequency conversion to all four
                    // ENGINEERING line/switch admittance fields, including g.
                    extras.insert(
                        out.into(),
                        matrix_value(&scaled(&divided(&m, zbase), to_nf)),
                    );
                }
            }
            extras.insert("pmd_length".into(), json!(1.0));
        }
        let mut out = DistSwitch::new(
            entry.from.clone(),
            bus_from,
            bus_to,
            s.connections("f_connections"),
            s.connections("t_connections"),
            s.int("state") == Some(0),
        );
        let current = s.floats("current_rating");
        if current.iter().any(|v| v.is_finite()) {
            out.i_max = Some(current.iter().map(|v| self.amps(&f_key, *v)).collect());
        }
        out.extras = extras;
        self.net.switches_mut().push(out);
    }

    // ----- shunts -----------------------------------------------------------------

    fn map_shunt(&mut self, entry: &MapEntry) {
        let what = format!("shunt {}", entry.from);
        let Some(id) = entry.to.first() else { return };
        let Some(shunt) = self.object(id) else {
            self.warn(
                &C::READ_PMD_MATH_REFERENCE_DROPPED,
                format!("{what}: `{id}` is not in the document"),
            );
            return;
        };
        self.consumed.insert(id.clone());
        let bus_key = shunt.index("shunt_bus").unwrap_or_default();
        let bus = self.bus_id(&bus_key, &what);
        let zbase = self.zbase(&bus_key);
        let g = divided(&shunt.matrix("gs").unwrap_or_default(), zbase);
        let b = divided(&shunt.matrix("bs").unwrap_or_default(), zbase);
        let mut out = DistShunt::new(
            entry.from.clone(),
            bus,
            shunt.connections("connections"),
            g,
            b,
        );
        if shunt.int("status") == Some(0) {
            out.extras.insert("pmd_status".into(), json!("DISABLED"));
        }
        self.net.shunts_mut().push(out);
    }

    // ----- loads ------------------------------------------------------------------

    #[allow(clippy::too_many_lines)] // one load folds in one pass: its parts, model, and bounds
    fn map_load(&mut self, entry: &MapEntry) {
        let what = format!("load {}", entry.from);
        let mut parts: Vec<Obj<'a>> = Vec::new();
        for id in &entry.to {
            let Some(obj) = self.object(id) else {
                self.warn(
                    &C::READ_PMD_MATH_REFERENCE_DROPPED,
                    format!("{what}: `{id}` is not in the document"),
                );
                continue;
            };
            self.consumed.insert(id.clone());
            parts.push(obj);
        }
        let Some(first) = parts.first().cloned() else {
            return;
        };
        let bus_key = first.index("load_bus").unwrap_or_default();
        let bus = self.bus_id(&bus_key, &what);
        let mut connections = first.connections("connections");
        let configuration = match first.str("configuration") {
            Some("DELTA") if connections.len() > 2 => Configuration::Delta,
            _ if connections.len() <= 2 => Configuration::SinglePhase,
            Some("DELTA") => Configuration::Delta,
            _ => Configuration::Wye,
        };
        // A wye load lists its neutral after its phases; one that lists as
        // many connections as powers (a Kron reduced document) returns on
        // the neutral the reduction eliminated, restored at the bus.
        let pd_len = first.floats("pd").len().max(1);
        if first.str("configuration") != Some("DELTA") && connections.len() == pd_len {
            connections.push("4".to_owned());
            self.restore_neutral(&bus);
        }
        let n = connections.len().max(1);
        let vnom_kv = first
            .f("vnom_kv")
            .map_or(0.0, |v| self.volts(&bus_key, v) / self.voltage_scale_factor);
        let v_nom =
            vec![vnom_kv * self.voltage_scale_factor; n.min(first.floats("pd").len().max(1))];
        let sum = |key: &str| -> Vec<f64> {
            let values: Vec<Vec<f64>> = parts.iter().map(|p| p.floats(key)).collect();
            let len = values.iter().map(Vec::len).max().unwrap_or(0);
            (0..len)
                .map(|k| {
                    values
                        .iter()
                        .map(|p| p.get(k).copied().unwrap_or(0.0))
                        .sum::<f64>()
                })
                .map(|pu| self.watts(pu))
                .collect()
        };
        let p_nom = sum("pd");
        let q_nom = sum("qd");
        let voltage_model = if parts.len() == 3 {
            let part = |model: &str, key: &str, total: &[f64]| -> Vec<f64> {
                let values = parts
                    .iter()
                    .find(|p| p.str("model") == Some(model))
                    .map(|p| p.floats(key))
                    .unwrap_or_default();
                total
                    .iter()
                    .enumerate()
                    .map(|(k, t)| {
                        if *t == 0.0 {
                            0.0
                        } else {
                            self.watts(values.get(k).copied().unwrap_or(0.0)) / t
                        }
                    })
                    .collect()
            };
            DistLoadVoltageModel::Zip {
                v_nom,
                alpha_z: part(super::LOAD_MODEL_IMPEDANCE, "pd", &p_nom),
                alpha_i: part(super::LOAD_MODEL_CURRENT, "pd", &p_nom),
                alpha_p: part(super::LOAD_MODEL_POWER, "pd", &p_nom),
                beta_z: part(super::LOAD_MODEL_IMPEDANCE, "qd", &q_nom),
                beta_i: part(super::LOAD_MODEL_CURRENT, "qd", &q_nom),
                beta_p: part(super::LOAD_MODEL_POWER, "qd", &q_nom),
            }
        } else {
            match first.str("model") {
                Some(super::LOAD_MODEL_IMPEDANCE) => {
                    DistLoadVoltageModel::ConstantImpedance { v_nom }
                }
                Some(super::LOAD_MODEL_CURRENT) => DistLoadVoltageModel::ConstantCurrent { v_nom },
                _ => DistLoadVoltageModel::ConstantPower { v_nom },
            }
        };
        let mut extras = Extras::new();
        extras.insert("kv".into(), json!(vnom_kv));
        match &voltage_model {
            DistLoadVoltageModel::ConstantImpedance { .. } => {
                extras.insert("model".into(), json!(2));
            }
            DistLoadVoltageModel::ConstantCurrent { .. } => {
                extras.insert("model".into(), json!(5));
            }
            DistLoadVoltageModel::Zip { .. } => {
                extras.insert("model".into(), json!(8));
            }
            _ => {}
        }
        if first.int("status") == Some(0) {
            extras.insert("pmd_status".into(), json!("DISABLED"));
        }
        let mut load = DistLoad::new(
            entry.from.clone(),
            bus,
            connections,
            configuration,
            p_nom,
            q_nom,
        );
        load.voltage_model = voltage_model;
        load.extras = extras;
        self.net.loads_mut().push(load);
    }

    // ----- generators ----------------------------------------------------------------

    fn gen_bounds(&self, g: &Obj<'_>) -> GenBounds {
        let bound = |key: &str| -> Option<Vec<f64>> {
            let values = g.floats(key);
            values
                .iter()
                .any(|v| v.is_finite())
                .then(|| values.iter().map(|v| self.watts(*v)).collect())
        };
        (bound("pmin"), bound("pmax"), bound("qmin"), bound("qmax"))
    }

    fn map_generator(&mut self, entry: &MapEntry) {
        let what = format!("generator {}", entry.from);
        let Some(id) = entry.to.first() else { return };
        let Some(g) = self.object(id) else {
            self.warn(
                &C::READ_PMD_MATH_REFERENCE_DROPPED,
                format!("{what}: `{id}` is not in the document"),
            );
            return;
        };
        self.consumed.insert(id.clone());
        let bus_key = g.index("gen_bus").unwrap_or_default();
        let bus = self.bus_id(&bus_key, &what);
        let (p_min, p_max, q_min, q_max) = self.gen_bounds(&g);
        let mut out = DistGenerator::new(
            entry.from.clone(),
            bus,
            g.connections("connections"),
            match g.str("configuration") {
                Some("DELTA") => Configuration::Delta,
                _ => Configuration::Wye,
            },
            g.floats("pg").iter().map(|v| self.watts(*v)).collect(),
            g.floats("qg").iter().map(|v| self.watts(*v)).collect(),
        );
        out.p_min = p_min;
        out.p_max = p_max;
        out.q_min = q_min;
        out.q_max = q_max;
        out.cost = self.gen_cost(&g);
        if g.int("gen_status") == Some(0) {
            out.extras.insert("pmd_status".into(), json!("DISABLED"));
        }
        self.net.generators_mut().push(out);
    }

    /// The linear cost in $/kWh when it is not PMD's default of 1 $/MWh.
    fn gen_cost(&self, g: &Obj<'_>) -> Option<Vec<f64>> {
        let cost = g.floats("cost");
        let linear = *cost.first()?;
        let per_mwh = if self.per_unit {
            linear * (1e6 / self.power_scale_factor) / self.sbase
        } else {
            linear
        };
        ((per_mwh - 1.0).abs() > 1e-9).then_some(vec![per_mwh / 1000.0])
    }

    fn map_solar(&mut self, entry: &MapEntry) {
        let what = format!("solar {}", entry.from);
        let Some(id) = entry.to.first() else { return };
        let Some(g) = self.object(id) else {
            self.warn(
                &C::READ_PMD_MATH_REFERENCE_DROPPED,
                format!("{what}: `{id}` is not in the document"),
            );
            return;
        };
        self.consumed.insert(id.clone());
        let bus_key = g.index("gen_bus").unwrap_or_default();
        let bus = self.bus_id(&bus_key, &what);
        let map = g.connections("connections");
        let hot = map.iter().filter(|t| *t != "4").count().max(1);
        let (p_min, p_max, q_min, q_max) = self.gen_bounds(&g);
        let s_max = p_max.clone().unwrap_or_else(|| vec![0.0; hot]);
        let mut ibr = DistIbr::new(
            entry.from.clone(),
            bus,
            map,
            if hot == 1 {
                IbrTopology::SinglePhase
            } else {
                IbrTopology::ThreeLeg
            },
            IbrPrimeMover::Pv,
            s_max,
        );
        ibr.p_avail = Some(g.floats("pg").iter().map(|v| self.watts(*v)).sum());
        ibr.p_min = p_min;
        ibr.p_max = p_max;
        ibr.q_min = q_min;
        ibr.q_max = q_max;
        if g.int("gen_status") == Some(0) {
            ibr.extras.insert("pmd_status".into(), json!("DISABLED"));
        }
        self.warn(&C::READ_PMD_MATH_VALUE_SUBSTITUTED, format!("{what}: the apparent power rating is taken as the active power bound; the MATHEMATICAL generator states none"));
        self.net.ibrs_mut().push(ibr);
    }

    fn map_voltage_source(&mut self, entry: &MapEntry) {
        let what = format!("voltage source {}", entry.from);
        let (mut g, mut virtual_bus, mut branch) = (None, None, None);
        for id in &entry.to {
            let Some(obj) = self.object(id) else {
                self.warn(
                    &C::READ_PMD_MATH_REFERENCE_DROPPED,
                    format!("{what}: `{id}` is not in the document"),
                );
                continue;
            };
            self.consumed.insert(id.clone());
            match obj.table {
                "gen" => g = Some(obj),
                "bus" => virtual_bus = Some(obj),
                "branch" => branch = Some(obj),
                _ => {}
            }
        }
        let Some(g) = g else {
            self.warn(
                &C::READ_PMD_MATH_RECORD_DROPPED,
                format!("{what}: no generator in its map entry; dropped"),
            );
            return;
        };
        // The source's voltage sits on the virtual bus behind its impedance,
        // else on the bus it connects to.
        let (voltage_bus, real_key) = if let (Some(vb), Some(b)) = (&virtual_bus, &branch) {
            (vb.clone(), b.index("t_bus").unwrap_or_default())
        } else {
            let key = g.index("gen_bus").unwrap_or_default();
            let Some(bus) = self.object(&format!("bus.{key}")) else {
                self.warn(
                    &C::READ_PMD_MATH_REFERENCE_DROPPED,
                    format!("{what}: bus {key} is not in the document; dropped"),
                );
                return;
            };
            (bus, key)
        };
        let bus = self.bus_id(&real_key, &what);
        let map = g.connections("connections");
        let vm: Vec<f64> = voltage_bus
            .floats("vm")
            .iter()
            .map(|v| self.volts(&voltage_bus.key, *v))
            .collect();
        let va: Vec<f64> = voltage_bus
            .floats("va")
            .iter()
            .map(|v| if self.per_unit { *v } else { v.to_radians() })
            .collect();
        // The bus arrays run over the bus terminals; the source's phasors
        // are the entries at its connections' positions.
        let terminals = voltage_bus.connections("terminals");
        let at = |values: &[f64]| -> Vec<f64> {
            map.iter()
                .enumerate()
                .map(|(k, conn)| {
                    let pos = terminals.iter().position(|t| t == conn).unwrap_or(k);
                    values.get(pos).copied().unwrap_or(0.0)
                })
                .collect()
        };
        let (v_magnitude, v_angle) = (at(&vm), at(&va));
        let n = map.len();
        let mut source = VoltageSource::new(entry.from.clone(), bus, map, v_magnitude, v_angle);
        if let Some(b) = &branch {
            let zbase = self.zbase(&real_key);
            for (key, out) in [("br_r", "rs"), ("br_x", "xs")] {
                if let Some(m) = b.matrix(key) {
                    source
                        .extras
                        .insert(out.into(), matrix_value(&scaled(&m, zbase)));
                }
            }
        }
        if let Some(cost) = self.gen_cost(&g) {
            source.energy_cost_rate = Some(vec![cost[0]; n.max(1)]);
        }
        if g.int("gen_status") == Some(0) {
            source.extras.insert("pmd_status".into(), json!("DISABLED"));
        }
        self.net.sources_mut().push(source);
    }

    fn map_storage(&mut self, entry: &MapEntry) {
        for id in &entry.to {
            self.consumed.insert(id.clone());
            self.keep_untyped(id, "storage has no typed multiconductor element");
        }
        self.warn(
            &C::READ_PMD_MATH_RETAINED_SOURCE_ONLY,
            format!(
                "storage {}: kept untyped; the model has no storage element",
                entry.from
            ),
        );
    }

    // ----- leftovers -----------------------------------------------------------------

    fn keep_untyped(&mut self, id: &str, why: &str) {
        let Some(obj) = self.object(id) else { return };
        let raw = serde_json::to_string(&Value::Object(obj.fields.clone())).unwrap_or_default();
        let name = obj.name();
        self.net.untyped_objects_mut().push(UntypedObject::new(
            obj.table,
            name,
            vec![
                (Some("pmd_math_reason".into()), why.to_owned()),
                (None, raw),
            ],
        ));
    }

    /// Objects no map entry claimed: without a map, every real object reads
    /// by its own table and the virtual ones stay untyped.
    fn unmapped(&mut self) {
        let has_map = !self.map.is_empty();
        for table in [
            "branch",
            "switch",
            "transformer",
            "gen",
            "load",
            "shunt",
            "storage",
        ] {
            for obj in self.table(table) {
                let id = obj.id();
                if self.consumed.contains(&id) {
                    continue;
                }
                let name = obj.name();
                let is_virtual = name.starts_with("_virtual_");
                if has_map || is_virtual || table == "transformer" || table == "storage" {
                    let why = if is_virtual {
                        "a virtual object no map entry claims"
                    } else {
                        "an object no map entry claims"
                    };
                    self.warn(
                        &C::READ_PMD_MATH_RETAINED_SOURCE_ONLY,
                        format!("{table} {name}: {why}; kept untyped"),
                    );
                    self.keep_untyped(&id, why);
                    continue;
                }
                // No map at all: read the real objects by their tables.
                let entry = MapEntry {
                    from: name,
                    to: vec![id.clone()],
                    unmap: String::new(),
                };
                match table {
                    "branch" => self.map_line(&entry),
                    "switch" => self.map_switch(&entry),
                    "gen" => {
                        if obj.int("control_mode") == Some(1) {
                            self.map_voltage_source(&entry);
                        } else {
                            self.map_generator(&entry);
                        }
                    }
                    "load" => self.map_load(&entry),
                    "shunt" => self.map_shunt(&entry),
                    _ => {}
                }
            }
        }
    }
}

/// The inverse of PMD's `_sc2br_impedance`: star branch impedances between
/// the windings' internal nodes back to the pairwise short circuit
/// impedances, through the nodal admittance matrix and its reduced bus
/// impedance matrix.
pub(super) fn br2sc_impedance(
    zbr: &BTreeMap<(usize, usize), Complex64>,
    n: usize,
) -> BTreeMap<(usize, usize), Complex64> {
    if n == 2 {
        return zbr.clone();
    }
    let mut y = vec![vec![Complex64::default(); n]; n];
    for (&(i, j), z) in zbr {
        if z.norm() == 0.0 || !z.re.is_finite() || !z.im.is_finite() {
            continue;
        }
        let yij = Complex64::new(1.0, 0.0) / z;
        y[i][j] -= yij;
        y[j][i] -= yij;
        y[i][i] += yij;
        y[j][j] += yij;
    }
    let reduced: Vec<Vec<Complex64>> = (1..n).map(|i| (1..n).map(|j| y[i][j]).collect()).collect();
    let Some(zb) = invert(&reduced) else {
        return zbr.clone();
    };
    let mut out = BTreeMap::new();
    for i in 0..n - 1 {
        out.insert((0, i + 1), zb[i][i]);
        for j in 0..i {
            out.insert((j + 1, i + 1), zb[i][i] + zb[j][j] - 2.0 * zb[i][j]);
        }
    }
    out
}

/// Gauss-Jordan inverse of a small complex matrix; `None` when singular.
fn invert(a: &[Vec<Complex64>]) -> Option<Vec<Vec<Complex64>>> {
    let n = a.len();
    let mut m: Vec<Vec<Complex64>> = a
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.extend((0..n).map(|j| {
                if i == j {
                    Complex64::new(1.0, 0.0)
                } else {
                    Complex64::default()
                }
            }));
            r
        })
        .collect();
    for col in 0..n {
        let pivot = (col..n).max_by(|&a, &b| m[a][col].norm().total_cmp(&m[b][col].norm()))?;
        if m[pivot][col].norm() < 1e-300 {
            return None;
        }
        m.swap(col, pivot);
        let p = m[col][col];
        for v in &mut m[col] {
            *v /= p;
        }
        for row in 0..n {
            if row != col {
                let factor = m[row][col];
                if factor != Complex64::default() {
                    let pivot_row = m[col].clone();
                    for (v, pv) in m[row].iter_mut().zip(&pivot_row) {
                        *v -= factor * pv;
                    }
                }
            }
        }
    }
    Some(m.into_iter().map(|row| row[n..].to_vec()).collect())
}

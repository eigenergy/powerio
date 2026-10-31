//! [`MulticonductorNetwork`] into the PMD MATHEMATICAL model: PMD's
//! `eng2math` lowering, `calc_voltage_bases` zone flood, and
//! `make_per_unit!` rebasing, performed on the typed network.
//!
//! Object order follows the network's own tables, so indices are stable
//! across writes; PMD numbers from its hash tables, so the oracle tests
//! compare by name and `source_id` rather than by index.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;
use std::rc::Rc;

use num_complex::Complex64;
use serde_json::{Map, Value, json};

use super::read::br2sc_impedance;
use super::{
    LOAD_MODEL_CURRENT, LOAD_MODEL_IMPEDANCE, LOAD_MODEL_POWER, PmdMathEmitOptions, Settings,
};
use crate::convert::TextEmission;
use crate::diagnostics::codes as C;
use crate::model::{
    ConductorMatrix, Configuration, DistBus, DistLoadVoltageModel, DistWindingConn, Extras,
    MulticonductorNetwork,
};
use crate::pmd::write::{capacitor_susceptance, matrix, renamed_terminals};

/// `Inf`, which the JSON writer turns into `null` the way PMD's does.
const INF: f64 = f64::INFINITY;

pub(crate) fn validate_emission(net: &MulticonductorNetwork) -> Result<(), powerio_core::Error> {
    let count = |name: &str, n: usize| {
        if n > super::read::MAX_MATRIX_DIM {
            return Err(powerio_core::Error::new(
                &C::BUILD_DIST_ELECTRICAL_INCOMPLETE,
                format!(
                    "{name}: dimension {n} exceeds {}",
                    super::read::MAX_MATRIX_DIM
                ),
            ));
        }
        Ok(())
    };
    let check = |name: &str, extras: &Extras, keys: &[&str]| {
        for key in keys {
            if let Some(value) = extras.get(*key)
                && super::read::matrix_dimension(value).is_none()
            {
                return Err(powerio_core::Error::new(
                    &C::BUILD_DIST_ELECTRICAL_INCOMPLETE,
                    format!(
                        "{name}: {key} must be a nonempty square numeric matrix of at most {} conductors",
                        super::read::MAX_MATRIX_DIM
                    ),
                ));
            }
        }
        Ok(())
    };
    for code in net.line_codes() {
        count(&format!("linecode {}", code.name), code.n_conductors)?;
    }
    for line in net.lines() {
        count(
            &format!("line {}", line.name),
            line.terminal_map_from.len().max(line.terminal_map_to.len()),
        )?;
    }
    for shunt in net.shunts() {
        count(&format!("shunt {}", shunt.name), shunt.terminal_map.len())?;
    }
    for load in net.loads() {
        count(&format!("load {}", load.name), load.terminal_map.len())?;
    }
    for transformer in net.transformers() {
        let name = format!("transformer {}", transformer.name);
        count(&name, transformer.phases)?;
        count(&name, transformer.windings.len())?;
        for winding in &transformer.windings {
            count(&name, winding.terminal_map.len())?;
        }
    }
    for switch in net.switches() {
        count(
            &format!("switch {}", switch.name),
            switch
                .terminal_map_from
                .len()
                .max(switch.terminal_map_to.len()),
        )?;
        check(
            &format!("switch {}", switch.name),
            &switch.extras,
            &[
                "pmd_rs", "pmd_xs", "pmd_g_fr", "pmd_g_to", "pmd_b_fr", "pmd_b_to",
            ],
        )?;
    }
    for source in net.sources() {
        count(
            &format!("voltage source {}", source.name),
            source.terminal_map.len(),
        )?;
        check(
            &format!("voltage source {}", source.name),
            &source.extras,
            &["rs", "xs"],
        )?;
    }
    Ok(())
}

/// Emit the MATHEMATICAL document.
///
/// # Errors
/// Kron reduction refused the network, or a bus has no voltage base: the
/// network has an island no stated base reaches.
pub(crate) fn emit_pmd_math_json_text(
    net: &MulticonductorNetwork,
    options: &PmdMathEmitOptions,
) -> TextEmission {
    let mut diagnostics = crate::diagnostics::Diagnostics::new();
    let mut reduced;
    let (net, kron_reduced) = if options.kron_reduce && has_grounded_neutral(net) {
        match crate::kron::neutral_kron_reduce(net, &crate::kron::NeutralKronOptions::default()) {
            Ok(reduction) => {
                diagnostics.absorb(reduction.report().diagnostics.iter().cloned());
                let did_reduce = !reduction.report().buses.is_empty();
                reduced = reduction.network().clone();
                // Neutral removal changes a reversed center-tap map from
                // [neutral, phase] to [phase]. Keep its original polarity
                // before that orientation disappears from the terminal map.
                for (winding_model, original) in reduced
                    .transformers_mut()
                    .iter_mut()
                    .zip(net.transformers())
                {
                    winding_model
                        .extras
                        .entry("pmd_polarity".into())
                        .or_insert_with(|| {
                            json!(crate::pmd::write::lag_polarity(&original.windings))
                        });
                }
                (&reduced, did_reduce)
            }
            Err(error) => {
                // PMD reduces only the neutrals it can; this writer reduces
                // all or none, and a network it cannot reduce keeps every
                // conductor, which the document states.
                diagnostics.push(
                    &C::EMIT_PMD_MATH_VALUE_SUBSTITUTED,
                    format!(
                        "Kron reduction skipped, so the document keeps every conductor \
                         (is_kron_reduced false): {error}"
                    ),
                );
                (net, false)
            }
        }
    } else {
        (net, false)
    };
    let settings = Settings::of(net);
    // A winding's rated voltage reaches the document only as the ideal
    // unit's `tm_nom` on the two bus bases, and every loss model value is
    // stated on it. The lowering runs again with each rated voltage replaced
    // by the voltage the reader will recover from its ratio, until the two
    // agree bit for bit, so a document read and written again states the
    // same loss model (one or two passes in practice).
    let mut snapped: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for _ in 0..4 {
        let mut w = Writer::new(
            settings.clone(),
            kron_reduced,
            diagnostics.clone(),
            net,
            snapped.clone(),
        );
        let doc = w.finish(net);
        if w.snap_next.iter().all(|(name, v)| {
            snapped
                .get(name)
                .is_some_and(|s| s.iter().zip(v).all(|(a, b)| a.to_bits() == b.to_bits()))
        }) {
            return TextEmission::new(
                serde_json::to_string_pretty(&doc).expect("maps and finite numbers") + "\n",
                Vec::new(),
                w.diagnostics,
            );
        }
        snapped = w.snap_next;
    }
    let mut w = Writer::new(settings, kron_reduced, diagnostics, net, snapped);
    let doc = w.finish(net);
    TextEmission::new(
        serde_json::to_string_pretty(&doc).expect("maps and finite numbers") + "\n",
        Vec::new(),
        w.diagnostics,
    )
}

/// Whether any bus declares a grounded terminal: the Kron reduction has
/// something to eliminate. A network without one lowers as it is, and the
/// document says `is_kron_reduced: false`, as PMD's does for such a case.
fn has_grounded_neutral(net: &MulticonductorNetwork) -> bool {
    net.buses().iter().any(|b| !b.grounded.is_empty())
}

/// Marker on a load object whose `vnom_kv` the per unit pass takes from the
/// bus voltage base; never emitted.
const VNOM_FROM_BASE: &str = "_pio_vnom_from_base";

/// A field's read and rewrite chain beyond the per unit step: given the
/// per unit value and the impedance base it was divided by, the value the
/// writer produces again from what the reader recovers.
type Cycle = Rc<dyn Fn(f64, f64) -> f64>;

/// One MATHEMATICAL object under construction: its fields before the per
/// unit pass, plus the bookkeeping that pass needs.
#[derive(Clone)]
struct Obj {
    fields: Map<String, Value>,
    /// Fields whose engineering value is derived (a winding resistance from
    /// a percentage on the winding base, referred to the internal side):
    /// the per unit pass stabilizes them over this whole chain rather than
    /// over the per unit step alone.
    cycles: BTreeMap<String, Cycle>,
}

impl std::fmt::Debug for Obj {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Obj")
            .field("fields", &self.fields)
            .finish_non_exhaustive()
    }
}

impl Obj {
    fn new(name: &str, source_id: &str, index: usize) -> Self {
        let mut fields = Map::new();
        fields.insert("name".into(), json!(name));
        fields.insert("source_id".into(), json!(source_id));
        fields.insert("index".into(), json!(index));
        Self {
            fields,
            cycles: BTreeMap::new(),
        }
    }

    fn with_cycle(&mut self, key: &str, cycle: impl Fn(f64, f64) -> f64 + 'static) -> &mut Self {
        self.cycles.insert(key.into(), Rc::new(cycle));
        self
    }

    /// `scale` for an impedance or admittance field whose chain the object
    /// recorded: the recorded cycle, given the base, replaces the per unit
    /// step's own.
    fn scale_on_base(
        &mut self,
        key: &str,
        base: f64,
        forward: impl Fn(f64) -> f64,
        cycle: impl Fn(f64) -> f64,
    ) {
        if let Some(chain) = self.cycles.get(key).cloned() {
            self.scale(key, forward, move |pu| chain(pu, base));
        } else {
            self.scale(key, forward, cycle);
        }
    }

    fn set(&mut self, key: &str, value: Value) -> &mut Self {
        self.fields.insert(key.into(), value);
        self
    }

    fn f(&self, key: &str) -> Option<f64> {
        self.fields.get(key).and_then(Value::as_f64)
    }

    fn usize(&self, key: &str) -> Option<usize> {
        self.fields
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
    }

    fn connections(&self, key: &str) -> Vec<i64> {
        self.fields
            .get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default()
    }

    /// Convert a scalar or array of numbers in place; `null` entries (an
    /// infinite bound) stay `null`. `forward` is this conversion and `cycle`
    /// the reader's inverse followed by it again, in the same operations: the
    /// result is iterated to its fixed point,
    /// so a document written, read, and written again is byte stable where a
    /// single multiply and divide could drift one unit in the last place.
    fn scale(&mut self, key: &str, forward: impl Fn(f64) -> f64, cycle: impl Fn(f64) -> f64) {
        if let Some(value) = self.fields.get_mut(key) {
            map_value(value, &|x| stabilize(forward(x), &cycle));
        }
    }

    fn map_numbers(&mut self, key: &str, f: impl Fn(f64) -> f64, cycle: impl Fn(f64) -> f64) {
        if let Some(value) = self.fields.get_mut(key) {
            map_value(value, &|x| stabilize(f(x), &cycle));
        }
    }
}

/// Iterate `cycle` from `y` to a fixed point (a guard that exits on the
/// first pass in practice), so the value a reader recovers and rewrites is
/// the value written.
#[allow(clippy::float_cmp)] // the fixed point check is bit exact by design
fn stabilize(y: f64, cycle: &dyn Fn(f64) -> f64) -> f64 {
    let mut y = y;
    for _ in 0..4 {
        let next = cycle(y);
        if next == y || !next.is_finite() {
            break;
        }
        y = next;
    }
    y
}

fn map_value(value: &mut Value, f: &dyn Fn(f64) -> f64) {
    match value {
        Value::Number(n) => {
            if let Some(x) = n.as_f64() {
                *value = num(f(x));
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| map_value(v, f)),
        _ => {}
    }
}

/// A finite number, or `null` for an infinite or undefined one (PMD's
/// `print_file` writes `Inf` and `NaN` as `null`).
fn num(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

fn nums(values: &[f64]) -> Value {
    Value::Array(values.iter().map(|&v| num(v)).collect())
}

fn ints(values: &[i64]) -> Value {
    Value::Array(values.iter().map(|&v| json!(v)).collect())
}

fn diag(value: f64, n: usize) -> ConductorMatrix {
    (0..n)
        .map(|i| (0..n).map(|j| if i == j { value } else { 0.0 }).collect())
        .collect()
}

fn zeros(n: usize) -> ConductorMatrix {
    vec![vec![0.0; n]; n]
}

/// The leading `n` by `n` block of a matrix, zero padded.
fn slice(m: &ConductorMatrix, n: usize) -> ConductorMatrix {
    (0..n)
        .map(|i| {
            (0..n)
                .map(|j| m.get(i).and_then(|r| r.get(j)).copied().unwrap_or(0.0))
                .collect()
        })
        .collect()
}

fn scaled(m: &ConductorMatrix, k: f64) -> ConductorMatrix {
    m.iter()
        .map(|row| row.iter().map(|v| v * k).collect())
        .collect()
}

fn configuration(c: Configuration) -> &'static str {
    match c {
        Configuration::Delta => "DELTA",
        Configuration::Wye | Configuration::SinglePhase => "WYE",
    }
}

fn winding_configuration(c: DistWindingConn) -> &'static str {
    match c {
        DistWindingConn::Delta => "DELTA",
        DistWindingConn::Wye => "WYE",
    }
}

/// The element status PMD states: `pmd_status` stash, else enabled.
fn status_of(extras: &Extras) -> i64 {
    match extras.get("pmd_status").and_then(Value::as_str) {
        Some(s) if s.eq_ignore_ascii_case("DISABLED") => 0,
        _ => 1,
    }
}

/// `voltage_scale_factor` units of a volt figure: kV by default.
fn kv(volts: f64, settings: &Settings) -> f64 {
    volts / settings.voltage_scale_factor
}

/// `power_scale_factor` units of a watt or volt-ampere figure: kW by default.
fn kw(watts: f64, settings: &Settings) -> f64 {
    watts / settings.power_scale_factor
}

struct Writer {
    settings: Settings,
    kron_reduced: bool,
    diagnostics: crate::diagnostics::Diagnostics,
    renamed: BTreeMap<String, i64>,
    /// Lowercased bus id to its MATHEMATICAL index.
    bus_index: BTreeMap<String, usize>,
    buses: Vec<Obj>,
    branches: Vec<Obj>,
    switches: Vec<Obj>,
    transformers: Vec<Obj>,
    gens: Vec<Obj>,
    loads: Vec<Obj>,
    shunts: Vec<Obj>,
    map: Vec<Value>,
    /// Rated winding voltages to lower in place of the network's: the
    /// voltages the reader recovers from the previous pass's ratios.
    snapped: BTreeMap<String, Vec<f64>>,
    /// The rated winding voltages the reader will recover from this pass,
    /// by transformer name.
    snap_next: BTreeMap<String, Vec<f64>>,
    /// Each ideal unit's position in `transformers`, its transformer, its
    /// winding, and the rated voltage it was lowered from.
    units: Vec<UnitRef>,
}

/// One ideal unit's bookkeeping for the winding voltage fixed point.
struct UnitRef {
    index: usize,
    transformer: String,
    winding: usize,
    v_ref: f64,
}

impl Writer {
    /// Lower every element of `net`; `finish` then applies the per unit pass.
    fn new(
        settings: Settings,
        kron_reduced: bool,
        diagnostics: crate::diagnostics::Diagnostics,
        net: &MulticonductorNetwork,
        snapped: BTreeMap<String, Vec<f64>>,
    ) -> Self {
        let mut w = Writer {
            settings,
            kron_reduced,
            diagnostics,
            renamed: renamed_terminals(net),
            bus_index: BTreeMap::new(),
            buses: Vec::new(),
            branches: Vec::new(),
            switches: Vec::new(),
            transformers: Vec::new(),
            gens: Vec::new(),
            loads: Vec::new(),
            shunts: Vec::new(),
            map: Vec::new(),
            snapped,
            snap_next: BTreeMap::new(),
            units: Vec::new(),
        };
        w.map.push(json!({"unmap_function": "_map_math2eng_root!"}));
        w.buses(net);
        w.lines(net);
        w.transformers(net);
        w.switches(net);
        w.shunts(net);
        w.loads(net);
        w.generators(net);
        w.ibrs(net);
        w.sources(net);
        w
    }

    fn warn(&mut self, info: &'static crate::diagnostics::DiagnosticInfo, msg: impl Into<String>) {
        self.diagnostics.push(info, msg);
    }

    /// Terminal names as PMD integer connections, the ENGINEERING writer's
    /// renaming for names that are not numeric.
    fn conns(&mut self, map: &[String], what: &str) -> Vec<i64> {
        map.iter()
            .map(|t| {
                if let Ok(value) = t.parse::<i64>() {
                    return value;
                }
                let id = self.renamed.get(t).copied().unwrap_or(0);
                self.warn(
                    &C::EMIT_PMD_MATH_VALUE_SUBSTITUTED,
                    format!("{what}: terminal `{t}` is not numeric; emitted as {id}"),
                );
                id
            })
            .collect()
    }

    fn bus_of(&mut self, bus: &str, what: &str) -> usize {
        if let Some(&index) = self.bus_index.get(&bus.to_ascii_lowercase()) {
            return index;
        }
        self.warn(
            &C::EMIT_PMD_MATH_VALUE_DEFAULTED,
            format!("{what}: bus `{bus}` is not declared; a bus with no terminals is added"),
        );
        let index = self.buses.len() + 1;
        let mut obj = Obj::new(bus, &format!("bus.{bus}"), index);
        obj.set("bus_i", json!(index))
            .set("bus_type", json!(1))
            .set("terminals", json!([]))
            .set("grounded", json!([]))
            .set("vmin", json!([]))
            .set("vmax", json!([]))
            .set("vm_pair_lb", json!([]))
            .set("vm_pair_ub", json!([]));
        self.buses.push(obj);
        self.bus_index.insert(bus.to_ascii_lowercase(), index);
        self.map.push(json!({
            "from": bus, "to": format!("bus.{index}"), "unmap_function": "_map_math2eng_bus!"
        }));
        index
    }

    fn push_virtual_bus(&mut self, obj: Obj) -> usize {
        let index = obj.usize("index").expect("virtual buses carry their index");
        self.buses.push(obj);
        index
    }

    // ----- buses ------------------------------------------------------------

    fn buses(&mut self, net: &MulticonductorNetwork) {
        for bus in net.buses() {
            let index = self.buses.len() + 1;
            let what = format!("bus {}", bus.id);
            let terminals = self.conns(&bus.terminals, &what);
            let grounded: Vec<bool> = bus
                .terminals
                .iter()
                .map(|t| bus.grounded.contains(t))
                .collect();
            let (vmin, vmax) = self.bus_bounds(bus, net);
            let mut obj = Obj::new(&bus.id, &format!("bus.{}", bus.id), index);
            obj.set("bus_i", json!(index))
                .set(
                    "bus_type",
                    json!(if status_of(&bus.extras) == 0 { 4 } else { 1 }),
                )
                .set("terminals", ints(&terminals))
                .set("grounded", json!(grounded))
                .set("vmin", nums(&vmin))
                .set("vmax", nums(&vmax))
                .set("vm_pair_lb", json!([]))
                .set("vm_pair_ub", json!([]));
            for (key, present) in [
                ("vpn", bus.vpn_min.is_some() || bus.vpn_max.is_some()),
                ("vpp", bus.vpp_min.is_some() || bus.vpp_max.is_some()),
                (
                    "sequence",
                    bus.vpos_min.is_some()
                        || bus.vneg_max.is_some()
                        || bus.vzero_max.is_some()
                        || bus.vn_max.is_some(),
                ),
            ] {
                if present {
                    self.warn(
                        &C::EMIT_PMD_MATH_FIELD_DROPPED,
                        format!(
                            "{what}: {key} voltage bounds have no MATHEMATICAL pair bound; dropped"
                        ),
                    );
                }
            }
            self.buses.push(obj);
            self.bus_index.insert(bus.id.to_ascii_lowercase(), index);
            self.map.push(json!({
                "from": bus.id, "to": format!("bus.{index}"), "unmap_function": "_map_math2eng_bus!"
            }));
        }
    }

    /// Per terminal absolute bounds in kV, `0` and `Inf` where unstated, as
    /// PMD's `_get_tight_absolute_voltage_magnitude_bounds` reads them.
    fn bus_bounds(&self, bus: &DistBus, net: &MulticonductorNetwork) -> (Vec<f64>, Vec<f64>) {
        let n = bus.terminals.len();
        let phases = bus.phase_indices(net.extras().get("bmopf_terminal_conventions"));
        let mut vmin = vec![0.0; n];
        let mut vmax = vec![INF; n];
        for (k, &i) in phases.iter().enumerate() {
            if let Some(v) = bus
                .v_min
                .or_else(|| bus.v_min_phase.as_ref().and_then(|p| p.get(k)).copied())
            {
                vmin[i] = kv(v, &self.settings);
            }
            if let Some(v) = bus
                .v_max
                .or_else(|| bus.v_max_phase.as_ref().and_then(|p| p.get(k)).copied())
            {
                vmax[i] = kv(v, &self.settings);
            }
        }
        (vmin, vmax)
    }

    // ----- lines ------------------------------------------------------------

    fn lines(&mut self, net: &MulticonductorNetwork) {
        let codes: BTreeMap<String, &crate::model::DistLineCode> = net
            .line_codes()
            .iter()
            .map(|c| (c.name.to_ascii_lowercase(), c))
            .collect();
        for line in net.lines() {
            let what = format!("line {}", line.name);
            let index = self.branches.len() + 1;
            let f_bus = self.bus_of(&line.bus_from, &what);
            let t_bus = self.bus_of(&line.bus_to, &what);
            let f_connections = self.conns(&line.terminal_map_from, &what);
            let t_connections = self.conns(&line.terminal_map_to, &what);
            let n = f_connections.len();
            let Some(code) = codes.get(&line.linecode.to_ascii_lowercase()).copied() else {
                self.warn(
                    &C::EMIT_PMD_MATH_RECORD_DROPPED,
                    format!(
                        "{what}: linecode `{}` is not declared; the branch is dropped",
                        line.linecode
                    ),
                );
                continue;
            };
            let length = if line.length.is_finite() {
                line.length
            } else {
                1.0
            };
            let mut obj = Obj::new(&line.name, &format!("line.{}", line.name), index);
            obj.set("f_bus", json!(f_bus))
                .set("t_bus", json!(t_bus))
                .set("f_connections", ints(&f_connections))
                .set("t_connections", ints(&t_connections))
                .set("br_r", matrix(&slice(&scaled(&code.r_series, length), n)))
                .set("br_x", matrix(&slice(&scaled(&code.x_series, length), n)))
                .set("g_fr", matrix(&slice(&scaled(&code.g_from, length), n)))
                .set("g_to", matrix(&slice(&scaled(&code.g_to, length), n)))
                .set("b_fr", matrix(&slice(&scaled(&code.b_from, length), n)))
                .set("b_to", matrix(&slice(&scaled(&code.b_to, length), n)))
                .set("angmin", nums(&vec![-60.0; n]))
                .set("angmax", nums(&vec![60.0; n]))
                .set("br_status", json!(status_of(&line.extras)));
            for (key, engineering_key) in [("angmin", "vad_lb"), ("angmax", "vad_ub")] {
                if let Some(bounds) = line
                    .extras
                    .get(&format!("pmd_{key}"))
                    .or_else(|| line.extras.get(engineering_key))
                {
                    obj.set(key, bounds.clone());
                }
            }
            let i_max = line.i_max.as_ref().or(code.i_max.as_ref());
            let s_max = line.s_max.as_ref().or(code.s_max.as_ref());
            let per_phase = |v: Option<&Vec<f64>>, scale: f64| -> Vec<f64> {
                v.map_or_else(
                    || vec![INF; n],
                    |v| {
                        (0..n)
                            .map(|k| v.get(k).copied().unwrap_or(INF) * scale)
                            .collect()
                    },
                )
            };
            obj.set("c_rating_a", nums(&per_phase(i_max, 1.0)))
                .set("c_rating_b", nums(&vec![INF; n]))
                .set("c_rating_c", nums(&vec![INF; n]))
                .set(
                    "rate_a",
                    nums(&per_phase(s_max, 1.0 / self.settings.power_scale_factor)),
                )
                .set("rate_b", nums(&vec![INF; n]))
                .set("rate_c", nums(&vec![INF; n]));
            if line.route.is_some() {
                self.warn(
                    &C::EMIT_PMD_MATH_FIELD_DROPPED,
                    format!("{what}: line route has no MATHEMATICAL field; dropped"),
                );
            }
            self.branches.push(obj);
            self.map.push(json!({
                "from": line.name, "to": format!("branch.{index}"), "unmap_function": "_map_math2eng_line!"
            }));
        }
    }

    // ----- transformers -------------------------------------------------------

    #[allow(clippy::too_many_lines)] // one transformer lowers in one pass: loss model, then one ideal unit per winding
    fn transformers(&mut self, net: &MulticonductorNetwork) {
        for xf in net.transformers() {
            let what = format!("transformer {}", xf.name);
            if xf.windings.len() < 2 {
                self.warn(
                    &C::EMIT_PMD_MATH_RECORD_DROPPED,
                    format!("{what}: fewer than two windings; dropped"),
                );
                continue;
            }
            let nrw = xf.windings.len();
            let mut to_map: Vec<String> = Vec::new();
            // The nominal voltages and powers in SI, the bases the catalog
            // values are stated on.
            let vnom: Vec<f64> = xf
                .windings
                .iter()
                .enumerate()
                .map(|(w, winding)| {
                    self.snapped
                        .get(&xf.name)
                        .and_then(|v| v.get(w).copied())
                        .unwrap_or(winding.v_ref)
                })
                .collect();
            let snom: Vec<f64> = xf.windings.iter().map(|w| w.s_rating).collect();
            if vnom
                .iter()
                .chain(&snom)
                .any(|v| !v.is_finite() || *v <= 0.0)
            {
                self.warn(
                    &C::EMIT_PMD_MATH_RECORD_DROPPED,
                    format!(
                        "{what}: a winding states no positive rated voltage and power; dropped"
                    ),
                );
                continue;
            }
            let zbase: Vec<f64> = vnom.iter().zip(&snom).map(|(v, s)| v * v / s).collect();
            let pairs = crate::model::pair_keys(nrw);
            let x_sc: Vec<f64> = pairs
                .iter()
                .enumerate()
                .map(|(k, _)| xf.xsc_pct.get(k).copied().unwrap_or(0.0) / 100.0 * zbase[0])
                .collect();
            let r_s: Vec<f64> = xf
                .windings
                .iter()
                .zip(&zbase)
                .map(|(w, z)| w.r_pct / 100.0 * z)
                .collect();
            let (noloadloss, cmag) = crate::model::transformer_no_load_percentages(xf).map_or_else(
                || {
                    (
                        extras_f64(&xf.extras, "%noloadloss").unwrap_or(0.0) / 100.0,
                        extras_f64(&xf.extras, "%imag").unwrap_or(0.0) / 100.0,
                    )
                },
                |(loss, imag)| (loss / 100.0, imag / 100.0),
            );
            let g_sh = noloadloss * snom[0] / (vnom[0] * vnom[0]);
            let b_sh = -(cmag * snom[0]) / (vnom[0] * vnom[0]);
            // Refer everything to the internal side: PMD divides by the
            // nominal voltages in kV, so the internal side is the 1 kV base.
            let ratios: Vec<f64> = vnom
                .iter()
                .map(|v| v / self.settings.voltage_scale_factor)
                .collect();
            let x_sc: Vec<f64> = x_sc.iter().map(|x| x / (ratios[0] * ratios[0])).collect();
            let r_s: Vec<f64> = r_s
                .iter()
                .zip(&ratios)
                .map(|(r, ratio)| r / (ratio * ratio))
                .collect();
            let y_sh = Complex64::new(g_sh * ratios[0] * ratios[0], b_sh * ratios[0] * ratios[0]);
            // Winding pairs are one based in PMD's loss model.
            let zsc: BTreeMap<(usize, usize), Complex64> = pairs
                .iter()
                .zip(&x_sc)
                .map(|(&(i, j), &x)| ((i + 1, j + 1), Complex64::new(0.0, x)))
                .collect();

            let connections: Vec<Vec<i64>> = xf
                .windings
                .iter()
                .map(|w| self.conns(&w.terminal_map, &what))
                .collect();
            let dims = xf.phases.max(1);
            let status = status_of(&xf.extras);
            let chains = LossChains {
                vnom: vnom.clone(),
                snom: snom.clone(),
                ratios: ratios.clone(),
            };
            let t_bus = self.loss_model(
                &xf.name,
                &mut to_map,
                &r_s,
                &zsc,
                y_sh,
                &chains,
                &connections[0],
                dims,
                status,
            );

            let polarity: Vec<i64> = xf
                .extras
                .get("pmd_polarity")
                .and_then(Value::as_array)
                .map_or_else(
                    || crate::pmd::write::lag_polarity(&xf.windings),
                    |a| a.iter().map(|p| p.as_i64().unwrap_or(1)).collect(),
                );
            let mut taps = Map::new();
            crate::pmd::write::insert_tap_fields(&mut taps, xf, dims);
            let per_winding = |key: &str, w: usize| -> Value {
                taps.get(key)
                    .and_then(Value::as_array)
                    .and_then(|a| a.get(w))
                    .cloned()
                    .unwrap_or(Value::Null)
            };
            let sm_ub = extras_f64(&xf.extras, "emerghkva").map_or_else(
                || kw(snom[0], &self.settings) * 1.5,
                |kva| kva * (1000.0 / self.settings.power_scale_factor),
            );
            for w in 0..nrw {
                let winding = &xf.windings[w];
                let index = self.transformers.len() + 1;
                let tm_nom = if winding.conn == DistWindingConn::Delta {
                    kv(vnom[w], &self.settings) * 3f64.sqrt()
                } else {
                    kv(vnom[w], &self.settings)
                };
                let f_bus = self.bus_of(&winding.bus, &what);
                let t_connections: Vec<i64> = if self.kron_reduced {
                    connections[0].clone()
                } else {
                    (1..=i64::try_from(dims + 1).unwrap_or(i64::MAX)).collect()
                };
                let mut obj = Obj::new(
                    &format!("_virtual_transformer.{}.{}", xf.name, w + 1),
                    &format!("_virtual_transformer.transformer.{}.{}", xf.name, w + 1),
                    index,
                );
                obj.set("f_bus", json!(f_bus))
                    .set("t_bus", json!(t_bus[w]))
                    .set("tm_nom", num(tm_nom))
                    .set("f_connections", ints(&connections[w]))
                    .set("t_connections", ints(&t_connections))
                    .set("configuration", json!(winding_configuration(winding.conn)))
                    .set("polarity", json!(polarity.get(w).copied().unwrap_or(1)))
                    .set("tm_set", per_winding("tm_set", w))
                    .set("tm_fix", per_winding("tm_fix", w))
                    .set("tm_lb", per_winding("tm_lb", w))
                    .set("tm_ub", per_winding("tm_ub", w))
                    .set("tm_step", per_winding("tm_step", w))
                    .set("sm_ub", num(sm_ub))
                    .set("cm_ub", Value::Null)
                    .set("status", json!(status));
                self.transformers.push(obj);
                self.units.push(UnitRef {
                    index: index - 1,
                    transformer: xf.name.clone(),
                    winding: w,
                    v_ref: vnom[w],
                });
                to_map.push(format!("transformer.{index}"));
            }
            if xf.extras.contains_key("controls") {
                self.warn(
                    &C::EMIT_PMD_MATH_FIELD_DROPPED,
                    format!("{what}: regulator controls are not lowered; dropped"),
                );
            }
            self.map.push(json!({
                "from": xf.name, "to": to_map, "unmap_function": "_map_math2eng_transformer!"
            }));
        }
    }

    /// PMD's `_build_loss_model!`: the virtual buses and branches that carry a
    /// transformer's winding resistances, leakage reactances, and no-load
    /// admittance on the internal 1 kV side. Returns the internal bus of each
    /// winding, which its ideal unit connects to.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one loss model, PMD's `_build_loss_model!` inputs plus the read chains
    #[allow(clippy::many_single_char_names)] // PMD's own names: n windings, edge l between i and j
    fn loss_model(
        &mut self,
        name: &str,
        to_map: &mut Vec<String>,
        r_s: &[f64],
        zsc: &BTreeMap<(usize, usize), Complex64>,
        y_sh: Complex64,
        chains: &LossChains,
        connections: &[i64],
        nphases: usize,
        status: i64,
    ) -> Vec<usize> {
        let n = r_s.len();
        let mut tr_t_bus: Vec<usize> = (1..=n).collect();
        let mut buses: BTreeSet<usize> = (1..=2 * n).collect();
        let zbr = sc2br_impedance(zsc, n);
        // Edges in PMD's order: each winding's resistance, then the star
        // reactances by pair.
        let mut lines: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        let mut z: BTreeMap<usize, Complex64> = BTreeMap::new();
        // The read chain of each edge's impedance: a winding resistance
        // comes back through its own winding base, a star reactance of a two
        // winding unit through the first winding's; a star of three or more
        // windings passes through a matrix inverse and has no scalar chain.
        let mut chain_of: BTreeMap<usize, Cycle> = BTreeMap::new();
        let mut l = 1;
        for i in 1..=n {
            lines.insert(l, (i, i + n));
            z.insert(l, Complex64::new(r_s[i - 1], 0.0));
            chain_of.insert(l, chains.winding_resistance(i - 1));
            l += 1;
        }
        for (&(i, j), &zij) in &zbr {
            lines.insert(l, (i + n, j + n));
            z.insert(l, zij);
            if n == 2 {
                chain_of.insert(l, chains.pair_reactance());
            }
            l += 1;
        }
        let mut shunts: BTreeMap<usize, Complex64> = BTreeMap::new();
        shunts.insert(2, y_sh);
        // An infinite impedance is no connection.
        lines.retain(|l, _| {
            let keep = z[l].re.is_finite() && z[l].im.is_finite();
            if !keep {
                z.remove(l);
            }
            keep
        });
        // Merge short circuits: a zero impedance edge joins its buses.
        let mut stack: Vec<usize> = lines.keys().copied().collect();
        while let Some(l) = stack.pop() {
            let Some(&(i, j)) = lines.get(&l) else {
                continue;
            };
            if z[&l] != Complex64::default() {
                continue;
            }
            lines.remove(&l);
            buses.remove(&j);
            let mut dropped = Vec::new();
            for (k, edge) in &mut lines {
                if edge.0 == j {
                    edge.0 = i;
                }
                if edge.1 == j {
                    edge.1 = i;
                }
                if edge.0 == edge.1 {
                    dropped.push(*k);
                }
            }
            for k in dropped {
                lines.remove(&k);
                stack.retain(|s| *s != k);
            }
            if let Some(y) = shunts.remove(&j) {
                *shunts.entry(i).or_default() += y;
            }
            for bus in &mut tr_t_bus {
                if *bus == j {
                    *bus = i;
                }
            }
        }
        let hot: Vec<i64> = connections.iter().take(nphases).copied().collect();
        let mut bus_ids: BTreeMap<usize, usize> = BTreeMap::new();
        for &bus in &buses {
            let index = self.buses.len() + 1;
            let internal = tr_t_bus.contains(&bus);
            let (terminals, grounded): (Vec<i64>, Vec<bool>) = if self.kron_reduced {
                (hot.clone(), vec![false; nphases])
            } else if internal {
                (
                    (1..=i64::try_from(nphases + 1).unwrap_or(i64::MAX)).collect(),
                    [vec![false; nphases], vec![true]].concat(),
                )
            } else {
                (
                    (1..=i64::try_from(nphases).unwrap_or(i64::MAX)).collect(),
                    vec![false; nphases],
                )
            };
            let count = terminals.len();
            let mut obj = Obj::new(
                &format!("_virtual_bus.transformer.{name}_{bus}"),
                &format!("transformer.{name}"),
                index,
            );
            obj.set("bus_i", json!(index))
                .set("vmin", nums(&vec![0.0; count]))
                .set("vmax", nums(&vec![INF; count]))
                .set("vm_pair_lb", json!([]))
                .set("vm_pair_ub", json!([]))
                .set("terminals", ints(&terminals))
                .set("grounded", json!(grounded))
                .set("base_kv", json!(1.0))
                .set("bus_type", json!(if status == 0 { 4 } else { 1 }));
            if !self.kron_reduced && internal {
                obj.set("rg", json!([0.0])).set("xg", json!([0.0]));
            }
            self.push_virtual_bus(obj);
            bus_ids.insert(bus, index);
            to_map.push(format!("bus.{index}"));
        }
        for (l, (i, j)) in &lines {
            let (mut g_fr, mut b_fr, mut g_to, mut b_to) = (0.0, 0.0, 0.0, 0.0);
            let mut shunt_side: Option<&str> = None;
            if let Some(y) = shunts.remove(i) {
                g_fr = y.re;
                b_fr = y.im;
                shunt_side = Some("fr");
            }
            if let Some(y) = shunts.remove(j) {
                g_to = y.re;
                b_to = y.im;
                shunt_side = Some("to");
            }
            let index = self.branches.len() + 1;
            let zl = z[l];
            let mut obj = Obj::new(
                &format!("_virtual_branch.transformer.{name}_{l}"),
                &format!("_virtual_branch.transformer.{name}_{l}"),
                index,
            );
            obj.set("br_status", json!(status))
                .set("f_bus", json!(bus_ids[i]))
                .set("t_bus", json!(bus_ids[j]))
                .set("f_connections", ints(&hot))
                .set("t_connections", ints(&hot))
                .set("br_r", matrix(&diag(zl.re, nphases)))
                .set("br_x", matrix(&diag(zl.im, nphases)))
                .set("g_fr", matrix(&diag(g_fr, nphases)))
                .set("b_fr", matrix(&diag(b_fr, nphases)))
                .set("g_to", matrix(&diag(g_to, nphases)))
                .set("b_to", matrix(&diag(b_to, nphases)));
            if let Some(chain) = chain_of.get(l).cloned() {
                let (re, im) = (Rc::clone(&chain), chain);
                obj.with_cycle("br_r", move |pu, base| re(pu, base))
                    .with_cycle("br_x", move |pu, base| im(pu, base));
            }
            if let Some(side) = shunt_side {
                let (g, b) = (chains.no_load_conductance(), chains.no_load_susceptance());
                obj.with_cycle(&format!("g_{side}"), move |pu, base| g(pu, base))
                    .with_cycle(&format!("b_{side}"), move |pu, base| b(pu, base));
            }
            obj.set("angmin", nums(&vec![-10.0; nphases]))
                .set("angmax", nums(&vec![10.0; nphases]))
                .set("c_rating_a", nums(&vec![INF; nphases]))
                .set("shift", nums(&vec![0.0; nphases]))
                .set("tap", nums(&vec![1.0; nphases]))
                .set("switch", json!(false))
                .set("transformer", json!(false));
            self.branches.push(obj);
            to_map.push(format!("branch.{index}"));
        }
        tr_t_bus.iter().map(|bus| bus_ids[bus]).collect()
    }

    // ----- switches -----------------------------------------------------------

    #[allow(clippy::too_many_lines)] // one switch lowers in one pass: the switch, then its impedance branch
    fn switches(&mut self, net: &MulticonductorNetwork) {
        for s in net.switches() {
            let what = format!("switch {}", s.name);
            let index = self.switches.len() + 1;
            let f_bus = self.bus_of(&s.bus_from, &what);
            let t_bus = self.bus_of(&s.bus_to, &what);
            let f_connections = self.conns(&s.terminal_map_from, &what);
            let t_connections = self.conns(&s.terminal_map_to, &what);
            let n = f_connections.len();
            let mut obj = Obj::new(&s.name, &format!("switch.{}", s.name), index);
            obj.set("f_bus", json!(f_bus))
                .set("t_bus", json!(t_bus))
                .set("f_connections", ints(&f_connections))
                .set("t_connections", ints(&t_connections))
                .set("status", json!(status_of(&s.extras)))
                .set("state", json!(i64::from(!s.open)))
                .set("dispatchable", json!(1));
            // `_apply_linecode!`: a switch that names a linecode takes the
            // rating and impedance it does not state from it.
            let code = s
                .extras
                .get("linecode")
                .and_then(Value::as_str)
                .and_then(|name| net.linecode(name));
            let i_max = s
                .i_max
                .as_ref()
                .or_else(|| code.and_then(|c| c.i_max.as_ref()));
            let current: Vec<f64> = i_max.map_or_else(
                || vec![INF; n],
                |v| (0..n).map(|k| v.get(k).copied().unwrap_or(INF)).collect(),
            );
            obj.set("current_rating", nums(&current))
                .set("c_rating_b", nums(&vec![INF; n]))
                .set("c_rating_c", nums(&vec![INF; n]))
                .set("thermal_rating", nums(&vec![INF; n]))
                .set("rate_b", nums(&vec![INF; n]))
                .set("rate_c", nums(&vec![INF; n]));
            let mut to_map = vec![format!("switch.{index}")];
            // A switch with a series impedance (the ENGINEERING stash)
            // lowers to the switch plus a virtual bus and branch carrying
            // that impedance, PMD's loss model for switches.
            let rs = s
                .extras
                .get("pmd_rs")
                .and_then(read_matrix)
                .or_else(|| code.map(|c| c.r_series.clone()));
            let xs = s
                .extras
                .get("pmd_xs")
                .and_then(read_matrix)
                .or_else(|| code.map(|c| c.x_series.clone()));
            let nonzero = |m: &Option<ConductorMatrix>| {
                m.as_ref()
                    .is_some_and(|m| m.iter().flatten().any(|v| *v != 0.0))
            };
            if nonzero(&rs) || nonzero(&xs) {
                let length = extras_f64(&s.extras, "pmd_length").unwrap_or(1.0);
                let bus_index = self.buses.len() + 1;
                let count = t_connections.len();
                let mut bus = Obj::new(
                    &format!("_virtual_bus.switch.{}", s.name),
                    &format!("switch.{}", s.name),
                    bus_index,
                );
                bus.set("bus_i", json!(bus_index))
                    .set(
                        "bus_type",
                        json!(if status_of(&s.extras) == 0 { 4 } else { 1 }),
                    )
                    .set("terminals", ints(&t_connections))
                    .set("grounded", json!(vec![false; count]))
                    .set("vmin", nums(&vec![0.0; count]))
                    .set("vmax", nums(&vec![INF; count]))
                    .set("vm_pair_lb", json!([]))
                    .set("vm_pair_ub", json!([]));
                self.push_virtual_bus(bus);
                obj.set("t_bus", json!(bus_index));
                let branch_index = self.branches.len() + 1;
                let admittance = |key: &str| -> ConductorMatrix {
                    // The ENGINEERING shunt halves are capacitance in nF per
                    // meter; the branch carries siemens.
                    let omega = 2.0 * PI * self.settings.base_frequency * length / 1e9;
                    s.extras
                        .get(key)
                        .and_then(read_matrix)
                        .map_or_else(|| zeros(n), |m| slice(&scaled(&m, omega), n))
                };
                let mut branch = Obj::new(
                    &format!("_virtual_branch.switch.{}", s.name),
                    &format!("switch.{}", s.name),
                    branch_index,
                );
                branch
                    .set("f_bus", json!(bus_index))
                    .set("t_bus", json!(t_bus))
                    .set("f_connections", ints(&t_connections))
                    .set("t_connections", ints(&t_connections))
                    .set(
                        "br_r",
                        matrix(&slice(&scaled(&rs.unwrap_or_else(|| zeros(n)), length), n)),
                    )
                    .set(
                        "br_x",
                        matrix(&slice(&scaled(&xs.unwrap_or_else(|| zeros(n)), length), n)),
                    )
                    .set("g_fr", matrix(&admittance("pmd_g_fr")))
                    .set("g_to", matrix(&admittance("pmd_g_to")))
                    .set("b_fr", matrix(&admittance("pmd_b_fr")))
                    .set("b_to", matrix(&admittance("pmd_b_to")))
                    .set("angmin", nums(&vec![-10.0; n]))
                    .set("angmax", nums(&vec![10.0; n]))
                    .set("c_rating_a", nums(&vec![INF; n]))
                    .set("br_status", json!(status_of(&s.extras)));
                self.branches.push(branch);
                to_map.push(format!("bus.{bus_index}"));
                to_map.push(format!("branch.{branch_index}"));
            }
            self.switches.push(obj);
            let to = if to_map.len() == 1 {
                json!(to_map[0])
            } else {
                json!(to_map)
            };
            self.map
                .push(json!({"from": s.name, "to": to, "unmap_function": "_map_math2eng_switch!"}));
        }
    }

    // ----- shunts -------------------------------------------------------------

    fn shunts(&mut self, net: &MulticonductorNetwork) {
        for c in net.capacitors() {
            let what = format!("capacitor {}", c.name);
            match capacitor_susceptance(c) {
                Ok(bs) => {
                    let n = bs.len();
                    let connections = self.conns(&c.terminal_map, &what);
                    self.push_shunt(
                        &c.name,
                        &format!("shunt.{}", c.name),
                        &c.bus,
                        &connections,
                        &zeros(n),
                        &bs,
                        &c.extras,
                    );
                }
                Err(reason) => self.warn(
                    &C::EMIT_PMD_MATH_RECORD_DROPPED,
                    format!("{what}: {reason}; dropped"),
                ),
            }
        }
        for s in net.shunts() {
            let what = format!("shunt {}", s.name);
            let connections = self.conns(&s.terminal_map, &what);
            let n = connections.len();
            self.push_shunt(
                &s.name,
                &format!("shunt.{}", s.name),
                &s.bus,
                &connections,
                &slice(&s.g, n),
                &slice(&s.b, n),
                &s.extras,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_shunt(
        &mut self,
        name: &str,
        source_id: &str,
        bus: &str,
        connections: &[i64],
        gs: &ConductorMatrix,
        bs: &ConductorMatrix,
        extras: &Extras,
    ) {
        let index = self.shunts.len() + 1;
        let shunt_bus = self.bus_of(bus, &format!("shunt {name}"));
        let mut obj = Obj::new(name, source_id, index);
        obj.set("shunt_bus", json!(shunt_bus))
            .set("connections", ints(connections))
            .set("gs", matrix(gs))
            .set("bs", matrix(bs))
            .set("status", json!(status_of(extras)))
            .set("dispatchable", json!(0));
        self.shunts.push(obj);
        self.map.push(json!({"from": name, "to": format!("shunt.{index}"), "unmap_function": "_map_math2eng_shunt!"}));
    }

    // ----- loads --------------------------------------------------------------

    #[allow(clippy::too_many_lines)] // one load lowers in one pass: configuration, model, and ZIP parts
    fn loads(&mut self, net: &MulticonductorNetwork) {
        let buses: BTreeMap<String, &DistBus> = net
            .buses()
            .iter()
            .map(|b| (b.id.to_ascii_lowercase(), b))
            .collect();
        for load in net.loads() {
            let what = format!("load {}", load.name);
            let load_bus = self.bus_of(&load.bus, &what);
            let connections = self.conns(&load.terminal_map, &what);
            let configuration = match load.configuration {
                Configuration::Delta => "DELTA",
                Configuration::Wye => "WYE",
                Configuration::SinglePhase => {
                    // Two hot, ungrounded terminals are a phase to phase
                    // load; one hot terminal, with or without its neutral
                    // (gone after Kron reduction), is wye.
                    let grounded = buses
                        .get(load.bus.to_ascii_lowercase().as_str())
                        .map(|b| b.grounded.as_slice())
                        .unwrap_or_default();
                    let hot = load
                        .terminal_map
                        .iter()
                        .filter(|t| *t != "4" && !grounded.contains(t))
                        .count();
                    if hot == 2 { "DELTA" } else { "WYE" }
                }
            };
            let pd: Vec<f64> = load.p_nom.iter().map(|p| kw(*p, &self.settings)).collect();
            let qd: Vec<f64> = load.q_nom.iter().map(|q| kw(*q, &self.settings)).collect();
            // `vm_nom` as the ENGINEERING writer states it: the dss kv when it
            // matches, else the typed nominal voltage, scalar when uniform.
            let nominal = load.voltage_model.without_voltage_bounds();
            let v_nom = nominal.v_nom();
            let vnom_kv = crate::pmd::write::source_vm_nom(&load.extras, v_nom)
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_array().and_then(|a| a.first()).and_then(Value::as_f64))
                })
                .or_else(|| v_nom.first().map(|v| kv(*v, &self.settings)))
                .or_else(|| extras_f64(&load.extras, "kv"));
            // A load without a nominal voltage (a BMOPF load) takes its bus's
            // voltage base once the bases are known: a marker the per unit
            // pass replaces.
            let vnom_from_base = vnom_kv.is_none();
            if vnom_from_base {
                self.warn(
                    &C::EMIT_PMD_MATH_VALUE_DEFAULTED,
                    format!("{what}: no nominal voltage; the bus voltage base stands in as vm_nom"),
                );
            }
            let vnom_kv = vnom_kv.unwrap_or(0.0);
            if matches!(
                load.voltage_model,
                DistLoadVoltageModel::BoundedZip { .. }
                    | DistLoadVoltageModel::BoundedExponential { .. }
                    | DistLoadVoltageModel::BoundedConstantPower { .. }
            ) {
                self.warn(&C::EMIT_PMD_MATH_VALUE_SUBSTITUTED, format!("{what}: voltage band transitions, current limits and dropout have no MATHEMATICAL load field; the nominal law is lowered"));
            }
            let status = status_of(&load.extras);
            let push = |w: &mut Writer,
                        suffix: Option<&str>,
                        model: &str,
                        pd: Vec<f64>,
                        qd: Vec<f64>|
             -> String {
                let index = w.loads.len() + 1;
                let name =
                    suffix.map_or_else(|| load.name.clone(), |s| format!("{}_{s}", load.name));
                let mut obj = Obj::new(&name, &format!("load.{}", load.name), index);
                obj.set("load_bus", json!(load_bus))
                    .set("connections", ints(&connections))
                    .set("configuration", json!(configuration))
                    .set("model", json!(model))
                    .set("pd", nums(&pd))
                    .set("qd", nums(&qd))
                    .set("vnom_kv", num(vnom_kv))
                    .set("status", json!(status))
                    .set("dispatchable", json!(0));
                if vnom_from_base {
                    obj.set(VNOM_FROM_BASE, Value::Bool(true));
                }
                w.loads.push(obj);
                format!("load.{index}")
            };
            let to = match &nominal {
                DistLoadVoltageModel::Zip {
                    alpha_z,
                    alpha_i,
                    alpha_p,
                    beta_z,
                    beta_i,
                    beta_p,
                    ..
                } => {
                    // The ENGINEERING `zipv` stash (Z, I, P for P; Z, I, P
                    // for Q; cutoff) stands in for empty coefficient vectors.
                    let zipv: Vec<f64> = load
                        .extras
                        .get("zipv")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_f64).collect())
                        .unwrap_or_default();
                    let coeff = |typed: &[f64], k: usize| -> Vec<f64> {
                        if typed.is_empty() {
                            zipv.get(k).map_or_else(Vec::new, |v| vec![*v])
                        } else {
                            typed.to_vec()
                        }
                    };
                    let (alpha_z, alpha_i, alpha_p) =
                        (coeff(alpha_z, 0), coeff(alpha_i, 1), coeff(alpha_p, 2));
                    let (beta_z, beta_i, beta_p) =
                        (coeff(beta_z, 3), coeff(beta_i, 4), coeff(beta_p, 5));
                    let (alpha_z, alpha_i, alpha_p, beta_z, beta_i, beta_p) =
                        (&alpha_z, &alpha_i, &alpha_p, &beta_z, &beta_i, &beta_p);
                    let part = |coeffs: &[f64], base: &[f64]| -> Vec<f64> {
                        base.iter()
                            .enumerate()
                            .map(|(k, v)| {
                                v * coeffs.get(k).or(coeffs.first()).copied().unwrap_or(0.0)
                            })
                            .collect()
                    };
                    let to = vec![
                        push(
                            self,
                            Some(LOAD_MODEL_IMPEDANCE),
                            LOAD_MODEL_IMPEDANCE,
                            part(alpha_z, &pd),
                            part(beta_z, &qd),
                        ),
                        push(
                            self,
                            Some(LOAD_MODEL_CURRENT),
                            LOAD_MODEL_CURRENT,
                            part(alpha_i, &pd),
                            part(beta_i, &qd),
                        ),
                        push(
                            self,
                            Some(LOAD_MODEL_POWER),
                            LOAD_MODEL_POWER,
                            part(alpha_p, &pd),
                            part(beta_p, &qd),
                        ),
                    ];
                    json!(to)
                }
                DistLoadVoltageModel::ConstantImpedance { .. } => {
                    json!(push(self, None, LOAD_MODEL_IMPEDANCE, pd, qd))
                }
                DistLoadVoltageModel::ConstantCurrent { .. } => {
                    json!(push(self, None, LOAD_MODEL_CURRENT, pd, qd))
                }
                DistLoadVoltageModel::Exponential { .. } => {
                    self.warn(&C::EMIT_PMD_MATH_VALUE_SUBSTITUTED, format!("{what}: exponential load model has no MATHEMATICAL law; lowered as constant power"));
                    json!(push(self, None, LOAD_MODEL_POWER, pd, qd))
                }
                _ => json!(push(self, None, LOAD_MODEL_POWER, pd, qd)),
            };
            self.map.push(
                json!({"from": load.name, "to": to, "unmap_function": "_map_math2eng_load!"}),
            );
        }
    }

    // ----- generators ---------------------------------------------------------

    fn generators(&mut self, net: &MulticonductorNetwork) {
        for g in net.generators() {
            let what = format!("generator {}", g.name);
            let index = self.gens.len() + 1;
            let gen_bus = self.bus_of(&g.bus, &what);
            let connections = self.conns(&g.terminal_map, &what);
            // One bound per phase conductor, whether or not the dispatch is
            // stated.
            let phases = g.terminal_map.iter().filter(|t| *t != "4").count();
            let n = g.p_nom.len().max(phases).max(1);
            let settings = self.settings.clone();
            let bound = |v: &Option<Vec<f64>>, default: f64| -> Vec<f64> {
                v.as_ref().map_or_else(
                    || vec![default; n],
                    |v| {
                        (0..n)
                            .map(|k| v.get(k).map_or(default, |x| kw(*x, &settings)))
                            .collect()
                    },
                )
            };
            let mut obj = Obj::new(&g.name, &format!("generator.{}", g.name), index);
            obj.set("gen_bus", json!(gen_bus))
                .set("connections", ints(&connections))
                .set("configuration", json!(configuration(g.configuration)))
                .set("gen_status", json!(status_of(&g.extras)))
                .set("control_mode", json!(0))
                .set(
                    "pg",
                    nums(
                        &g.p_nom
                            .iter()
                            .map(|p| kw(*p, &self.settings))
                            .collect::<Vec<_>>(),
                    ),
                )
                .set(
                    "qg",
                    nums(
                        &g.q_nom
                            .iter()
                            .map(|q| kw(*q, &self.settings))
                            .collect::<Vec<_>>(),
                    ),
                )
                .set("pmin", nums(&bound(&g.p_min, -INF)))
                .set("pmax", nums(&bound(&g.p_max, INF)))
                .set("qmin", nums(&bound(&g.q_min, -INF)))
                .set("qmax", nums(&bound(&g.q_max, INF)));
            Self::gen_cost(&mut obj, g.cost.as_deref());
            self.gens.push(obj);
            self.mark_generator_bus(gen_bus, status_of(&g.extras), 0);
            self.map.push(json!({"from": g.name, "to": format!("gen.{index}"), "unmap_function": "_map_math2eng_generator!"}));
        }
    }

    /// PMD's default polynomial cost, `[1.0, 0.0]` per unit of power in $/MWh,
    /// which the per unit pass rescales to the power base. A typed $/kWh
    /// cost becomes the linear coefficient.
    fn gen_cost(obj: &mut Obj, cost: Option<&[f64]>) {
        let linear = cost
            .and_then(|c| c.first().copied())
            .map_or(1.0, |per_kwh| per_kwh * 1000.0);
        obj.set("model", json!(2))
            .set("startup", json!(0.0))
            .set("shutdown", json!(0.0))
            .set("cost", nums(&[linear, 0.0]))
            .set("ncost", json!(2));
    }

    /// `_compute_bus_type`: an in service generator makes its bus PV (2), an
    /// isochronous one the reference (3).
    fn mark_generator_bus(&mut self, bus: usize, status: i64, control_mode: i64) {
        let Some(obj) = self.buses.get_mut(bus - 1) else {
            return;
        };
        let current = obj
            .fields
            .get("bus_type")
            .and_then(Value::as_i64)
            .unwrap_or(1);
        if status == 1 && current != 4 {
            let next = if control_mode == 1 || current == 3 {
                3
            } else {
                2
            };
            obj.set("bus_type", json!(next));
        }
    }

    fn ibrs(&mut self, net: &MulticonductorNetwork) {
        for ibr in net.ibrs() {
            let what = format!("ibr {}", ibr.name);
            let index = self.gens.len() + 1;
            let gen_bus = self.bus_of(&ibr.bus, &what);
            let connections = self.conns(&ibr.terminal_map, &what);
            let hot = ibr.terminal_map.iter().filter(|t| *t != "4").count().max(1);
            let n = ibr.s_max.len().max(hot);
            let settings = self.settings.clone();
            let bound = |v: &Option<Vec<f64>>, default: f64| -> Vec<f64> {
                v.as_ref().map_or_else(
                    || vec![default; n],
                    |v| {
                        (0..n)
                            .map(|k| v.get(k).map_or(default, |x| kw(*x, &settings)))
                            .collect()
                    },
                )
            };
            let pg = ibr.p_avail.map_or_else(
                || vec![0.0; n],
                |p| vec![kw(p, &self.settings) / n as f64; n],
            );
            let kind = match ibr.prime_mover {
                crate::model::IbrPrimeMover::Pv => "solar",
                other => {
                    self.warn(
                        &C::EMIT_PMD_MATH_VALUE_SUBSTITUTED,
                        format!("{what}: prime mover {other:?} is lowered as a generator"),
                    );
                    "generator"
                }
            };
            let mut obj = Obj::new(&ibr.name, &format!("{kind}.{}", ibr.name), index);
            obj.set("gen_bus", json!(gen_bus))
                .set("connections", ints(&connections))
                .set("configuration", json!("WYE"))
                .set("gen_status", json!(status_of(&ibr.extras)))
                .set("control_mode", json!(0))
                .set("pg", nums(&pg))
                .set("qg", nums(&vec![0.0; n]))
                .set("pmin", nums(&bound(&ibr.p_min, -INF)))
                .set("pmax", nums(&bound(&ibr.p_max, INF)))
                .set("qmin", nums(&bound(&ibr.q_min, -INF)))
                .set("qmax", nums(&bound(&ibr.q_max, INF)));
            Self::gen_cost(&mut obj, None);
            self.gens.push(obj);
            self.mark_generator_bus(gen_bus, status_of(&ibr.extras), 0);
            if ibr.control_profile.is_some() {
                self.warn(
                    &C::EMIT_PMD_MATH_FIELD_DROPPED,
                    format!("{what}: inverter control profile has no MATHEMATICAL field; dropped"),
                );
            }
            let unmap = if kind == "solar" {
                "_map_math2eng_solar!"
            } else {
                "_map_math2eng_generator!"
            };
            self.map.push(
                json!({"from": ibr.name, "to": format!("gen.{index}"), "unmap_function": unmap}),
            );
        }
    }

    // ----- voltage sources ------------------------------------------------------

    #[allow(clippy::too_many_lines)] // one source lowers in one pass: the generator, then the optional Thevenin bus and branch
    fn sources(&mut self, net: &MulticonductorNetwork) {
        for vs in net.sources() {
            let what = format!("voltage source {}", vs.name);
            let index = self.gens.len() + 1;
            let gen_bus = self.bus_of(&vs.bus, &what);
            let connections = self.conns(&vs.terminal_map, &what);
            let nconductors = connections.len();
            let hot: Vec<usize> = vs
                .terminal_map
                .iter()
                .enumerate()
                .filter(|(_, t)| *t != "4")
                .map(|(k, _)| k)
                .collect();
            let nphases = if self.kron_reduced {
                nconductors
            } else {
                hot.len()
            }
            .max(1);
            let vm: Vec<f64> = (0..nconductors)
                .map(|k| {
                    kv(
                        vs.v_magnitude.get(k).copied().unwrap_or(0.0),
                        &self.settings,
                    )
                })
                .collect();
            let va: Vec<f64> = (0..nconductors)
                .map(|k| vs.v_angle.get(k).copied().unwrap_or(0.0).to_degrees())
                .collect();
            let status = status_of(&vs.extras);
            let mut obj = Obj::new(
                &format!("_virtual_gen.voltage_source.{}", vs.name),
                &format!("voltage_source.{}", vs.name),
                index,
            );
            obj.set("gen_bus", json!(gen_bus))
                .set("connections", ints(&connections))
                .set("configuration", json!("WYE"))
                .set("gen_status", json!(status))
                .set("control_mode", json!(1))
                .set("pg", nums(&vec![0.0; nphases]))
                .set("qg", nums(&vec![0.0; nphases]))
                .set("vg", nums(&vm))
                .set("pmin", nums(&vec![-INF; nphases]))
                .set("pmax", nums(&vec![INF; nphases]))
                .set("qmin", nums(&vec![-INF; nphases]))
                .set("qmax", nums(&vec![INF; nphases]));
            Self::gen_cost(&mut obj, vs.energy_cost_rate.as_deref());
            let rs = vs.extras.get("rs").and_then(read_matrix);
            let xs = vs.extras.get("xs").and_then(read_matrix);
            let nonzero = |m: &Option<ConductorMatrix>| {
                m.as_ref()
                    .is_some_and(|m| m.iter().flatten().any(|v| *v != 0.0))
            };
            let mut to_map = vec![format!("gen.{index}")];
            if nonzero(&rs) || nonzero(&xs) {
                // The Thevenin source: an ideal bus behind the impedance.
                let bus_index = self.buses.len() + 1;
                let grounded: Vec<bool> = self.buses[gen_bus - 1]
                    .fields
                    .get("grounded")
                    .and_then(Value::as_array)
                    .map_or_else(
                        || vec![false; nconductors],
                        |a| a.iter().map(|v| v.as_bool().unwrap_or(false)).collect(),
                    );
                let mut vm_bus = vm.clone();
                let mut vmin = vm.clone();
                let mut vmax = vm.clone();
                for (k, g) in grounded.iter().enumerate().take(nconductors) {
                    if *g {
                        vm_bus[k] = 0.0;
                        vmin[k] = 0.0;
                        vmax[k] = INF;
                    }
                }
                let mut bus = Obj::new(
                    &format!("_virtual_bus.voltage_source.{}", vs.name),
                    &format!("voltage_source.{}", vs.name),
                    bus_index,
                );
                bus.set("bus_i", json!(bus_index))
                    .set("terminals", ints(&connections))
                    .set("grounded", json!(grounded))
                    .set("bus_type", json!(if status == 0 { 4 } else { 3 }))
                    .set("vm", nums(&vm_bus))
                    .set("va", nums(&va))
                    .set("vmin", nums(&vmin))
                    .set("vmax", nums(&vmax))
                    .set("vm_pair_lb", json!([]))
                    .set("vm_pair_ub", json!([]));
                self.push_virtual_bus(bus);
                obj.set("gen_bus", json!(bus_index));
                let branch_index = self.branches.len() + 1;
                let mut branch = Obj::new(
                    &format!("_virtual_branch.voltage_source.{}", vs.name),
                    &format!("voltage_source.{}", vs.name),
                    branch_index,
                );
                branch
                    .set("f_bus", json!(bus_index))
                    .set("t_bus", json!(gen_bus))
                    .set("f_connections", ints(&connections))
                    .set("t_connections", ints(&connections))
                    .set("angmin", nums(&vec![-10.0; nconductors]))
                    .set("angmax", nums(&vec![10.0; nconductors]))
                    .set("c_rating_a", nums(&vec![INF; nconductors]))
                    .set("br_status", json!(status))
                    .set(
                        "br_r",
                        matrix(&slice(
                            &rs.unwrap_or_else(|| zeros(nconductors)),
                            nconductors,
                        )),
                    )
                    .set(
                        "br_x",
                        matrix(&slice(
                            &xs.unwrap_or_else(|| zeros(nconductors)),
                            nconductors,
                        )),
                    )
                    .set("g_fr", matrix(&zeros(nconductors)))
                    .set("g_to", matrix(&zeros(nconductors)))
                    .set("b_fr", matrix(&zeros(nconductors)))
                    .set("b_to", matrix(&zeros(nconductors)));
                self.branches.push(branch);
                to_map.push(format!("bus.{bus_index}"));
                to_map.push(format!("branch.{branch_index}"));
            } else {
                // The source fixes the bus voltage on its own conductors:
                // each phasor lands at its connection's position among the
                // bus terminals, and several single conductor sources at one
                // bus (one ideal source per phase) fill it between them.
                let bus = &mut self.buses[gen_bus - 1];
                let terminals: Vec<i64> = bus.connections("terminals");
                let count = if terminals.is_empty() {
                    nconductors
                } else {
                    terminals.len()
                };
                let existing = |key: &str, fill: f64| -> Vec<f64> {
                    let stated = bus
                        .fields
                        .get(key)
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .map(|v| v.as_f64().unwrap_or(fill))
                                .collect::<Vec<f64>>()
                        })
                        .filter(|a| a.len() == count);
                    stated.unwrap_or_else(|| vec![fill; count])
                };
                let (mut vmin, mut vmax, mut vm_bus, mut va_bus) = (
                    existing("vmin", 0.0),
                    existing("vmax", INF),
                    existing("vm", 0.0),
                    existing("va", 0.0),
                );
                for (k, conn) in connections.iter().enumerate() {
                    let pos = terminals.iter().position(|t| t == conn).unwrap_or(k);
                    if pos < count {
                        vmin[pos] = vm[k];
                        vmax[pos] = vm[k];
                        vm_bus[pos] = vm[k];
                        va_bus[pos] = va[k];
                    }
                }
                bus.set("vmin", nums(&vmin))
                    .set("vmax", nums(&vmax))
                    .set("vm", nums(&vm_bus))
                    .set("va", nums(&va_bus));
                self.mark_generator_bus(gen_bus, status, 1);
            }
            if vs.reference_terminal.is_some() {
                self.warn(&C::EMIT_PMD_MATH_FIELD_DROPPED, format!("{what}: a declared reference terminal has no MATHEMATICAL field; the source is lowered earth referenced"));
            }
            self.gens.push(obj);
            let to = if to_map.len() == 1 {
                json!(to_map[0])
            } else {
                json!(to_map)
            };
            self.map.push(json!({"from": vs.name, "to": to, "unmap_function": "_map_math2eng_voltage_source!"}));
        }
    }

    // ----- per unit and assembly -------------------------------------------------

    /// The per unit pass and the document: `calc_voltage_bases` floods the
    /// stated bases across lines and switches and through the ideal
    /// transformers' ratios, then every object rebases as
    /// `_make_math_per_unit!` does.
    #[allow(clippy::too_many_lines)] // one per unit pass over every table, then the document
    fn finish(&mut self, net: &MulticonductorNetwork) -> Map<String, Value> {
        let vbase = self.voltage_bases(net);
        let sbase = self.settings.sbase;
        let vsf = self.settings.voltage_scale_factor;
        let psf = self.settings.power_scale_factor;
        // Each cycle is the reader's inverse (per unit back to SI) followed
        // by this writer's conversion, exactly as both compute it.
        let power = move |pu: f64| ((pu * sbase) * psf) / psf / sbase;
        let voltage = move |base: f64| move |pu: f64| ((pu * base * vsf) / vsf) / base;
        let impedance = move |z_new: f64| move |pu: f64| (pu * z_new) / z_new;
        let admittance = move |z_new: f64| move |pu: f64| (pu / z_new) * z_new;
        let current = move |base: f64| move |pu: f64| (pu * sbase / base) * base / sbase;
        let angle = |rad: f64| rad.to_degrees().to_radians();
        for (k, bus) in self.buses.iter_mut().enumerate() {
            let base = vbase[k];
            for key in ["vm", "vmin", "vmax"] {
                bus.scale(key, |kv| kv / base, voltage(base));
            }
            bus.map_numbers("va", f64::to_radians, angle);
            // Grounding impedance rebases on the bus impedance base.
            let z_new = base * base / sbase * vsf;
            for key in ["rg", "xg"] {
                bus.scale(key, |z| z / z_new, impedance(z_new));
            }
            bus.set("vbase", num(base));
        }
        for branch in &mut self.branches {
            let f_bus = branch.usize("f_bus").unwrap_or(1);
            let base = vbase[f_bus - 1];
            let z_new = base * base / sbase * vsf;
            for key in ["br_r", "br_x"] {
                branch.scale_on_base(key, z_new, |z| z / z_new, impedance(z_new));
            }
            for key in ["b_fr", "g_fr", "b_to", "g_to"] {
                branch.scale_on_base(key, z_new, |y| y * z_new, admittance(z_new));
            }
            for key in ["c_rating_a", "c_rating_b", "c_rating_c"] {
                branch.scale(key, |a| a * base / sbase, current(base));
            }
            for key in ["rate_a", "rate_b", "rate_c"] {
                branch.scale(key, |kw| kw / sbase, power);
            }
            branch.map_numbers("angmin", f64::to_radians, angle);
            branch.map_numbers("angmax", f64::to_radians, angle);
            branch.set("vbase", num(base));
        }
        for shunt in &mut self.shunts {
            let bus = shunt.usize("shunt_bus").unwrap_or(1);
            let base = vbase[bus - 1];
            let z_new = base * base / sbase * vsf;
            shunt.scale("gs", |y| y * z_new, admittance(z_new));
            shunt.scale("bs", |y| y * z_new, admittance(z_new));
            shunt.set("vbase", num(base));
        }
        for load in &mut self.loads {
            let bus = load.usize("load_bus").unwrap_or(1);
            let base = vbase[bus - 1];
            if load.fields.remove(VNOM_FROM_BASE).is_some() {
                // The bus base is line to neutral; a delta load's nominal
                // voltage is line to line.
                let delta =
                    load.fields.get("configuration").and_then(Value::as_str) == Some("DELTA");
                load.set(
                    "vnom_kv",
                    num(if delta { base * 3f64.sqrt() } else { base }),
                );
            }
            load.scale("vnom_kv", |kv| kv / base, voltage(base));
            load.scale("pd", |kw| kw / sbase, power);
            load.scale("qd", |kw| kw / sbase, power);
            load.set("vbase", num(base));
        }
        // A ZIP load's three parts are re-derived by the reader from their
        // sum, so they reach their fixed point together.
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, load) in self.loads.iter().enumerate() {
            if let Some(source_id) = load.fields.get("source_id").and_then(Value::as_str) {
                groups.entry(source_id.to_owned()).or_default().push(i);
            }
        }
        for indices in groups.values().filter(|g| g.len() == 3) {
            for key in ["pd", "qd"] {
                let phases = self.loads[indices[0]]
                    .connections("connections")
                    .len()
                    .max(1);
                for k in 0..phases {
                    let mut parts: Vec<f64> = indices
                        .iter()
                        .map(|&i| {
                            self.loads[i].fields[key]
                                .as_array()
                                .and_then(|a| a.get(k))
                                .and_then(Value::as_f64)
                                .unwrap_or(0.0)
                        })
                        .collect();
                    for _ in 0..4 {
                        let watts: Vec<f64> = parts.iter().map(|pu| (pu * sbase) * psf).collect();
                        let total: f64 = watts.iter().sum();
                        if total == 0.0 {
                            break;
                        }
                        let next: Vec<f64> = watts
                            .iter()
                            .map(|w| ((total / psf) * (w / total)) / sbase)
                            .collect();
                        if next
                            .iter()
                            .zip(&parts)
                            .all(|(a, b)| a.to_bits() == b.to_bits())
                        {
                            break;
                        }
                        parts = next;
                    }
                    for (&i, pu) in indices.iter().zip(parts) {
                        if let Some(Value::Array(values)) = self.loads[i].fields.get_mut(key)
                            && let Some(slot) = values.get_mut(k)
                        {
                            *slot = num(pu);
                        }
                    }
                }
            }
        }
        for g in &mut self.gens {
            let bus = g.usize("gen_bus").unwrap_or(1);
            let base = vbase[bus - 1];
            for key in ["pg", "qg", "pmin", "qmin", "pmax", "qmax"] {
                g.scale(key, |kw| kw / sbase, power);
            }
            // PMD's `_rebase_pu_generator!`: the old base is
            // `1 / voltage_scale_factor`, so a kV setpoint lands at
            // kV / (vbase * voltage_scale_factor).
            g.scale("vg", |kv| kv * ((1.0 / vsf) / base), |pu| pu);
            // Costs start in $/MWh: the old base is 1e6 / power_scale_factor.
            let sbase_old_cost = 1e6 / psf;
            g.scale(
                "cost",
                move |mwh| mwh * sbase / sbase_old_cost,
                move |pu| {
                    (((pu * sbase_old_cost / sbase) / 1000.0) * 1000.0) * sbase / sbase_old_cost
                },
            );
            g.set("vbase", num(base));
        }
        for switch in &mut self.switches {
            let f_bus = switch.usize("f_bus").unwrap_or(1);
            let base = vbase[f_bus - 1];
            for key in ["current_rating", "c_rating_b", "c_rating_c"] {
                switch.scale(key, |a| a * base / sbase, current(base));
            }
            for key in ["thermal_rating", "rate_b", "rate_c"] {
                switch.scale(key, |kw| kw / sbase, power);
            }
            switch.set("vbase", num(base));
        }
        for k in 0..self.transformers.len() {
            let f_bus = self.transformers[k].usize("f_bus").unwrap_or(1);
            let t_bus = self.transformers[k].usize("t_bus").unwrap_or(1);
            let (f_vbase, t_vbase) = (vbase[f_bus - 1], vbase[t_bus - 1]);
            let delta = self.transformers[k]
                .fields
                .get("configuration")
                .and_then(Value::as_str)
                == Some("DELTA");
            // The reader takes the ratio back to kV, to volts, and the next
            // write to kV and the ratio again. The voltage it recovers is
            // recorded, so the lowering can run again from it.
            if let Some(unit) = self.units.iter().find(|u| u.index == k) {
                let recovered = |v: f64| {
                    let kv = v / vsf;
                    let kv_ll = if delta { kv * 3f64.sqrt() } else { kv };
                    let pu = kv_ll * ((1.0 / f_vbase) / (1.0 / t_vbase));
                    let kv_ll2 = pu * f_vbase / t_vbase;
                    let kv2 = if delta { kv_ll2 / 3f64.sqrt() } else { kv_ll2 };
                    kv2 * vsf
                };
                let v = stabilize(unit.v_ref, &recovered);
                let entry = self.snap_next.entry(unit.transformer.clone()).or_default();
                if entry.len() <= unit.winding {
                    entry.resize(unit.winding + 1, 0.0);
                }
                entry[unit.winding] = v;
            }
            let xf = &mut self.transformers[k];
            xf.scale(
                "tm_nom",
                move |kv_ll| kv_ll * ((1.0 / f_vbase) / (1.0 / t_vbase)),
                move |pu| {
                    let kv_ll = pu * f_vbase / t_vbase;
                    let kv = if delta { kv_ll / 3f64.sqrt() } else { kv_ll };
                    let v_ref = kv * vsf;
                    let kv2 = v_ref / vsf;
                    let kv_ll2 = if delta { kv2 * 3f64.sqrt() } else { kv2 };
                    kv_ll2 * ((1.0 / f_vbase) / (1.0 / t_vbase))
                },
            );
            xf.scale("sm_ub", |kw| kw / sbase, power);
            xf.set("f_vbase", num(f_vbase)).set("t_vbase", num(t_vbase));
        }

        let mut settings = self.settings.raw.clone();
        settings.insert("sbase".into(), num(sbase));
        let vbases_default: Map<String, Value> = self
            .settings
            .vbases_default
            .iter()
            .filter_map(|(bus, kv)| Some((self.bus_index.get(bus)?.to_string(), num(*kv))))
            .collect();
        settings.insert("vbases_default".into(), Value::Object(vbases_default));

        let max_conductor = self
            .buses
            .iter()
            .flat_map(|b| b.connections("terminals"))
            .max()
            .unwrap_or(3)
            .clamp(1, 64);
        let table = |objs: &[Obj]| -> Value {
            Value::Object(
                objs.iter()
                    .map(|o| {
                        (
                            o.usize("index").unwrap_or(0).to_string(),
                            Value::Object(o.fields.clone()),
                        )
                    })
                    .collect(),
            )
        };
        let bus_lookup: Map<String, Value> = net
            .buses()
            .iter()
            .filter_map(|b| {
                Some((
                    b.id.clone(),
                    json!(self.bus_index.get(&b.id.to_ascii_lowercase())?),
                ))
            })
            .collect();
        let mut doc = Map::new();
        doc.insert("data_model".into(), json!("MATHEMATICAL"));
        doc.insert(
            "name".into(),
            json!(net.name().clone().unwrap_or_default().to_lowercase()),
        );
        doc.insert("per_unit".into(), json!(true));
        doc.insert("is_projected".into(), json!(false));
        doc.insert("is_kron_reduced".into(), json!(self.kron_reduced));
        doc.insert("settings".into(), Value::Object(settings));
        doc.insert("time_elapsed".into(), json!(1.0));
        doc.insert(
            "conductor_ids".into(),
            Value::Array((1..=max_conductor).map(|i| json!(i)).collect()),
        );
        doc.insert("bus_lookup".into(), Value::Object(bus_lookup));
        doc.insert("map".into(), Value::Array(std::mem::take(&mut self.map)));
        doc.insert("bus".into(), table(&self.buses));
        doc.insert("branch".into(), table(&self.branches));
        doc.insert("switch".into(), table(&self.switches));
        doc.insert("transformer".into(), table(&self.transformers));
        doc.insert("gen".into(), table(&self.gens));
        doc.insert("load".into(), table(&self.loads));
        doc.insert("shunt".into(), table(&self.shunts));
        doc.insert("storage".into(), json!({}));
        for u in net.untyped_objects() {
            self.warn(
                &C::EMIT_PMD_MATH_RECORD_DROPPED,
                format!(
                    "{} {}: class is not lowered to the MATHEMATICAL model; dropped",
                    u.class, u.name
                ),
            );
        }
        doc
    }

    /// `calc_voltage_bases`: buses joined by branches or switches share a
    /// base, an ideal transformer scales it by its nominal ratio, and the
    /// stated bases seed the flood.
    ///
    /// # Errors
    /// A bus no stated base reaches: the network has an island the settings
    /// do not cover.
    #[allow(clippy::too_many_lines)] // zones, seeds, the flood through the ratios, then the fallbacks
    fn voltage_bases(&mut self, net: &MulticonductorNetwork) -> Vec<f64> {
        fn find(parent: &mut [usize], i: usize) -> usize {
            let mut i = i;
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let n = self.buses.len();
        let mut parent: Vec<usize> = (0..n).collect();
        let mut union = |a: usize, b: usize| {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent[ra] = rb;
            }
        };
        for obj in self.branches.iter().chain(&self.switches) {
            if let (Some(f), Some(t)) = (obj.usize("f_bus"), obj.usize("t_bus")) {
                union(f - 1, t - 1);
            }
        }
        let zone: Vec<usize> = (0..n).map(|i| find(&mut parent, i)).collect();
        let mut zone_base: BTreeMap<usize, f64> = BTreeMap::new();
        for (bus, kv) in &self.settings.vbases_default {
            if let Some(&index) = self.bus_index.get(bus) {
                zone_base.insert(zone[index - 1], *kv);
            }
        }
        // A base a MATHEMATICAL document stated for a bus (kept by the
        // reader as `pmd_vbase`) seeds its zone too, so the flood
        // reproduces the document's bases rather than recomputing them
        // through the transformer ratios.
        for (i, bus) in net.buses().iter().enumerate() {
            if let Some(vbase) = bus.extras.get("pmd_vbase").and_then(Value::as_f64)
                && vbase.is_finite()
                && vbase > 0.0
            {
                zone_base.entry(zone[i]).or_insert(vbase);
            }
        }
        // Transformer edges between zones, with the ratio from the from side
        // to the to side.
        let mut edges: BTreeMap<usize, Vec<(usize, f64)>> = BTreeMap::new();
        for xf in &self.transformers {
            let (Some(f), Some(t), Some(tm_nom)) =
                (xf.usize("f_bus"), xf.usize("t_bus"), xf.f("tm_nom"))
            else {
                continue;
            };
            let tm_nom = if xf.fields.get("configuration").and_then(Value::as_str) == Some("DELTA")
            {
                tm_nom / 3f64.sqrt()
            } else {
                tm_nom
            };
            if !(tm_nom.is_finite() && tm_nom > 0.0) {
                continue;
            }
            edges
                .entry(zone[f - 1])
                .or_default()
                .push((zone[t - 1], 1.0 / tm_nom));
            edges
                .entry(zone[t - 1])
                .or_default()
                .push((zone[f - 1], tm_nom));
        }
        let mut stack: Vec<usize> = zone_base.keys().copied().collect();
        while let Some(from) = stack.pop() {
            let base = zone_base[&from];
            for (to, scale) in edges.get(&from).cloned().unwrap_or_default() {
                if let std::collections::btree_map::Entry::Vacant(entry) = zone_base.entry(to) {
                    entry.insert(base * scale);
                    stack.push(to);
                }
            }
        }
        // An island no stated base reaches (a transformer a previous hop
        // dropped) takes the nominal voltage of a load in it, else 1 kV, and
        // says so: PMD's `calc_voltage_bases` stops at such an island.
        let mut fallback: Vec<String> = Vec::new();
        for (i, bus) in self.buses.iter().enumerate() {
            if zone_base.contains_key(&zone[i]) {
                continue;
            }
            let name = bus
                .fields
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned();
            let from_load = self.loads.iter().find_map(|load| {
                let at = load.usize("load_bus")?;
                if zone[at - 1] != zone[i] || load.fields.contains_key(VNOM_FROM_BASE) {
                    return None;
                }
                let kv = load.f("vnom_kv").filter(|kv| kv.is_finite() && *kv > 0.0)?;
                let delta =
                    load.fields.get("configuration").and_then(Value::as_str) == Some("DELTA");
                let load_name = load
                    .fields
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_owned();
                Some((if delta { kv / 3f64.sqrt() } else { kv }, load_name))
            });
            let (base, stands_in) = from_load.map_or_else(
                || (1.0, "1 kV".to_owned()),
                |(kv, load)| (kv, format!("load `{load}`'s nominal voltage")),
            );
            zone_base.insert(zone[i], base);
            fallback.push(format!("bus `{name}`: {stands_in}"));
        }
        for what in fallback {
            self.warn(
                &C::EMIT_PMD_MATH_VALUE_DEFAULTED,
                format!(
                    "{what} stands in as the voltage base of an island no stated base reaches \
                     (settings.vbases_default names {} bus(es)); PMD's calc_voltage_bases \
                     would stop here",
                    self.settings.vbases_default.len()
                ),
            );
        }
        self.buses
            .iter()
            .enumerate()
            .map(|(i, _)| zone_base[&zone[i]])
            .collect()
    }
}

/// The read and rewrite chains of a transformer loss model's values: what
/// the MATHEMATICAL reader recovers (a percentage on the winding base) and
/// what this writer makes of it again, in the reader's and the writer's own
/// operations, so the per unit pass can iterate each value to a fixed point.
struct LossChains {
    /// Rated winding voltages, volts.
    vnom: Vec<f64>,
    /// Rated winding powers, volt amperes.
    snom: Vec<f64>,
    /// `vnom` in kV: the referral ratios to the internal 1 kV side.
    ratios: Vec<f64>,
}

impl LossChains {
    /// Winding `w`'s resistance: per unit on the internal base, through
    /// `r_pct` on the winding base and back.
    fn winding_resistance(&self, w: usize) -> Cycle {
        let (v, s, ratio) = (self.vnom[w], self.snom[w], self.ratios[w]);
        Rc::new(move |pu: f64, base: f64| {
            let z_w = v * v / s;
            let r_pct = ((pu * base) * ratio * ratio) / z_w * 100.0;
            (r_pct / 100.0 * z_w / (ratio * ratio)) / base
        })
    }

    /// The single pair impedance of a two winding unit: through `xsc_pct`
    /// on the first winding's base and both star conversions.
    fn pair_reactance(&self) -> Cycle {
        let (v, s, ratio) = (self.vnom[0], self.snom[0], self.ratios[0]);
        Rc::new(move |pu: f64, base: f64| {
            let z1 = v * v / s;
            let mut zbr = BTreeMap::new();
            zbr.insert((0usize, 1usize), Complex64::new(0.0, pu * base));
            let zsc = br2sc_impedance(&zbr, 2);
            let x = zsc.get(&(0, 1)).map_or(0.0, |z| z.im);
            let xsc_pct = x * ratio * ratio / z1 * 100.0;
            let mut zsc = BTreeMap::new();
            zsc.insert(
                (1usize, 2usize),
                Complex64::new(0.0, xsc_pct / 100.0 * z1 / (ratio * ratio)),
            );
            let zbr = sc2br_impedance(&zsc, 2);
            zbr.get(&(1, 2)).map_or(0.0, |z| z.im) / base
        })
    }

    /// The no load conductance: through `%noloadloss` and back.
    fn no_load_conductance(&self) -> Cycle {
        let (v, s, ratio) = (self.vnom[0], self.snom[0], self.ratios[0]);
        Rc::new(move |pu: f64, base: f64| {
            let g_sh = (pu / base) / (ratio * ratio);
            let pct = g_sh * v * v / s * 100.0;
            let g_sh = pct / 100.0 * s / (v * v);
            g_sh * ratio * ratio * base
        })
    }

    /// The magnetizing susceptance: through `%imag` and back.
    fn no_load_susceptance(&self) -> Cycle {
        let (v, s, ratio) = (self.vnom[0], self.snom[0], self.ratios[0]);
        Rc::new(move |pu: f64, base: f64| {
            let b_sh = (pu / base) / (ratio * ratio);
            let pct = -b_sh * v * v / s * 100.0;
            let b_sh = -(pct / 100.0 * s) / (v * v);
            b_sh * ratio * ratio * base
        })
    }
}

/// PMD's `_sc2br_impedance`: the short circuit impedances between winding
/// pairs become the branch impedances of a star connected loss model. For
/// two windings the pair impedance is the branch; for more, the pairwise
/// values build the reduced bus impedance matrix whose inverse gives the
/// branch admittances.
fn sc2br_impedance(
    zsc: &BTreeMap<(usize, usize), Complex64>,
    n: usize,
) -> BTreeMap<(usize, usize), Complex64> {
    if zsc.values().all(|z| *z == Complex64::default()) {
        return zsc.clone();
    }
    let m = n - 1;
    let at = |i: usize, j: usize| -> Complex64 {
        zsc.get(&(i.min(j), i.max(j))).copied().unwrap_or_default()
    };
    let mut zb = vec![vec![Complex64::default(); m]; m];
    for (i, row) in zb.iter_mut().enumerate() {
        row[i] = at(1, i + 2);
    }
    for i in 0..m {
        for j in 0..i {
            zb[i][j] = (zb[i][i] + zb[j][j] - at(j + 2, i + 2)) / 2.0;
            zb[j][i] = zb[i][j];
        }
    }
    let Some(y) = invert(&zb) else {
        // A singular reduced matrix has no star equivalent; the pairs are
        // carried as stated, PMD's pseudo-inverse giving the same for the
        // nonsingular part is not reproduced here.
        return zsc.clone();
    };
    // Y_full = [[-1' Y 1, -1' Y], [-Y 1, Y]], the (n x n) admittance over
    // the first winding's node and the others.
    let mut full = vec![vec![Complex64::default(); n]; n];
    for i in 0..m {
        for j in 0..m {
            full[i + 1][j + 1] = y[i][j];
        }
    }
    for i in 0..m {
        let row: Complex64 = y[i].iter().sum();
        full[i + 1][0] = -row;
        full[0][i + 1] = -y.iter().map(|r| r[i]).sum::<Complex64>();
    }
    full[0][0] = y.iter().flatten().sum();
    zsc.keys()
        .map(|&(i, j)| {
            let yij = full[i - 1][j - 1];
            let z = if yij.norm() == 0.0 {
                Complex64::new(INF, INF)
            } else {
                -Complex64::new(1.0, 0.0) / yij
            };
            ((i, j), z)
        })
        .collect()
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

/// A stashed PMD matrix (array of columns) or a plain row major array.
fn read_matrix(v: &Value) -> Option<ConductorMatrix> {
    let cols = v.as_array()?;
    let n = super::read::matrix_dimension(v)?;
    let mut m = vec![vec![0.0; n]; n];
    for (j, col) in cols.iter().enumerate() {
        for (i, x) in col.as_array()?.iter().enumerate().take(n) {
            m[i][j] = x.as_f64().unwrap_or(0.0);
        }
    }
    Some(m)
}

fn extras_f64(extras: &Extras, key: &str) -> Option<f64> {
    extras.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

//! Solver-independent preparation for multiconductor rectangular IVR OPF.
//!
//! All currents are retained, including at exact zero series impedance. No
//! admittance inversion, neutral reduction, or switch contraction takes place.
//! Canonical instance inputs use SI; coefficients use explicitly chosen bases.
//! This is an exact AC IVR preparation profile, not a network validity test or a
//! solver. Rejection of an ideal cycle, initial point, or angle window indicates
//! a profile limitation, not electrical infeasibility. See the MC IVR guide for
//! supported equipment and the downstream expression/derivative boundary.
use crate::diagnostics::codes;
use powerio_core::Error;
use powerio_dist::{Configuration, DistLoadVoltageModel, MulticonductorNetwork};
use powerio_prob::{ConstraintSelection, McAcOpfInstance, ObjectiveTerm};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod inverter;
mod transformer;
pub use transformer::{
    McOpfComplexRow, McOpfTransformer, McOpfTransformerCoil, McOpfTransformerPort,
};

type Positions = BTreeMap<(String, String), usize>;
type CoilPairs = Vec<(usize, Option<usize>)>;
type Result<T> = std::result::Result<T, Error>;

/// Explicit system bases; no physical bound is inferred from these values.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McAcOpfAssemblyOptions {
    /// Voltage base in volts, shared by terminal and coil voltages.
    pub voltage_base_v: f64,
    /// Power base in VA per coil; I_base = S_base / V_base.
    pub power_base_va: f64,
}
impl McAcOpfAssemblyOptions {
    /// Choose voltage (V) and power (VA) bases. The preparation builder validates
    /// that both and their derived current/impedance bases are finite and positive.
    #[must_use]
    pub const fn new(voltage_base_v: f64, power_base_va: f64) -> Self {
        Self {
            voltage_base_v,
            power_base_va,
        }
    }
}
impl Default for McAcOpfAssemblyOptions {
    fn default() -> Self {
        Self {
            voltage_base_v: 230.0,
            power_base_va: 1000.0,
        }
    }
}
/// A source terminal in canonical bus/terminal order.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McOpfTerminal {
    /// Canonical bus name.
    pub bus: String,
    /// Canonical terminal label within the bus.
    pub terminal: String,
    /// True for an explicit physical ground.
    pub grounded: bool,
    /// Prescribed rectangular [real, imaginary] voltage in per unit, when fixed.
    pub fixed: Option<[f64; 2]>,
    /// Rectangular per-unit voltage seed; not an additional physical reference.
    pub start: [f64; 2],
}
/// A magnitude bound on a difference of terminal voltages, in per unit.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McOpfVoltageLimit {
    /// Stable diagnostic identity derived from the source limit.
    pub identity: String,
    /// Index into preparation.terminals for ordinary voltage differences.
    pub positive: usize,
    /// Return terminal index, or physical ground when absent.
    pub negative: Option<usize>,
    /// Optional per-unit magnitude lower bound, not squared.
    pub lower: Option<f64>,
    /// Optional per-unit magnitude upper bound, not squared.
    pub upper: Option<f64>,
    /// For sequence limits: (terminal index, complex coefficient) of the measured voltage; empty for ordinary differences.
    pub combination: Vec<(usize, [f64; 2])>,
}
/// Centered angle of V[first] * conj(V[second]), in radians.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McOpfAngleLimit {
    /// Source limit identity.
    pub identity: String,
    /// First index into preparation.terminals.
    pub first: usize,
    /// Second index into preparation.terminals.
    pub second: usize,
    /// Reference rotation in radians subtracted from the measured angle.
    pub offset: f64,
    /// Lower bound after reference rotation, radians.
    pub lower: f64,
    /// Upper bound after reference rotation, radians.
    pub upper: f64,
}
/// A line or ideal switch, with dense *local* conductor matrices.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McOpfBranch {
    /// Source kind and name (`line:name` or `switch:name`).
    pub identity: String,
    /// From-terminal global indices, in local conductor order.
    pub from: Vec<usize>,
    /// To-terminal global indices paired with from.
    pub to: Vec<usize>,
    /// Local square series resistance matrix divided by Z_base.
    pub r: Vec<Vec<f64>>,
    /// Local square series reactance matrix divided by Z_base.
    pub x: Vec<Vec<f64>>,
    /// Local square from-end conductance matrix multiplied by Z_base.
    pub g_from: Vec<Vec<f64>>,
    /// Local square from-end susceptance matrix multiplied by Z_base.
    pub b_from: Vec<Vec<f64>>,
    /// Local square to-end conductance matrix multiplied by Z_base.
    pub g_to: Vec<Vec<f64>>,
    /// Local square to-end susceptance matrix multiplied by Z_base.
    pub b_to: Vec<Vec<f64>>,
    /// Optional current magnitude cap per conductor, per unit.
    pub current_max: Vec<Option<f64>>,
    /// Optional apparent-power cap per conductor, per unit.
    pub apparent_max: Vec<Option<f64>>,
    /// Open contacts carry no series current.
    pub open: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McOpfShunt {
    /// Source shunt or synthesized capacitor/grounding identity.
    pub identity: String,
    /// Global terminal indices defining the local matrix axis.
    pub terminals: Vec<usize>,
    /// Local square conductance matrix multiplied by Z_base.
    pub g: Vec<Vec<f64>>,
    /// Local square susceptance matrix multiplied by Z_base.
    pub b: Vec<Vec<f64>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Distinguishes demand (positive consumption) from injection device tables.
#[non_exhaustive]
pub enum McOpfDeviceKind {
    Load,
    Generator,
    Ibr,
    Source,
}
/// A sum of voltage power laws: coefficient times (|U|/nominal)^exponent.
/// Coefficients are per-unit power; exponents are dimensionless.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McOpfLoadLaw {
    /// Coil nominal voltage divided by V_base.
    pub nominal: f64,
    /// Active then reactive terms; each [coefficient, exponent] contributes coefficient * (|U| / nominal)^exponent in per-unit power.
    pub terms: [Vec<[f64; 2]>; 2],
}
/// Smooth clipped piecewise-linear controller, with SI-derived per-unit knots.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McOpfDroop {
    /// Voltage-difference terminal pairs; None means physical ground. Multiple pairs use averaged magnitudes.
    pub monitors: Vec<(usize, Option<usize>)>,
    /// Strictly increasing voltage knots divided by V_base.
    pub knots: Vec<f64>,
    /// Control power at each knot, divided by S_base.
    pub values: Vec<f64>,
    /// Positive softplus smoothing width in per-unit voltage.
    pub epsilon: f64,
}
/// One two-terminal coil; a missing negative terminal means physical ground.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McOpfCoil {
    /// Positive global terminal index; current points from positive to negative.
    pub positive: usize,
    /// Negative global terminal index, or physical ground.
    pub negative: Option<usize>,
    /// Fixed [P,Q] per-unit power: consumption for loads, injection otherwise.
    pub prescribed: Option<[f64; 2]>,
    /// Voltage-dependent demand, normalized to its physical coil voltage.
    pub load_law: Option<McOpfLoadLaw>,
    /// Signed Q/P ratio for fixed power factor.
    pub reactive_slope: Option<f64>,
    /// Reactive injection as a smoothed function of monitored voltage.
    pub volt_var: Option<McOpfDroop>,
    /// Active injection as a smoothed function of monitored voltage.
    pub volt_watt: Option<McOpfDroop>,
    /// Optional active injection lower bound in per unit.
    pub p_min: Option<f64>,
    /// Optional active injection upper bound in per unit.
    pub p_max: Option<f64>,
    /// Optional reactive injection lower bound in per unit.
    pub q_min: Option<f64>,
    /// Optional reactive injection upper bound in per unit.
    pub q_max: Option<f64>,
    /// Optional coil current magnitude cap in per unit.
    pub current_max: Option<f64>,
    /// Optional coil apparent-power cap in per unit.
    pub apparent_max: Option<f64>,
    /// Currency/hour per per-unit active injection (same sign for sources).
    pub cost: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McOpfDevice {
    /// Original device name.
    pub identity: String,
    /// Source table and injection/consumption convention.
    pub kind: McOpfDeviceKind,
    /// Zero-based row in the canonical table identified by kind.
    pub source_row: usize,
    /// Original terminal-map order, including neutral, for result projection.
    pub terminals: Vec<usize>,
    /// Connection-defined coil order, distinct from terminal order.
    pub coils: Vec<McOpfCoil>,
    /// Optional magnitude cap on the return current sum, per unit.
    pub neutral_current_max: Option<f64>,
    /// Optional bounds on the sum of coil active injections, per unit, including isolated DC-link balance.
    pub net_active_bounds: Option<[f64; 2]>,
}
/// Canonical IVR coefficients. These contain no optimizer or expression types.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McAcOpfPreparation {
    /// Explicit physical bases used throughout this record.
    pub bases: McAcOpfAssemblyOptions,
    /// Global voltage axis in canonical bus and terminal storage order.
    pub terminals: Vec<McOpfTerminal>,
    /// Selected magnitude and sequence bounds.
    pub voltage_limits: Vec<McOpfVoltageLimit>,
    /// Selected supported angle windows.
    pub angle_limits: Vec<McOpfAngleLimit>,
    /// Conductor-current branch descriptors.
    pub branches: Vec<McOpfBranch>,
    /// Terminal admittance descriptors.
    pub shunts: Vec<McOpfShunt>,
    /// Loads, generators, IBRs and ideal-source injection descriptors.
    pub devices: Vec<McOpfDevice>,
    /// Winding-current descriptors with their own local current axes.
    pub transformers: Vec<McOpfTransformer>,
}
fn invalid(owner: &str, message: impl std::fmt::Display) -> Error {
    Error::new(&codes::BUILD_MC_OPF_INVALID, format!("{owner}: {message}"))
}
fn unsupported(owner: &str, message: impl std::fmt::Display) -> Error {
    Error::new(
        &codes::BUILD_MULTI_PHYSICS_UNSUPPORTED,
        format!("{owner}: {message}"),
    )
}
fn extras(owner: &str, data: &BTreeMap<String, serde_json::Value>, allowed: &[&str]) -> Result<()> {
    for key in data.keys() {
        if !allowed.contains(&key.as_str())
            && !["meta", "provenance", "description"].contains(&key.as_str())
        {
            return Err(unsupported(owner, format!("uninterpreted field `{key}`")));
        }
    }
    Ok(())
}
fn finite(owner: &str, values: &[f64]) -> Result<()> {
    if values.iter().any(|x| !x.is_finite()) {
        return Err(invalid(owner, "nonfinite coefficient"));
    }
    Ok(())
}
fn vector(owner: &str, values: &[f64], n: usize) -> Result<()> {
    if values.len() != n {
        return Err(invalid(
            owner,
            format!("expected {n} entries, got {}", values.len()),
        ));
    }
    finite(owner, values)
}
fn matrix(owner: &str, values: &[Vec<f64>], n: usize, scale: f64) -> Result<Vec<Vec<f64>>> {
    if values.len() != n {
        return Err(invalid(
            owner,
            "matrix row count disagrees with conductor map",
        ));
    }
    values
        .iter()
        .map(|row| {
            vector(owner, row, n)?;
            let scaled: Vec<_> = row.iter().map(|v| v * scale).collect();
            finite(owner, &scaled)?;
            Ok(scaled)
        })
        .collect()
}
fn caps(owner: &str, values: Option<&Vec<f64>>, n: usize, base: f64) -> Result<Vec<Option<f64>>> {
    let Some(v) = values else {
        return Ok(vec![None; n]);
    };
    if v.len() != n {
        return Err(invalid(
            owner,
            "limit count disagrees with physical channels",
        ));
    }
    v.iter()
        .map(|&x| {
            if x.is_nan() || x < 0.0 {
                return Err(invalid(owner, "magnitude cap must be nonnegative"));
            }
            if x == f64::INFINITY {
                return Ok(None);
            }
            let y = x / base;
            if !y.is_finite()
                || (x > 0.0 && (y * y == 0.0 || !(1.0 / (y * y)).is_finite()))
                || !(y * y).is_finite()
            {
                return Err(invalid(
                    owner,
                    "magnitude cap cannot be represented on the selected base",
                ));
            }
            Ok(Some(y))
        })
        .collect()
}
fn bounds(owner: &str, values: Option<&Vec<f64>>, n: usize, base: f64) -> Result<Vec<Option<f64>>> {
    let Some(v) = values else {
        return Ok(vec![None; n]);
    };
    vector(owner, v, n)?;
    v.iter()
        .map(|&x| {
            let y = x / base;
            finite(owner, &[y])?;
            Ok(Some(y))
        })
        .collect()
}
fn selection(selection: &ConstraintSelection, known: &[String], family: &str) -> Result<()> {
    match selection {
        ConstraintSelection::All | ConstraintSelection::None => Ok(()),
        ConstraintSelection::Only(ids) => {
            for id in ids {
                if !known.contains(id) {
                    return Err(invalid(family, format!("unknown selected identity `{id}`")));
                }
            }
            Ok(())
        }
        _ => Err(unsupported(family, "unknown constraint selection")),
    }
}
fn unique(family: &str, names: impl Iterator<Item = String>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for name in names {
        if name.is_empty() || !seen.insert(name.clone()) {
            return Err(invalid(
                family,
                format!("empty or duplicate identity `{name}`"),
            ));
        }
    }
    Ok(())
}
fn root(parent: &mut [usize], mut n: usize) -> usize {
    while parent[n] != n {
        parent[n] = parent[parent[n]];
        n = parent[n];
    }
    n
}
fn join(parent: &mut [usize], a: usize, b: usize) -> bool {
    let a = root(parent, a);
    let b = root(parent, b);
    if a == b {
        false
    } else {
        parent[a] = b;
        true
    }
}

/// Prepare the AC multiconductor IVR profile without creating solver expressions.
///
/// Supports coupled lines, shunts, capacitors, ideal switches/sources, WYE and
/// DELTA loads/generators, voltage-dependent loads, winding-current transformer
/// descriptors (including supported continuous taps), and supported IBR controls.
/// SI instance values become coefficients on the explicitly selected bases.
/// No impedance inverse or neutral reduction is performed.
///
/// Voltage selections use bus names; conductor selections use `line:name`,
/// `switch:name`, and `transformer:name`; capability selections use generator
/// names and `ibr:name`. Deselection removes bounds, never device physics.
///
/// Initial points, unreferenced islands, floating source neutrals, ideal conductor
/// cycles/paths between fixed references, and angle windows outside (-pi/2,pi/2)
/// are unsupported by this profile. This is not a claim of infeasibility.
/// The caller owns expression compilation, differentiation and optimization.
///
/// # Errors
/// Invalid shapes/units/identities, unsupported semantic fields or equipment,
/// missing source coverage, and structurally dependent ideal conductor cycles.
pub fn build_mc_ac_opf_preparation(
    instance: &McAcOpfInstance,
    bases: &McAcOpfAssemblyOptions,
) -> Result<McAcOpfPreparation> {
    let (vb, sb, ib, zb, cost_weight) = validate_profile(instance, bases)?;
    let net = instance.network();
    let (mut terminals, positions) = prepare_terminals(net)?;
    let ctx = Context {
        net,
        positions: &positions,
    };
    prepare_sources(&ctx, vb, &mut terminals)?;
    validate_selections(instance)?;
    let voltage_limits = prepare_voltage_limits(&ctx, instance, vb)?;
    let mut branches = prepare_branches(&ctx, instance, ib, sb, zb)?;
    let mut shunts = prepare_shunts(&ctx, zb)?;
    let mut devices = prepare_loads(&ctx, sb, vb)?;
    devices.extend(prepare_generators(&ctx, instance, ib, sb, cost_weight)?);
    devices.extend(inverter::prepare(&ctx, instance, ib, sb, vb, cost_weight)?);
    devices.extend(prepare_source_devices(&ctx, &terminals, sb, cost_weight)?);
    let mut transformers =
        transformer::prepare(&ctx, bases, &mut branches, &mut shunts, &terminals)?;
    for transformer in &mut transformers {
        if !instance
            .constraints()
            .conductor_limits
            .selects(&format!("transformer:{}", transformer.identity))
        {
            for port in &mut transformer.ports {
                port.current_max = None;
                port.apparent_max = None;
            }
        }
    }
    check_ideal_topology(&branches, &terminals)?;
    if transformers.is_empty() {
        check_topology(&branches, &mut terminals)?;
    } else {
        transformer::initialize(&branches, &transformers, &mut terminals)?;
    }
    Ok(McAcOpfPreparation {
        bases: *bases,
        terminals,
        voltage_limits,
        angle_limits: prepare_angles(&ctx, instance)?,
        branches,
        shunts,
        devices,
        transformers,
    })
}
fn validate_profile(
    instance: &McAcOpfInstance,
    bases: &McAcOpfAssemblyOptions,
) -> Result<(f64, f64, f64, f64, f64)> {
    let net = instance.network();
    powerio_dist::require_electrical_readiness(net)?;
    if !bases.voltage_base_v.is_finite()
        || !bases.power_base_va.is_finite()
        || bases.voltage_base_v <= 0.0
        || bases.power_base_va <= 0.0
    {
        return Err(invalid(
            "bases",
            "voltage and power bases must be finite and positive",
        ));
    }
    let vb = bases.voltage_base_v;
    let sb = bases.power_base_va;
    let ib = sb / vb;
    let zb = vb / ib;
    if !ib.is_finite() || !zb.is_finite() || ib <= 0.0 || zb <= 0.0 {
        return Err(invalid("bases", "derived base is unrepresentable"));
    }
    extras(
        "network",
        net.extras(),
        &["bmopf_terminal_conventions", "bmopf_meta"],
    )?;
    if !net.untyped_objects().is_empty() || !net.commands().is_empty() || !net.options().is_empty()
    {
        return Err(unsupported(
            "network",
            "untyped electrical objects, commands, and options require explicit canonical semantics",
        ));
    }
    if instance.initial_point().is_some() {
        return Err(unsupported(
            "initial_point",
            "initial-point projection is not implemented by this profile",
        ));
    }
    let mut cost_weight = 0.0;
    for term in instance.objective().terms() {
        match term {
            ObjectiveTerm::ActivePowerDispatchCost => cost_weight += 1.0,
            _ => {
                return Err(unsupported(
                    "objective",
                    "only active-power dispatch cost is supported",
                ));
            }
        }
    }
    for (kind, names) in [
        (
            "transformer",
            net.transformers()
                .iter()
                .map(|x| x.name.clone())
                .collect::<Vec<_>>(),
        ),
        (
            "capacitor",
            net.capacitors().iter().map(|x| x.name.clone()).collect(),
        ),
        ("ibr", net.ibrs().iter().map(|x| x.name.clone()).collect()),
        (
            "control_profile",
            net.control_profiles()
                .iter()
                .map(|x| x.name.clone())
                .collect(),
        ),
        (
            "bus",
            net.buses().iter().map(|x| x.id.clone()).collect::<Vec<_>>(),
        ),
        (
            "linecode",
            net.line_codes().iter().map(|x| x.name.clone()).collect(),
        ),
        ("line", net.lines().iter().map(|x| x.name.clone()).collect()),
        (
            "switch",
            net.switches().iter().map(|x| x.name.clone()).collect(),
        ),
        (
            "shunt",
            net.shunts().iter().map(|x| x.name.clone()).collect(),
        ),
        ("load", net.loads().iter().map(|x| x.name.clone()).collect()),
        (
            "generator",
            net.generators().iter().map(|x| x.name.clone()).collect(),
        ),
        (
            "source",
            net.sources().iter().map(|x| x.name.clone()).collect(),
        ),
    ] {
        unique(kind, names.into_iter())?;
    }
    Ok((vb, sb, ib, zb, cost_weight))
}
fn prepare_terminals(net: &MulticonductorNetwork) -> Result<(Vec<McOpfTerminal>, Positions)> {
    let mut terminals = Vec::new();
    let mut positions = BTreeMap::new();
    let conventions = net.extras().get("bmopf_terminal_conventions");
    for bus in net.buses() {
        extras(
            &bus.id,
            &bus.extras,
            &["va_diff_min", "va_diff_max", "va_nom"],
        )?;
        if bus.terminals.is_empty() {
            return Err(invalid(&bus.id, "no terminals"));
        }
        unique(&bus.id, bus.terminals.iter().cloned())?;
        for grounded in &bus.grounded {
            if !bus.terminals.contains(grounded) {
                return Err(invalid(&bus.id, "unknown grounded terminal"));
            }
        }
        let phases: Vec<_> = bus
            .phase_indices(conventions)
            .into_iter()
            .filter(|&i| bus.terminals[i] != "0")
            .collect();
        for (k, t) in bus.terminals.iter().enumerate() {
            let grounded = t == "0" || bus.grounded.contains(t);
            let angle = phases
                .iter()
                .position(|p| *p == k)
                .map_or(0.0, |p| -(p as f64) * std::f64::consts::TAU / 3.0);
            let start = if grounded || !phases.contains(&k) {
                [0.0, 0.0]
            } else {
                [angle.cos(), angle.sin()]
            };
            positions.insert((bus.id.clone(), t.clone()), terminals.len());
            terminals.push(McOpfTerminal {
                bus: bus.id.clone(),
                terminal: t.clone(),
                grounded,
                fixed: grounded.then_some([0.0, 0.0]),
                start,
            });
        }
    }
    Ok((terminals, positions))
}
struct Context<'a> {
    net: &'a MulticonductorNetwork,
    positions: &'a Positions,
}
impl Context<'_> {
    fn resolve(&self, owner: &str, bus: &str, map: &[String]) -> Result<Vec<usize>> {
        unique(owner, map.iter().cloned())?;
        if map.is_empty() {
            return Err(invalid(owner, "empty terminal map"));
        }
        map.iter()
            .map(|t| {
                self.positions
                    .get(&(bus.to_owned(), t.clone()))
                    .copied()
                    .ok_or_else(|| invalid(owner, format!("unknown terminal {bus}:{t}")))
            })
            .collect()
    }
    fn coils(
        &self,
        name: &str,
        bus_id: &str,
        tm: &[String],
        config: Configuration,
    ) -> Result<(Vec<usize>, CoilPairs)> {
        let ids = self.resolve(name, bus_id, tm)?;
        let bus = self
            .net
            .bus(bus_id)
            .ok_or_else(|| invalid(name, "unknown bus"))?;
        let phases: Vec<_> = bus
            .phase_indices(self.net.extras().get("bmopf_terminal_conventions"))
            .into_iter()
            .filter(|&i| bus.terminals[i] != "0")
            .collect();
        let nonphase: Vec<_> = tm
            .iter()
            .enumerate()
            .filter(|(_, t)| !phases.iter().any(|i| bus.terminals[*i] == **t))
            .map(|(i, _)| i)
            .collect();
        let coils = match config {
            Configuration::Delta => {
                if ids.len() == 2 {
                    vec![(ids[0], Some(ids[1]))]
                } else if ids.len() == 3 {
                    (0..3).map(|i| (ids[i], Some(ids[(i + 1) % 3]))).collect()
                } else {
                    return Err(invalid(name, "delta requires two or three terminals"));
                }
            }
            Configuration::SinglePhase => {
                if ids.len() != 2 {
                    return Err(invalid(name, "single-phase requires two terminals"));
                }
                vec![(ids[0], Some(ids[1]))]
            }
            Configuration::Wye => {
                if nonphase.len() > 1 {
                    return Err(invalid(name, "ambiguous neutral"));
                }
                let neutral = nonphase.first().map(|i| ids[*i]);
                ids.iter()
                    .enumerate()
                    .filter(|(i, _)| !nonphase.contains(i))
                    .map(|(_, id)| (*id, neutral))
                    .collect()
            }
            _ => return Err(unsupported(name, "unknown configuration")),
        };
        if coils.is_empty() {
            return Err(invalid(name, "no coils"));
        }
        Ok((ids, coils))
    }
}
fn prepare_sources(ctx: &Context<'_>, vb: f64, terminals: &mut [McOpfTerminal]) -> Result<()> {
    let net = ctx.net;
    let resolve = |owner: &str, bus: &str, map: &[String]| ctx.resolve(owner, bus, map);
    let mut source_owners = BTreeSet::new();
    for src in net.sources() {
        extras(&src.name, &src.extras, &[])?;
        let ids = resolve(&src.name, &src.bus, &src.terminal_map)?;
        let bus = net
            .bus(&src.bus)
            .ok_or_else(|| invalid(&src.name, "unknown source bus"))?;
        let phases = bus.phase_indices(net.extras().get("bmopf_terminal_conventions"));
        for (k, terminal) in bus.terminals.iter().enumerate() {
            if !phases.contains(&k)
                && !terminals[ctx.positions[&(bus.id.clone(), terminal.clone())]].grounded
            {
                return Err(unsupported(
                    &src.name,
                    "source-bus neutral must be explicitly grounded, including when omitted from its map",
                ));
            }
        }
        vector(&src.name, &src.v_magnitude, ids.len())?;
        vector(&src.name, &src.v_angle, ids.len())?;
        for (k, &id) in ids.iter().enumerate() {
            if src.v_magnitude[k] < 0.0 {
                return Err(invalid(&src.name, "negative source magnitude"));
            }
            let value = [
                src.v_magnitude[k] / vb * src.v_angle[k].cos(),
                src.v_magnitude[k] / vb * src.v_angle[k].sin(),
            ];
            finite(&src.name, &value)?;
            if terminals[id].grounded {
                if value != [0.0, 0.0] {
                    return Err(invalid(&src.name, "nonzero grounded reference"));
                }
            } else if !source_owners.insert(id) {
                return Err(unsupported(
                    &src.name,
                    "colocated ideal sources have ambiguous current allocation",
                ));
            }
            terminals[id].fixed = Some(value);
            terminals[id].start = value;
        }
    }
    Ok(())
}
fn validate_selections(instance: &McAcOpfInstance) -> Result<()> {
    let net = instance.network();
    let selected = instance.constraints();
    selection(
        &selected.terminal_voltage_bounds,
        &net.buses().iter().map(|b| b.id.clone()).collect::<Vec<_>>(),
        "voltage",
    )?;
    selection(
        &selected.generator_capability,
        &net.generators()
            .iter()
            .map(|g| g.name.clone())
            .chain(net.ibrs().iter().map(|i| format!("ibr:{}", i.name)))
            .collect::<Vec<_>>(),
        "generator",
    )?;
    let branch_ids: Vec<_> = net
        .lines()
        .iter()
        .map(|l| format!("line:{}", l.name))
        .chain(net.switches().iter().map(|s| format!("switch:{}", s.name)))
        .chain(
            net.transformers()
                .iter()
                .map(|t| format!("transformer:{}", t.name)),
        )
        .collect();
    selection(&selected.conductor_limits, &branch_ids, "conductor")?;
    Ok(())
}
fn prepare_voltage_limits(
    ctx: &Context<'_>,
    instance: &McAcOpfInstance,
    vb: f64,
) -> Result<Vec<McOpfVoltageLimit>> {
    let net = ctx.net;
    let positions = ctx.positions;
    let conventions = net.extras().get("bmopf_terminal_conventions");
    let selected = instance.constraints();
    let mut voltage_limits = Vec::new();
    for bus in net.buses() {
        let phases: Vec<_> = bus
            .phase_indices(conventions)
            .into_iter()
            .filter(|&i| bus.terminals[i] != "0")
            .collect();
        let nonph: Vec<_> = (0..bus.terminals.len())
            .filter(|i| !phases.contains(i))
            .collect();
        if nonph.len() > 1 {
            return Err(unsupported(&bus.id, "multiple nonphase terminals"));
        }
        let neutral = nonph
            .first()
            .map(|i| positions[&(bus.id.clone(), bus.terminals[*i].clone())]);
        if !selected.terminal_voltage_bounds.selects(&bus.id) {
            continue;
        }
        let pglo = bus
            .v_min
            .map(|x| vec![x; phases.len()])
            .or_else(|| bus.v_min_phase.clone());
        let pghi = bus
            .v_max
            .map(|x| vec![x; phases.len()])
            .or_else(|| bus.v_max_phase.clone());
        let pids: Vec<_> = phases
            .iter()
            .map(|i| positions[&(bus.id.clone(), bus.terminals[*i].clone())])
            .collect();
        prepare_sequences(bus, &pids, neutral, vb, &mut voltage_limits)?;
        let mut add_limits = |family: &str, pairs, lo, hi| {
            push_voltage_limits(&mut voltage_limits, &bus.id, family, pairs, lo, hi, vb)
        };
        add_limits(
            "ground",
            pids.iter().map(|i| (*i, None)).collect(),
            pglo.as_ref(),
            pghi.as_ref(),
        )?;
        if (bus.vpn_min.is_some() || bus.vpn_max.is_some() || bus.vn_max.is_some())
            && neutral.is_none()
        {
            return Err(invalid(
                &bus.id,
                "neutral-referenced limit without a neutral",
            ));
        }
        add_limits(
            "neutral",
            pids.iter().map(|i| (*i, neutral)).collect(),
            bus.vpn_min.as_ref(),
            bus.vpn_max.as_ref(),
        )?;
        let pairs: Vec<_> = pids
            .iter()
            .enumerate()
            .flat_map(|(i, a)| pids.iter().skip(i + 1).map(move |b| (*a, Some(*b))))
            .collect();
        add_limits("phase", pairs, bus.vpp_min.as_ref(), bus.vpp_max.as_ref())?;
        if let (Some(n), Some(cap)) = (neutral, bus.vn_max) {
            add_limits("neutral_ground", vec![(n, None)], None, Some(&vec![cap]))?;
        }
    }
    Ok(voltage_limits)
}
fn prepare_branches(
    ctx: &Context<'_>,
    instance: &McAcOpfInstance,
    ib: f64,
    sb: f64,
    zb: f64,
) -> Result<Vec<McOpfBranch>> {
    let net = ctx.net;
    let resolve = |owner: &str, bus: &str, map: &[String]| ctx.resolve(owner, bus, map);
    let selected = instance.constraints();
    let mut branches = Vec::new();
    for line in net.lines() {
        extras(&line.name, &line.extras, &["va_diff_min", "va_diff_max"])?;
        let lc = net
            .linecode(&line.linecode)
            .ok_or_else(|| invalid(&line.name, "unknown linecode"))?;
        extras(&lc.name, &lc.extras, &[])?;
        let from = resolve(&line.name, &line.bus_from, &line.terminal_map_from)?;
        let to = resolve(&line.name, &line.bus_to, &line.terminal_map_to)?;
        let n = from.len();
        if to.len() != n || lc.n_conductors != n || !line.length.is_finite() || line.length < 0.0 {
            return Err(invalid(&line.name, "invalid conductor count or length"));
        }
        let identity = format!("line:{}", line.name);
        let active = selected.conductor_limits.selects(&identity);
        branches.push(McOpfBranch {
            identity,
            from,
            to,
            r: matrix(&line.name, &lc.r_series, n, line.length / zb)?,
            x: matrix(&line.name, &lc.x_series, n, line.length / zb)?,
            g_from: matrix(&line.name, &lc.g_from, n, line.length * zb)?,
            b_from: matrix(&line.name, &lc.b_from, n, line.length * zb)?,
            g_to: matrix(&line.name, &lc.g_to, n, line.length * zb)?,
            b_to: matrix(&line.name, &lc.b_to, n, line.length * zb)?,
            current_max: caps(
                &line.name,
                if active {
                    line.i_max.as_ref().or(lc.i_max.as_ref())
                } else {
                    None
                },
                n,
                ib,
            )?,
            apparent_max: caps(
                &line.name,
                if active {
                    line.s_max.as_ref().or(lc.s_max.as_ref())
                } else {
                    None
                },
                n,
                sb,
            )?,
            open: false,
        });
    }
    for sw in net.switches() {
        extras(&sw.name, &sw.extras, &[])?;
        let from = resolve(&sw.name, &sw.bus_from, &sw.terminal_map_from)?;
        let to = resolve(&sw.name, &sw.bus_to, &sw.terminal_map_to)?;
        let n = from.len();
        if to.len() != n {
            return Err(invalid(&sw.name, "switch map lengths differ"));
        }
        let z = vec![vec![0.0; n]; n];
        let identity = format!("switch:{}", sw.name);
        let active = selected.conductor_limits.selects(&identity);
        branches.push(McOpfBranch {
            identity,
            from,
            to,
            r: z.clone(),
            x: z.clone(),
            g_from: z.clone(),
            b_from: z.clone(),
            g_to: z.clone(),
            b_to: z,
            current_max: caps(
                &sw.name,
                if active { sw.i_max.as_ref() } else { None },
                n,
                ib,
            )?,
            apparent_max: vec![None; n],
            open: sw.open,
        });
    }
    Ok(branches)
}
fn prepare_shunts(ctx: &Context<'_>, zb: f64) -> Result<Vec<McOpfShunt>> {
    let net = ctx.net;
    let resolve = |owner: &str, bus: &str, map: &[String]| ctx.resolve(owner, bus, map);
    let mut shunts = Vec::new();
    for sh in net.shunts() {
        extras(&sh.name, &sh.extras, &[])?;
        let ids = resolve(&sh.name, &sh.bus, &sh.terminal_map)?;
        let n = ids.len();
        shunts.push(McOpfShunt {
            identity: sh.name.clone(),
            terminals: ids,
            g: matrix(&sh.name, &sh.g, n, zb)?,
            b: matrix(&sh.name, &sh.b, n, zb)?,
        });
    }
    for cap in net.capacitors() {
        extras(&cap.name, &cap.extras, &[])?;
        let (ids, pairs) = ctx.coils(&cap.name, &cap.bus, &cap.terminal_map, cap.configuration)?;
        let n = pairs.len();
        let ratings = vec![cap.q_rated / n as f64; n];
        // The canonical WYE bank nameplate is line-to-line when it has an
        // explicit return, including a one-coil bank. SINGLE_PHASE is across
        // its two terminals. BMOPF per-coil arrays already arrive as shunts.
        let nominal = if cap.configuration == Configuration::Wye && ids.len() > 1 {
            cap.v_nom / 3f64.sqrt()
        } else {
            cap.v_nom
        };
        finite(&cap.name, &[cap.q_rated, cap.v_nom])?;
        if nominal <= 0.0 || !nominal.is_finite() || ratings.iter().any(|q| *q < 0.0) {
            return Err(invalid(
                &cap.name,
                "capacitor requires positive voltage and nonnegative var",
            ));
        }
        let mut b = vec![vec![0.0; ids.len()]; ids.len()];
        for (k, (p, q)) in pairs.iter().enumerate() {
            let positive_slot = ids
                .iter()
                .position(|v| v == p)
                .ok_or_else(|| invalid(&cap.name, "missing capacitor terminal"))?;
            let admittance = ratings[k] / nominal.powi(2) * zb;
            finite(&cap.name, &[admittance])?;
            b[positive_slot][positive_slot] += admittance;
            if let Some(q) = q {
                let negative_slot = ids
                    .iter()
                    .position(|v| v == q)
                    .ok_or_else(|| invalid(&cap.name, "missing capacitor return"))?;
                b[negative_slot][negative_slot] += admittance;
                b[positive_slot][negative_slot] -= admittance;
                b[negative_slot][positive_slot] -= admittance;
            }
        }
        shunts.push(McOpfShunt {
            identity: format!("capacitor:{}", cap.name),
            g: vec![vec![0.0; ids.len()]; ids.len()],
            b,
            terminals: ids,
        });
    }
    Ok(shunts)
}
fn prepare_loads(ctx: &Context<'_>, sb: f64, vb: f64) -> Result<Vec<McOpfDevice>> {
    let net = ctx.net;
    let coil_map = |name: &str, bus: &str, tm: &[String], config| ctx.coils(name, bus, tm, config);
    let mut devices = Vec::new();
    for (row, load) in net.loads().iter().enumerate() {
        extras(&load.name, &load.extras, &[])?;
        let (ids, pairs) = coil_map(
            &load.name,
            &load.bus,
            &load.terminal_map,
            load.configuration,
        )?;
        let n = pairs.len();
        vector(&load.name, &load.p_nom, n)?;
        vector(&load.name, &load.q_nom, n)?;
        for (&p, &q) in load.p_nom.iter().zip(&load.q_nom) {
            finite(&load.name, &[p / sb, q / sb])?;
        }
        let laws = prepare_load_laws(load, n, sb, vb)?;
        let coils = pairs
            .into_iter()
            .enumerate()
            .map(|(k, (positive, negative))| McOpfCoil {
                positive,
                negative,
                prescribed: Some([load.p_nom[k] / sb, load.q_nom[k] / sb]),
                load_law: laws[k].clone(),
                reactive_slope: None,
                volt_var: None,
                volt_watt: None,
                p_min: None,
                p_max: None,
                q_min: None,
                q_max: None,
                current_max: None,
                apparent_max: None,
                cost: 0.0,
            })
            .collect();
        devices.push(McOpfDevice {
            identity: load.name.clone(),
            kind: McOpfDeviceKind::Load,
            source_row: row,
            terminals: ids,
            coils,
            neutral_current_max: None,
            net_active_bounds: None,
        });
    }
    Ok(devices)
}
fn prepare_generators(
    ctx: &Context<'_>,
    instance: &McAcOpfInstance,
    ib: f64,
    sb: f64,
    cost_weight: f64,
) -> Result<Vec<McOpfDevice>> {
    let net = ctx.net;
    let coil_map = |name: &str, bus: &str, tm: &[String], config| ctx.coils(name, bus, tm, config);
    let mut devices = Vec::new();
    let selected = instance.constraints();
    for (row, generator) in net.generators().iter().enumerate() {
        extras(&generator.name, &generator.extras, &[])?;
        let (ids, pairs) = coil_map(
            &generator.name,
            &generator.bus,
            &generator.terminal_map,
            generator.configuration,
        )?;
        let n = pairs.len();
        let active = selected.generator_capability.selects(&generator.name);
        let pmin = bounds(
            &generator.name,
            generator.p_min.as_ref().filter(|_| active),
            n,
            sb,
        )?;
        let pmax = bounds(
            &generator.name,
            generator.p_max.as_ref().filter(|_| active),
            n,
            sb,
        )?;
        let qmin = bounds(
            &generator.name,
            generator.q_min.as_ref().filter(|_| active),
            n,
            sb,
        )?;
        let qmax = bounds(
            &generator.name,
            generator.q_max.as_ref().filter(|_| active),
            n,
            sb,
        )?;
        let smax = caps(
            &generator.name,
            generator.s_max.as_ref().filter(|_| active),
            n,
            sb,
        )?;
        // Star current ratings include a neutral entry; delta ratings are coil currents.
        let star = matches!(
            generator.configuration,
            Configuration::Wye | Configuration::SinglePhase
        ) && ids.len() == n + 1;
        let mut imax = caps(
            &generator.name,
            generator.i_max.as_ref().filter(|_| active),
            n + usize::from(star),
            ib,
        )?;
        if star && n == 1 {
            collapse_return_rating(&mut imax);
        }
        for nominal in [&generator.p_nom, &generator.q_nom] {
            if !nominal.is_empty() {
                vector(&generator.name, nominal, n)?;
            }
        }
        let costs = costs(&generator.name, generator.cost.as_ref(), n, sb)?;
        let mut coils = Vec::new();
        for (k, (positive, negative)) in pairs.into_iter().enumerate() {
            if pmin[k].zip(pmax[k]).is_some_and(|(a, b)| a > b)
                || qmin[k].zip(qmax[k]).is_some_and(|(a, b)| a > b)
            {
                return Err(invalid(&generator.name, "reversed capability bounds"));
            }
            coils.push(McOpfCoil {
                positive,
                negative,
                prescribed: None,
                load_law: None,
                reactive_slope: None,
                volt_var: None,
                volt_watt: None,
                p_min: pmin[k],
                p_max: pmax[k],
                q_min: qmin[k],
                q_max: qmax[k],
                current_max: imax[k],
                apparent_max: smax[k],
                cost: costs[k].unwrap_or(0.0) * cost_weight,
            });
        }
        devices.push(McOpfDevice {
            identity: generator.name.clone(),
            kind: McOpfDeviceKind::Generator,
            source_row: row,
            terminals: ids,
            coils,
            neutral_current_max: if star { imax[n] } else { None },
            net_active_bounds: None,
        });
    }
    Ok(devices)
}
fn prepare_source_devices(
    ctx: &Context<'_>,
    terminals: &[McOpfTerminal],
    sb: f64,
    cost_weight: f64,
) -> Result<Vec<McOpfDevice>> {
    let net = ctx.net;
    let coil_map = |name: &str, bus: &str, tm: &[String], config| ctx.coils(name, bus, tm, config);
    let mut devices = Vec::new();
    for (row, src) in net.sources().iter().enumerate() {
        let (ids, pairs) = coil_map(&src.name, &src.bus, &src.terminal_map, Configuration::Wye)?;
        let n = pairs.len();
        if pairs
            .iter()
            .any(|(_, neg)| neg.is_some_and(|i| !terminals[i].grounded))
        {
            return Err(unsupported(
                &src.name,
                "source neutral must be explicitly grounded",
            ));
        }
        let costs = costs(&src.name, src.energy_cost_rate.as_ref(), n, sb)?;
        let coils = pairs
            .into_iter()
            .enumerate()
            .map(|(k, (positive, negative))| McOpfCoil {
                positive,
                negative,
                prescribed: None,
                load_law: None,
                reactive_slope: None,
                volt_var: None,
                volt_watt: None,
                p_min: None,
                p_max: None,
                q_min: None,
                q_max: None,
                current_max: None,
                apparent_max: None,
                cost: costs[k].unwrap_or(0.0) * cost_weight,
            })
            .collect();
        devices.push(McOpfDevice {
            identity: src.name.clone(),
            kind: McOpfDeviceKind::Source,
            source_row: row,
            terminals: ids,
            coils,
            neutral_current_max: None,
            net_active_bounds: None,
        });
    }
    Ok(devices)
}
fn check_ideal_topology(branches: &[McOpfBranch], terminals: &[McOpfTerminal]) -> Result<()> {
    // Conservative structural checks. Do not classify numerical near-zero Z as ideal.
    let mut ideal: Vec<_> = (0..terminals.len()).collect();
    for br in branches.iter().filter(|b| !b.open) {
        for k in 0..br.from.len() {
            if br.r[k].iter().chain(&br.x[k]).all(|x| *x == 0.0) {
                if terminals[br.from[k]].fixed.is_some() && terminals[br.to[k]].fixed.is_some() {
                    return Err(unsupported(
                        &br.identity,
                        "ideal row between fixed terminals has undetermined current",
                    ));
                }
                if !join(&mut ideal, br.from[k], br.to[k]) {
                    return Err(invalid(&br.identity, "dependent ideal-conductor cycle"));
                }
            }
        }
    }
    let mut ideal_fixed = BTreeSet::new();
    for (i, t) in terminals.iter().enumerate() {
        if t.fixed.is_some() && !ideal_fixed.insert(root(&mut ideal, i)) {
            return Err(unsupported(
                &t.bus,
                "ideal path joins multiple fixed voltage references",
            ));
        }
    }
    Ok(())
}
fn check_topology(branches: &[McOpfBranch], terminals: &mut [McOpfTerminal]) -> Result<()> {
    let mut connected: Vec<_> = (0..terminals.len()).collect();
    for br in branches.iter().filter(|b| !b.open) {
        for (&a, &b) in br.from.iter().zip(&br.to) {
            join(&mut connected, a, b);
        }
    }
    let mut anchors = BTreeMap::new();
    for (i, t) in terminals.iter().enumerate() {
        if let Some(value) = t.fixed {
            anchors.entry(root(&mut connected, i)).or_insert(value);
        }
    }
    for (i, t) in terminals.iter_mut().enumerate() {
        let Some(value) = anchors.get(&root(&mut connected, i)) else {
            return Err(unsupported(
                &format!("{}:{}", t.bus, t.terminal),
                "conductor component has no fixed voltage reference",
            ));
        };
        // Propagate physical source phasors, not an implicit one-per-unit guess.
        // This is a start only; no source equality is added to remote terminals.
        if t.fixed.is_none() {
            t.start = *value;
        }
    }
    Ok(())
}
fn costs(owner: &str, values: Option<&Vec<f64>>, n: usize, sb: f64) -> Result<Vec<Option<f64>>> {
    let expanded = values.filter(|v| v.len() == 1).map(|v| vec![v[0]; n]);
    bounds(owner, expanded.as_ref().or(values), n, 1000.0 / sb)
}
fn push_voltage_limits(
    output: &mut Vec<McOpfVoltageLimit>,
    bus_id: &str,
    family: &str,
    pairs: CoilPairs,
    lo: Option<&Vec<f64>>,
    hi: Option<&Vec<f64>>,
    vb: f64,
) -> Result<()> {
    if let Some(lo) = lo {
        finite(bus_id, lo)?;
    }
    let lows = caps(bus_id, lo, pairs.len(), vb)?;
    let highs = caps(bus_id, hi, pairs.len(), vb)?;
    for (k, ((positive, negative), (lower, upper))) in pairs
        .into_iter()
        .zip(lows.into_iter().zip(highs))
        .enumerate()
    {
        if lower.zip(upper).is_some_and(|(l, u)| l > u) {
            return Err(invalid(bus_id, "reversed voltage limits"));
        }
        if lower.is_some() || upper.is_some() {
            output.push(McOpfVoltageLimit {
                combination: Vec::new(),
                identity: format!("{bus_id}:{family}:{k}"),
                positive,
                negative,
                lower,
                upper,
            });
        }
    }
    Ok(())
}

fn prepare_load_laws(
    load: &powerio_dist::DistLoad,
    n: usize,
    sb: f64,
    vb: f64,
) -> Result<Vec<Option<McOpfLoadLaw>>> {
    use DistLoadVoltageModel as Law;
    if matches!(load.voltage_model, Law::ConstantPower { .. }) {
        return Ok(vec![None; n]);
    }
    let at = |v: &[f64], k: usize, default: f64| -> Result<f64> {
        if v.is_empty() {
            return Ok(default);
        }
        if v.len() != 1 && v.len() != n {
            return Err(invalid(
                &load.name,
                "load law vector must be scalar or per coil",
            ));
        }
        finite(&load.name, v)?;
        Ok(v[if v.len() == 1 { 0 } else { k }])
    };
    (0..n)
        .map(|k| {
            let nominal = at(load.voltage_model.v_nom(), k, f64::NAN)? / vb;
            if !nominal.is_finite() || nominal <= 0.0 {
                return Err(invalid(
                    &load.name,
                    "voltage-dependent load requires positive nominal coil voltage",
                ));
            }
            let p = load.p_nom[k] / sb;
            let q = load.q_nom[k] / sb;
            let terms = match &load.voltage_model {
                Law::ConstantImpedance { .. } => [vec![[p, 2.0]], vec![[q, 2.0]]],
                Law::ConstantCurrent { .. } => [vec![[p, 1.0]], vec![[q, 1.0]]],
                Law::Exponential {
                    gamma_p, gamma_q, ..
                } => [
                    vec![[p, at(gamma_p, k, 0.0)?]],
                    vec![[q, at(gamma_q, k, 0.0)?]],
                ],
                Law::Zip {
                    alpha_z,
                    alpha_i,
                    alpha_p,
                    beta_z,
                    beta_i,
                    beta_p,
                    ..
                } => {
                    let a_empty = alpha_z.is_empty() && alpha_i.is_empty() && alpha_p.is_empty();
                    let b_empty = beta_z.is_empty() && beta_i.is_empty() && beta_p.is_empty();
                    [
                        vec![
                            [p * at(alpha_z, k, 0.0)?, 2.0],
                            [p * at(alpha_i, k, 0.0)?, 1.0],
                            [p * at(alpha_p, k, f64::from(a_empty))?, 0.0],
                        ],
                        vec![
                            [q * at(beta_z, k, 0.0)?, 2.0],
                            [q * at(beta_i, k, 0.0)?, 1.0],
                            [q * at(beta_p, k, f64::from(b_empty))?, 0.0],
                        ],
                    ]
                }
                _ => return Err(unsupported(&load.name, "unknown load voltage law")),
            };
            let terms = terms.map(|v| v.into_iter().filter(|t| t[0] != 0.0).collect::<Vec<_>>());
            for row in &terms {
                for t in row {
                    finite(&load.name, t)?;
                }
            }
            Ok(Some(McOpfLoadLaw { nominal, terms }))
        })
        .collect()
}

fn prepare_angles(ctx: &Context<'_>, instance: &McAcOpfInstance) -> Result<Vec<McOpfAngleLimit>> {
    let mut out = Vec::new();
    let window =
        |name: &str, data: &BTreeMap<String, serde_json::Value>| -> Result<Option<[f64; 2]>> {
            if !data.contains_key("va_diff_min") && !data.contains_key("va_diff_max") {
                return Ok(None);
            }
            let lo = data
                .get("va_diff_min")
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| invalid(name, "angle needs two scalar bounds"))?;
            let hi = data
                .get("va_diff_max")
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| invalid(name, "angle needs two scalar bounds"))?;
            if !lo.is_finite()
                || !hi.is_finite()
                || lo > hi
                || lo <= -std::f64::consts::FRAC_PI_2
                || hi >= std::f64::consts::FRAC_PI_2
            {
                return Err(invalid(
                    name,
                    "angle window must lie strictly within +/- pi/2",
                ));
            }
            Ok(Some([lo, hi]))
        };
    for line in ctx.net.lines() {
        if let Some([lower, upper]) = window(&line.name, &line.extras)? {
            let from = ctx.resolve(&line.name, &line.bus_from, &line.terminal_map_from)?;
            let to = ctx.resolve(&line.name, &line.bus_to, &line.terminal_map_to)?;
            for (k, (&first, &second)) in from.iter().zip(&to).enumerate() {
                out.push(McOpfAngleLimit {
                    identity: format!("line:{}:{k}", line.name),
                    first,
                    second,
                    offset: 0.0,
                    lower,
                    upper,
                });
            }
        }
    }
    for bus in ctx.net.buses() {
        if !instance
            .constraints()
            .terminal_voltage_bounds
            .selects(&bus.id)
        {
            continue;
        }
        if let Some([lower, upper]) = window(&bus.id, &bus.extras)? {
            let phase = bus.phase_indices(ctx.net.extras().get("bmopf_terminal_conventions"));
            let nominal: Vec<f64> = bus
                .extras
                .get("va_nom")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(|_| invalid(&bus.id, "invalid nominal angles"))?
                .unwrap_or_else(|| vec![0.0; phase.len()]);
            vector(&bus.id, &nominal, phase.len())?;
            for a in 0..phase.len() {
                for b in a + 1..phase.len() {
                    out.push(McOpfAngleLimit {
                        identity: format!("bus:{}:{a}:{b}", bus.id),
                        first: ctx.positions[&(bus.id.clone(), bus.terminals[phase[a]].clone())],
                        second: ctx.positions[&(bus.id.clone(), bus.terminals[phase[b]].clone())],
                        offset: nominal[a] - nominal[b],
                        lower,
                        upper,
                    });
                }
            }
        }
    }
    Ok(out)
}

fn prepare_sequences(
    bus: &powerio_dist::DistBus,
    pids: &[usize],
    neutral: Option<usize>,
    vb: f64,
    voltage_limits: &mut Vec<McOpfVoltageLimit>,
) -> Result<()> {
    if [bus.vpos_min, bus.vpos_max, bus.vneg_max, bus.vzero_max]
        .iter()
        .any(Option::is_some)
    {
        if pids.len() != 3 {
            return Err(invalid(
                &bus.id,
                "sequence bounds require exactly three phases",
            ));
        }
        for (name, sequence, lower, upper) in [
            ("positive", 1, bus.vpos_min, bus.vpos_max),
            ("negative", -1, None, bus.vneg_max),
            ("zero", 0, None, bus.vzero_max),
        ] {
            if lower.is_none() && upper.is_none() {
                continue;
            }
            let lo = caps(&bus.id, lower.map(|v| vec![v]).as_ref(), 1, vb)?[0];
            let hi = caps(&bus.id, upper.map(|v| vec![v]).as_ref(), 1, vb)?[0];
            if lower.is_some_and(|v| !v.is_finite()) || lo.zip(hi).is_some_and(|(l, h)| l > h) {
                return Err(invalid(&bus.id, "invalid sequence interval"));
            }
            let mut combination = Vec::new();
            for (k, &id) in pids.iter().enumerate() {
                let angle = f64::from(sequence) * (k as f64) * std::f64::consts::TAU / 3.0;
                combination.push((id, [angle.cos() / 3.0, angle.sin() / 3.0]));
            }
            if sequence == 0
                && let Some(n) = neutral
            {
                combination.push((n, [-1.0, 0.0]));
            }
            voltage_limits.push(McOpfVoltageLimit {
                identity: format!("{}:sequence:{name}", bus.id),
                positive: pids[0],
                negative: None,
                lower: lo,
                upper: hi,
                combination,
            });
        }
    }
    Ok(())
}

// A two-wire device has equal outgoing and return current magnitudes.
fn collapse_return_rating(ratings: &mut [Option<f64>]) {
    ratings[0] = match (ratings[0], ratings[1]) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    ratings[1] = None;
}

//! The universal normalization shared by the PowerModels reader/writer and
//! [`BalancedNetwork::to_normalized`].
//!
//! Two things live here so there is one implementation of each:
//!
//! - **Per-unit scaling factors and the gen-cost rescale** ([`cost_to_pu`] /
//!   [`cost_from_pu`], [`DEG_TO_RAD`] / [`RAD_TO_DEG`], [`GEN_PU_KEYS`]). The
//!   PowerModels writer scales raw model values into its per-unit JSON; the
//!   reader inverts it; [`BalancedNetwork::to_normalized`] scales the same way into a new
//!   `BalancedNetwork`. The cost rescale is the one piece subtle enough that a second copy
//!   would drift, so it has a single home.
//! - **[`BalancedNetwork::to_normalized`]**: a derived, computation-ready form, per unit,
//!   radians, out of service filtered, source ID preserving, bus types canonicalized.

use std::collections::{HashMap, HashSet};

use crate::network::{
    BalancedNetwork, BalancedNetworkTables, Branch, Bus, BusId, BusType, GEN_EXTRA_KEYS, GenCost,
    Generator, Hvdc, Load, LoadVoltageModel, Shunt, SourceFormat, Storage, Switch, Transformer3W,
};
use crate::{Error, Result};

/// Degrees → radians. The per-unit convention stores angles in radians; the raw
/// model keeps MATPOWER degrees.
pub(crate) const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

/// Radians → degrees, the inverse of [`DEG_TO_RAD`], used when reading a per-unit
/// source back into the neutral degree model.
pub(crate) const RAD_TO_DEG: f64 = 180.0 / std::f64::consts::PI;

/// The gen capability columns that are per-unitized (the ramp rates). The PQ-curve
/// points (`pc1`/`pc2`/`qc*`) and `apf` stay raw, exactly as PowerModels'
/// `make_per_unit!` leaves them, so a column is scaled in one place and can't drift
/// between the reader, the writer, and [`BalancedNetwork::to_normalized`].
pub(crate) const GEN_PU_KEYS: [&str; 4] = ["ramp_agc", "ramp_10", "ramp_30", "ramp_q"];

/// Default branch angle difference bound used by PowerModels parse time repair.
#[allow(clippy::approx_constant)]
pub const POWER_MODELS_ANGLE_BOUND_PAD: f64 = 1.0472;

/// Options for [`BalancedNetwork::to_normalized_with_options`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizeOptions {
    /// Clamp branch angle difference bounds to the interval PowerModels relaxations
    /// accept. Disabled by default so [`BalancedNetwork::to_normalized`] stays unchanged.
    pub clamp_angle_bounds: bool,
    /// Replacement magnitude, in radians, for clamped angle bounds.
    pub angle_bound_pad: f64,
}

impl Default for NormalizeOptions {
    fn default() -> Self {
        Self {
            clamp_angle_bounds: false,
            angle_bound_pad: POWER_MODELS_ANGLE_BOUND_PAD,
        }
    }
}

/// Output of [`BalancedNetwork::to_normalized_with_options`].
#[derive(Clone, Debug)]
// Frozen 0.9 reader plumbing: the legacy09 upgrade in the powerio crate is
// the one remaining consumer, so the type stays reachable but leaves the
// documented surface. It goes when legacy09 retires.
#[doc(hidden)]
pub struct NormalizedNetwork {
    pub network: BalancedNetwork,
    /// The pass's findings as structured records.
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
    /// The same findings as `CODE: message` lines.
    pub warnings: Vec<String>,
}

/// Row provenance for one normalize pass: for each dense position in the
/// [`IndexedNetwork`](crate::IndexedNetwork) view of the normalized network,
/// the row of the same element family in the source network. `None` marks an
/// element the pipeline synthesized, which has no source row.
///
/// Every field is positional over that view, so `buses[dense_index]` resolves
/// a matrix row back to its source element. Each length equals the matching
/// element table of the view: `buses` equals `view.n()`, `branches` equals
/// `view.branches().len()`.
///
/// The star lowering that the view applies to a 3-winding transformer appends
/// one bus, its star branches, and a magnetizing shunt. Those entries are
/// `None`. The lowering also consumes the transformer itself, so the view
/// holds none; `transformers_3w` therefore stays positional over the
/// normalized network's own list.
///
/// The map is valid only for the [`NormalizedNetwork`] returned beside it. A
/// later mutation of that network ([`BalancedNetwork::merge_bus`],
/// [`BalancedNetwork::reduce_zero_impedance`], [`BalancedNetwork::reduce_passthrough_buses`],
/// [`BalancedNetwork::subset`], or a hand edit) invalidates every entry; run the pass
/// again instead of patching the map.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NormalizeSourceRows {
    pub buses: Vec<Option<usize>>,
    pub loads: Vec<Option<usize>>,
    pub shunts: Vec<Option<usize>>,
    pub branches: Vec<Option<usize>>,
    pub switches: Vec<Option<usize>>,
    pub generators: Vec<Option<usize>>,
    pub storage: Vec<Option<usize>>,
    pub hvdc: Vec<Option<usize>>,
    pub transformers_3w: Vec<Option<usize>>,
}

impl NormalizeSourceRows {
    /// The map for a network that is already normalized: it is its own source,
    /// so every element maps to its own row. Positional over `net` itself —
    /// [`Self::pad_to_lowered`] extends it to the star-lowered view.
    pub(crate) fn identity(net: &BalancedNetwork) -> Self {
        let ident = |n: usize| (0..n).map(Some).collect();
        Self {
            buses: ident(net.buses().len()),
            loads: ident(net.loads().len()),
            shunts: ident(net.shunts().len()),
            branches: ident(net.branches().len()),
            switches: ident(net.switches().len()),
            generators: ident(net.generators().len()),
            storage: ident(net.storage().len()),
            hvdc: ident(net.hvdc().len()),
            transformers_3w: ident(net.transformers_3w().len()),
        }
    }

    /// Grow the families the star lowering appends to so each length matches the
    /// lowered form of `net`. The appended entries have no source row. The
    /// lengths come from [`BalancedNetwork::lowered_lengths`], which counts them off the
    /// transformer records, so padding never builds the lowering itself.
    pub(crate) fn pad_to_lowered(&mut self, net: &BalancedNetwork) {
        let lengths = net.lowered_lengths();
        self.buses.resize(lengths.buses, None);
        self.branches.resize(lengths.branches, None);
        self.shunts.resize(lengths.shunts, None);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CostModel {
    Piecewise,
    Polynomial,
    Unknown,
}

impl From<u8> for CostModel {
    fn from(value: u8) -> Self {
        match value {
            1 => CostModel::Piecewise,
            2 => CostModel::Polynomial,
            _ => CostModel::Unknown,
        }
    }
}

/// Gen cost coefficients rescaled into the per-unit basis, trimmed to the length
/// the model implies (a polynomial keeps `ncost` coeffs; a piecewise curve keeps
/// `2·ncost` `(mw, cost)` values). MATPOWER pads every gencost row to the matrix
/// width with trailing zeros; the padding would make a polynomial read as a
/// higher-degree curve and mis-scale, so it is dropped here.
///
/// Polynomial (model 2): coeff `i` is the term `p^(k-1-i)`, so per unit scales it
/// by `base^(k-1-i)`. Piecewise (model 1): the MW breakpoints (even positions) are
/// divided by `base`; the cost ordinates (odd positions) stay. Any other model has
/// unknown coefficient semantics, so it passes through untouched — the exact
/// inverse of [`cost_from_pu`]'s own passthrough.
pub(crate) fn cost_to_pu(cost: &GenCost, base: f64) -> Vec<f64> {
    let mut coeffs = cost.coeffs.clone();
    scale_coeffs_to_pu(&mut coeffs, cost.ncost, cost.model, base);
    coeffs
}

/// [`cost_to_pu`] over a vector the caller already owns, so a rescale in place
/// keeps its allocation.
pub(crate) fn scale_coeffs_to_pu(coeffs: &mut Vec<f64>, ncost: usize, model: u8, base: f64) {
    match CostModel::from(model) {
        CostModel::Polynomial => {
            coeffs.truncate(ncost.min(coeffs.len()));
            let k = coeffs.len();
            // The exponent k-1-i is in [0, k-1]; a polynomial never has i32::MAX-many
            // terms, so the conversion can't fail (loud, not silent, if it ever did).
            for (i, c) in coeffs.iter_mut().enumerate() {
                *c *= base.powi(i32::try_from(k - 1 - i).expect("cost degree fits i32"));
            }
        }
        CostModel::Piecewise => {
            // saturating_mul: `ncost` comes from input (JSON deserializes it
            // unchecked), so an oversized count must clamp to the coefficient
            // length instead of overflowing.
            coeffs.truncate(ncost.saturating_mul(2).min(coeffs.len()));
            for c in coeffs.iter_mut().step_by(2) {
                *c /= base;
            }
        }
        CostModel::Unknown => {}
    }
}

/// Undo [`cost_to_pu`] for the neutral MW basis: a polynomial (model 2) divides
/// coeff `i` by `base^(k-1-i)`, a piecewise curve (model 1) multiplies its MW
/// breakpoints (even positions) by `base`. The exact inverse of [`cost_to_pu`] on
/// the trimmed coefficient vector — JSON-sourced coefficients arrive already
/// trimmed, so this does no trimming; other models pass through unchanged.
pub(crate) fn cost_from_pu(coeffs: &[f64], model: u8, base: f64) -> Vec<f64> {
    let k = coeffs.len();
    match CostModel::from(model) {
        CostModel::Polynomial => coeffs
            .iter()
            .enumerate()
            .map(|(i, &c)| c / base.powi(i32::try_from(k - 1 - i).expect("cost degree fits i32")))
            .collect(),
        CostModel::Piecewise => coeffs
            .iter()
            .enumerate()
            .map(|(i, &c)| if i % 2 == 0 { c * base } else { c })
            .collect(),
        CostModel::Unknown => coeffs.to_vec(),
    }
}

/// Map a source bus id to its surviving normalized id, or `None` if the bus was dropped.
fn remap(map: &HashMap<BusId, BusId>, id: BusId) -> Option<BusId> {
    map.get(&id).copied()
}

fn norm_loads(
    loads: &[Load],
    base: f64,
    map: &HashMap<BusId, BusId>,
) -> (Vec<Load>, Vec<Option<usize>>) {
    loads
        .iter()
        .enumerate()
        .filter(|(_, l)| l.in_service)
        .filter_map(|(row, l)| {
            Some((
                Load {
                    bus: remap(map, l.bus)?,
                    p: l.p / base,
                    q: l.q / base,
                    voltage_model: l
                        .voltage_model
                        .as_ref()
                        .map(|m| norm_load_voltage_model(m, base)),
                    ..l.clone()
                },
                Some(row),
            ))
        })
        .unzip()
}

fn norm_load_voltage_model(model: &LoadVoltageModel, base: f64) -> LoadVoltageModel {
    match model {
        LoadVoltageModel::ConstantPower => LoadVoltageModel::ConstantPower,
        LoadVoltageModel::Zip {
            p_constant_power,
            q_constant_power,
            p_constant_current,
            q_constant_current,
            p_constant_impedance,
            q_constant_impedance,
            v_nom,
            load_type,
            scaling,
        } => LoadVoltageModel::Zip {
            p_constant_power: p_constant_power / base,
            q_constant_power: q_constant_power / base,
            p_constant_current: p_constant_current / base,
            q_constant_current: q_constant_current / base,
            p_constant_impedance: p_constant_impedance / base,
            q_constant_impedance: q_constant_impedance / base,
            v_nom: *v_nom,
            load_type: *load_type,
            scaling: *scaling,
        },
        LoadVoltageModel::Exponential {
            p,
            q,
            v_nom,
            gamma_p,
            gamma_q,
        } => LoadVoltageModel::Exponential {
            p: p / base,
            q: q / base,
            v_nom: *v_nom,
            gamma_p: *gamma_p,
            gamma_q: *gamma_q,
        },
    }
}

fn norm_shunts(
    shunts: &[Shunt],
    base: f64,
    map: &HashMap<BusId, BusId>,
) -> (Vec<Shunt>, Vec<Option<usize>>) {
    shunts
        .iter()
        .enumerate()
        .filter(|(_, s)| s.in_service)
        .filter_map(|(row, s)| {
            let mut shunt = s.clone();
            shunt.bus = remap(map, s.bus)?;
            shunt.g = s.g / base;
            shunt.b = s.b / base;
            // Remap the switched-shunt control bus and drop it if its target was
            // filtered out, so the normalized network has no dangling reference.
            if let Some(c) = &mut shunt.control {
                c.control_bus = c.control_bus.and_then(|b| remap(map, b));
            }
            Some((shunt, Some(row)))
        })
        .unzip()
}

fn norm_branches(
    branches: &[Branch],
    base: f64,
    map: &HashMap<BusId, BusId>,
) -> (Vec<Branch>, Vec<Option<usize>>) {
    branches
        .iter()
        .enumerate()
        .filter(|(_, br)| br.in_service)
        .filter_map(|(row, br)| {
            let mut branch = br.clone();
            branch.from = remap(map, br.from)?;
            branch.to = remap(map, br.to)?;
            branch.rate_a = br.rate_a / base;
            branch.rate_b = br.rate_b / base;
            branch.rate_c = br.rate_c / base;
            for set in &mut branch.rating_sets {
                set.rate_mva /= base;
            }
            branch.tap = br.calc_effective_tap();
            branch.shift = br.shift * DEG_TO_RAD;
            branch.angmin = br.angmin * DEG_TO_RAD;
            branch.angmax = br.angmax * DEG_TO_RAD;
            if let Some(s) = &mut branch.solution {
                s.pf /= base;
                s.qf /= base;
                s.pt /= base;
                s.qt /= base;
            }
            // Remap the regulated-bus reference through the id map and drop it
            // if its target was filtered out (out of service / isolated), so the
            // normalized network has no dangling control reference.
            if let Some(c) = &mut branch.control {
                c.controlled_bus = c.controlled_bus.and_then(|b| remap(map, b));
            }
            Some((branch, Some(row)))
        })
        .unzip()
}

fn validate_normalize_options(options: &NormalizeOptions) -> Result<()> {
    if options.clamp_angle_bounds
        && (!options.angle_bound_pad.is_finite()
            || options.angle_bound_pad <= 0.0
            || options.angle_bound_pad >= std::f64::consts::FRAC_PI_2)
    {
        return Err(Error::InvalidNormalizeOption {
            field: "angle_bound_pad",
            value: options.angle_bound_pad,
        });
    }
    Ok(())
}

fn clamp_angle_bounds(
    branches: &mut [Branch],
    pad: f64,
    warnings: &mut crate::diagnostics::Diagnostics,
) {
    for (idx, br) in branches.iter_mut().enumerate() {
        let old_min = br.angmin;
        let old_max = br.angmax;
        let mut changes = Vec::new();

        if old_min <= -std::f64::consts::FRAC_PI_2 {
            br.angmin = -pad;
            changes.push(format!("angmin {old_min} -> {}", br.angmin));
        }
        if old_max >= std::f64::consts::FRAC_PI_2 {
            br.angmax = pad;
            changes.push(format!("angmax {old_max} -> {}", br.angmax));
        }
        if old_min == 0.0 && old_max == 0.0 {
            br.angmin = -pad;
            br.angmax = pad;
            changes.push(format!("angmin/angmax 0 -> [{}, {}]", br.angmin, br.angmax));
        }
        if !changes.is_empty() && br.angmin > br.angmax {
            let repaired_min = br.angmin;
            let repaired_max = br.angmax;
            br.angmin = -pad;
            br.angmax = pad;
            changes.push(format!(
                "repaired interval {repaired_min}..{repaired_max} widened to [{}, {}]",
                br.angmin, br.angmax
            ));
        }

        if !changes.is_empty() {
            warnings.push(
                &crate::diagnostics::codes::CANONICALIZE_NORMALIZE_BOUNDS_CLAMPED,
                format!(
                    "branch {idx} angle difference bounds clamped: {}",
                    changes.join(", ")
                ),
            );
        }
    }
}

fn norm_gens(
    gens: &[Generator],
    base: f64,
    map: &HashMap<BusId, BusId>,
) -> (Vec<Generator>, Vec<Option<usize>>) {
    gens.iter()
        .enumerate()
        .filter(|(_, g)| g.in_service)
        .filter_map(|(row, g)| {
            let mut generator = g.clone();
            generator.bus = remap(map, g.bus)?;
            generator.pg = g.pg / base;
            generator.qg = g.qg / base;
            generator.pmax = g.pmax / base;
            generator.pmin = g.pmin / base;
            generator.qmax = g.qmax / base;
            generator.qmin = g.qmin / base;
            if let Some(c) = &mut generator.cost {
                scale_coeffs_to_pu(&mut c.coeffs, c.ncost, c.model, base);
            }
            // `GenCaps` is indexed by `GEN_EXTRA_KEYS`, so the two zip exactly.
            for (cap, key) in generator.caps.iter_mut().zip(GEN_EXTRA_KEYS) {
                if GEN_PU_KEYS.contains(&key)
                    && let Some(v) = cap
                {
                    *v /= base;
                }
            }
            // Remap the regulated bus through the same id map; drop it if its
            // target was filtered out so the normalized form stays consistent.
            generator.regulated_bus = g.regulated_bus.and_then(|b| remap(map, b));
            Some((generator, Some(row)))
        })
        .unzip()
}

fn norm_switches(
    switches: &[Switch],
    base: f64,
    map: &HashMap<BusId, BusId>,
) -> (Vec<Switch>, Vec<Option<usize>>) {
    switches
        .iter()
        .enumerate()
        .filter_map(|(row, s)| {
            let switch = Switch {
                from: remap(map, s.from)?,
                to: remap(map, s.to)?,
                thermal_rating: s.thermal_rating.map(|v| v / base),
                pf: s.pf.map(|v| v / base),
                qf: s.qf.map(|v| v / base),
                pt: s.pt.map(|v| v / base),
                qt: s.qt.map(|v| v / base),
                ..s.clone()
            };
            Some((switch, Some(row)))
        })
        .unzip()
}

fn norm_storage(
    storage: &[Storage],
    base: f64,
    map: &HashMap<BusId, BusId>,
) -> (Vec<Storage>, Vec<Option<usize>>) {
    storage
        .iter()
        .enumerate()
        .filter(|(_, s)| s.in_service)
        .filter_map(|(row, s)| {
            // ps/qs stay raw (PowerModels' make_per_unit! leaves the dispatch
            // setpoint alone); the energy, ratings, limits, and losses scale.
            let unit = Storage {
                bus: remap(map, s.bus)?,
                energy: s.energy / base,
                energy_rating: s.energy_rating / base,
                charge_rating: s.charge_rating / base,
                discharge_rating: s.discharge_rating / base,
                thermal_rating: s.thermal_rating / base,
                qmin: s.qmin / base,
                qmax: s.qmax / base,
                p_loss: s.p_loss / base,
                q_loss: s.q_loss / base,
                ..s.clone()
            };
            Some((unit, Some(row)))
        })
        .unzip()
}

fn norm_hvdc(
    hvdc: &[Hvdc],
    base: f64,
    map: &HashMap<BusId, BusId>,
) -> (Vec<Hvdc>, Vec<Option<usize>>) {
    hvdc.iter()
        .enumerate()
        .filter(|(_, d)| d.in_service)
        .filter_map(|(row, d)| {
            // No sign flip: the writer's Pt/Qf/Qt negation is a PowerModels output
            // convention, not part of per-unit normalization. The aggregate
            // pmin/pmax stay raw, matching make_per_unit!.
            let mut link = d.clone();
            link.from = remap(map, d.from)?;
            link.to = remap(map, d.to)?;
            link.pf = d.pf / base;
            link.pt = d.pt / base;
            link.qf = d.qf / base;
            link.qt = d.qt / base;
            link.qminf = d.qminf / base;
            link.qmaxf = d.qmaxf / base;
            link.qmint = d.qmint / base;
            link.qmaxt = d.qmaxt / base;
            link.loss0 = d.loss0 / base;
            if let Some(c) = &mut link.cost {
                scale_coeffs_to_pu(&mut c.coeffs, c.ncost, c.model, base);
            }
            Some((link, Some(row)))
        })
        .unzip()
}

fn norm_transformers_3w(
    xfmrs: &[Transformer3W],
    base: f64,
    map: &HashMap<BusId, BusId>,
) -> (Vec<Transformer3W>, Vec<Option<usize>>) {
    xfmrs
        .iter()
        .enumerate()
        .filter(|(_, t)| t.in_service)
        .filter_map(|(row, t)| {
            // Remap each winding terminal and drop the whole unit if any was filtered
            // out (a 3-winding transformer can't keep a dangling winding). Phase
            // shifts and the star angle go to radians; winding ratings go per unit;
            // the pairwise impedances are already per unit on the system base.
            let mut windings = t.windings.clone();
            for w in &mut windings {
                w.bus = remap(map, w.bus)?;
                w.shift *= DEG_TO_RAD;
                w.rate_a /= base;
                w.rate_b /= base;
                w.rate_c /= base;
            }
            Some((
                Transformer3W {
                    windings,
                    star_va: t.star_va * DEG_TO_RAD,
                    ..t.clone()
                },
                Some(row),
            ))
        })
        .unzip()
}

/// No reference survived the bus type pass: anchor the slack at the largest
/// pmax in-service generator's bus and record the designation on the coded
/// channel, or refuse when there is no generator to anchor it.
fn designate_reference(
    buses: &mut [Bus],
    generators: &[Generator],
    warnings: &mut crate::diagnostics::Diagnostics,
) -> Result<()> {
    let slack = generators
        .iter()
        .max_by(|a, b| {
            // A NaN pmax must never win the slack: map it below every real
            // bound so the choice stays deterministic (an unbounded +Inf
            // pmax still wins, as the largest capacity).
            let key = |p: f64| if p.is_nan() { f64::NEG_INFINITY } else { p };
            key(a.pmax).total_cmp(&key(b.pmax))
        })
        .map(|g| g.bus)
        .ok_or(Error::NoReferenceBus)?;
    if let Some(b) = buses.iter_mut().find(|b| b.id == slack) {
        b.kind = BusType::Ref;
        warnings.push(
            &crate::diagnostics::codes::CANONICALIZE_NORMALIZE_REFERENCE_DESIGNATED,
            format!(
                "the case states no reference bus that survives normalization; bus {slack} \
                 hosts the largest pmax in-service generator and was designated the slack"
            ),
        );
    }
    Ok(())
}

impl BalancedNetwork {
    /// A normalized, computation-ready copy of this network. The raw `BalancedNetwork` is
    /// kept lossless (MATPOWER units, 1-based sparse ids, out-of-service elements
    /// retained); `to_normalized` derives the form a solver or ML pipeline wants:
    ///
    /// - **Per unit** (÷`base_mva`): gen `pg/qg/pmax/pmin/qmax/qmin` and the ramp
    ///   caps (`GEN_PU_KEYS`); load `p/q`; shunt `g/b`; branch `rate_a/b/c`;
    ///   storage energy/ratings/limits/losses; HVDC `pf/pt/qf/qt`, reactive limits,
    ///   `loss0`; gen-cost coefficients (`cost_to_pu`). Storage `ps/qs` and HVDC
    ///   aggregate `pmin/pmax` stay raw, matching the PowerModels per-unit
    ///   convention. Voltages, impedances, tap, and `loss1` are already
    ///   dimensionless.
    /// - **Radians**: bus `va`; branch `shift/angmin/angmax`.
    /// - **Tap**: `0 → 1.0` (an explicit `1` is kept).
    /// - **Filtered**: drop buses typed isolated (`BusType::Isolated`) and every
    ///   out-of-service element, then drop any element left referencing a dropped
    ///   bus. A bus orphaned by the out-of-service filter (no in-service branch,
    ///   but not typed isolated) is kept — its load is real — and surfaces as its
    ///   own island, which the grounding check reports if it has no reference.
    /// - **IDs**: kept buses retain their source bus ids, and every surviving
    ///   endpoint stays in the same id space. Consumers that need dense rows should
    ///   use [`IndexedNetwork`](crate::IndexedNetwork), which derives `[0, n)`
    ///   indices without destroying source ids.
    /// - **Bus types**: a bus hosting a surviving generator keeps `REF` if the file
    ///   marked it `REF`, otherwise becomes `PV`; a generator-less bus is `PQ` (so a
    ///   generator-less `REF` is demoted). The file's `REF` buses are kept, several
    ///   included, and the consumer picks the slack. Only when no reference bus
    ///   survives is the largest-`pmax` in-service generator's bus promoted to
    ///   `REF`.
    ///
    /// This is a derived product, not a source for write-back: `source` is dropped
    /// and `source_format` is [`SourceFormat::Normalized`], so writing it serializes
    /// the per-unit/radian model instead of echoing the raw bytes, and a consumer
    /// can tell it apart from a raw in-memory network.
    ///
    /// Scope is the universal canonicalization only. It does not synthesize a
    /// missing `rate_a` or restrict the gen-cost model — those are solver
    /// preparation choices a consumer applies on top. Use
    /// [`BalancedNetwork::to_normalized_with_options`] for the opt in PowerModels angle
    /// bound repair. The cost *rescale* is
    /// universal and lives here; the model *restriction* does not.
    ///
    /// # Errors
    /// [`Error::InvalidBaseMva`] if `base_mva` is not a positive, finite number
    /// (every per-unit divisor), so a malformed base can't silently poison the
    /// whole network with `NaN`/`Inf` or sign-flipped values.
    /// [`Error::NoReferenceBus`] if no reference bus can be established — no `REF`
    /// survives and there is no in-service generator to anchor one.
    pub fn to_normalized(&self) -> Result<BalancedNetwork> {
        Ok(self
            .to_normalized_with_options(&NormalizeOptions::default())?
            .network)
    }

    /// Like [`BalancedNetwork::to_normalized`], with opt in solver preparation repairs
    /// that report fidelity warnings.
    pub fn to_normalized_with_options(
        &self,
        options: &NormalizeOptions,
    ) -> Result<NormalizedNetwork> {
        Ok(self.normalize_inner(options)?.0)
    }

    /// Like [`BalancedNetwork::to_normalized_with_options`], also returning the
    /// [`NormalizeSourceRows`] row provenance.
    ///
    /// The rows are positional over the
    /// [`IndexedNetwork`](crate::IndexedNetwork) view of the returned network,
    /// which is the index space a matrix row or a solver table row lives in.
    /// That view star-lowers a 3-winding transformer, so it holds more buses,
    /// branches, and shunts than the returned [`NormalizedNetwork`] does;
    /// indexing the returned network by a row position is out of bounds on any
    /// case that carries one. Resolve a row through the view:
    ///
    /// ```
    /// # use powerio_tx::{IndexedNetwork, BalancedNetwork, NormalizeOptions};
    /// # fn f(raw: &BalancedNetwork) -> powerio_tx::Result<()> {
    /// let (normalized, rows) = raw.to_normalized_with_source_rows(&NormalizeOptions::default())?;
    /// let view = IndexedNetwork::new(&normalized.network);
    /// for (dense, source) in rows.buses.iter().enumerate() {
    ///     let bus = &view.network().buses()[dense];
    ///     // `source` is `None` for the synthetic star bus the view appended.
    ///     let _ = (bus, source);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[doc(hidden)]
    pub fn to_normalized_with_source_rows(
        &self,
        options: &NormalizeOptions,
    ) -> Result<(NormalizedNetwork, NormalizeSourceRows)> {
        let (normalized, mut rows) = self.normalize_inner(options)?;
        rows.pad_to_lowered(&normalized.network);
        Ok((normalized, rows))
    }

    /// The pass itself. The rows it gives cover the normalized network before
    /// the star lowering, so only [`Self::to_normalized_with_source_rows`] pays
    /// for the lowered lengths.
    fn normalize_inner(
        &self,
        options: &NormalizeOptions,
    ) -> Result<(NormalizedNetwork, NormalizeSourceRows)> {
        validate_normalize_options(options)?;
        self.check_base_mva()?;
        let base = self.base_mva();

        // Kept buses keep their original `kind` for now (the reference scan below
        // reads it) and their source ids. Isolated buses are dropped.
        let mut id_map: HashMap<BusId, BusId> = HashMap::with_capacity(self.buses().len());
        let mut buses: Vec<Bus> = Vec::with_capacity(self.buses().len());
        // The pass keeps only elements that came from a source row, so each row
        // here is `Some`; the `None` entries appear later, when
        // `pad_to_lowered` extends the map over what the star lowering appends.
        let mut bus_rows: Vec<Option<usize>> = Vec::with_capacity(self.buses().len());
        for (row, b) in self.buses().iter().enumerate() {
            if b.kind == BusType::Isolated {
                continue;
            }
            id_map.insert(b.id, b.id);
            buses.push(Bus {
                va: b.va * DEG_TO_RAD,
                ..b.clone()
            });
            bus_rows.push(Some(row));
        }
        let (loads, load_rows) = norm_loads(self.loads(), base, &id_map);
        let (shunts, shunt_rows) = norm_shunts(self.shunts(), base, &id_map);
        let (mut branches, branch_rows) = norm_branches(self.branches(), base, &id_map);
        let mut warnings = crate::diagnostics::Diagnostics::new();
        if options.clamp_angle_bounds {
            clamp_angle_bounds(&mut branches, options.angle_bound_pad, &mut warnings);
        }
        let (switches, switch_rows) = norm_switches(self.switches(), base, &id_map);
        let (generators, generator_rows) = norm_gens(self.generators(), base, &id_map);
        let (storage, storage_rows) = norm_storage(self.storage(), base, &id_map);
        let (hvdc, hvdc_rows) = norm_hvdc(self.hvdc(), base, &id_map);
        let (transformers_3w, transformer_3w_rows) =
            norm_transformers_3w(self.transformers_3w(), base, &id_map);
        let source_rows = NormalizeSourceRows {
            buses: bus_rows,
            loads: load_rows,
            shunts: shunt_rows,
            branches: branch_rows,
            switches: switch_rows,
            generators: generator_rows,
            storage: storage_rows,
            hvdc: hvdc_rows,
            transformers_3w: transformer_3w_rows,
        };

        // Bus types: a bus hosting an in-service generator keeps `Ref` if the
        // file marked it `Ref`, else becomes `Pv`; a gen-less bus is `Pq`.
        // Multiple file `Ref` buses are kept as-is, and only when no `Ref`
        // survives is the largest-pmax generator's bus promoted.
        let gen_buses: HashSet<BusId> = generators.iter().map(|g| g.bus).collect();
        for b in &mut buses {
            b.kind = match (gen_buses.contains(&b.id), b.kind) {
                (true, BusType::Ref) => BusType::Ref,
                (true, _) => BusType::Pv,
                (false, _) => BusType::Pq,
            };
        }
        if !buses.iter().any(|b| b.kind == BusType::Ref) {
            designate_reference(&mut buses, &generators, &mut warnings)?;
        }
        // The other silent semantic decision this gateway announces: a
        // solver-ready copy whose cost objective is identically zero.
        if !generators.is_empty() && generators.iter().all(|g| g.cost.is_none()) {
            warnings.push(
                &crate::diagnostics::codes::CANONICALIZE_NORMALIZE_GEN_COST_ABSENT,
                format!(
                    "the case has {} in-service generator(s) and no cost data; any cost \
                     objective built from it is identically zero",
                    generators.len()
                ),
            );
        }

        let net = BalancedNetwork::from_tables(BalancedNetworkTables {
            name: self.name().clone(),
            base_mva: base,
            base_frequency: self.base_frequency(),
            geo: self.geo().clone(),
            buses: buses.into(),
            loads: loads.into(),
            shunts: shunts.into(),
            branches: branches.into(),
            switches: switches.into(),
            generators: generators.into(),
            storage: storage.into(),
            hvdc: hvdc.into(),
            transformers_3w: transformers_3w.into(),
            // Areas (interchange schedule, per-area swing) are interchange metadata,
            // not part of the per unit electrical view, so they are not carried.
            areas: Vec::new().into(),
            solver: None,
            source_format: SourceFormat::Normalized,
        });
        // The filter drops every reference to a dropped bus by
        // construction, so the result is reference-consistent. Assert it in
        // debug builds to catch a future regression in the filtering logic.
        debug_assert!(
            net.validate().is_ok(),
            "to_normalized produced a dangling reference"
        );
        Ok((
            NormalizedNetwork {
                network: net,
                warnings: warnings.lines(),
                diagnostics: warnings.into_records(),
            },
            source_rows,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn angle_bound_fixture() -> BalancedNetwork {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/angle_bounds_clamp.m");
        crate::parse_file(path, None).unwrap().network
    }

    #[test]
    fn angle_bound_clamp_is_opt_in_and_matches_powermodels_rules() {
        let net = angle_bound_fixture();

        let plain = net.to_normalized().unwrap();
        assert!(approx(plain.branches()[0].angmin, -std::f64::consts::TAU));
        assert!(approx(plain.branches()[0].angmax, std::f64::consts::TAU));
        assert!(approx(plain.branches()[1].angmin, 0.0));
        assert!(approx(plain.branches()[1].angmax, 0.0));
        assert!(approx(plain.branches()[3].angmin, -120.0 * DEG_TO_RAD));
        assert!(approx(plain.branches()[3].angmax, -100.0 * DEG_TO_RAD));
        assert!(approx(plain.branches()[4].angmin, 100.0 * DEG_TO_RAD));
        assert!(approx(plain.branches()[4].angmax, 120.0 * DEG_TO_RAD));

        let out = net
            .to_normalized_with_options(&NormalizeOptions {
                clamp_angle_bounds: true,
                ..NormalizeOptions::default()
            })
            .unwrap();
        // The fixture also carries no gencost, so the costless-case warning
        // rides beside the clamp lines; hold the clamp set on its own code.
        let clamps: Vec<&String> = out
            .warnings
            .iter()
            .filter(|w| w.contains("BOUNDS_CLAMPED"))
            .collect();
        assert_eq!(clamps.len(), 4, "{:?}", out.warnings);
        assert!(clamps[0].contains("branch 0"));
        assert!(clamps[1].contains("branch 1"));
        assert!(clamps[2].contains("branch 3"));
        assert!(clamps[3].contains("branch 4"));

        let branches = &out.network.branches();
        assert!(approx(branches[0].angmin, -POWER_MODELS_ANGLE_BOUND_PAD));
        assert!(approx(branches[0].angmax, POWER_MODELS_ANGLE_BOUND_PAD));
        assert!(approx(branches[1].angmin, -POWER_MODELS_ANGLE_BOUND_PAD));
        assert!(approx(branches[1].angmax, POWER_MODELS_ANGLE_BOUND_PAD));
        assert!(approx(branches[2].angmin, -30.0 * DEG_TO_RAD));
        assert!(approx(branches[2].angmax, 30.0 * DEG_TO_RAD));
        assert!(approx(branches[3].angmin, -POWER_MODELS_ANGLE_BOUND_PAD));
        assert!(approx(branches[3].angmax, POWER_MODELS_ANGLE_BOUND_PAD));
        assert!(approx(branches[4].angmin, -POWER_MODELS_ANGLE_BOUND_PAD));
        assert!(approx(branches[4].angmax, POWER_MODELS_ANGLE_BOUND_PAD));
        assert!(branches.iter().all(|br| br.angmin <= br.angmax));
    }

    #[test]
    fn angle_bound_clamp_rejects_invalid_pad() {
        let net = angle_bound_fixture();
        let err = net
            .to_normalized_with_options(&NormalizeOptions {
                clamp_angle_bounds: true,
                angle_bound_pad: std::f64::consts::FRAC_PI_2,
            })
            .unwrap_err();
        assert!(matches!(
            err,
            Error::InvalidNormalizeOption {
                field: "angle_bound_pad",
                ..
            }
        ));
    }

    #[test]
    fn to_normalized_drops_a_control_bus_whose_target_was_filtered_out() {
        use crate::network::{Extras, SwitchedShuntControl, SwitchedShuntMode};

        let mkbus = |id: usize, kind: BusType| Bus {
            id: BusId(id),
            kind,
            vm: 1.0,
            va: 0.0,
            base_kv: 230.0,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: None,
            uid: None,
            location: None,
            extras: Extras::new(),
        };
        let branch = Branch {
            from: BusId(1),
            to: BusId(2),
            r: 0.0,
            x: 0.1,
            b: 0.0,
            charging: None,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
            rating_sets: Vec::new(),
            current_ratings: None,
            tap: 0.0,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            solution: None,
            uid: None,
            route: None,
            extras: Extras::new(),
        };
        // Bus 3 is isolated, so to_normalized drops it.
        let mut net = BalancedNetwork::in_memory(
            "n",
            100.0,
            vec![
                mkbus(1, BusType::Ref),
                mkbus(2, BusType::Pq),
                mkbus(3, BusType::Isolated),
            ],
            vec![branch],
        );
        net.generators_mut().push(Generator {
            bus: BusId(1),
            pg: 10.0,
            qg: 0.0,
            pmax: 100.0,
            pmin: 0.0,
            qmax: 50.0,
            qmin: -50.0,
            vg: 1.0,
            mbase: 100.0,
            in_service: true,
            cost: None,
            caps: Default::default(),
            regulated_bus: None,
            uid: None,
        });
        // A switched shunt on bus 2 whose control bus is the (dropped) isolated bus 3.
        net.shunts_mut().push(Shunt {
            bus: BusId(2),
            g: 0.0,
            b: 10.0,
            in_service: true,
            control: Some(SwitchedShuntControl {
                mode: SwitchedShuntMode::Discrete,
                vhigh: 1.05,
                vlow: 0.95,
                control_bus: Some(BusId(3)),
                rmpct: 100.0,
                blocks: Vec::new(),
            }),
            uid: None,
            extras: Extras::new(),
        });

        let norm = net.to_normalized().unwrap();
        norm.validate().unwrap();
        let c = norm.shunts()[0].control.as_ref().expect("control retained");
        assert_eq!(
            c.control_bus, None,
            "a control bus pointing at a filtered-out isolated bus is dropped, not left dangling"
        );
    }

    #[test]
    fn normalized_slack_tiebreak_ignores_nan_pmax() {
        use crate::network::Extras;

        let mkbus = |id: usize| Bus {
            id: BusId(id),
            kind: BusType::Pq,
            vm: 1.0,
            va: 0.0,
            base_kv: 230.0,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: None,
            uid: None,
            location: None,
            extras: Extras::new(),
        };
        let mkgen = |bus: usize, pmax: f64| Generator {
            bus: BusId(bus),
            pg: 0.0,
            qg: 0.0,
            pmax,
            pmin: 0.0,
            qmax: 0.0,
            qmin: 0.0,
            vg: 1.0,
            mbase: 100.0,
            in_service: true,
            cost: None,
            caps: Default::default(),
            regulated_bus: None,
            uid: None,
        };
        let mut net = BalancedNetwork::in_memory("n", 100.0, vec![mkbus(1), mkbus(2)], Vec::new());
        *net.generators_mut() = vec![mkgen(1, f64::NAN), mkgen(2, 10.0)];
        let norm = net.to_normalized().unwrap();

        assert_eq!(
            norm.buses().iter().find(|b| b.id == BusId(1)).unwrap().kind,
            BusType::Pv
        );
        assert_eq!(
            norm.buses().iter().find(|b| b.id == BusId(2)).unwrap().kind,
            BusType::Ref
        );
    }

    #[test]
    fn cost_to_pu_polynomial_scales_and_trims() {
        // Model 2: the coeff of p^j scales by base^j; MATPOWER's trailing-zero
        // padding (beyond ncost) is dropped.
        let cost = GenCost {
            model: 2,
            startup: 0.0,
            shutdown: 0.0,
            ncost: 2,
            coeffs: vec![24.035, -403.5, 0.0, 0.0, 0.0, 0.0],
        };
        let out = cost_to_pu(&cost, 100.0);
        assert_eq!(out.len(), 2, "padding dropped");
        assert!(approx(out[0], 2403.5)); // 24.035 · 100^1
        assert!(approx(out[1], -403.5)); // -403.5 · 100^0
    }

    #[test]
    fn cost_to_pu_piecewise_scales_mw_only_and_trims() {
        // Model 1: MW breakpoints (even positions) ÷ base; cost ordinates (odd) raw.
        let cost = GenCost {
            model: 1,
            startup: 0.0,
            shutdown: 0.0,
            ncost: 4,
            coeffs: vec![
                0.0, 0.0, 100.0, 2500.0, 200.0, 5500.0, 250.0, 7250.0, 0.0, 0.0,
            ],
        };
        let out = cost_to_pu(&cost, 100.0);
        assert_eq!(out.len(), 8, "trimmed to 2·ncost, padding dropped");
        assert!(
            approx(out[0], 0.0)
                && approx(out[2], 1.0)
                && approx(out[4], 2.0)
                && approx(out[6], 2.5)
        );
        assert!(
            approx(out[1], 0.0)
                && approx(out[3], 2500.0)
                && approx(out[5], 5500.0)
                && approx(out[7], 7250.0)
        );
    }

    #[test]
    fn cost_rescale_round_trips() {
        // c2 p² + c1 p + c0 with base 100: per unit then back is the identity.
        let cost = GenCost {
            model: 2,
            startup: 0.0,
            shutdown: 0.0,
            ncost: 3,
            coeffs: vec![0.11, 5.0, 150.0],
        };
        let pu = cost_to_pu(&cost, 100.0);
        // p^2 coeff scales by 100^2, p^1 by 100, constant unchanged.
        assert!((pu[0] - 0.11 * 100.0 * 100.0).abs() < 1e-9);
        assert!((pu[1] - 5.0 * 100.0).abs() < 1e-9);
        assert!((pu[2] - 150.0).abs() < 1e-9);
        let back = cost_from_pu(&pu, 2, 100.0);
        for (a, b) in back.iter().zip(&cost.coeffs) {
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn cost_rescale_passes_through_unknown_model() {
        // A model outside {1,2} has unknown coefficient semantics, so neither
        // direction may touch it; to_pu and from_pu must both be the identity,
        // or the round trip silently corrupts a curve we don't understand.
        let cost = GenCost {
            model: 0,
            startup: 0.0,
            shutdown: 0.0,
            ncost: 2,
            coeffs: vec![3.0, 7.0, 9.0],
        };
        let pu = cost_to_pu(&cost, 100.0);
        assert_eq!(pu, cost.coeffs, "to_pu must not scale an unknown model");
        let back = cost_from_pu(&pu, cost.model, 100.0);
        assert_eq!(back, cost.coeffs, "from_pu must not scale an unknown model");
    }

    #[test]
    fn cost_rescale_round_trips_piecewise() {
        // Model 1: cost_from_pu multiplies the MW breakpoints back by base and
        // leaves the cost ordinates, the exact inverse of cost_to_pu's even/odd
        // split. (cost_to_pu trims, cost_from_pu doesn't, so feed a trimmed row.)
        let cost = GenCost {
            model: 1,
            startup: 0.0,
            shutdown: 0.0,
            ncost: 4,
            coeffs: vec![0.0, 0.0, 100.0, 2500.0, 200.0, 5500.0, 250.0, 7250.0],
        };
        let pu = cost_to_pu(&cost, 100.0);
        let back = cost_from_pu(&pu, 1, 100.0);
        for (a, b) in back.iter().zip(&cost.coeffs) {
            assert!((a - b).abs() < 1e-9, "{a} != {b}");
        }
    }
}

//! The universal normalization shared by the PowerModels reader/writer and
//! [`Network::to_normalized`].
//!
//! Two things live here so there is one implementation of each:
//!
//! - **Per-unit scaling factors and the gen-cost rescale** ([`cost_to_pu`] /
//!   [`cost_from_pu`], [`DEG_TO_RAD`] / [`RAD_TO_DEG`], [`GEN_PU_KEYS`]). The
//!   PowerModels writer scales raw model values into its per-unit JSON; the
//!   reader inverts it; [`Network::to_normalized`] scales the same way into a new
//!   `Network`. The cost rescale is the one piece subtle enough that a second copy
//!   would drift, so it has a single home.
//! - **[`Network::to_normalized`]**: a derived, computation-ready view — per unit,
//!   radians, out-of-service filtered, densely reindexed, bus types canonicalized.

use std::collections::{HashMap, HashSet};

use crate::network::{
    Branch, Bus, BusId, BusType, GEN_EXTRA_KEYS, GenCost, Generator, Hvdc, Load, Network, Shunt,
    SourceFormat, Storage,
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
/// between the reader, the writer, and [`Network::to_normalized`].
pub(crate) const GEN_PU_KEYS: [&str; 4] = ["ramp_agc", "ramp_10", "ramp_30", "ramp_q"];

/// Gen cost coefficients rescaled into the per-unit basis, trimmed to the length
/// the model implies (a polynomial keeps `ncost` coeffs; a piecewise curve keeps
/// `2·ncost` `(mw, cost)` values). MATPOWER pads every gencost row to the matrix
/// width with trailing zeros; the padding would make a polynomial read as a
/// higher-degree curve and mis-scale, so it is dropped here.
///
/// Polynomial (model 2): coeff `i` is the term `p^(k-1-i)`, so per unit scales it
/// by `base^(k-1-i)`. Piecewise (model 1): the MW breakpoints (even positions) are
/// divided by `base`; the dollar costs (odd positions) stay. Any other model has
/// unknown coefficient semantics, so it passes through untouched — the exact
/// inverse of [`cost_from_pu`]'s own passthrough.
pub(crate) fn cost_to_pu(cost: &GenCost, base: f64) -> Vec<f64> {
    match cost.model {
        2 => {
            let coeffs = &cost.coeffs[..cost.ncost.min(cost.coeffs.len())];
            let k = coeffs.len();
            // The exponent k-1-i is in [0, k-1]; a polynomial never has i32::MAX-many
            // terms, so the conversion can't fail (loud, not silent, if it ever did).
            coeffs
                .iter()
                .enumerate()
                .map(|(i, &c)| {
                    c * base.powi(i32::try_from(k - 1 - i).expect("cost degree fits i32"))
                })
                .collect()
        }
        1 => {
            let coeffs = &cost.coeffs[..(cost.ncost * 2).min(cost.coeffs.len())];
            coeffs
                .iter()
                .enumerate()
                .map(|(i, &c)| if i % 2 == 0 { c / base } else { c })
                .collect()
        }
        _ => cost.coeffs.clone(),
    }
}

/// Undo [`cost_to_pu`] for the neutral MW basis: a polynomial (model 2) divides
/// coeff `i` by `base^(k-1-i)`, a piecewise curve (model 1) multiplies its MW
/// breakpoints (even positions) by `base`. The exact inverse of [`cost_to_pu`] on
/// the trimmed coefficient vector — JSON-sourced coefficients arrive already
/// trimmed, so this does no trimming; other models pass through unchanged.
pub(crate) fn cost_from_pu(coeffs: &[f64], model: u8, base: f64) -> Vec<f64> {
    let k = coeffs.len();
    if model == 2 {
        coeffs
            .iter()
            .enumerate()
            .map(|(i, &c)| c / base.powi(i32::try_from(k - 1 - i).expect("cost degree fits i32")))
            .collect()
    } else if model == 1 {
        coeffs
            .iter()
            .enumerate()
            .map(|(i, &c)| if i % 2 == 0 { c * base } else { c })
            .collect()
    } else {
        coeffs.to_vec()
    }
}

/// Map an old bus id to its new dense id, or `None` if the bus was dropped.
fn remap(map: &HashMap<BusId, BusId>, id: BusId) -> Option<BusId> {
    map.get(&id).copied()
}

fn norm_loads(loads: &[Load], base: f64, map: &HashMap<BusId, BusId>) -> Vec<Load> {
    loads
        .iter()
        .filter(|l| l.in_service)
        .filter_map(|l| {
            Some(Load {
                bus: remap(map, l.bus)?,
                p: l.p / base,
                q: l.q / base,
                ..l.clone()
            })
        })
        .collect()
}

fn norm_shunts(shunts: &[Shunt], base: f64, map: &HashMap<BusId, BusId>) -> Vec<Shunt> {
    shunts
        .iter()
        .filter(|s| s.in_service)
        .filter_map(|s| {
            Some(Shunt {
                bus: remap(map, s.bus)?,
                g: s.g / base,
                b: s.b / base,
                ..s.clone()
            })
        })
        .collect()
}

fn norm_branches(branches: &[Branch], base: f64, map: &HashMap<BusId, BusId>) -> Vec<Branch> {
    branches
        .iter()
        .filter(|br| br.in_service)
        .filter_map(|br| {
            Some(Branch {
                from: remap(map, br.from)?,
                to: remap(map, br.to)?,
                rate_a: br.rate_a / base,
                rate_b: br.rate_b / base,
                rate_c: br.rate_c / base,
                tap: br.effective_tap(),
                shift: br.shift * DEG_TO_RAD,
                angmin: br.angmin * DEG_TO_RAD,
                angmax: br.angmax * DEG_TO_RAD,
                ..br.clone()
            })
        })
        .collect()
}

fn norm_gens(gens: &[Generator], base: f64, map: &HashMap<BusId, BusId>) -> Vec<Generator> {
    gens.iter()
        .filter(|g| g.in_service)
        .filter_map(|g| {
            let bus = remap(map, g.bus)?;
            let mut caps = g.caps;
            for (i, key) in GEN_EXTRA_KEYS.iter().enumerate() {
                if GEN_PU_KEYS.contains(key) {
                    if let Some(v) = caps[i] {
                        caps[i] = Some(v / base);
                    }
                }
            }
            Some(Generator {
                bus,
                pg: g.pg / base,
                qg: g.qg / base,
                pmax: g.pmax / base,
                pmin: g.pmin / base,
                qmax: g.qmax / base,
                qmin: g.qmin / base,
                cost: g.cost.as_ref().map(|c| GenCost {
                    coeffs: cost_to_pu(c, base),
                    ..c.clone()
                }),
                caps,
                ..g.clone()
            })
        })
        .collect()
}

fn norm_storage(storage: &[Storage], base: f64, map: &HashMap<BusId, BusId>) -> Vec<Storage> {
    storage
        .iter()
        .filter(|s| s.in_service)
        .filter_map(|s| {
            // ps/qs stay raw (PowerModels' make_per_unit! leaves the dispatch
            // setpoint alone); the energy, ratings, limits, and losses scale.
            Some(Storage {
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
            })
        })
        .collect()
}

fn norm_hvdc(hvdc: &[Hvdc], base: f64, map: &HashMap<BusId, BusId>) -> Vec<Hvdc> {
    hvdc.iter()
        .filter(|d| d.in_service)
        .filter_map(|d| {
            // No sign flip: the writer's Pt/Qf/Qt negation is a PowerModels output
            // convention, not part of per-unit normalization. The aggregate
            // pmin/pmax stay raw, matching make_per_unit!.
            Some(Hvdc {
                from: remap(map, d.from)?,
                to: remap(map, d.to)?,
                pf: d.pf / base,
                pt: d.pt / base,
                qf: d.qf / base,
                qt: d.qt / base,
                qminf: d.qminf / base,
                qmaxf: d.qmaxf / base,
                qmint: d.qmint / base,
                qmaxt: d.qmaxt / base,
                loss0: d.loss0 / base,
                ..d.clone()
            })
        })
        .collect()
}

impl Network {
    /// A normalized, computation-ready copy of this network. The raw `Network` is
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
    /// - **Reindexed**: kept buses get a dense 1-based id (their position among the
    ///   survivors), and every endpoint is remapped to match.
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
    /// Scope is the universal canonicalization only. It does not pad angle bounds,
    /// synthesize a missing `rate_a`, or restrict the gen-cost model — those are
    /// solver-prep choices a consumer applies on top. The cost *rescale* is
    /// universal and lives here; the model *restriction* does not.
    ///
    /// # Errors
    /// [`Error::InvalidBaseMva`] if `base_mva` is not a positive, finite number
    /// (every per-unit divisor), so a malformed base can't silently poison the
    /// whole network with `NaN`/`Inf` or sign-flipped values.
    /// [`Error::ReferenceBusCount`] if no reference bus can be established — no `REF`
    /// survives and there is no in-service generator to anchor one.
    pub fn to_normalized(&self) -> Result<Network> {
        self.check_base_mva()?;
        let base = self.base_mva;

        // Kept buses keep their original `kind` for now (the reference scan below
        // reads it); the new id is the 1-based position among survivors. Isolated
        // buses are dropped.
        let mut id_map: HashMap<BusId, BusId> = HashMap::with_capacity(self.buses.len());
        let mut buses: Vec<Bus> = Vec::with_capacity(self.buses.len());
        for b in &self.buses {
            if b.kind == BusType::Isolated {
                continue;
            }
            let new_id = BusId(buses.len() + 1);
            id_map.insert(b.id, new_id);
            buses.push(Bus {
                id: new_id,
                va: b.va * DEG_TO_RAD,
                ..b.clone()
            });
        }
        let loads = norm_loads(&self.loads, base, &id_map);
        let shunts = norm_shunts(&self.shunts, base, &id_map);
        let branches = norm_branches(&self.branches, base, &id_map);
        let generators = norm_gens(&self.generators, base, &id_map);
        let storage = norm_storage(&self.storage, base, &id_map);
        let hvdc = norm_hvdc(&self.hvdc, base, &id_map);

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
            // No reference survived: anchor the slack at the largest-pmax in-service
            // generator's bus, or error when there is no generator to anchor it.
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
                .ok_or(Error::ReferenceBusCount { found: 0 })?;
            if let Some(b) = buses.iter_mut().find(|b| b.id == slack) {
                b.kind = BusType::Ref;
            }
        }

        let net = Network {
            name: self.name.clone(),
            base_mva: base,
            buses,
            loads,
            shunts,
            branches,
            generators,
            storage,
            hvdc,
            source_format: SourceFormat::Normalized,
            source: None,
        };
        // The filter+remap drops every reference to a dropped bus by
        // construction, so the result is reference-consistent. Assert it in
        // debug builds to catch a future regression in the remap logic.
        debug_assert!(
            net.validate().is_ok(),
            "to_normalized produced a dangling reference"
        );
        Ok(net)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
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
        // Model 1: MW breakpoints (even positions) ÷ base; dollar costs (odd) raw.
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
        // leaves the dollar costs, the exact inverse of cost_to_pu's even/odd
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

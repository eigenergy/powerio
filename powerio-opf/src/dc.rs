//! Index based DC-OPF instance data derived from the network's generators and
//! their cost curves: cost, bounds, thermal limits, the generator→bus map, and
//! nodal load.
//!
//! The instance is input data keyed by per class position indices, and carries
//! no matrices. The generator→bus map is the `bus_of_col` index vector, the
//! index space stand-in for a sparse `C_g`, so a consumer builds whatever
//! incidence or susceptance structure its formulation needs. `powerio-matrix`
//! keeps the graphical form (a sparse `C_g` and the Matrix Market bundle writer)
//! for consumers that want it.

use serde::{Deserialize, Serialize};

use powerio::IndexedNetwork;

use crate::error::{Error, Result};

/// Unit system for the emitted quantities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Units {
    /// Power divided by `baseMVA`, with cost coefficients scaled so the cost is
    /// a function of per unit power (`q ← 2c₂·base²`, `c ← c₁·base`). An
    /// already normalized network is per unit, so this leaves it unchanged (the
    /// scaling divisor is `1.0`).
    #[default]
    PerUnit,
    /// Raw MATPOWER units: power in MW, cost in native `$·MWh⁻¹` coefficients.
    Native,
}

/// Length-n bus-indexed cost and bound vectors. All share index space; each is
/// zero at buses with no generator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BusCosts {
    /// Quadratic cost diagonal `q`, `cost = ½ q p² + c p`.
    pub q: Vec<f64>,
    /// Linear cost `c`.
    pub c: Vec<f64>,
    /// Upper generation bound, summed over generators at the bus.
    pub pmax: Vec<f64>,
    /// Lower generation bound.
    pub pmin: Vec<f64>,
    /// Nodal load `p_d`.
    pub p_d: Vec<f64>,
}

/// Generator-space cost and bound vectors (length n_gen, in column order).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GenCosts {
    /// Quadratic cost per generator.
    pub q: Vec<f64>,
    /// Linear cost per generator.
    pub c: Vec<f64>,
    /// Upper generation bound per generator.
    pub pmax: Vec<f64>,
    /// Lower generation bound per generator.
    pub pmin: Vec<f64>,
    /// Column `g` → generator index in the case.
    pub gen_of_col: Vec<usize>,
}

/// Static, index based DC-OPF instance data for a case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DcOpfInstance {
    /// Number of buses.
    pub n: usize,
    /// Number of in-service branches.
    pub m: usize,
    /// Bus-indexed cost, bounds, and load (length n).
    pub bus: BusCosts,
    /// Generator-space cost and bounds (length n_gen).
    pub gen_costs: GenCosts,
    /// Generator column `g` → bus index (length n_gen). The index space stand-in
    /// for the sparse `C_g`; [`project_gen_to_bus`] scatters through it.
    pub bus_of_col: Vec<usize>,
    /// Thermal limit `f̄` (`RATE_A`); `0` means unlimited per MATPOWER (length m).
    pub f_max: Vec<f64>,
    /// Column `k` → branch index in the case, for the in-service branches that
    /// `f_max` is ordered by.
    pub branch_of_col: Vec<usize>,
}

impl DcOpfInstance {
    /// Number of in-service generators (the generator-space vector length).
    #[must_use]
    pub fn n_gen(&self) -> usize {
        self.gen_costs.q.len()
    }
}

/// Build the index based DC-OPF instance from an indexed network.
///
/// # Errors
/// [`Error::NoGenerators`] if the case has no in-service generators,
/// [`Error::MissingGenCost`] if a generator has no cost row,
/// [`Error::UnsupportedCostModel`] if a cost is present but not a polynomial of
/// degree at most 2, or [`Error::UnknownBus`] if a generator names a bus absent
/// from the case.
pub fn build_dc_opf_instance(case: &IndexedNetwork, units: Units) -> Result<DcOpfInstance> {
    let n = case.n();
    // per_unit_base is 1.0 for an already normalized network, so Units::PerUnit
    // on a normalized case divides by 1 instead of scaling a second time.
    let base = case.per_unit_base();

    let p_scale = match units {
        Units::PerUnit => 1.0 / base,
        Units::Native => 1.0,
    };
    // Native cost is c₂p² + c₁p with p in MW. For per unit p (p_MW = base·p_pu),
    // q_pu = 2c₂·base² and c_pu = c₁·base.
    let (q_scale, c_scale) = match units {
        Units::PerUnit => (base * base, base),
        Units::Native => (1.0, 1.0),
    };

    let in_service: Vec<_> = case.in_service_gens().collect();
    let n_gen = in_service.len();
    if n_gen == 0 {
        return Err(Error::NoGenerators);
    }

    let mut q_gen = Vec::with_capacity(n_gen);
    let mut c_gen = Vec::with_capacity(n_gen);
    let mut pmax_gen = Vec::with_capacity(n_gen);
    let mut pmin_gen = Vec::with_capacity(n_gen);
    let mut gen_of_col = Vec::with_capacity(n_gen);
    let mut bus_of_col = Vec::with_capacity(n_gen);

    for &(gidx, generator) in &in_service {
        let bus = case.bus_index(generator.bus).ok_or(Error::UnknownBus {
            bus_id: generator.bus,
            element_index: gidx,
        })?;
        // Distinguish a genuinely absent cost from a present but unsupported one
        // (a piecewise model, or a polynomial of degree ≥ 3) so the error is
        // specific about which case the file hit.
        let cost = generator
            .cost
            .as_ref()
            .ok_or(Error::MissingGenCost { gen_index: gidx })?;
        let (q_raw, c_raw) = cost.quadratic().ok_or(Error::UnsupportedCostModel {
            gen_index: gidx,
            model: cost.model,
            ncost: cost.ncost,
        })?;
        q_gen.push(q_raw * q_scale);
        c_gen.push(c_raw * c_scale);
        pmax_gen.push(generator.pmax * p_scale);
        pmin_gen.push(generator.pmin * p_scale);
        gen_of_col.push(gidx);
        bus_of_col.push(bus);
    }

    let q_bus = project_gen_to_bus(&bus_of_col, &q_gen, n);
    let c_bus = project_gen_to_bus(&bus_of_col, &c_gen, n);
    let pmax_bus = project_gen_to_bus(&bus_of_col, &pmax_gen, n);
    let pmin_bus = project_gen_to_bus(&bus_of_col, &pmin_gen, n);

    let p_d: Vec<f64> = case.pd().iter().map(|&p| p * p_scale).collect();

    let mut f_max = Vec::new();
    let mut branch_of_col = Vec::new();
    for (k, branch) in case.branches().iter().enumerate() {
        if branch.in_service {
            branch_of_col.push(k);
            f_max.push(branch.rate_a * p_scale);
        }
    }
    let m = branch_of_col.len();

    Ok(DcOpfInstance {
        n,
        m,
        bus: BusCosts {
            q: q_bus,
            c: c_bus,
            pmax: pmax_bus,
            pmin: pmin_bus,
            p_d,
        },
        gen_costs: GenCosts {
            q: q_gen,
            c: c_gen,
            pmax: pmax_gen,
            pmin: pmin_gen,
            gen_of_col,
        },
        bus_of_col,
        f_max,
        branch_of_col,
    })
}

/// Scatter-sum a generator-space vector onto `n` buses through `bus_of_col`:
/// `out[bus_of_col[g]] += v[g]`. Buses with several generators get the sum. The
/// index space form of `C_g v`.
#[must_use]
pub fn project_gen_to_bus(bus_of_col: &[usize], v: &[f64], n: usize) -> Vec<f64> {
    let mut out = vec![0.0; n];
    for (g, &bus) in bus_of_col.iter().enumerate() {
        out[bus] += v[g];
    }
    out
}

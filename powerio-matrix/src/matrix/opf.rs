//! DC-OPF instance data derived from the network's generators and their cost
//! curves: cost, bounds, thermal limits, the generator→bus map, and nodal load.
//!
//! The paper treats generation as a nodal variable `p_g ∈ ℝⁿ`, so the
//! canonical vectors here are bus-indexed (length `n`), formed by scattering
//! the generator-space data through `C_g`. The generator-space vectors and
//! `C_g` ride along so a MATPOWER-faithful per-generator formulation can be
//! reconstructed exactly.

use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::matrix::incidence::{IncidenceParts, diagonal};
use crate::matrix::triplet::CooBuilder;
use crate::{Error, Result};

/// Unit system for the emitted quantities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Units {
    /// Power divided by `baseMVA`; cost coefficients scaled so the cost is a
    /// function of per-unit power (`q ← 2c₂·base²`, `c ← c₁·base`). Keeps the
    /// whole instance dimensionally consistent with the per-unit Laplacian.
    #[default]
    PerUnit,
    /// Raw MATPOWER units: power in MW, cost in native `$·MWh⁻¹` coefficients.
    Native,
}

/// Length-n bus-indexed cost and bound vectors (paper form). All share index
/// space; each is zero at buses with no generator.
#[derive(Debug, Clone)]
pub struct BusCosts {
    /// Quadratic cost diagonal `q`, `cost = ½ q p² + c p`.
    pub q: Vec<f64>,
    /// Linear cost `c`.
    pub c: Vec<f64>,
    /// Upper generation bound (summed over generators at the bus).
    pub pmax: Vec<f64>,
    /// Lower generation bound.
    pub pmin: Vec<f64>,
    /// Nodal load `p_d`.
    pub p_d: Vec<f64>,
}

/// Generator-space provenance (length n_gen, in `C_g` column order).
#[derive(Debug, Clone)]
pub struct GenCosts {
    pub q: Vec<f64>,
    pub c: Vec<f64>,
    pub pmax: Vec<f64>,
    pub pmin: Vec<f64>,
    /// Column `g` → index into the in-service generators.
    pub gen_of_col: Vec<usize>,
}

/// Static DC-OPF instance data for a case.
#[derive(Debug, Clone)]
pub struct OpfInstance {
    pub n: usize,
    pub m: usize,
    /// Bus-indexed cost/bounds/load (length n).
    pub bus: BusCosts,
    /// Generator-space provenance (length n_gen).
    pub gen_space: GenCosts,
    /// Thermal limit `f̄` (`RATE_A`); `0` means unlimited per MATPOWER. Length m.
    pub f_max: Vec<f64>,
    /// Generator→bus incidence, `n × n_gen`, one `1` per column.
    pub c_g: CsMat<f64>,
}

impl OpfInstance {
    /// Number of in-service generators (the generator-space vector length).
    #[must_use]
    pub fn n_gen(&self) -> usize {
        self.gen_space.q.len()
    }
}

/// Build the OPF instance. Errors with [`Error::NoGenerators`] if the case has
/// no in-service generators, [`Error::MissingGenCost`] if a generator has no
/// cost row, or [`Error::UnsupportedCostModel`] if its cost is present but not
/// a polynomial of degree ≤ 2.
pub fn build_opf_instance(
    case: &IndexedNetwork,
    incidence: &IncidenceParts,
    units: Units,
) -> Result<OpfInstance> {
    let n = case.n();
    let m = incidence.m();
    let base = case.base_mva();

    let p_scale = match units {
        Units::PerUnit => 1.0 / base,
        Units::Native => 1.0,
    };
    // Native cost is c₂p² + c₁p with p in MW. For per-unit p (p_MW = base·p_pu),
    // q_pu = 2c₂·base² and c_pu = c₁·base.
    let (q_scale, c_scale) = match units {
        Units::PerUnit => (base * base, base),
        Units::Native => (1.0, 1.0),
    };

    let in_service: Vec<(usize, &crate::network::Generator)> = case.in_service_gens().collect();
    let n_gen = in_service.len();
    if n_gen == 0 {
        return Err(Error::NoGenerators);
    }

    let mut q_gen = Vec::with_capacity(n_gen);
    let mut c_gen = Vec::with_capacity(n_gen);
    let mut pmax_gen = Vec::with_capacity(n_gen);
    let mut pmin_gen = Vec::with_capacity(n_gen);
    let mut gen_of_col = Vec::with_capacity(n_gen);
    let mut cg = CooBuilder::with_capacity_rect(n, n_gen, n_gen);

    for (col, &(gidx, generator)) in in_service.iter().enumerate() {
        let bus = case.bus_index(generator.bus).ok_or(Error::UnknownBus {
            bus_id: generator.bus,
            row: gidx,
        })?;
        // Distinguish a genuinely absent cost from a present-but-unsupported
        // one (piecewise model 1, or polynomial degree ≥ 3) so the error tells
        // the truth about which case the file hit.
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
        cg.add(bus, col, 1.0);
    }
    let c_g = cg.finish_csr();

    let q_bus = project_gen_to_bus(&c_g, &q_gen);
    let c_bus = project_gen_to_bus(&c_g, &c_gen);
    let pmax_bus = project_gen_to_bus(&c_g, &pmax_gen);
    let pmin_bus = project_gen_to_bus(&c_g, &pmin_gen);

    let p_d: Vec<f64> = case.pd().iter().map(|&p| p * p_scale).collect();

    let f_max: Vec<f64> = incidence
        .branch_of_col
        .iter()
        .map(|&k| case.branches()[k].rate_a * p_scale)
        .collect();

    Ok(OpfInstance {
        n,
        m,
        bus: BusCosts {
            q: q_bus,
            c: c_bus,
            pmax: pmax_bus,
            pmin: pmin_bus,
            p_d,
        },
        gen_space: GenCosts {
            q: q_gen,
            c: c_gen,
            pmax: pmax_gen,
            pmin: pmin_gen,
            gen_of_col,
        },
        f_max,
        c_g,
    })
}

/// Scatter-sum a generator-space vector onto buses: `out = C_g v`. Buses with
/// several generators get the sum; one generator per bus is exact.
pub fn project_gen_to_bus(c_g: &CsMat<f64>, v: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0; c_g.rows()];
    for (&val, (bus, g)) in c_g {
        out[bus] += val * v[g];
    }
    out
}

/// `Q = diag(q)` as a sparse matrix — the quadratic-cost analog of
/// [`susceptance_diag`](crate::matrix::susceptance_diag). Feeds the DC-OPF QP
/// objective `½ pᵀ Q p + cᵀ p` consumed by the `kkt` interior-point operators.
pub fn cost_quadratic_diag(q: &[f64]) -> CsMat<f64> {
    diagonal(q)
}

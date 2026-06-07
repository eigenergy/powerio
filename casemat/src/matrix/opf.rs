//! DC-OPF instance data derived from `mpc.gen` / `mpc.gencost`: cost,
//! bounds, thermal limits, the generator→bus map, and nodal load.
//!
//! The paper treats generation as a nodal variable `p_g ∈ ℝⁿ`, so the
//! canonical vectors here are bus-indexed (length `n`), formed by scattering
//! the generator-space data through `C_g`. The generator-space vectors and
//! `C_g` ride along so a MATPOWER-faithful per-generator formulation can be
//! reconstructed exactly.

use sprs::CsMat;

use crate::case::MpcCase;
use crate::matrix::incidence::IncidenceParts;
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

/// Static DC-OPF instance data for a case.
#[derive(Debug, Clone)]
pub struct OpfInstance {
    pub n: usize,
    pub m: usize,
    pub n_gen: usize,

    // Bus-indexed (paper form, length n). Zero at buses with no generator.
    /// Quadratic cost diagonal `q`, `cost = ½ q p² + c p`.
    pub q_bus: Vec<f64>,
    /// Linear cost `c`.
    pub c_bus: Vec<f64>,
    /// Upper generation bound (summed over generators at the bus).
    pub pmax_bus: Vec<f64>,
    /// Lower generation bound.
    pub pmin_bus: Vec<f64>,
    /// Nodal load `p_d`.
    pub p_d: Vec<f64>,

    // Branch-indexed (length m, incidence column order).
    /// Thermal limit `f̄` (`RATE_A`); `0` means unlimited per MATPOWER.
    pub f_max: Vec<f64>,

    // Generator-space provenance (length n_gen, in-service generators).
    pub q_gen: Vec<f64>,
    pub c_gen: Vec<f64>,
    pub pmax_gen: Vec<f64>,
    pub pmin_gen: Vec<f64>,
    /// Generator→bus incidence, `n × n_gen`, one `1` per column.
    pub c_g: CsMat<f64>,
    /// Column `g` → index into `case.gens`.
    pub gen_of_col: Vec<usize>,
}

/// Build the OPF instance. Errors with [`Error::NoGenerators`] if the case has
/// no in-service generators, [`Error::MissingGenCost`] if a generator has no
/// cost row, or [`Error::UnsupportedCostModel`] if its cost is present but not
/// a polynomial of degree ≤ 2.
pub fn build_opf_instance(
    case: &MpcCase,
    incidence: &IncidenceParts,
    units: Units,
) -> Result<OpfInstance> {
    let n = case.n();
    let m = incidence.m();
    let base = case.base_mva;

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

    let in_service: Vec<(usize, &crate::case::Generator)> = case.in_service_gens().collect();
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

    for (col, &(gidx, gen)) in in_service.iter().enumerate() {
        let bus = case
            .bus_index(gen.bus_id)
            .ok_or(Error::UnknownBus { bus_id: gen.bus_id, row: gidx })?;
        // Distinguish a genuinely absent cost from a present-but-unsupported
        // one (piecewise model 1, or polynomial degree ≥ 3) so the error tells
        // the truth about which case the file hit.
        let cost = gen.cost.as_ref().ok_or(Error::MissingGenCost { gen: gidx })?;
        let (q_raw, c_raw) = cost.quadratic().ok_or(Error::UnsupportedCostModel {
            gen: gidx,
            model: cost.model,
            ncost: cost.ncost,
        })?;
        q_gen.push(q_raw * q_scale);
        c_gen.push(c_raw * c_scale);
        pmax_gen.push(gen.pmax * p_scale);
        pmin_gen.push(gen.pmin * p_scale);
        gen_of_col.push(gidx);
        cg.add(bus, col, 1.0);
    }
    let c_g = cg.finish_csr();

    let q_bus = project_gen_to_bus(&c_g, &q_gen);
    let c_bus = project_gen_to_bus(&c_g, &c_gen);
    let pmax_bus = project_gen_to_bus(&c_g, &pmax_gen);
    let pmin_bus = project_gen_to_bus(&c_g, &pmin_gen);

    let p_d: Vec<f64> = case.buses.iter().map(|b| b.pd * p_scale).collect();

    let f_max: Vec<f64> = incidence
        .branch_of_col
        .iter()
        .map(|&k| case.branches[k].rate_a * p_scale)
        .collect();

    Ok(OpfInstance {
        n,
        m,
        n_gen,
        q_bus,
        c_bus,
        pmax_bus,
        pmin_bus,
        p_d,
        f_max,
        q_gen,
        c_gen,
        pmax_gen,
        pmin_gen,
        c_g,
        gen_of_col,
    })
}

/// Scatter-sum a generator-space vector onto buses: `out = C_g v`. Buses with
/// several generators get the sum; one generator per bus is exact.
pub fn project_gen_to_bus(c_g: &CsMat<f64>, v: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0; c_g.rows()];
    for (&val, (bus, gen)) in c_g.iter() {
        out[bus] += val * v[gen];
    }
    out
}

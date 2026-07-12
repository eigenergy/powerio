use serde::{Deserialize, Serialize};

use powerio::{BusId, DcConvention, Error, IndexedNetwork, Result};

/// Unit system for power and generator cost data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Units {
    /// Power is per unit. Cost coefficients are scaled for per unit power.
    #[default]
    PerUnit,
    /// Power remains in the source unit, normally MW.
    Native,
}

impl Units {
    /// `(power, admittance)` multipliers for source data on `base` MVA. MW
    /// valued quantities (demand, bounds, limits, MW valued shunts) scale by
    /// the first; per unit admittances and susceptances by the second.
    pub(crate) fn power_scales(self, base: f64) -> (f64, f64) {
        match self {
            Self::PerUnit => (1.0 / base, 1.0),
            Self::Native => (1.0, base),
        }
    }

    /// `(quadratic, linear)` generator cost coefficient multipliers for the
    /// same unit selection. The constant term never scales.
    pub(crate) fn cost_scales(self, base: f64) -> (f64, f64) {
        match self {
            Self::PerUnit => (base * base, base),
            Self::Native => (1.0, 1.0),
        }
    }
}

/// Options for DC OPF instance assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DcOpfOptions {
    pub convention: DcConvention,
    pub units: Units,
    /// Skip non-self-loop branches with zero reactance. If false, assembly
    /// returns [`Error::ZeroImpedance`].
    pub skip_zero_impedance: bool,
}

impl Default for DcOpfOptions {
    fn default() -> Self {
        Self {
            convention: DcConvention::default(),
            units: Units::default(),
            skip_zero_impedance: true,
        }
    }
}

/// Generator data in generator column order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DcGeneratorData {
    /// Generator column to dense bus index.
    pub bus_of_gen: Vec<usize>,
    /// Generator column to source generator row.
    pub source_rows: Vec<usize>,
    /// Quadratic objective diagonal in `0.5 * q * p^2 + c * p`.
    pub q: Vec<f64>,
    /// Linear objective coefficient.
    pub c: Vec<f64>,
    pub pmax: Vec<f64>,
    pub pmin: Vec<f64>,
}

/// Branch data in active branch column order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DcBranchData {
    pub from_bus: Vec<usize>,
    pub to_bus: Vec<usize>,
    /// Branch coefficient in the selected power unit per radian.
    pub b: Vec<f64>,
    /// Phase shift in radians. Zero under [`DcConvention::PaperPure`].
    pub shift: Vec<f64>,
    /// Thermal limit in the selected power unit. Zero means unlimited.
    pub f_max: Vec<f64>,
    /// Branch angle bounds in radians.
    pub angle_min: Vec<f64>,
    pub angle_max: Vec<f64>,
    /// Branch column to source branch row.
    pub source_rows: Vec<usize>,
    /// Source branch rows omitted because their reactance was zero.
    pub skipped_zero_impedance: Vec<usize>,
}

/// Exact nodal generator data for cases with at most one generator per bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NodalGeneratorData {
    pub q: Vec<f64>,
    pub c: Vec<f64>,
    pub pmax: Vec<f64>,
    pub pmin: Vec<f64>,
}

/// Matrix free DC OPF input data.
///
/// A problem instance is complete numerical input for one problem family. It
/// is separate from the source network, a matrix projection, a solver
/// formulation, and a solution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DcOpfInstance {
    pub name: String,
    pub n_buses: usize,
    pub n_source_generators: usize,
    pub n_source_branches: usize,
    pub base_mva: f64,
    pub units: Units,
    pub convention: DcConvention,
    pub skip_zero_impedance: bool,
    /// Dense bus index to external bus ID.
    pub bus_ids: Vec<BusId>,
    pub reference_buses: Vec<usize>,
    /// Nodal active demand in dense bus order.
    pub p_d: Vec<f64>,
    /// Nodal phase shift injection in dense bus order.
    pub p_shift: Vec<f64>,
    pub generators: DcGeneratorData,
    pub branches: DcBranchData,
}

impl DcOpfInstance {
    #[must_use]
    pub fn n_generators(&self) -> usize {
        self.generators.q.len()
    }

    #[must_use]
    pub fn n_branches(&self) -> usize {
        self.branches.b.len()
    }

    /// Project generator cost and bounds to buses when the reduction is exact.
    ///
    /// A bus with several generators is rejected because summing their
    /// quadratic coefficients does not preserve the original objective.
    pub fn nodal_generator_data(&self) -> Result<NodalGeneratorData> {
        let mut occupied = vec![false; self.n_buses];
        let mut q = vec![0.0; self.n_buses];
        let mut c = vec![0.0; self.n_buses];
        let mut pmax = vec![0.0; self.n_buses];
        let mut pmin = vec![0.0; self.n_buses];

        for generator in 0..self.n_generators() {
            let bus = self.generators.bus_of_gen[generator];
            if occupied[bus] {
                return Err(Error::MultipleGeneratorsAtBus {
                    bus_id: self.bus_ids[bus],
                });
            }
            occupied[bus] = true;
            q[bus] = self.generators.q[generator];
            c[bus] = self.generators.c[generator];
            pmax[bus] = self.generators.pmax[generator];
            pmin[bus] = self.generators.pmin[generator];
        }

        Ok(NodalGeneratorData { q, c, pmax, pmin })
    }
}

/// Build a matrix free DC OPF instance from an indexed network.
#[allow(clippy::too_many_lines)]
pub fn build_dc_opf_instance(
    case: &IndexedNetwork,
    options: &DcOpfOptions,
) -> Result<DcOpfInstance> {
    case.check_reference_coverage()?;

    let n_buses = case.n();
    let base = case.per_unit_base();
    let (p_scale, b_scale) = options.units.power_scales(base);
    let (q_scale, c_scale) = options.units.cost_scales(base);

    let mut bus_of_gen = Vec::new();
    let mut generator_rows = Vec::new();
    let mut q = Vec::new();
    let mut c = Vec::new();
    let mut pmax = Vec::new();
    let mut pmin = Vec::new();

    for (source_row, generator) in case.in_service_gens() {
        let bus = case.bus_index(generator.bus).ok_or(Error::UnknownBus {
            bus_id: generator.bus,
            element_index: source_row,
        })?;
        let cost = generator.cost.as_ref().ok_or(Error::MissingGenCost {
            gen_index: source_row,
        })?;
        let (q_raw, c_raw) = cost.quadratic().ok_or(Error::UnsupportedCostModel {
            gen_index: source_row,
            model: cost.model,
            ncost: cost.ncost,
        })?;
        bus_of_gen.push(bus);
        generator_rows.push(source_row);
        q.push(q_raw * q_scale);
        c.push(c_raw * c_scale);
        pmax.push(generator.pmax * p_scale);
        pmin.push(generator.pmin * p_scale);
    }
    if q.is_empty() {
        return Err(Error::NoGenerators);
    }

    let mut from_bus = Vec::new();
    let mut to_bus = Vec::new();
    let mut b = Vec::new();
    let mut shift = Vec::new();
    let mut f_max = Vec::new();
    let mut angle_min = Vec::new();
    let mut angle_max = Vec::new();
    let mut branch_rows = Vec::new();
    let mut skipped_zero_impedance = Vec::new();
    let mut p_shift = vec![0.0; n_buses];

    for (source_row, branch) in case.in_service_branches() {
        let from = case.bus_index(branch.from).ok_or(Error::UnknownBus {
            bus_id: branch.from,
            element_index: source_row,
        })?;
        let to = case.bus_index(branch.to).ok_or(Error::UnknownBus {
            bus_id: branch.to,
            element_index: source_row,
        })?;
        if from == to {
            continue;
        }
        if branch.x == 0.0 {
            if options.skip_zero_impedance {
                skipped_zero_impedance.push(source_row);
                continue;
            }
            return Err(Error::ZeroImpedance { row: source_row });
        }
        let branch_b = options
            .convention
            .branch_susceptance(branch.x, branch.effective_tap())
            * b_scale;
        if !branch_b.is_finite() {
            return Err(Error::NonFiniteSusceptance { row: source_row });
        }
        let shift_rad = if options.convention.includes_phase_shifts() {
            case.angle_radians(branch.shift)
        } else {
            0.0
        };
        if shift_rad != 0.0 {
            p_shift[from] -= branch_b * shift_rad;
            p_shift[to] += branch_b * shift_rad;
        }
        from_bus.push(from);
        to_bus.push(to);
        b.push(branch_b);
        shift.push(shift_rad);
        f_max.push(branch.rate_a * p_scale);
        angle_min.push(case.angle_radians(branch.angmin));
        angle_max.push(case.angle_radians(branch.angmax));
        branch_rows.push(source_row);
    }

    Ok(DcOpfInstance {
        name: case.name().to_owned(),
        n_buses,
        n_source_generators: case.generators().len(),
        n_source_branches: case.branches().len(),
        base_mva: case.base_mva(),
        units: options.units,
        convention: options.convention,
        skip_zero_impedance: options.skip_zero_impedance,
        bus_ids: (0..n_buses).map(|index| case.bus_id(index)).collect(),
        reference_buses: case.reference_bus_indices(),
        p_d: case.pd().iter().map(|value| value * p_scale).collect(),
        p_shift,
        generators: DcGeneratorData {
            bus_of_gen,
            source_rows: generator_rows,
            q,
            c,
            pmax,
            pmin,
        },
        branches: DcBranchData {
            from_bus,
            to_bus,
            b,
            shift,
            f_max,
            angle_min,
            angle_max,
            source_rows: branch_rows,
            skipped_zero_impedance,
        },
    })
}

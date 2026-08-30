use serde::{Deserialize, Serialize};

use powerio_tx::{BalancedNetwork, BranchSusceptanceFormula, BusId, IndexedNetwork};

use crate::{Error, Result};
use powerio_prob::ReferenceBuses;

use super::{limits, nodal};
use crate::{PiecewiseLinearCost, PreparedObjective};

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

impl std::str::FromStr for Units {
    type Err = String;

    /// The one alias table for the bindings: `per-unit`/`perunit`/`pu` and
    /// `native`, case insensitive, `-`/`_` ignored.
    fn from_str(name: &str) -> std::result::Result<Self, Self::Err> {
        match name.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "perunit" | "pu" => Ok(Units::PerUnit),
            "native" => Ok(Units::Native),
            other => Err(format!(
                "unknown units `{other}`; expected \"per-unit\" or \"native\""
            )),
        }
    }
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DcOpfOptions {
    pub convention: BranchSusceptanceFormula,
    pub units: Units,
    /// Skip non-self-loop branches with zero reactance. Off by default:
    /// zero impedance branches are preserved in networks and instances, so
    /// assembly refuses them with [`powerio_tx::Error::ZeroImpedance`] until
    /// the caller resolves them explicitly
    /// ([`powerio_prob::merge_zero_impedance_buses`]) or opts into skipping.
    pub skip_zero_impedance: bool,
    /// Give a branch with no thermal rating the bound
    /// [`Branch::synthesize_rate_a`](powerio_tx::Branch::synthesize_rate_a)
    /// states. If false, `rate_a <= 0` reaches `f_max` as zero, which reads as
    /// unlimited. `#[serde(default)]`: documents serialized before the field
    /// existed deserialize to the default (off), the pre-field behavior.
    #[serde(default)]
    pub synthesize_unrated_limits: bool,
    /// The already validated instance objective to compile into the arrays.
    pub objective: PreparedObjective,
}

/// Generator data in generator column order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DcGeneratorData {
    /// Stable generator identity aligned with every following column.
    pub identities: Vec<String>,
    /// Generator column to dense bus index.
    pub bus_of_gen: Vec<usize>,
    /// Generator column to row in the star-lowered analysis network.
    pub analysis_rows: Vec<usize>,
    /// Generator column to source generator row. A synthetic analysis row has
    /// no source row.
    pub source_rows: Vec<Option<usize>>,
    /// Quadratic objective diagonal in `0.5 * q * p^2 + c * p + c0`.
    pub q: Vec<f64>,
    /// Linear objective coefficient.
    pub c: Vec<f64>,
    /// Constant objective term. Unscaled in both unit systems: it carries no
    /// power dimension. It does not move the argmin, but a consumer reporting
    /// or comparing objective values needs it.
    pub c0: Vec<f64>,
    /// Convex piecewise linear costs aligned with the generator columns.
    ///
    /// `Some` is the complete objective term for that generator; its `q`, `c`,
    /// and `c0` entries above are zero. `None` means the three polynomial
    /// columns carry the complete constant, linear, or quadratic term.
    pub piecewise_linear: Vec<Option<PiecewiseLinearCost>>,
    pub pmax: Vec<f64>,
    pub pmin: Vec<f64>,
    /// Whether the instance activates this generator's capability bounds.
    pub capability_active: Vec<bool>,
}

/// Branch data in active branch column order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DcBranchData {
    /// Stable branch identity aligned with every following column.
    pub identities: Vec<String>,
    pub from_bus: Vec<usize>,
    pub to_bus: Vec<usize>,
    /// Branch susceptance in the selected power unit per radian, positive for
    /// an inductive branch.
    pub b: Vec<f64>,
    /// Phase shift in radians. Zero unless the convention carries phase shift
    /// injections.
    pub shift: Vec<f64>,
    /// Thermal limit in the selected power unit. Zero means unlimited.
    pub f_max: Vec<f64>,
    /// Branch angle bounds in radians.
    pub angle_min: Vec<f64>,
    pub angle_max: Vec<f64>,
    /// Branch column to row in the star-lowered analysis network.
    pub analysis_rows: Vec<usize>,
    /// Branch column to source branch row. Synthetic winding branches have no
    /// source row.
    pub source_rows: Vec<Option<usize>>,
    /// Analysis branch rows omitted because their reactance was zero.
    pub skipped_zero_impedance: Vec<usize>,
    /// Whether the instance activates each thermal limit.
    pub thermal_limit_active: Vec<bool>,
    /// Whether the instance activates each angle difference bound.
    pub angle_bound_active: Vec<bool>,
}

/// Generator data in dense bus order, aggregated over the generators at each
/// bus. See [`DcOpfPreparation::nodal_generator_data`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NodalGeneratorData {
    pub q: Vec<f64>,
    pub c: Vec<f64>,
    pub c0: Vec<f64>,
    pub pmax: Vec<f64>,
    pub pmin: Vec<f64>,
    /// Which buses host a generator. A bus without one has a zero range and a
    /// zero cost, which a formulation must not read as a free generator.
    pub has_gen: Vec<bool>,
}

/// Matrix free DC OPF input data.
///
/// A problem instance is complete numerical input for one problem family. It
/// is separate from the source network, a matrix projection, a solver
/// formulation, and a solution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DcOpfPreparation {
    pub name: String,
    pub n_buses: usize,
    pub n_source_generators: usize,
    pub n_source_branches: usize,
    pub base_mva: f64,
    pub units: Units,
    pub convention: BranchSusceptanceFormula,
    /// The objective represented by the generator cost columns.
    pub objective: PreparedObjective,
    pub skip_zero_impedance: bool,
    /// Whether zero and negative source thermal ratings were replaced with
    /// synthesized limits while assembling this instance.
    ///
    /// `#[serde(default)]` keeps documents written before this field readable;
    /// their limits retain the old unsynthesized meaning.
    #[serde(default)]
    pub synthesize_unrated_limits: bool,
    /// Dense bus index to external bus ID.
    pub bus_ids: Vec<BusId>,
    /// Dense bus index to row in the star-lowered analysis network.
    pub bus_analysis_rows: Vec<usize>,
    /// Dense bus index to source bus row. A synthetic star bus has no source
    /// row; an explicitly isolated source bus has no dense row here.
    pub bus_source_rows: Vec<Option<usize>>,
    pub reference_buses: ReferenceBuses,
    /// Nodal active demand in dense bus order.
    pub p_d: Vec<f64>,
    /// Nodal shunt conductance in dense bus order.
    ///
    /// The DC approximation holds the voltage magnitude at one per unit, so a
    /// shunt draws the constant real power `g_s` and does not depend on the
    /// angle. It belongs in the injection: the bus susceptance matrix keeps
    /// zero row sums and carries no shunt. A nodal balance subtracts it
    /// beside [`Self::p_d`], as MATPOWER `runpf` does.
    pub g_s: Vec<f64>,
    /// Nodal phase shift injection in dense bus order. The complete fixed
    /// withdrawal in `L theta = Cg pg - fixed` is `p_d + g_s + p_shift`.
    pub p_shift: Vec<f64>,
    pub generators: DcGeneratorData,
    pub branches: DcBranchData,
}

impl DcOpfPreparation {
    #[must_use]
    pub fn n_generators(&self) -> usize {
        self.generators.q.len()
    }

    #[must_use]
    pub fn n_branches(&self) -> usize {
        self.branches.b.len()
    }

    /// Fixed nodal withdrawal in dense bus order.
    ///
    /// With `A` oriented from bus to bus and
    /// `L = A diag(b) A^T`, the DC balance is
    /// `L theta = Cg pg - (p_d + g_s + p_shift)`.
    #[must_use]
    pub fn fixed_nodal_withdrawal(&self) -> Vec<f64> {
        (0..self.n_buses)
            .map(|bus| self.p_d[bus] + self.g_s[bus] + self.p_shift[bus])
            .collect()
    }

    /// Fixed branch flow offset in active branch column order.
    ///
    /// The complete branch flow over this preparation's internal positive
    /// weights is `f = diag(b) A^T theta + branch_flow_offset`, where the
    /// offset is `-b * shift` elementwise. In the public PowerModels sign
    /// spelling the same flow is `p_branch = -Bf va + b .* shift` with the
    /// negated susceptances ([`crate::DcOperators`] emits that
    /// form); the two agree term for term because this `b` is the negation
    /// of the public one.
    #[must_use]
    pub fn branch_flow_offset(&self) -> Vec<f64> {
        (0..self.n_branches())
            .map(|branch| -self.branches.b[branch] * self.branches.shift[branch])
            .collect()
    }

    /// Project generator cost and bounds to bus space.
    ///
    /// The bounds at a bus are the sum of the generator bounds, which is the
    /// range the bus total can reach. The cost curves at a bus combine by the
    /// parallel rule `q = 1 / Σ(1/qᵢ)`, the curve that the least cost split of
    /// the bus total follows. That combination is an approximation: it agrees
    /// with generator space only while the split stays inside the bound of
    /// each generator. A bus with one generator keeps that generator's own
    /// coefficients.
    pub fn nodal_generator_data(&self) -> Result<NodalGeneratorData> {
        let n = self.n_buses;
        let generators = &self.generators;
        if let Some(gen_index) = generators.piecewise_linear.iter().position(Option::is_some) {
            return Err(Error::PiecewiseNodalCost { gen_index });
        }
        let bus_of_gen = &generators.bus_of_gen;
        let costs =
            nodal::combine_costs(n, bus_of_gen, &generators.q, &generators.c, &generators.c0);
        Ok(NodalGeneratorData {
            q: costs.q,
            c: costs.c,
            c0: costs.c0,
            pmax: nodal::sum_by_bus(n, bus_of_gen, &generators.pmax),
            pmin: nodal::sum_by_bus(n, bus_of_gen, &generators.pmin),
            has_gen: nodal::buses_with_generators(n, bus_of_gen),
        })
    }
}

/// Build the matrix free DC OPF arrays from an indexed network view. The
/// public instance level entry is
/// [`build_dc_opf_preparation`](crate::build_dc_opf_preparation), which
/// derives the view and the options from a
/// [`DcOpfInstance`](powerio_prob::DcOpfInstance).
#[allow(clippy::too_many_lines)]
pub(crate) fn preparation_from_view(
    case: &IndexedNetwork,
    options: DcOpfOptions,
) -> Result<DcOpfPreparation> {
    case.network().check_base_mva()?;

    let active_buses = crate::opf::active_bus_index(case)?;
    let n_buses = active_buses.analysis_rows.len();
    let base = case.per_unit_base();
    let (p_scale, b_scale) = options.units.power_scales(base);
    let thermal = limits::ThermalLimits {
        synthesize_unrated: options.synthesize_unrated_limits,
        power_scale: p_scale,
        admittance_scale: b_scale,
    };
    let (q_scale, c_scale) = options.units.cost_scales(base);

    let mut bus_of_gen = Vec::new();
    let mut generator_identities = Vec::new();
    let mut generator_rows = Vec::new();
    let mut q = Vec::new();
    let mut c = Vec::new();
    let mut c0 = Vec::new();
    let mut piecewise_linear = Vec::new();
    let mut pmax = Vec::new();
    let mut pmin = Vec::new();

    for (source_row, generator) in case.in_service_gens() {
        let analysis_bus = case
            .bus_index(generator.bus)
            .ok_or(powerio_tx::Error::UnknownBus {
                bus_id: generator.bus,
                element_index: source_row,
            })?;
        let Some(bus) = active_buses.dense_by_analysis[analysis_bus] else {
            continue;
        };
        let terms = match options.objective {
            PreparedObjective::Feasibility => nodal::GeneratorCostTerms {
                q: 0.0,
                c: 0.0,
                c0: 0.0,
                piecewise_linear: None,
            },
            PreparedObjective::NetworkGeneratorCost => {
                let cost = generator
                    .cost
                    .as_ref()
                    .ok_or(powerio_tx::Error::MissingGenCost {
                        gen_index: source_row,
                    })?;
                nodal::generator_cost_terms(cost, source_row, p_scale)?
            }
        };
        generator_identities.push(crate::opf::row_identity(
            generator.uid.as_deref(),
            "generators",
            source_row,
        ));
        bus_of_gen.push(bus);
        generator_rows.push(source_row);
        q.push(terms.q * q_scale);
        c.push(terms.c * c_scale);
        c0.push(terms.c0);
        piecewise_linear.push(terms.piecewise_linear);
        pmax.push(generator.pmax * p_scale);
        pmin.push(generator.pmin * p_scale);
    }
    if q.is_empty() {
        return Err(Error::NoGenerators);
    }

    let mut from_bus = Vec::new();
    let mut branch_identities = Vec::new();
    let mut to_bus = Vec::new();
    let mut b = Vec::new();
    let mut shift = Vec::new();
    let mut f_max = Vec::new();
    let mut angle_min = Vec::new();
    let mut angle_max = Vec::new();
    let mut branch_rows = Vec::new();
    let mut skipped_zero_impedance = Vec::new();
    let mut p_shift = vec![0.0; n_buses];
    // Dense bus order is the position order of `network().buses`.
    let buses = &case.network().buses();

    for (source_row, branch) in case.in_service_branches() {
        let from_analysis = case
            .bus_index(branch.from)
            .ok_or(powerio_tx::Error::UnknownBus {
                bus_id: branch.from,
                element_index: source_row,
            })?;
        let to_analysis = case
            .bus_index(branch.to)
            .ok_or(powerio_tx::Error::UnknownBus {
                bus_id: branch.to,
                element_index: source_row,
            })?;
        let (Some(from), Some(to)) = (
            active_buses.dense_by_analysis[from_analysis],
            active_buses.dense_by_analysis[to_analysis],
        ) else {
            continue;
        };
        if from == to {
            // A self-loop carries no angle difference, so it contributes no
            // DC flow, and its shift injection cancels at its own bus.
            continue;
        }
        // The reactance the DC matrix builders bound, on the same rule: an
        // `x = 1e-300` gives a finite `b = 1e300` that annihilates every real
        // branch sharing a bus with it. Exact zero used to be the whole test.
        if branch.x.abs() < powerio_tx::dc::MIN_DIVISIBLE_MAGNITUDE {
            if options.skip_zero_impedance {
                skipped_zero_impedance.push(source_row);
                continue;
            }
            return Err(powerio_tx::Error::ZeroImpedance { row: source_row }.into());
        }
        // Only the tap-reading formula can be bounded by a tap (#324).
        let tap = if options.convention.reads_tap() {
            branch.divisible_tap(source_row)?
        } else {
            1.0
        };
        let branch_b = options
            .convention
            .solver_edge_weight(branch.r, branch.x, tap)
            * b_scale;
        if !branch_b.is_finite() {
            return Err(powerio_tx::Error::NonFiniteSusceptance { row: source_row }.into());
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
        let amin = case.angle_radians(branch.angmin);
        let amax = case.angle_radians(branch.angmax);
        from_bus.push(from);
        branch_identities.push(crate::opf::row_identity(
            branch.uid.as_deref(),
            "branches",
            source_row,
        ));
        to_bus.push(to);
        b.push(branch_b);
        shift.push(shift_rad);
        f_max.push(thermal.of(
            branch,
            amin,
            amax,
            &buses[from_analysis],
            &buses[to_analysis],
        ));
        angle_min.push(amin);
        angle_max.push(amax);
        branch_rows.push(source_row);
    }

    let n_active_generators = q.len();
    let n_active_branches = b.len();
    let bus_analysis_rows = active_buses.analysis_rows;
    let p_d = bus_analysis_rows
        .iter()
        .map(|&row| case.pd()[row] * p_scale)
        .collect();
    let g_s = bus_analysis_rows
        .iter()
        .map(|&row| case.gs()[row] * p_scale)
        .collect();
    let bus_source_rows = bus_analysis_rows.iter().copied().map(Some).collect();
    Ok(DcOpfPreparation {
        name: case.name().to_owned(),
        n_buses,
        n_source_generators: case.generators().len(),
        n_source_branches: case.branches().len(),
        base_mva: case.base_mva(),
        units: options.units,
        convention: options.convention,
        objective: options.objective,
        skip_zero_impedance: options.skip_zero_impedance,
        synthesize_unrated_limits: options.synthesize_unrated_limits,
        bus_ids: active_buses.bus_ids,
        bus_analysis_rows,
        bus_source_rows,
        reference_buses: active_buses.reference_buses,
        p_d,
        g_s,
        p_shift,
        generators: DcGeneratorData {
            identities: generator_identities,
            bus_of_gen,
            analysis_rows: generator_rows.clone(),
            source_rows: generator_rows.into_iter().map(Some).collect(),
            q,
            c,
            c0,
            piecewise_linear,
            pmax,
            pmin,
            capability_active: vec![true; n_active_generators],
        },
        branches: DcBranchData {
            identities: branch_identities,
            from_bus,
            to_bus,
            b,
            shift,
            f_max,
            angle_min,
            angle_max,
            analysis_rows: branch_rows.clone(),
            source_rows: branch_rows.into_iter().map(Some).collect(),
            skipped_zero_impedance,
            thermal_limit_active: vec![true; n_active_branches],
            angle_bound_active: vec![true; n_active_branches],
        },
    })
}

/// Apply the source instance's active constraint selections and source row
/// provenance after the numerical view has been star-lowered.
pub(crate) fn apply_instance_semantics(
    preparation: &mut DcOpfPreparation,
    source: &BalancedNetwork,
    constraints: &powerio_prob::ActiveConstraints,
) -> Result<()> {
    let source_generator_ids: Vec<String> = source
        .generators()
        .iter()
        .enumerate()
        .map(|(row, generator)| {
            crate::opf::row_identity(generator.uid.as_deref(), "generators", row)
        })
        .collect();
    let source_branch_ids: Vec<String> = source
        .branches()
        .iter()
        .enumerate()
        .map(|(row, branch)| crate::opf::row_identity(branch.uid.as_deref(), "branches", row))
        .collect();

    preparation.generators.capability_active = crate::opf::constraint_mask(
        "generator capability",
        &constraints.generator_capability,
        &source_generator_ids,
        &preparation.generators.identities,
    )?;

    // DC fixes every voltage magnitude at one per unit, so it has no voltage
    // bound rows to expose. Still validate an explicit identity selection:
    // a misspelled bus must not disappear merely because this formulation
    // has no corresponding variable.
    let source_bus_ids: Vec<String> = source
        .buses()
        .iter()
        .map(|bus| bus.id.to_string())
        .collect();
    let _ = crate::opf::constraint_mask(
        "bus voltage bounds",
        &constraints.voltage_bounds,
        &source_bus_ids,
        &[],
    )?;

    // Synthetic winding branches are part of the analysis family and are
    // addressable by the identities returned in the preparation.
    let mut analysis_branch_ids = source_branch_ids;
    analysis_branch_ids.extend(
        preparation
            .branches
            .identities
            .iter()
            .zip(&preparation.branches.analysis_rows)
            .filter(|(_, row)| **row >= source.branches().len())
            .map(|(identity, _)| identity.clone()),
    );
    preparation.branches.thermal_limit_active = crate::opf::constraint_mask(
        "branch thermal limits",
        &constraints.thermal_limits,
        &analysis_branch_ids,
        &preparation.branches.identities,
    )?;
    for (active, limit) in preparation
        .branches
        .thermal_limit_active
        .iter_mut()
        .zip(&preparation.branches.f_max)
    {
        *active &= *limit > 0.0;
    }
    preparation.branches.angle_bound_active = crate::opf::constraint_mask(
        "branch angle bounds",
        &constraints.angle_bounds,
        &analysis_branch_ids,
        &preparation.branches.identities,
    )?;

    preparation.n_source_generators = source.generators().len();
    preparation.n_source_branches = source.branches().len();
    preparation.bus_source_rows = preparation
        .bus_analysis_rows
        .iter()
        .map(|&row| (row < source.buses().len()).then_some(row))
        .collect();
    preparation.generators.source_rows = preparation
        .generators
        .analysis_rows
        .iter()
        .map(|&row| (row < source.generators().len()).then_some(row))
        .collect();
    preparation.branches.source_rows = preparation
        .branches
        .analysis_rows
        .iter()
        .map(|&row| (row < source.branches().len()).then_some(row))
        .collect();
    Ok(())
}

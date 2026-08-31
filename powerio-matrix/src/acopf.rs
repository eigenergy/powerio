//! AC OPF assembly: the matrix free numerical arrays derived from an
//! [`AcOpfInstance`], on the branch pi model. 0.9 exposed this surface as
//! `powerio_prob::build_ac_opf_instance`; it lives here now beside the DC
//! preparation so every solver formulates over the one shared assembly.

use serde::{Deserialize, Serialize};

use powerio_prob::{AcOpfInstance, ReferenceBuses};
use powerio_tx::{BalancedNetwork, BusId, IndexedNetwork};

use crate::dcopf::{Units, limits, nodal};
use crate::{Error, PiecewiseLinearCost, PreparedObjective, Result};

/// Assembly choices that select the numerical content derived from an AC
/// instance without changing the instance itself. There is no convention
/// field: the branch pi model always carries taps, shifts, and charging.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AcOpfAssemblyOptions {
    /// Power and cost scaling of the derived arrays.
    pub units: Units,
    /// Skip non-self-loop branches with `r² + x² = 0`. Off by default: zero
    /// impedance branches are preserved in networks and instances, so
    /// assembly refuses them until the caller resolves them explicitly
    /// ([`powerio_prob::merge_zero_impedance_buses`]) or opts into skipping.
    pub skip_zero_impedance: bool,
    /// Give a branch with no thermal rating the bound
    /// [`Branch::synthesize_rate_a`](powerio_tx::Branch::synthesize_rate_a)
    /// states. If false, `rate_a <= 0` reaches `s_max` as zero, which reads
    /// as unlimited.
    pub synthesize_unrated_limits: bool,
}

impl AcOpfAssemblyOptions {
    #[must_use]
    pub const fn with_units(mut self, units: Units) -> Self {
        self.units = units;
        self
    }

    #[must_use]
    pub const fn with_skip_zero_impedance(mut self, skip: bool) -> Self {
        self.skip_zero_impedance = skip;
        self
    }

    #[must_use]
    pub const fn with_synthesize_unrated_limits(mut self, synthesize: bool) -> Self {
        self.synthesize_unrated_limits = synthesize;
        self
    }
}

/// Bus data in dense bus order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcBusData {
    /// Nodal active demand in the selected power unit.
    pub p_d: Vec<f64>,
    /// Nodal reactive demand in the selected power unit.
    pub q_d: Vec<f64>,
    /// Nodal shunt conductance in the selected admittance unit. Includes the
    /// folded pi model stamp of any self-loop branch, matching `calc_admittance_matrix`.
    pub g_s: Vec<f64>,
    /// Nodal shunt susceptance in the selected admittance unit. Includes the
    /// folded pi model stamp of any self-loop branch, matching `calc_admittance_matrix`.
    pub b_s: Vec<f64>,
    /// Voltage magnitude lower bound, per unit.
    pub vm_min: Vec<f64>,
    /// Voltage magnitude upper bound, per unit.
    pub vm_max: Vec<f64>,
    /// Case voltage magnitude, per unit: the raw initial guess, zero when the
    /// source has none.
    pub vm: Vec<f64>,
    /// Whether the instance activates each bus's voltage magnitude bounds.
    pub voltage_bound_active: Vec<bool>,
}

/// Branch data in active branch column order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcBranchData {
    /// Stable branch identity aligned with every following column.
    pub identities: Vec<String>,
    pub from_bus: Vec<usize>,
    pub to_bus: Vec<usize>,
    /// Series conductance `r / (r² + x²)` in the selected admittance unit.
    pub g: Vec<f64>,
    /// Series susceptance `−x / (r² + x²)` in the selected admittance unit.
    pub b: Vec<f64>,
    /// Charging conductance at the from terminal.
    pub g_fr: Vec<f64>,
    /// Charging susceptance at the from terminal.
    pub b_fr: Vec<f64>,
    /// Charging conductance at the to terminal.
    pub g_to: Vec<f64>,
    /// Charging susceptance at the to terminal.
    pub b_to: Vec<f64>,
    /// Tap ratio magnitude; one for a line. Kept separate from `shift` so a
    /// consumer stamps the complex tap itself.
    pub tap: Vec<f64>,
    /// Phase shift in radians.
    pub shift: Vec<f64>,
    /// Apparent power limit in the selected power unit. Zero means unlimited.
    pub s_max: Vec<f64>,
    /// Branch angle bounds in radians, as the source states them.
    pub angle_min: Vec<f64>,
    pub angle_max: Vec<f64>,
    /// Branch column to row in the star-lowered analysis network.
    pub analysis_rows: Vec<usize>,
    /// Branch column to source branch row. Synthetic winding branches have no
    /// source row.
    pub source_rows: Vec<Option<usize>>,
    /// Analysis branch rows omitted because `r² + x² = 0`.
    pub skipped_zero_impedance: Vec<usize>,
    /// Whether the instance activates each apparent power limit.
    pub thermal_limit_active: Vec<bool>,
    /// Whether the instance activates each angle difference bound.
    pub angle_bound_active: Vec<bool>,
}

/// Generator data in generator column order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcGeneratorData {
    /// Stable generator identity aligned with every following column.
    pub identities: Vec<String>,
    /// Generator column to dense bus index.
    pub bus_of_gen: Vec<usize>,
    /// Generator column to row in the analysis network.
    pub analysis_rows: Vec<usize>,
    /// Generator column to source generator row.
    pub source_rows: Vec<Option<usize>>,
    /// Quadratic objective diagonal in `0.5 * q * p^2 + c * p + c0`.
    pub q: Vec<f64>,
    /// Linear objective coefficient.
    pub c: Vec<f64>,
    /// Constant objective term. Unscaled in both unit systems: it carries no
    /// power dimension.
    pub c0: Vec<f64>,
    /// Convex piecewise linear costs aligned with the generator columns. A
    /// present curve is the complete objective term for that generator; its
    /// `q`, `c`, and `c0` entries are zero.
    pub piecewise_linear: Vec<Option<PiecewiseLinearCost>>,
    pub pmax: Vec<f64>,
    pub pmin: Vec<f64>,
    pub qmax: Vec<f64>,
    pub qmin: Vec<f64>,
    /// Scheduled active output in the selected power unit.
    pub pg: Vec<f64>,
    /// Scheduled reactive output in the selected power unit.
    pub qg: Vec<f64>,
    /// Voltage magnitude setpoint, per unit; zero when the source has none.
    pub vg: Vec<f64>,
    /// Whether the instance activates this generator's capability bounds.
    pub capability_active: Vec<bool>,
}

/// Generator data in dense bus order, aggregated over the generators at each
/// bus. See [`AcOpfPreparation::calc_nodal_generator_data`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NodalAcGeneratorData {
    pub q: Vec<f64>,
    pub c: Vec<f64>,
    pub c0: Vec<f64>,
    pub pmax: Vec<f64>,
    pub pmin: Vec<f64>,
    pub qmax: Vec<f64>,
    pub qmin: Vec<f64>,
    /// Which buses host a generator. A bus without one has a zero range and a
    /// zero cost, which a formulation must not read as a free generator. A
    /// reactive limit loop reads it to tell a bus that holds its voltage from
    /// one that cannot.
    pub has_gen: Vec<bool>,
}

/// Matrix free AC OPF input data on the branch pi model.
///
/// Units follow [`Units`]. Under [`Units::PerUnit`], powers are per unit on
/// `base_mva` and admittances are per unit on the system base. Under
/// [`Units::Native`], powers stay in MW/MVAr and every admittance vector is
/// scaled by `base_mva`, so power computed from admittances and per unit
/// voltages lands in MW/MVAr. Voltage magnitudes are per unit and angles are
/// radians in both systems. Relaxations of AC OPF, the SOC forms included,
/// consume this same preparation; the relaxation is a formulation choice
/// made downstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcOpfPreparation {
    pub name: String,
    pub n_buses: usize,
    pub n_source_generators: usize,
    pub n_source_branches: usize,
    pub base_mva: f64,
    pub units: Units,
    /// The objective represented by the generator cost columns.
    pub objective: PreparedObjective,
    pub skip_zero_impedance: bool,
    /// Whether absent source ratings were replaced with synthesized limits.
    pub synthesize_unrated_limits: bool,
    /// Dense bus index to external bus ID.
    pub bus_ids: Vec<BusId>,
    /// Dense bus index to row in the star-lowered analysis network.
    pub bus_analysis_rows: Vec<usize>,
    /// Dense bus index to source bus row. A synthetic star bus has no source
    /// row; an explicitly isolated source bus has no dense row here.
    pub bus_source_rows: Vec<Option<usize>>,
    pub reference_buses: ReferenceBuses,
    pub buses: AcBusData,
    pub generators: AcGeneratorData,
    pub branches: AcBranchData,
}

impl AcOpfPreparation {
    #[must_use]
    pub fn n_generators(&self) -> usize {
        self.generators.q.len()
    }

    #[must_use]
    pub fn n_branches(&self) -> usize {
        self.branches.g.len()
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
    pub fn calc_nodal_generator_data(&self) -> Result<NodalAcGeneratorData> {
        let n = self.n_buses;
        let generators = &self.generators;
        if let Some(gen_index) = generators.piecewise_linear.iter().position(Option::is_some) {
            return Err(Error::PiecewiseNodalCost { gen_index });
        }
        let bus_of_gen = &generators.bus_of_gen;
        let costs =
            nodal::combine_costs(n, bus_of_gen, &generators.q, &generators.c, &generators.c0);
        Ok(NodalAcGeneratorData {
            q: costs.q,
            c: costs.c,
            c0: costs.c0,
            pmax: nodal::sum_by_bus(n, bus_of_gen, &generators.pmax),
            pmin: nodal::sum_by_bus(n, bus_of_gen, &generators.pmin),
            qmax: nodal::sum_by_bus(n, bus_of_gen, &generators.qmax),
            qmin: nodal::sum_by_bus(n, bus_of_gen, &generators.qmin),
            has_gen: nodal::buses_with_generators(n, bus_of_gen),
        })
    }

    /// Conventional voltage magnitude start: the case voltage, overwritten by
    /// each generator's positive setpoint in generator column order (last
    /// wins), with a non-positive case voltage falling back to 1.0.
    ///
    /// The result is not clamped to `[vm_min, vm_max]`; feasibility repair is
    /// solver preparation and stays downstream.
    #[must_use]
    pub fn calc_vm_setpoints(&self) -> Vec<f64> {
        let mut vm: Vec<f64> = self
            .buses
            .vm
            .iter()
            .map(|&value| if value > 0.0 { value } else { 1.0 })
            .collect();
        for generator in 0..self.n_generators() {
            let vg = self.generators.vg[generator];
            if vg > 0.0 {
                vm[self.generators.bus_of_gen[generator]] = vg;
            }
        }
        vm
    }
}

/// Derive the complete matrix free AC OPF arrays from the instance: demand,
/// shunt, and voltage columns per bus, the full pi model per active branch,
/// and generator costs, bounds, and schedules with their source row mapping.
/// The AC counterpart of [`build_dc_opf_preparation`](crate::build_dc_opf_preparation).
///
/// # Errors
/// A network the pi model cannot assemble: missing reference coverage, an
/// unresolved zero impedance branch, or an unusable cost curve.
pub fn build_ac_opf_preparation(
    instance: &AcOpfInstance,
    options: &AcOpfAssemblyOptions,
) -> Result<AcOpfPreparation> {
    let view = IndexedNetwork::new(instance.network());
    let objective = crate::opf::compile_objective(instance.objective())?;
    let mut preparation = preparation_from_view(&view, *options, objective)?;
    apply_instance_semantics(&mut preparation, instance.network(), instance.constraints())?;
    Ok(preparation)
}

/// Build the matrix free AC OPF arrays from an indexed network view.
#[allow(clippy::too_many_lines)]
fn preparation_from_view(
    case: &IndexedNetwork,
    options: AcOpfAssemblyOptions,
    objective: PreparedObjective,
) -> Result<AcOpfPreparation> {
    case.network().check_base_mva()?;

    let active_buses = crate::opf::active_bus_index(case)?;
    let n_buses = active_buses.analysis_rows.len();
    let base = case.per_unit_base();
    let (p_scale, y_scale) = options.units.power_scales(base);
    let thermal = limits::ThermalLimits {
        synthesize_unrated: options.synthesize_unrated_limits,
        power_scale: p_scale,
        admittance_scale: y_scale,
    };
    let (q_scale, c_scale) = options.units.cost_scales(base);

    let mut bus_of_gen = Vec::new();
    let mut generator_identities = Vec::new();
    let mut generator_rows = Vec::new();
    let mut cost_q = Vec::new();
    let mut cost_c = Vec::new();
    let mut cost_c0 = Vec::new();
    let mut piecewise_linear = Vec::new();
    let mut pmax = Vec::new();
    let mut pmin = Vec::new();
    let mut qmax = Vec::new();
    let mut qmin = Vec::new();
    let mut pg = Vec::new();
    let mut qg = Vec::new();
    let mut vg = Vec::new();

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
        let terms = match objective {
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
        cost_q.push(terms.q * q_scale);
        cost_c.push(terms.c * c_scale);
        cost_c0.push(terms.c0);
        piecewise_linear.push(terms.piecewise_linear);
        pmax.push(generator.pmax * p_scale);
        pmin.push(generator.pmin * p_scale);
        qmax.push(generator.qmax * p_scale);
        qmin.push(generator.qmin * p_scale);
        pg.push(generator.pg * p_scale);
        qg.push(generator.qg * p_scale);
        vg.push(generator.vg);
    }
    if cost_q.is_empty() {
        return Err(Error::NoGenerators);
    }

    let mut g_s: Vec<f64> = active_buses
        .analysis_rows
        .iter()
        .map(|&row| case.gs()[row] * p_scale)
        .collect();
    let mut b_s: Vec<f64> = active_buses
        .analysis_rows
        .iter()
        .map(|&row| case.bs()[row] * p_scale)
        .collect();

    let mut from_bus = Vec::new();
    let mut branch_identities = Vec::new();
    let mut to_bus = Vec::new();
    let mut g = Vec::new();
    let mut b = Vec::new();
    let mut g_fr = Vec::new();
    let mut b_fr = Vec::new();
    let mut g_to = Vec::new();
    let mut b_to = Vec::new();
    let mut tap = Vec::new();
    let mut shift = Vec::new();
    let mut s_max = Vec::new();
    let mut angle_min = Vec::new();
    let mut angle_max = Vec::new();
    let mut branch_rows = Vec::new();
    let mut skipped_zero_impedance = Vec::new();
    // Dense bus order is the position order of `network().buses()`; the view
    // already holds the star-lowered network when 3-winding expansion ran.
    let network = case.network();

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
        let Some((series_g, series_b)) = branch.calc_series_admittance(source_row)? else {
            if options.skip_zero_impedance {
                skipped_zero_impedance.push(source_row);
                continue;
            }
            return Err(powerio_tx::Error::ZeroImpedance { row: source_row }.into());
        };
        let charging = branch.calc_terminal_charging();
        if from == to {
            // A self-loop is not a flow element; its whole pi model stamp
            // lands on the bus diagonal, exactly as `calc_admittance_matrix` folds it.
            // With t = tap·e^{jθ}: Yff + Yft + Ytf + Ytt
            //   = (y + y_fr)/tap² + (y + y_to) − y·2cos(θ)/tap.
            let tap = branch.calc_divisible_tap(source_row)?;
            let tap_squared = tap * tap;
            let cross = 2.0 * case.to_radians(branch.shift).cos() / tap;
            g_s[from] += ((series_g + charging.g_fr) / tap_squared + (series_g + charging.g_to)
                - series_g * cross)
                * y_scale;
            b_s[from] += ((series_b + charging.b_fr) / tap_squared + (series_b + charging.b_to)
                - series_b * cross)
                * y_scale;
            continue;
        }
        from_bus.push(from);
        branch_identities.push(crate::opf::row_identity(
            branch.uid.as_deref(),
            "branches",
            source_row,
        ));
        to_bus.push(to);
        g.push(series_g * y_scale);
        b.push(series_b * y_scale);
        g_fr.push(charging.g_fr * y_scale);
        b_fr.push(charging.b_fr * y_scale);
        g_to.push(charging.g_to * y_scale);
        b_to.push(charging.b_to * y_scale);
        let amin = case.to_radians(branch.angmin);
        let amax = case.to_radians(branch.angmax);
        tap.push(branch.calc_divisible_tap(source_row)?);
        shift.push(case.to_radians(branch.shift));
        s_max.push(thermal.of(
            branch,
            amin,
            amax,
            &network.buses()[from_analysis],
            &network.buses()[to_analysis],
        ));
        angle_min.push(amin);
        angle_max.push(amax);
        branch_rows.push(source_row);
    }

    let mut vm_min = Vec::with_capacity(n_buses);
    let mut vm_max = Vec::with_capacity(n_buses);
    let mut vm = Vec::with_capacity(n_buses);
    for &analysis_row in &active_buses.analysis_rows {
        let bus = &network.buses()[analysis_row];
        vm_min.push(bus.vmin);
        vm_max.push(bus.vmax);
        vm.push(bus.vm);
    }

    let n_active_generators = cost_q.len();
    let n_active_branches = g.len();
    let p_d = active_buses
        .analysis_rows
        .iter()
        .map(|&row| case.pd()[row] * p_scale)
        .collect();
    let q_d = active_buses
        .analysis_rows
        .iter()
        .map(|&row| case.qd()[row] * p_scale)
        .collect();
    let bus_analysis_rows = active_buses.analysis_rows;
    let bus_source_rows = bus_analysis_rows.iter().copied().map(Some).collect();
    Ok(AcOpfPreparation {
        name: case.name().to_owned(),
        n_buses,
        n_source_generators: case.generators().len(),
        n_source_branches: case.branches().len(),
        base_mva: case.base_mva(),
        units: options.units,
        objective,
        skip_zero_impedance: options.skip_zero_impedance,
        synthesize_unrated_limits: options.synthesize_unrated_limits,
        bus_ids: active_buses.bus_ids,
        bus_analysis_rows,
        bus_source_rows,
        reference_buses: active_buses.reference_buses,
        buses: AcBusData {
            p_d,
            q_d,
            g_s,
            b_s,
            vm_min,
            vm_max,
            vm,
            voltage_bound_active: vec![true; n_buses],
        },
        generators: AcGeneratorData {
            identities: generator_identities,
            bus_of_gen,
            analysis_rows: generator_rows.clone(),
            source_rows: generator_rows.into_iter().map(Some).collect(),
            q: cost_q,
            c: cost_c,
            c0: cost_c0,
            piecewise_linear,
            pmax,
            pmin,
            qmax,
            qmin,
            pg,
            qg,
            vg,
            capability_active: vec![true; n_active_generators],
        },
        branches: AcBranchData {
            identities: branch_identities,
            from_bus,
            to_bus,
            g,
            b,
            g_fr,
            b_fr,
            g_to,
            b_to,
            tap,
            shift,
            s_max,
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

fn apply_instance_semantics(
    preparation: &mut AcOpfPreparation,
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
    let mut analysis_bus_ids: Vec<String> = source
        .buses()
        .iter()
        .map(|bus| bus.id.to_string())
        .collect();
    for bus in &preparation.bus_ids {
        let identity = bus.to_string();
        if !analysis_bus_ids.iter().any(|known| known == &identity) {
            analysis_bus_ids.push(identity);
        }
    }

    preparation.buses.voltage_bound_active = crate::opf::constraint_mask(
        "bus voltage bounds",
        &constraints.voltage_bounds,
        &analysis_bus_ids,
        &preparation
            .bus_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    )?;
    preparation.generators.capability_active = crate::opf::constraint_mask(
        "generator capability",
        &constraints.generator_capability,
        &source_generator_ids,
        &preparation.generators.identities,
    )?;
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
        .zip(&preparation.branches.s_max)
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

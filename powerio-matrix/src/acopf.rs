//! AC OPF assembly: the matrix free numerical arrays derived from an
//! [`AcOpfInstance`], on the branch pi model. 0.9 exposed this surface as
//! `powerio_prob::build_ac_opf_instance`; it lives here now beside the DC
//! preparation so every solver formulates over the one shared assembly.

use serde::{Deserialize, Serialize};

use powerio_prob::{AcBusSpecification, AcOpfInstance, AcPfInstance, ReferenceBuses};
use powerio_tx::{BalancedNetwork, BusId, IndexedNetwork};

use crate::dcopf::{Units, limits, nodal};
use crate::{AnalysisBranchSource, Error, PiecewiseLinearCost, PreparedObjective, Result};

/// Assembly choices that select the numerical content derived from an AC
/// instance without changing the instance itself. There is no convention
/// field: the branch pi model always carries taps, shifts, and charging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Apply PowerModels' ±60 degree correction to unconstrained or unusable
    /// branch angle difference intervals in the prepared arrays.
    pub correct_angle_difference_bounds: bool,
}

impl Default for AcOpfAssemblyOptions {
    fn default() -> Self {
        Self {
            units: Units::default(),
            skip_zero_impedance: false,
            synthesize_unrated_limits: false,
            correct_angle_difference_bounds: true,
        }
    }
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

    #[must_use]
    pub const fn with_correct_angle_difference_bounds(mut self, correct: bool) -> Self {
        self.correct_angle_difference_bounds = correct;
        self
    }
}

/// Assembly choices for an AC power flow instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct AcPfAssemblyOptions {
    /// Power and admittance scaling of the prepared values.
    pub units: Units,
    /// Skip non-self-loop branches with `r² + x² = 0`.
    pub skip_zero_impedance: bool,
    /// Apply PowerModels' ±60 degree correction to unconstrained or unusable
    /// branch angle difference intervals in the prepared arrays.
    pub correct_angle_difference_bounds: bool,
}

impl Default for AcPfAssemblyOptions {
    fn default() -> Self {
        Self {
            units: Units::default(),
            skip_zero_impedance: false,
            correct_angle_difference_bounds: true,
        }
    }
}

impl AcPfAssemblyOptions {
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
    pub const fn with_correct_angle_difference_bounds(mut self, correct: bool) -> Self {
        self.correct_angle_difference_bounds = correct;
        self
    }
}

/// One AC power flow bus specification in preparation units.
///
/// This has the same cases as [`AcBusSpecification`], but active and
/// reactive power use the preparation's selected [`Units`] and reference
/// angles are radians. The builder converts the caller's exact case and
/// values; it never derives a replacement from the network bus type.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum PreparedAcBusSpecification {
    Pq { p: f64, q: f64 },
    Pv { p: f64, vm: f64 },
    Reference { vm: f64, va: f64 },
    Isolated,
}

/// Bus values needed to start and evaluate an AC power flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcPfBusData {
    /// Nodal reactive demand in the selected power unit. Synthetic transformer
    /// star buses carry zero.
    pub q_d: Vec<f64>,
    /// Nodal shunt conductance in the selected admittance unit.
    pub g_s: Vec<f64>,
    /// Nodal shunt susceptance in the selected admittance unit.
    pub b_s: Vec<f64>,
    /// Initial voltage magnitude, per unit.
    pub initial_vm: Vec<f64>,
    /// Initial voltage angle, radians.
    pub initial_va: Vec<f64>,
}

/// Generator data used by PV to PQ reactive limit handling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcPfGeneratorData {
    pub identities: Vec<String>,
    /// Generator column to dense bus index.
    pub bus_of_gen: Vec<usize>,
    /// Generator column to row in the star-lowered analysis network.
    pub analysis_rows: Vec<usize>,
    /// Generator column to source generator row.
    pub source_rows: Vec<Option<usize>>,
    /// Initial reactive output in the selected power unit.
    pub qg: Vec<f64>,
    pub qmax: Vec<f64>,
    pub qmin: Vec<f64>,
}

/// Matrix free AC power flow input on the branch pi model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcPfPreparation {
    pub name: String,
    pub n_buses: usize,
    pub n_source_generators: usize,
    pub n_source_branches: usize,
    pub base_mva: f64,
    pub units: Units,
    pub skip_zero_impedance: bool,
    /// Whether PowerModels' angle difference correction was applied.
    pub correct_angle_difference_bounds: bool,
    /// Dense bus index to external bus ID.
    pub bus_ids: Vec<BusId>,
    /// Dense bus index to row in the star-lowered analysis network.
    pub bus_analysis_rows: Vec<usize>,
    /// Dense bus index to source bus row. A synthetic transformer star bus has
    /// no source row; a source bus specified as isolated has no dense row.
    pub bus_source_rows: Vec<Option<usize>>,
    /// Bus specifications in dense bus order. A synthetic transformer star
    /// bus is a zero injection PQ junction. Source rows specified as isolated
    /// are absent from the numerical rows but remain on the `AcPfInstance`.
    pub specifications: Vec<PreparedAcBusSpecification>,
    pub reference_buses: ReferenceBuses,
    pub buses: AcPfBusData,
    pub generators: AcPfGeneratorData,
    pub branches: AcBranchData,
}

impl AcPfPreparation {
    #[must_use]
    pub fn n_generators(&self) -> usize {
        self.generators.qg.len()
    }

    #[must_use]
    pub fn n_branches(&self) -> usize {
        self.branches.g.len()
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
    /// Initial voltage magnitude, per unit. The case voltage is used when the
    /// instance does not supply an override.
    pub initial_vm: Vec<f64>,
    /// Initial voltage angle, radians. The case angle is used when the
    /// instance does not supply an override.
    pub initial_va: Vec<f64>,
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
    /// Source component for each analysis branch column. Lowered transformer
    /// windings remain mapped to their typed transformer row and winding.
    pub analysis_sources: Vec<AnalysisBranchSource>,
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

/// Storage data in active storage column order.
///
/// Power, energy, ratings, reactive limits, and fixed losses use the
/// preparation's selected [`Units`]. Efficiencies, impedance, and service
/// status are dimensionless. Out of service storage and storage attached to
/// an isolated bus are omitted, matching the other prepared element tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcStorageData {
    /// Stable storage identity aligned with every following column.
    pub identities: Vec<String>,
    /// Storage column to dense bus index.
    pub bus_of_storage: Vec<usize>,
    /// Storage column to source storage row.
    pub source_rows: Vec<usize>,
    pub p: Vec<f64>,
    pub q: Vec<f64>,
    pub energy: Vec<f64>,
    pub energy_rating: Vec<f64>,
    pub charge_rating: Vec<f64>,
    pub discharge_rating: Vec<f64>,
    pub charge_efficiency: Vec<f64>,
    pub discharge_efficiency: Vec<f64>,
    pub s_max: Vec<f64>,
    pub qmin: Vec<f64>,
    pub qmax: Vec<f64>,
    pub r: Vec<f64>,
    pub x: Vec<f64>,
    pub p_loss: Vec<f64>,
    pub q_loss: Vec<f64>,
    pub in_service: Vec<bool>,
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
    /// Whether PowerModels' angle difference correction was applied.
    pub correct_angle_difference_bounds: bool,
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
    pub storage: AcStorageData,
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

    #[must_use]
    pub fn n_storage(&self) -> usize {
        self.storage.identities.len()
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
            .initial_vm
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
    if let Some(point) = instance.initial_point() {
        let (power_scale, _) = options.units.power_scales(preparation.base_mva);
        for (dense, bus) in preparation.bus_ids.iter().copied().enumerate() {
            if let Some(value) = point.bus_voltage_magnitude(bus) {
                preparation.buses.initial_vm[dense] = value;
            }
            if let Some(value) = point.bus_voltage_angle(bus) {
                preparation.buses.initial_va[dense] = value;
            }
        }
        for (generator, identity) in preparation.generators.identities.iter().enumerate() {
            if let Some(value) = point.generator_active_power(identity) {
                preparation.generators.pg[generator] = value * power_scale;
            }
            if let Some(value) = point.generator_reactive_power(identity) {
                preparation.generators.qg[generator] = value * power_scale;
            }
            if let Some(value) = point.generator_voltage_setpoint(identity) {
                preparation.generators.vg[generator] = value;
            }
        }
    }
    Ok(preparation)
}

/// Derive the complete matrix free AC power flow arrays from an instance.
/// The caller's bus specifications remain authoritative. Network bus types
/// are used only by `AcPfInstance::from_network` when it creates those
/// specifications; this function does not infer them again.
///
/// # Errors
/// A specification layout that leaves an energized component without a
/// reference bus, or a branch pi model that cannot be assembled.
pub fn build_ac_pf_preparation(
    instance: &AcPfInstance,
    options: &AcPfAssemblyOptions,
) -> Result<AcPfPreparation> {
    let source = instance.network();
    let mut analysis_network = source.clone();
    for (bus, specification) in analysis_network
        .buses_mut()
        .iter_mut()
        .zip(instance.specifications())
    {
        bus.kind = match specification {
            AcBusSpecification::Pq { .. } => powerio_tx::BusType::Pq,
            AcBusSpecification::Pv { .. } => powerio_tx::BusType::Pv,
            AcBusSpecification::Reference { .. } => powerio_tx::BusType::Ref,
            AcBusSpecification::Isolated => powerio_tx::BusType::Isolated,
            _ => return Err(Error::UnsupportedAcPfSpecification),
        };
    }

    let view = IndexedNetwork::new(&analysis_network);
    let mut common = preparation_from_view(
        &view,
        AcOpfAssemblyOptions {
            units: options.units,
            skip_zero_impedance: options.skip_zero_impedance,
            synthesize_unrated_limits: false,
            correct_angle_difference_bounds: options.correct_angle_difference_bounds,
        },
        PreparedObjective::Feasibility,
    )?;
    apply_source_mappings(&mut common, source);

    let (power_scale, _) = options.units.power_scales(view.per_unit_base());
    let specifications = common
        .bus_source_rows
        .iter()
        .map(|source_row| match source_row {
            Some(row) => prepare_bus_specification(
                instance.specifications()[*row],
                power_scale,
                source.is_normalized(),
            ),
            None => Ok(PreparedAcBusSpecification::Pq { p: 0.0, q: 0.0 }),
        })
        .collect::<Result<Vec<_>>>()?;

    let mut initial_vm = common.buses.initial_vm.clone();
    let mut initial_va = common
        .bus_analysis_rows
        .iter()
        .map(|&row| view.to_radians(view.network().buses()[row].va))
        .collect::<Vec<_>>();
    if let Some(point) = instance.initial_point() {
        for (dense, bus) in common.bus_ids.iter().copied().enumerate() {
            if common.bus_source_rows[dense].is_none() {
                continue;
            }
            if let Some(value) = point.bus_voltage_magnitude(bus) {
                initial_vm[dense] = value;
            }
            if let Some(value) = point.bus_voltage_angle(bus) {
                initial_va[dense] = value;
            }
        }
    }

    Ok(AcPfPreparation {
        name: common.name,
        n_buses: common.n_buses,
        n_source_generators: common.n_source_generators,
        n_source_branches: common.n_source_branches,
        base_mva: common.base_mva,
        units: common.units,
        skip_zero_impedance: common.skip_zero_impedance,
        correct_angle_difference_bounds: common.correct_angle_difference_bounds,
        bus_ids: common.bus_ids,
        bus_analysis_rows: common.bus_analysis_rows,
        bus_source_rows: common.bus_source_rows,
        specifications,
        reference_buses: common.reference_buses,
        buses: AcPfBusData {
            q_d: common.buses.q_d,
            g_s: common.buses.g_s,
            b_s: common.buses.b_s,
            initial_vm,
            initial_va,
        },
        generators: AcPfGeneratorData {
            identities: common.generators.identities,
            bus_of_gen: common.generators.bus_of_gen,
            analysis_rows: common.generators.analysis_rows,
            source_rows: common.generators.source_rows,
            qg: common.generators.qg,
            qmax: common.generators.qmax,
            qmin: common.generators.qmin,
        },
        branches: common.branches,
    })
}

fn prepare_bus_specification(
    specification: AcBusSpecification,
    power_scale: f64,
    source_is_normalized: bool,
) -> Result<PreparedAcBusSpecification> {
    Ok(match specification {
        AcBusSpecification::Pq { p, q } => PreparedAcBusSpecification::Pq {
            p: p * power_scale,
            q: q * power_scale,
        },
        AcBusSpecification::Pv { p, vm } => PreparedAcBusSpecification::Pv {
            p: p * power_scale,
            vm,
        },
        AcBusSpecification::Reference { vm, va } => PreparedAcBusSpecification::Reference {
            vm,
            va: if source_is_normalized {
                va
            } else {
                va.to_radians()
            },
        },
        AcBusSpecification::Isolated => PreparedAcBusSpecification::Isolated,
        _ => return Err(Error::UnsupportedAcPfSpecification),
    })
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

    let mut storage_identities = Vec::new();
    let mut bus_of_storage = Vec::new();
    let mut storage_source_rows = Vec::new();
    let mut storage_p = Vec::new();
    let mut storage_q = Vec::new();
    let mut storage_energy = Vec::new();
    let mut storage_energy_rating = Vec::new();
    let mut storage_charge_rating = Vec::new();
    let mut storage_discharge_rating = Vec::new();
    let mut storage_charge_efficiency = Vec::new();
    let mut storage_discharge_efficiency = Vec::new();
    let mut storage_s_max = Vec::new();
    let mut storage_qmin = Vec::new();
    let mut storage_qmax = Vec::new();
    let mut storage_r = Vec::new();
    let mut storage_x = Vec::new();
    let mut storage_p_loss = Vec::new();
    let mut storage_q_loss = Vec::new();
    let mut storage_in_service = Vec::new();
    for (source_row, storage) in case.network().storage().iter().enumerate() {
        if !storage.in_service {
            continue;
        }
        let analysis_bus = case
            .bus_index(storage.bus)
            .ok_or(powerio_tx::Error::UnknownBus {
                bus_id: storage.bus,
                element_index: source_row,
            })?;
        let Some(bus) = active_buses.dense_by_analysis[analysis_bus] else {
            continue;
        };
        storage_identities.push(crate::opf::row_identity(
            storage.uid.as_deref(),
            "storage",
            source_row,
        ));
        bus_of_storage.push(bus);
        storage_source_rows.push(source_row);
        storage_p.push(storage.ps * p_scale);
        storage_q.push(storage.qs * p_scale);
        storage_energy.push(storage.energy * p_scale);
        storage_energy_rating.push(storage.energy_rating * p_scale);
        storage_charge_rating.push(storage.charge_rating * p_scale);
        storage_discharge_rating.push(storage.discharge_rating * p_scale);
        storage_charge_efficiency.push(storage.charge_efficiency);
        storage_discharge_efficiency.push(storage.discharge_efficiency);
        storage_s_max.push(storage.thermal_rating * p_scale);
        storage_qmin.push(storage.qmin * p_scale);
        storage_qmax.push(storage.qmax * p_scale);
        storage_r.push(storage.r);
        storage_x.push(storage.x);
        storage_p_loss.push(storage.p_loss * p_scale);
        storage_q_loss.push(storage.q_loss * p_scale);
        storage_in_service.push(storage.in_service);
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
        let source_amin = case.to_radians(branch.angmin);
        let source_amax = case.to_radians(branch.angmax);
        let (amin, amax) = if options.correct_angle_difference_bounds {
            powerio_tx::correct_angle_difference_bounds(source_amin, source_amax)
        } else {
            (source_amin, source_amax)
        };
        tap.push(branch.calc_divisible_tap(source_row)?);
        shift.push(case.to_radians(branch.shift));
        s_max.push(thermal.of(
            branch,
            source_amin,
            source_amax,
            &network.buses()[from_analysis],
            &network.buses()[to_analysis],
        ));
        angle_min.push(amin);
        angle_max.push(amax);
        branch_rows.push(source_row);
    }

    let mut vm_min = Vec::with_capacity(n_buses);
    let mut vm_max = Vec::with_capacity(n_buses);
    let mut initial_vm = Vec::with_capacity(n_buses);
    let mut initial_va = Vec::with_capacity(n_buses);
    for &analysis_row in &active_buses.analysis_rows {
        let bus = &network.buses()[analysis_row];
        vm_min.push(bus.vmin);
        vm_max.push(bus.vmax);
        initial_vm.push(bus.vm);
        initial_va.push(case.to_radians(bus.va));
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
        correct_angle_difference_bounds: options.correct_angle_difference_bounds,
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
            initial_vm,
            initial_va,
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
        storage: AcStorageData {
            identities: storage_identities,
            bus_of_storage,
            source_rows: storage_source_rows,
            p: storage_p,
            q: storage_q,
            energy: storage_energy,
            energy_rating: storage_energy_rating,
            charge_rating: storage_charge_rating,
            discharge_rating: storage_discharge_rating,
            charge_efficiency: storage_charge_efficiency,
            discharge_efficiency: storage_discharge_efficiency,
            s_max: storage_s_max,
            qmin: storage_qmin,
            qmax: storage_qmax,
            r: storage_r,
            x: storage_x,
            p_loss: storage_p_loss,
            q_loss: storage_q_loss,
            in_service: storage_in_service,
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
            analysis_sources: branch_rows
                .iter()
                .copied()
                .map(|row| AnalysisBranchSource::Branch { row })
                .collect(),
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

    apply_source_mappings(preparation, source);
    Ok(())
}

fn apply_source_mappings(preparation: &mut AcOpfPreparation, source: &BalancedNetwork) {
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
    let analysis_sources = crate::opf::analysis_branch_sources(source);
    preparation.branches.analysis_sources = preparation
        .branches
        .analysis_rows
        .iter()
        .map(|&row| analysis_sources[row])
        .collect();
}

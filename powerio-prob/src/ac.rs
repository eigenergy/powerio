use serde::{Deserialize, Serialize};

use powerio::{BusId, Error, IndexedNetwork, Result};

use crate::Units;

/// Options for AC OPF instance assembly.
///
/// There is no convention enum: the branch pi model always carries taps,
/// shifts, and charging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcOpfOptions {
    pub units: Units,
    /// Skip non-self-loop branches with `r² + x² = 0`. If false, assembly
    /// returns [`Error::ZeroImpedance`].
    pub skip_zero_impedance: bool,
}

impl Default for AcOpfOptions {
    fn default() -> Self {
        Self {
            units: Units::default(),
            skip_zero_impedance: true,
        }
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
    /// folded pi model stamp of any self-loop branch, matching `build_ybus`.
    pub g_s: Vec<f64>,
    /// Nodal shunt susceptance in the selected admittance unit. Includes the
    /// folded pi model stamp of any self-loop branch, matching `build_ybus`.
    pub b_s: Vec<f64>,
    /// Voltage magnitude lower bound, per unit.
    pub vm_min: Vec<f64>,
    /// Voltage magnitude upper bound, per unit.
    pub vm_max: Vec<f64>,
    /// Case voltage magnitude, per unit: the raw initial guess, zero when the
    /// source has none.
    pub vm: Vec<f64>,
}

/// Branch data in active branch column order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcBranchData {
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
    /// Branch column to source branch row.
    pub source_rows: Vec<usize>,
    /// Source branch rows omitted because `r² + x² = 0`.
    pub skipped_zero_impedance: Vec<usize>,
}

/// Generator data in generator column order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcGeneratorData {
    /// Generator column to dense bus index.
    pub bus_of_gen: Vec<usize>,
    /// Generator column to source generator row.
    pub source_rows: Vec<usize>,
    /// Quadratic objective diagonal in `0.5 * q * p^2 + c * p + c0`.
    pub q: Vec<f64>,
    /// Linear objective coefficient.
    pub c: Vec<f64>,
    /// Constant objective term. Unscaled in both unit systems: it carries no
    /// power dimension.
    pub c0: Vec<f64>,
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
}

/// Matrix free AC OPF input data on the branch pi model.
///
/// A problem instance is complete numerical input for one problem family. It
/// is separate from the source network, a matrix projection, a solver
/// formulation, and a solution. Relaxations of AC OPF, the SOC forms
/// included, consume this same instance; the relaxation is a formulation
/// choice made downstream.
///
/// Units follow [`Units`]. Under [`Units::PerUnit`], powers are per unit on
/// `base_mva` and admittances are per unit on the system base. Under
/// [`Units::Native`], powers stay in MW/MVAr and every admittance vector is
/// scaled by `base_mva`, so power computed from admittances and per unit
/// voltages lands in MW/MVAr. Voltage magnitudes are per unit and angles are
/// radians in both systems.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AcOpfInstance {
    pub name: String,
    pub n_buses: usize,
    pub n_source_generators: usize,
    pub n_source_branches: usize,
    pub base_mva: f64,
    pub units: Units,
    pub skip_zero_impedance: bool,
    /// Dense bus index to external bus ID.
    pub bus_ids: Vec<BusId>,
    pub reference_buses: Vec<usize>,
    pub buses: AcBusData,
    pub generators: AcGeneratorData,
    pub branches: AcBranchData,
}

impl AcOpfInstance {
    #[must_use]
    pub fn n_generators(&self) -> usize {
        self.generators.q.len()
    }

    #[must_use]
    pub fn n_branches(&self) -> usize {
        self.branches.g.len()
    }

    /// Conventional voltage magnitude start: the case voltage, overwritten by
    /// each generator's positive setpoint in generator column order (last
    /// wins), with a non-positive case voltage falling back to 1.0.
    ///
    /// The result is not clamped to `[vm_min, vm_max]`; feasibility repair is
    /// solver preparation and stays downstream.
    #[must_use]
    pub fn vm_setpoints(&self) -> Vec<f64> {
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

/// Build a matrix free AC OPF instance from an indexed network.
#[allow(clippy::too_many_lines)]
pub fn build_ac_opf_instance(
    case: &IndexedNetwork,
    options: &AcOpfOptions,
) -> Result<AcOpfInstance> {
    case.check_reference_coverage()?;
    case.network().check_base_mva()?;

    let n_buses = case.n();
    let base = case.per_unit_base();
    let (p_scale, y_scale) = options.units.power_scales(base);
    let (q_scale, c_scale) = options.units.cost_scales(base);

    let mut bus_of_gen = Vec::new();
    let mut generator_rows = Vec::new();
    let mut cost_q = Vec::new();
    let mut cost_c = Vec::new();
    let mut cost_c0 = Vec::new();
    let mut pmax = Vec::new();
    let mut pmin = Vec::new();
    let mut qmax = Vec::new();
    let mut qmin = Vec::new();
    let mut pg = Vec::new();
    let mut qg = Vec::new();
    let mut vg = Vec::new();

    for (source_row, generator) in case.in_service_gens() {
        let bus = case.bus_index(generator.bus).ok_or(Error::UnknownBus {
            bus_id: generator.bus,
            element_index: source_row,
        })?;
        let cost = generator.cost.as_ref().ok_or(Error::MissingGenCost {
            gen_index: source_row,
        })?;
        let (q_raw, c_raw, c0_raw) =
            cost.quadratic_with_constant()
                .ok_or(Error::UnsupportedCostModel {
                    gen_index: source_row,
                    model: cost.model,
                    ncost: cost.ncost,
                })?;
        bus_of_gen.push(bus);
        generator_rows.push(source_row);
        cost_q.push(q_raw * q_scale);
        cost_c.push(c_raw * c_scale);
        cost_c0.push(c0_raw);
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

    let mut g_s: Vec<f64> = case.gs().iter().map(|value| value * p_scale).collect();
    let mut b_s: Vec<f64> = case.bs().iter().map(|value| value * p_scale).collect();

    let mut from_bus = Vec::new();
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

    for (source_row, branch) in case.in_service_branches() {
        let from = case.bus_index(branch.from).ok_or(Error::UnknownBus {
            bus_id: branch.from,
            element_index: source_row,
        })?;
        let to = case.bus_index(branch.to).ok_or(Error::UnknownBus {
            bus_id: branch.to,
            element_index: source_row,
        })?;
        let Some((series_g, series_b)) = branch.series_admittance(source_row)? else {
            if options.skip_zero_impedance {
                skipped_zero_impedance.push(source_row);
                continue;
            }
            return Err(Error::ZeroImpedance { row: source_row });
        };
        let charging = branch.terminal_charging();
        if from == to {
            // A self-loop is not a flow element; its whole pi model stamp
            // lands on the bus diagonal, exactly as `build_ybus` folds it.
            // With t = tap·e^{jθ}: Yff + Yft + Ytf + Ytt
            //   = (y + y_fr)/tap² + (y + y_to) − y·2cos(θ)/tap.
            let tap = branch.effective_tap();
            let tap_squared = tap * tap;
            let cross = 2.0 * case.angle_radians(branch.shift).cos() / tap;
            g_s[from] += ((series_g + charging.g_fr) / tap_squared + (series_g + charging.g_to)
                - series_g * cross)
                * y_scale;
            b_s[from] += ((series_b + charging.b_fr) / tap_squared + (series_b + charging.b_to)
                - series_b * cross)
                * y_scale;
            continue;
        }
        from_bus.push(from);
        to_bus.push(to);
        g.push(series_g * y_scale);
        b.push(series_b * y_scale);
        g_fr.push(charging.g_fr * y_scale);
        b_fr.push(charging.b_fr * y_scale);
        g_to.push(charging.g_to * y_scale);
        b_to.push(charging.b_to * y_scale);
        tap.push(branch.effective_tap());
        shift.push(case.angle_radians(branch.shift));
        s_max.push(branch.rate_a * p_scale);
        angle_min.push(case.angle_radians(branch.angmin));
        angle_max.push(case.angle_radians(branch.angmax));
        branch_rows.push(source_row);
    }

    // Dense bus order is the position order of `network().buses`; the view
    // already holds the star-lowered network when 3-winding expansion ran.
    let network = case.network();
    let mut vm_min = Vec::with_capacity(n_buses);
    let mut vm_max = Vec::with_capacity(n_buses);
    let mut vm = Vec::with_capacity(n_buses);
    for bus in &network.buses {
        vm_min.push(bus.vmin);
        vm_max.push(bus.vmax);
        vm.push(bus.vm);
    }

    Ok(AcOpfInstance {
        name: case.name().to_owned(),
        n_buses,
        n_source_generators: case.generators().len(),
        n_source_branches: case.branches().len(),
        base_mva: case.base_mva(),
        units: options.units,
        skip_zero_impedance: options.skip_zero_impedance,
        bus_ids: (0..n_buses).map(|index| case.bus_id(index)).collect(),
        reference_buses: case.reference_bus_indices(),
        buses: AcBusData {
            p_d: case.pd().iter().map(|value| value * p_scale).collect(),
            q_d: case.qd().iter().map(|value| value * p_scale).collect(),
            g_s,
            b_s,
            vm_min,
            vm_max,
            vm,
        },
        generators: AcGeneratorData {
            bus_of_gen,
            source_rows: generator_rows,
            q: cost_q,
            c: cost_c,
            c0: cost_c0,
            pmax,
            pmin,
            qmax,
            qmin,
            pg,
            qg,
            vg,
        },
        branches: AcBranchData {
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
            source_rows: branch_rows,
            skipped_zero_impedance,
        },
    })
}

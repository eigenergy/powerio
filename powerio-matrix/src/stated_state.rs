//! The AC bus balance at the voltages a case states:
//! [`calc_stated_state_mismatch`] and [`calc_stated_branch_flows`].

use std::collections::BTreeMap;
use std::ops::BitOr;

use num_complex::Complex64;

use powerio_tx::{
    BalancedNetwork, BusId, BusType, HvdcConverterKind, HvdcTreatment, IndexedNetwork,
    LoadVoltageModel,
};

use crate::matrix::{YbusFlags, branch_admittance, branch_flows};
use crate::opf::{AnalysisBranchSource, analysis_branch_sources};
use crate::{Error, Result};

/// Equipment at a bus that a stated-state residual is commonly traced to.
///
/// A set of named bits; combine them with `|`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct StatedBusFlags(u16);

impl StatedBusFlags {
    /// No flag.
    pub const NONE: Self = Self(0);
    /// A terminal of an in service HVDC line.
    pub const HVDC: Self = Self(1);
    /// A terminal of an in service HVDC line with a voltage source converter.
    pub const VSC: Self = Self(1 << 1);
    /// An in service static VAR compensator.
    pub const FACTS: Self = Self(1 << 2);
    /// An in service generator on a bus the case types as PQ.
    pub const GENERATOR_ON_PQ_BUS: Self = Self(1 << 3);
    /// The star point a three winding transformer was lowered to.
    pub const STAR_BUS: Self = Self(1 << 4);
    /// The bus that survived a bus merge
    /// ([`StatedStateOptions::merged_buses`]).
    pub const MERGED_GROUP: Self = Self(1 << 5);
    /// An in service load with a constant current or constant impedance part,
    /// or an exponential voltage model.
    pub const VOLTAGE_DEPENDENT_LOAD: Self = Self(1 << 6);
    /// An in service switched shunt.
    pub const SWITCHED_SHUNT: Self = Self(1 << 7);
    /// A reference bus.
    pub const REFERENCE: Self = Self(1 << 8);
    /// An in service storage unit.
    pub const STORAGE: Self = Self(1 << 9);
    /// A terminal of an in service branch or winding whose series impedance
    /// magnitude is below [`StatedStateOptions::low_impedance_threshold`]. The
    /// flow such a branch carries is the small difference of two stored
    /// voltages divided by a small impedance, so the rounding of the stored
    /// values alone can leave a residual here.
    pub const LOW_IMPEDANCE_BRANCH: Self = Self(1 << 10);

    const NAMED: [(Self, &'static str); 11] = [
        (Self::HVDC, "hvdc"),
        (Self::VSC, "vsc"),
        (Self::FACTS, "facts"),
        (Self::GENERATOR_ON_PQ_BUS, "generator_on_pq_bus"),
        (Self::STAR_BUS, "star_bus"),
        (Self::MERGED_GROUP, "merged_group"),
        (Self::VOLTAGE_DEPENDENT_LOAD, "voltage_dependent_load"),
        (Self::SWITCHED_SHUNT, "switched_shunt"),
        (Self::REFERENCE, "reference"),
        (Self::STORAGE, "storage"),
        (Self::LOW_IMPEDANCE_BRANCH, "low_impedance_branch"),
    ];

    /// Every flag.
    pub const ALL: Self = Self((1 << 11) - 1);

    /// Whether every flag in `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any flag in `other` is set.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether no flag is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The snake case names of the set flags, in declaration order.
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMED
            .iter()
            .filter(|(flag, _)| self.contains(*flag))
            .map(|(_, name)| *name)
            .collect()
    }

    /// The flag a snake case name spells, as [`names`](Self::names) writes it.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::NAMED
            .iter()
            .find(|(_, spelled)| *spelled == name)
            .map(|(flag, _)| *flag)
    }
}

impl BitOr for StatedBusFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for StatedBusFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Options for [`calc_stated_state_mismatch`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StatedStateOptions {
    /// How HVDC lines enter the stated injections.
    pub hvdc_treatment: HvdcTreatment,
    /// How many buses [`StatedStateMismatch::top`] lists. Default 20.
    pub top_k: usize,
    /// Merged bus to surviving bus, as a bus merge reports it
    /// (`ZeroImpedanceMerge::merged_buses` in `powerio-prob`). Every
    /// survivor is flagged [`StatedBusFlags::MERGED_GROUP`]; the map is not
    /// otherwise read.
    pub merged_buses: BTreeMap<BusId, BusId>,
    /// Series impedance magnitude, per unit, below which a branch's
    /// terminals are flagged [`StatedBusFlags::LOW_IMPEDANCE_BRANCH`].
    /// Default `1e-3`: a case that states voltage magnitudes to five decimals
    /// leaves such a branch's flow uncertain by about one MW on a 100 MVA
    /// base.
    pub low_impedance_threshold: f64,
}

impl Default for StatedStateOptions {
    fn default() -> Self {
        Self {
            hvdc_treatment: HvdcTreatment::default(),
            top_k: 20,
            merged_buses: BTreeMap::new(),
            low_impedance_threshold: 1e-3,
        }
    }
}

impl StatedStateOptions {
    #[must_use]
    pub const fn with_hvdc_treatment(mut self, treatment: HvdcTreatment) -> Self {
        self.hvdc_treatment = treatment;
        self
    }

    #[must_use]
    pub const fn with_top_k(mut self, top_k: usize) -> Self {
        self.top_k = top_k;
        self
    }

    #[must_use]
    pub fn with_merged_buses(mut self, merged_buses: BTreeMap<BusId, BusId>) -> Self {
        self.merged_buses = merged_buses;
        self
    }

    #[must_use]
    pub const fn with_low_impedance_threshold(mut self, threshold: f64) -> Self {
        self.low_impedance_threshold = threshold;
        self
    }
}

/// The balance at one bus.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct StatedBusMismatch {
    pub bus: BusId,
    /// Index into [`StatedStateMismatch::islands`]. `None` for a bus the case
    /// types as isolated, which has no balance equation.
    pub island: Option<usize>,
    /// Active mismatch, MW: drawn by the network minus stated injection.
    pub p_mw: f64,
    /// Reactive mismatch, MVAr, same sign.
    pub q_mvar: f64,
    pub flags: StatedBusFlags,
}

impl StatedBusMismatch {
    /// `|ΔP + jΔQ|`, MVA.
    #[must_use]
    pub fn calc_magnitude_mva(&self) -> f64 {
        self.p_mw.hypot(self.q_mvar)
    }
}

/// The balance summed over one island of the in service topology.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct StatedIslandMismatch {
    pub n_buses: usize,
    /// Signed sums, MW and MVAr.
    pub p_mw: f64,
    pub q_mvar: f64,
    /// Sums of magnitudes, MW and MVAr.
    pub abs_p_mw: f64,
    pub abs_q_mvar: f64,
    /// The bus with the largest `|ΔS|` in the island.
    pub largest_bus: BusId,
}

/// An injection that closes one bus's stated-state mismatch.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ClosureInjection {
    pub bus: BusId,
    /// Active injection, MW.
    pub p_mw: f64,
    /// Reactive injection, MVAr.
    pub q_mvar: f64,
    pub flags: StatedBusFlags,
}

/// The AC bus balance at the stored voltages, from
/// [`calc_stated_state_mismatch`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StatedStateMismatch {
    /// One row per bus of the analysis network: the source buses in table
    /// order, then one star bus per in service three winding transformer.
    pub buses: Vec<StatedBusMismatch>,
    /// The islands of the in service topology, largest first. Isolated buses
    /// belong to none.
    pub islands: Vec<StatedIslandMismatch>,
    /// Indices into [`buses`](Self::buses) of the largest `|ΔS|`, largest
    /// first, at most [`StatedStateOptions::top_k`] of them.
    pub top: Vec<usize>,
}

impl StatedStateMismatch {
    /// `Σ|ΔP|` over every bus in an island, MW.
    #[must_use]
    pub fn calc_total_abs_p_mw(&self) -> f64 {
        self.islands.iter().map(|island| island.abs_p_mw).sum()
    }

    /// `Σ|ΔQ|` over every bus in an island, MVAr.
    #[must_use]
    pub fn calc_total_abs_q_mvar(&self) -> f64 {
        self.islands.iter().map(|island| island.abs_q_mvar).sum()
    }

    /// The largest `|ΔS|` of any bus in an island, MVA.
    #[must_use]
    pub fn calc_max_magnitude_mva(&self) -> f64 {
        self.top
            .first()
            .map_or(0.0, |&row| self.buses[row].calc_magnitude_mva())
    }

    /// The fixed injections that close the mismatch at every bus in an island
    /// that carries any flag in `mask`: each is the bus's own `ΔP` and `ΔQ`,
    /// so adding it to the bus's stated injection makes that bus balance at
    /// the stored voltages exactly. A calculation that cannot model a device
    /// (an HVDC converter's reactive demand, a FACTS controller) can hold it
    /// at this injection and start from the stored solution.
    #[must_use]
    pub fn closure_injections(&self, mask: StatedBusFlags) -> Vec<ClosureInjection> {
        self.buses
            .iter()
            .filter(|bus| bus.island.is_some() && bus.flags.intersects(mask))
            .map(|bus| ClosureInjection {
                bus: bus.bus,
                p_mw: bus.p_mw,
                q_mvar: bus.q_mvar,
                flags: bus.flags,
            })
            .collect()
    }
}

/// Active and reactive power entering every analysis branch at both ends, at
/// the stored voltages, from [`calc_stated_branch_flows`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StatedBranchFlows {
    /// The source of each analysis branch: every source branch row in order,
    /// then the three windings of each in service three winding transformer.
    pub sources: Vec<AnalysisBranchSource>,
    /// Whether the branch is in service. An out of service branch carries
    /// zero flow.
    pub in_service: Vec<bool>,
    /// Power entering the branch at its from end, MW and MVAr.
    pub p_from_mw: Vec<f64>,
    pub q_from_mvar: Vec<f64>,
    /// Power entering the branch at its to end, MW and MVAr.
    pub p_to_mw: Vec<f64>,
    pub q_to_mvar: Vec<f64>,
}

/// The complex bus voltages a case stores, per unit and radians, in analysis
/// bus order.
fn stored_voltages(view: &IndexedNetwork<'_>) -> Vec<Complex64> {
    view.network()
        .buses()
        .iter()
        .map(|bus| Complex64::from_polar(bus.vm, view.to_radians(bus.va)))
        .collect()
}

/// A description of an analysis branch for an error message.
fn describe_branch(source: AnalysisBranchSource) -> String {
    match source {
        AnalysisBranchSource::Branch { row } => format!("branch row {row}"),
        AnalysisBranchSource::ThreeWindingTransformerWinding {
            transformer_row,
            winding,
        } => format!(
            "winding {} of three winding transformer row {transformer_row}",
            winding + 1
        ),
    }
}

/// One in service analysis branch at the stored voltages: its terminal bus
/// indices and the per unit complex power entering it at each end.
struct TerminalPowers {
    from: usize,
    to: usize,
    s_from: Complex64,
    s_to: Complex64,
}

/// [`TerminalPowers`] of each analysis branch, `None` for an out of service
/// branch, in analysis branch order.
fn branch_terminal_powers(
    view: &IndexedNetwork<'_>,
    source: &BalancedNetwork,
    voltages: &[Complex64],
) -> Result<Vec<Option<TerminalPowers>>> {
    let sources = analysis_branch_sources(source);
    let mut powers = Vec::with_capacity(view.branches().len());
    for (row, branch) in view.branches().iter().enumerate() {
        if !branch.in_service {
            powers.push(None);
            continue;
        }
        let from = view
            .bus_index(branch.from)
            .ok_or(powerio_tx::Error::UnknownBus {
                bus_id: branch.from,
                element_index: row,
            })?;
        let to = view
            .bus_index(branch.to)
            .ok_or(powerio_tx::Error::UnknownBus {
                bus_id: branch.to,
                element_index: row,
            })?;
        let shift = view.to_radians(branch.shift);
        let admittance = match branch_admittance(branch, YbusFlags::default(), shift, row) {
            Ok(Some(admittance)) => admittance,
            Ok(None) | Err(Error::Transmission(powerio_tx::Error::ZeroImpedance { .. })) => {
                return Err(Error::UnmergedZeroImpedance {
                    element: describe_branch(sources[row]),
                    from: branch.from,
                    to: branch.to,
                });
            }
            Err(other) => return Err(other),
        };
        let (s_from, s_to) = branch_flows(&admittance, voltages[from], voltages[to]);
        powers.push(Some(TerminalPowers {
            from,
            to,
            s_from,
            s_to,
        }));
    }
    Ok(powers)
}

/// The power a load draws at voltage magnitude `vm` (per unit), in the
/// load's own unit.
fn load_power_at(load: &powerio_tx::Load, vm: f64, base_kv: f64) -> Complex64 {
    match &load.voltage_model {
        Some(LoadVoltageModel::Zip {
            p_constant_power,
            q_constant_power,
            p_constant_current,
            q_constant_current,
            p_constant_impedance,
            q_constant_impedance,
            ..
        }) => Complex64::new(
            p_constant_power + p_constant_current * vm + p_constant_impedance * vm * vm,
            q_constant_power + q_constant_current * vm + q_constant_impedance * vm * vm,
        ),
        Some(LoadVoltageModel::Exponential {
            p,
            q,
            v_nom,
            gamma_p,
            gamma_q,
        }) => {
            // A nominal voltage is stated in kV; without one the ratio is
            // the per unit magnitude itself.
            let ratio = match v_nom {
                Some(v_nom) if *v_nom > 0.0 && base_kv > 0.0 => vm * base_kv / v_nom,
                _ => vm,
            };
            Complex64::new(p * ratio.powf(*gamma_p), q * ratio.powf(*gamma_q))
        }
        _ => Complex64::new(load.p, load.q),
    }
}

fn is_voltage_dependent(load: &powerio_tx::Load) -> bool {
    match &load.voltage_model {
        Some(LoadVoltageModel::Zip {
            p_constant_current,
            q_constant_current,
            p_constant_impedance,
            q_constant_impedance,
            ..
        }) => [
            p_constant_current,
            q_constant_current,
            p_constant_impedance,
            q_constant_impedance,
        ]
        .iter()
        .any(|part| **part != 0.0),
        Some(LoadVoltageModel::Exponential { .. }) => true,
        _ => false,
    }
}

/// Calculate the AC bus balance at the voltages `net` stores, with every
/// device at its stated output.
///
/// A case saved from a converged power flow stores bus voltage magnitudes
/// and angles that balance the network's injections. Evaluating every bus
/// balance at those stored voltages measures how faithfully the case was
/// read: a bus whose equipment PowerIO reads the way the solving program
/// read it closes to the precision of the stored values, and a residual
/// points at the equipment read differently.
///
/// A bus mismatch is the power the network draws from the bus at the stored
/// voltages minus the power the bus's devices state they inject,
/// `S_calc - S_stated`, the mismatch MATPOWER's `newtonpf` drives to zero. An
/// extra injection equal to the mismatch closes the bus
/// ([`StatedStateMismatch::closure_injections`]).
///
/// The network side is every in service branch and three winding
/// transformer winding at its stated tap and shift, plus every in service
/// fixed and switched shunt at its stated admittance, through the branch
/// admittances [`calc_admittance_matrix`](crate::calc_admittance_matrix)
/// uses. The device side is every in service generator at its stated `pg`
/// and `qg`, every in service load at the stored voltage through its voltage
/// model, every in service static VAR compensator and storage unit at its
/// stated terminal power, and every HVDC line under
/// [`StatedStateOptions::hvdc_treatment`].
///
/// # Errors
/// [`Error::UnmergedZeroImpedance`] for an in service branch or winding with
/// zero series impedance: the stored voltages do not determine its flow, so
/// merge the buses it joins first
/// (`powerio_prob::merge_zero_impedance_buses`). An element naming an
/// undeclared bus, a non-finite admittance, or an unusable tap or base fail
/// as the admittance builders do.
#[allow(clippy::too_many_lines)] // one pass per element table, stated in full
pub fn calc_stated_state_mismatch(
    net: &BalancedNetwork,
    opts: &StatedStateOptions,
) -> Result<StatedStateMismatch> {
    net.check_base_mva()?;
    let view = IndexedNetwork::new(net);
    let n = view.n();
    let base = view.per_unit_base();
    let base_mva = view.base_mva();
    let voltages = stored_voltages(&view);

    // The network side: power drawn into branches and shunts, per unit.
    let mut drawn = vec![Complex64::new(0.0, 0.0); n];
    let mut flags = vec![StatedBusFlags::NONE; n];
    let branch_powers = branch_terminal_powers(&view, net, &voltages)?;
    for (branch, powers) in view.branches().iter().zip(&branch_powers) {
        let Some(powers) = powers else {
            continue;
        };
        drawn[powers.from] += powers.s_from;
        drawn[powers.to] += powers.s_to;
        if branch.r.hypot(branch.x) < opts.low_impedance_threshold {
            flags[powers.from] |= StatedBusFlags::LOW_IMPEDANCE_BRANCH;
            flags[powers.to] |= StatedBusFlags::LOW_IMPEDANCE_BRANCH;
        }
    }
    for (idx, voltage) in voltages.iter().enumerate() {
        let vm2 = voltage.norm_sqr();
        drawn[idx] += Complex64::new(view.gs()[idx], -view.bs()[idx]) * vm2 / base;
    }

    // The device side: stated injections, per unit.
    let mut stated = vec![Complex64::new(0.0, 0.0); n];
    let buses = view.network().buses();
    for (idx, bus) in buses.iter().enumerate() {
        if bus.kind == BusType::Ref {
            flags[idx] |= StatedBusFlags::REFERENCE;
        }
        if idx >= net.buses().len() {
            flags[idx] |= StatedBusFlags::STAR_BUS;
        }
    }
    for survivor in opts.merged_buses.values() {
        if let Some(idx) = view.bus_index(*survivor) {
            flags[idx] |= StatedBusFlags::MERGED_GROUP;
        }
    }
    for generator in view.generators().iter().filter(|g| g.in_service) {
        if let Some(idx) = view.bus_index(generator.bus) {
            stated[idx] += Complex64::new(generator.pg, generator.qg) / base;
            if buses[idx].kind == BusType::Pq {
                flags[idx] |= StatedBusFlags::GENERATOR_ON_PQ_BUS;
            }
        }
    }
    for load in view.network().loads().iter().filter(|l| l.in_service) {
        if let Some(idx) = view.bus_index(load.bus) {
            let vm = voltages[idx].norm();
            stated[idx] -= load_power_at(load, vm, buses[idx].base_kv) / base;
            if is_voltage_dependent(load) {
                flags[idx] |= StatedBusFlags::VOLTAGE_DEPENDENT_LOAD;
            }
        }
    }
    for shunt in view.network().shunts().iter().filter(|s| s.in_service) {
        if shunt.control.is_some()
            && let Some(idx) = view.bus_index(shunt.bus)
        {
            flags[idx] |= StatedBusFlags::SWITCHED_SHUNT;
        }
    }
    // Terminal power in the load sign the static VAR compensator readers
    // state: positive flows from the bus into the device.
    for svc in view
        .network()
        .static_var_compensators()
        .iter()
        .filter(|s| s.in_service)
    {
        if let Some(idx) = view.bus_index(svc.bus) {
            stated[idx] -= Complex64::new(svc.p, svc.q) / base;
            flags[idx] |= StatedBusFlags::FACTS;
        }
    }
    // Storage power is a withdrawal, PowerModels' `ps`/`qs`.
    for storage in view.network().storage().iter().filter(|s| s.in_service) {
        if let Some(idx) = view.bus_index(storage.bus) {
            stated[idx] -= Complex64::new(storage.ps, storage.qs) / base;
            flags[idx] |= StatedBusFlags::STORAGE;
        }
    }
    if opts.hvdc_treatment == HvdcTreatment::FixedInjection {
        for ((injection, p), q) in stated.iter_mut().zip(view.p_hvdc()).zip(view.q_hvdc()) {
            *injection += Complex64::new(*p, *q) / base;
        }
    }
    for line in view.network().hvdc().iter().filter(|line| line.in_service) {
        let vsc = [&line.converter1, &line.converter2]
            .iter()
            .any(|c| c.as_ref().is_some_and(|c| c.kind == HvdcConverterKind::Vsc));
        for bus in [line.from, line.to] {
            if let Some(idx) = view.bus_index(bus) {
                flags[idx] |= StatedBusFlags::HVDC;
                if vsc {
                    flags[idx] |= StatedBusFlags::VSC;
                }
            }
        }
    }

    // Islands, largest first, over the energized buses.
    let labels = view.calc_island_labels();
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (idx, bus) in buses.iter().enumerate() {
        if bus.kind != BusType::Isolated {
            members.entry(labels[idx]).or_default().push(idx);
        }
    }
    let mut groups: Vec<Vec<usize>> = members.into_values().collect();
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    let mut island_of = vec![None; n];
    for (island, group) in groups.iter().enumerate() {
        for &idx in group {
            island_of[idx] = Some(island);
        }
    }

    let rows: Vec<StatedBusMismatch> = (0..n)
        .map(|idx| {
            let mismatch = (drawn[idx] - stated[idx]) * base_mva;
            StatedBusMismatch {
                bus: buses[idx].id,
                island: island_of[idx],
                p_mw: mismatch.re,
                q_mvar: mismatch.im,
                flags: flags[idx],
            }
        })
        .collect();

    let islands = groups
        .iter()
        .map(|group| {
            let mut island = StatedIslandMismatch {
                n_buses: group.len(),
                p_mw: 0.0,
                q_mvar: 0.0,
                abs_p_mw: 0.0,
                abs_q_mvar: 0.0,
                largest_bus: rows[group[0]].bus,
            };
            let mut largest = -1.0;
            for &idx in group {
                let row = &rows[idx];
                island.p_mw += row.p_mw;
                island.q_mvar += row.q_mvar;
                island.abs_p_mw += row.p_mw.abs();
                island.abs_q_mvar += row.q_mvar.abs();
                let magnitude = row.calc_magnitude_mva();
                if magnitude > largest {
                    largest = magnitude;
                    island.largest_bus = row.bus;
                }
            }
            island
        })
        .collect();

    let mut top: Vec<usize> = (0..n).filter(|&idx| island_of[idx].is_some()).collect();
    top.sort_by(|&a, &b| {
        rows[b]
            .calc_magnitude_mva()
            .total_cmp(&rows[a].calc_magnitude_mva())
            .then(a.cmp(&b))
    });
    top.truncate(opts.top_k);

    Ok(StatedStateMismatch {
        buses: rows,
        islands,
        top,
    })
}

/// Calculate the active and reactive power entering every analysis branch
/// at both ends, at the voltages `net` stores.
///
/// # Errors
/// As [`calc_stated_state_mismatch`].
pub fn calc_stated_branch_flows(net: &BalancedNetwork) -> Result<StatedBranchFlows> {
    net.check_base_mva()?;
    let view = IndexedNetwork::new(net);
    let base_mva = view.base_mva();
    let voltages = stored_voltages(&view);
    let powers = branch_terminal_powers(&view, net, &voltages)?;
    let m = powers.len();
    let mut flows = StatedBranchFlows {
        sources: analysis_branch_sources(net),
        in_service: Vec::with_capacity(m),
        p_from_mw: Vec::with_capacity(m),
        q_from_mvar: Vec::with_capacity(m),
        p_to_mw: Vec::with_capacity(m),
        q_to_mvar: Vec::with_capacity(m),
    };
    for power in powers {
        let (s_from, s_to) = power
            .as_ref()
            .map_or_else(Default::default, |p| (p.s_from, p.s_to));
        flows.in_service.push(power.is_some());
        flows.p_from_mw.push(s_from.re * base_mva);
        flows.q_from_mvar.push(s_from.im * base_mva);
        flows.p_to_mw.push(s_to.re * base_mva);
        flows.q_to_mvar.push(s_to.im * base_mva);
    }
    Ok(flows)
}

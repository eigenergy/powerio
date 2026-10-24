//! The AC optimal transmission switching instance.
//!
//! An [`AcOtsInstance`] is an AC optimal power flow problem whose branch
//! statuses are decisions. It keeps every physical branch row of the network,
//! takes the network's `in_service` bits as the initial topology, and states
//! which rows may change status, what a change costs, how many changes and
//! open rows are allowed, whether islands may form, and which branch current
//! limits apply. It names no solver or formulation: a solver chooses the
//! final branch statuses, [`AcOtsInstance::fix_topology`] turns them into the
//! AC optimal power flow it then solves, and
//! [`AcOtsSolution`](crate::AcOtsSolution) records the result.

use std::collections::{BTreeSet, HashMap};

use powerio_core::{Diagnostic, Error};
use powerio_tx::{BalancedNetwork, BusId, BusType, correct_angle_difference_bounds};

use crate::diagnostics::codes;
use crate::instance::balanced::{AcOpfInstance, transform_assumption, transform_discarded};
use crate::instance::constraints::ActiveConstraints;
use crate::instance::objective::{Objective, ObjectiveTerm};
use crate::operating::{BalancedOperatingPointFlag, OperatingPoint, row_identity};

/// Which physical branches may change status, by stable branch identity (the
/// payload `uid`, else `branches:{row}`).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum BranchSelection {
    /// Every physical branch, including the initially open ones.
    #[default]
    All,
    /// No branch; the topology is fixed.
    None,
    /// Exactly the named branches. Each identity must name one branch, once.
    Only(Vec<String>),
}

impl BranchSelection {
    /// The stable spelling of the selection kind: `all`, `none`, or `only`.
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::None => "none",
            Self::Only(_) => "only",
        }
    }
}

/// The cost of changing one branch's status, in objective units. Only a
/// change against the initial topology costs anything: opening an initially
/// closed branch costs `open_cost`, closing an initially open one
/// `close_cost`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BranchSwitchingCost {
    /// The branch identity.
    pub id: String,
    /// The cost of opening the branch.
    pub open_cost: f64,
    /// The cost of closing the branch.
    pub close_cost: f64,
}

impl BranchSwitchingCost {
    #[must_use]
    pub fn new(id: impl Into<String>, open_cost: f64, close_cost: f64) -> Self {
        Self {
            id: id.into(),
            open_cost,
            close_cost,
        }
    }
}

/// Whether the final topology may split the network into several AC islands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum IslandingPolicy {
    /// Every energized bus stays in one AC island.
    #[default]
    Forbidden,
    /// Islands may form. Each final island gets one reference bus and keeps
    /// every load; no load is shed, and a feasible dispatch is not implied.
    Allowed,
}

impl IslandingPolicy {
    /// The stable spelling: `forbidden` or `allowed`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::Allowed => "allowed",
        }
    }
}

/// Which branch current limits the instance carries. Current limits are
/// separate from the MVA thermal limits the base AC OPF selects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum CurrentLimitPolicy {
    /// No current limits.
    #[default]
    Skip,
    /// The explicit [`BranchCurrentLimit`] records.
    Stated,
    /// The limits PowerModels' `calc_current_limits!` derives from the
    /// active voltage, angle difference, and thermal limits.
    DeriveFromRateA,
}

impl CurrentLimitPolicy {
    /// The stable spelling: `skip`, `stated`, or `derive_from_rate_a`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Stated => "stated",
            Self::DeriveFromRateA => "derive_from_rate_a",
        }
    }
}

/// An explicit current magnitude limit at a branch's two terminals, per unit
/// on the system MVA base and each terminal bus's nominal voltage. `None` is
/// unbounded, not zero. Source current ratings are never read as these.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BranchCurrentLimit {
    /// The branch identity.
    pub id: String,
    /// The from terminal limit, per unit.
    pub from_pu: Option<f64>,
    /// The to terminal limit, per unit.
    pub to_pu: Option<f64>,
}

impl BranchCurrentLimit {
    #[must_use]
    pub fn new(id: impl Into<String>, from_pu: Option<f64>, to_pu: Option<f64>) -> Self {
        Self {
            id: id.into(),
            from_pu,
            to_pu,
        }
    }
}

/// The switching options of an [`AcOtsInstance`].
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct AcOtsOptions {
    /// The branches that may change status.
    pub switchable: BranchSelection,
    /// Transition costs; a branch with no record switches at zero cost.
    pub switching_costs: Vec<BranchSwitchingCost>,
    /// Whether the final topology may form islands.
    pub islanding: IslandingPolicy,
    /// The most branches open in the final topology, counting every open
    /// row, including fixed ones.
    pub max_open: Option<usize>,
    /// The most status changes against the initial topology.
    pub max_switched: Option<usize>,
    /// Which current limits apply.
    pub current_limits: CurrentLimitPolicy,
    /// The explicit current limits [`CurrentLimitPolicy::Stated`] uses.
    pub stated_current_limits: Vec<BranchCurrentLimit>,
}

/// The two terminal current bounds of one branch, per unit; `None` is
/// unbounded.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TerminalCurrentBounds {
    pub from_pu: Option<f64>,
    pub to_pu: Option<f64>,
}

impl TerminalCurrentBounds {
    fn is_bounded(self) -> bool {
        self.from_pu.is_some() || self.to_pu.is_some()
    }
}

/// The opening and closing cost of one branch.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SwitchingCost {
    pub open: f64,
    pub close: f64,
}

/// One physical branch as the switching instance sees it, in branch table
/// order.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct AcOtsBranch {
    /// The stable branch identity.
    pub id: String,
    /// Whether the branch is in service in the initial topology.
    pub initial_closed: bool,
    /// Whether the branch may change status.
    pub switchable: bool,
    /// The stated transition costs, if any.
    pub switching_cost: Option<SwitchingCost>,
    /// The explicit current limits stated for the branch.
    pub stated_current: TerminalCurrentBounds,
    /// The current limits that apply under the instance's policy.
    pub current: TerminalCurrentBounds,
}

/// The AC optimal transmission switching instance: an AC optimal power flow
/// base, whose network supplies the candidate branches and their initial
/// statuses, plus the switching options.
///
/// The base objective is kept exactly; switching costs enter the objective
/// only when it states [`ObjectiveTerm::BranchSwitchingCost`]
/// ([`with_switching_objective`](Self::with_switching_objective)).
#[derive(Clone, Debug)]
pub struct AcOtsInstance {
    base: AcOpfInstance,
    options: AcOtsOptions,
    branches: Vec<AcOtsBranch>,
}

impl AcOtsInstance {
    /// Combine an AC optimal power flow base with explicit switching options.
    /// The base objective is kept exactly.
    ///
    /// # Errors
    /// An option names an unknown branch or names one twice, states a
    /// negative or non-finite cost, a nonpositive or non-finite current
    /// limit, a switching budget above the switchable branch count, or an
    /// open budget below the number of fixed open branches.
    pub fn new(base: AcOpfInstance, options: AcOtsOptions) -> Result<Self, Error> {
        let branches = resolve_branches(&base, &options)?;
        Ok(Self {
            base,
            options,
            branches,
        })
    }

    /// The default switching instance over an AC optimal power flow base:
    /// every initially in-service branch may switch, at zero cost, with no
    /// budget, no islands, and no current limits, and the objective gains
    /// the switching cost term.
    ///
    /// # Errors
    /// As [`new`](Self::new).
    pub fn from_ac_opf(base: AcOpfInstance) -> Result<Self, Error> {
        let options = AcOtsOptions {
            switchable: initially_closed_selection(base.network()),
            ..AcOtsOptions::default()
        };
        Ok(Self::new(base, options)?.with_switching_objective())
    }

    /// The default switching instance over a network: the AC optimal power
    /// flow [`AcOpfInstance::from_network`] states, with the defaults of
    /// [`from_ac_opf`](Self::from_ac_opf).
    ///
    /// # Errors
    /// As [`AcOpfInstance::from_network`] and [`new`](Self::new).
    pub fn from_network(network: BalancedNetwork) -> Result<Self, Error> {
        Self::from_ac_opf(AcOpfInstance::from_network(network)?)
    }

    /// Replace the switching options, keeping the base problem and its
    /// objective.
    ///
    /// # Errors
    /// As [`new`](Self::new).
    pub fn with_options(self, options: AcOtsOptions) -> Result<Self, Error> {
        Self::new(self.base, options)
    }

    /// Add the switching cost objective term, once.
    #[must_use]
    pub fn with_switching_objective(mut self) -> Self {
        if !self
            .base
            .objective()
            .contains(&ObjectiveTerm::BranchSwitchingCost)
        {
            self.base = self
                .base
                .with_objective_term(ObjectiveTerm::BranchSwitchingCost);
        }
        self
    }

    /// The AC optimal power flow base: the network with its initial
    /// topology, the objective, the constraint selections, and the initial
    /// point.
    #[must_use]
    pub const fn base(&self) -> &AcOpfInstance {
        &self.base
    }

    /// The network with every candidate branch at its initial status.
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        self.base.network()
    }

    /// The typed objective.
    #[must_use]
    pub const fn objective(&self) -> &Objective {
        self.base.objective()
    }

    /// The active constraint selections.
    #[must_use]
    pub const fn constraints(&self) -> &ActiveConstraints {
        self.base.constraints()
    }

    /// The optional solver initial point.
    #[must_use]
    pub const fn initial_point(&self) -> Option<&OperatingPoint<BalancedNetwork>> {
        self.base.initial_point()
    }

    /// The switching options as stated.
    #[must_use]
    pub const fn options(&self) -> &AcOtsOptions {
        &self.options
    }

    /// Every physical branch in table order, with its resolved switching
    /// settings.
    #[must_use]
    pub fn branches(&self) -> &[AcOtsBranch] {
        &self.branches
    }

    /// The branch identities in table order.
    pub fn branch_order(&self) -> impl ExactSizeIterator<Item = &str> {
        self.branches.iter().map(|branch| branch.id.as_str())
    }

    /// The initial branch statuses in table order.
    #[must_use]
    pub fn initial_closed(&self) -> Vec<bool> {
        self.branches
            .iter()
            .map(|branch| branch.initial_closed)
            .collect()
    }

    /// Check a complete final branch status vector, in branch table order,
    /// against the fixed branches, the budgets, and the islanding policy, and
    /// return its switching cost. This does not certify AC feasibility.
    ///
    /// # Errors
    /// A vector of the wrong length, a changed fixed branch, a budget
    /// exceeded, or, when islands are forbidden, a final topology that splits
    /// the energized buses.
    pub fn validate_topology(&self, branch_closed: &[bool]) -> Result<f64, Error> {
        if branch_closed.len() != self.branches.len() {
            return Err(topology_invalid(format!(
                "the branch status vector carries {} values; the network has {} branches",
                branch_closed.len(),
                self.branches.len()
            )));
        }
        let mut open = 0;
        let mut switched = 0;
        let mut cost = 0.0;
        for (branch, &closed) in self.branches.iter().zip(branch_closed) {
            if !closed {
                open += 1;
            }
            if closed == branch.initial_closed {
                continue;
            }
            if !branch.switchable {
                return Err(topology_invalid(format!(
                    "branch `{}` cannot switch, but the final topology {} it",
                    branch.id,
                    if closed { "closes" } else { "opens" }
                )));
            }
            switched += 1;
            if let Some(costs) = branch.switching_cost {
                cost += if closed { costs.close } else { costs.open };
            }
        }
        if let Some(max_open) = self.options.max_open
            && open > max_open
        {
            return Err(topology_invalid(format!(
                "the final topology opens {open} branches; at most {max_open} may be open"
            )));
        }
        if let Some(max_switched) = self.options.max_switched
            && switched > max_switched
        {
            return Err(topology_invalid(format!(
                "the final topology changes {switched} branch statuses; at most {max_switched} may change"
            )));
        }
        if self.options.islanding == IslandingPolicy::Forbidden {
            let islands = self.network_with(branch_closed).calc_islands().islands;
            if islands.len() > 1 {
                return Err(topology_invalid(format!(
                    "the final topology splits the energized buses into {} islands, and islanding is forbidden",
                    islands.len()
                )));
            }
        }
        Ok(cost)
    }

    /// The AC optimal power flow over a validated final topology. Every
    /// branch row stays in the network with its final status. When islands
    /// are allowed, each final island gets exactly one reference bus. The
    /// switching options, the current limits, and the switching cost term
    /// are not part of the derived problem, and each discard is reported.
    /// An initial point that states branch statuses describes the initial
    /// topology, so it is discarded too.
    ///
    /// # Errors
    /// As [`validate_topology`](Self::validate_topology).
    pub fn fix_topology(
        &self,
        branch_closed: &[bool],
    ) -> Result<(AcOpfInstance, Vec<Diagnostic>), Error> {
        self.validate_topology(branch_closed)?;
        let mut network = self.network_with(branch_closed);
        let mut diagnostics = vec![transform_discarded(
            "the switchable branch selection, switching costs, switching budgets, and islanding policy",
        )];
        if self.options.islanding == IslandingPolicy::Allowed {
            diagnostics.extend(assign_island_references(&mut network));
        }
        if self
            .branches
            .iter()
            .any(|branch| branch.current.is_bounded())
        {
            diagnostics.push(transform_discarded("the branch current limits"));
        }
        let mut objective = self.base.objective().clone();
        if objective.contains(&ObjectiveTerm::BranchSwitchingCost) {
            objective = objective.without(&ObjectiveTerm::BranchSwitchingCost);
            diagnostics.push(transform_discarded(
                "the branch switching cost objective term",
            ));
        }
        let mut fixed = AcOpfInstance::from_network(network.clone())?
            .with_objective(objective)
            .with_constraints(self.base.constraints().clone());
        if let Some(point) = self.base.initial_point() {
            if point
                .flags(BalancedOperatingPointFlag::BranchInService)
                .is_some()
            {
                diagnostics.push(transform_discarded(
                    "the initial point, which states the initial branch statuses,",
                ));
            } else {
                fixed = fixed.with_initial_point(point.clone().rebind_network(network)?);
            }
        }
        Ok((fixed, diagnostics))
    }

    /// The network with `branch_closed` as the branch statuses. The caller
    /// checked the length.
    fn network_with(&self, branch_closed: &[bool]) -> BalancedNetwork {
        let mut network = self.base.network().clone();
        let changed = network
            .branches()
            .iter()
            .zip(branch_closed)
            .any(|(branch, &closed)| branch.in_service != closed);
        if changed {
            for (branch, &closed) in network.branches_mut().iter_mut().zip(branch_closed) {
                branch.in_service = closed;
            }
        }
        network
    }
}

/// `Only` the branches the network states in service.
fn initially_closed_selection(network: &BalancedNetwork) -> BranchSelection {
    BranchSelection::Only(
        network
            .branches()
            .iter()
            .enumerate()
            .filter(|(_, branch)| branch.in_service)
            .map(|(row, branch)| row_identity(branch.uid.as_deref(), "branches", row))
            .collect(),
    )
}

fn options_invalid(message: String) -> Error {
    Error::new(&codes::BUILD_INSTANCE_SWITCHING_OPTIONS_INVALID, message)
}

fn topology_invalid(message: String) -> Error {
    Error::new(&codes::BUILD_INSTANCE_TOPOLOGY_INVALID, message)
}

/// Resolve the options to one row per physical branch, checking every
/// identity, value, and budget.
fn resolve_branches(
    base: &AcOpfInstance,
    options: &AcOtsOptions,
) -> Result<Vec<AcOtsBranch>, Error> {
    let network = base.network();
    let mut branches: Vec<AcOtsBranch> = network
        .branches()
        .iter()
        .enumerate()
        .map(|(row, branch)| AcOtsBranch {
            id: row_identity(branch.uid.as_deref(), "branches", row),
            initial_closed: branch.in_service,
            switchable: matches!(options.switchable, BranchSelection::All),
            switching_cost: None,
            stated_current: TerminalCurrentBounds::default(),
            current: TerminalCurrentBounds::default(),
        })
        .collect();
    let mut position = HashMap::with_capacity(branches.len());
    for (row, branch) in branches.iter().enumerate() {
        if position.insert(branch.id.clone(), row).is_some() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                format!("two branches share the identity `{}`", branch.id),
            ));
        }
    }
    let resolve = |what: &str, id: &str, seen: &mut BTreeSet<usize>| -> Result<usize, Error> {
        let row = *position
            .get(id)
            .ok_or_else(|| options_invalid(format!("{what} names unknown branch `{id}`")))?;
        if !seen.insert(row) {
            return Err(options_invalid(format!(
                "{what} names branch `{id}` more than once"
            )));
        }
        Ok(row)
    };

    if let BranchSelection::Only(identities) = &options.switchable {
        let mut seen = BTreeSet::new();
        for id in identities {
            let row = resolve("the switchable selection", id, &mut seen)?;
            branches[row].switchable = true;
        }
    }

    let mut seen = BTreeSet::new();
    for cost in &options.switching_costs {
        let row = resolve("the switching costs", &cost.id, &mut seen)?;
        for (side, value) in [("opening", cost.open_cost), ("closing", cost.close_cost)] {
            if !value.is_finite() || value < 0.0 {
                return Err(options_invalid(format!(
                    "branch `{}` has {side} cost {value}; costs must be finite and nonnegative",
                    cost.id
                )));
            }
        }
        branches[row].switching_cost = Some(SwitchingCost {
            open: cost.open_cost,
            close: cost.close_cost,
        });
    }

    let mut seen = BTreeSet::new();
    for limit in &options.stated_current_limits {
        let row = resolve("the stated current limits", &limit.id, &mut seen)?;
        for (side, value) in [("from", limit.from_pu), ("to", limit.to_pu)] {
            if let Some(value) = value
                && (!value.is_finite() || value <= 0.0)
            {
                return Err(options_invalid(format!(
                    "branch `{}` has {side} terminal current limit {value}; a limit must be finite and positive",
                    limit.id
                )));
            }
        }
        branches[row].stated_current = TerminalCurrentBounds {
            from_pu: limit.from_pu,
            to_pu: limit.to_pu,
        };
    }

    check_budgets(&branches, options)?;
    apply_current_policy(base, options.current_limits, &mut branches);
    Ok(branches)
}

/// The budgets must leave at least one admissible topology and stay within
/// the branch table.
fn check_budgets(branches: &[AcOtsBranch], options: &AcOtsOptions) -> Result<(), Error> {
    let switchable = branches.iter().filter(|branch| branch.switchable).count();
    let fixed_open = branches
        .iter()
        .filter(|branch| !branch.switchable && !branch.initial_closed)
        .count();
    if let Some(max_switched) = options.max_switched
        && max_switched > switchable
    {
        return Err(options_invalid(format!(
            "the switching budget {max_switched} exceeds the {switchable} switchable branches"
        )));
    }
    if let Some(max_open) = options.max_open {
        if max_open > branches.len() {
            return Err(options_invalid(format!(
                "the open budget {max_open} exceeds the {} branches",
                branches.len()
            )));
        }
        if max_open < fixed_open {
            return Err(options_invalid(format!(
                "the open budget {max_open} is below the {fixed_open} branches that are open and cannot switch"
            )));
        }
    }
    Ok(())
}

/// Set each branch's applicable current limits under `policy`.
fn apply_current_policy(
    base: &AcOpfInstance,
    policy: CurrentLimitPolicy,
    branches: &mut [AcOtsBranch],
) {
    match policy {
        CurrentLimitPolicy::Skip => {}
        CurrentLimitPolicy::Stated => {
            for branch in branches.iter_mut() {
                branch.current = branch.stated_current;
            }
        }
        CurrentLimitPolicy::DeriveFromRateA => {
            let derived = derive_current_limits(base);
            for (branch, limit) in branches.iter_mut().zip(derived) {
                branch.current = TerminalCurrentBounds {
                    from_pu: limit,
                    to_pu: limit,
                };
            }
        }
    }
}

/// The per unit current limit of every branch, following PowerModels'
/// `calc_current_limits!` (`src/core/data.jl`):
///
/// ```text
/// c = |1/(r + jx)| · sqrt(vmax_f² + vmax_t² − 2·vmax_f·vmax_t·cos θmax)
/// c = min(c, rate_a / min(vmin_f, vmin_t))      when rate_a > 0
/// ```
///
/// `θmax` is the larger magnitude of the branch's angle difference bounds
/// after PowerModels' parse time correction. Both terminals take the same
/// limit. A term enters only when the constraints it rests on are active:
/// the impedance term needs both terminals' voltage bounds (and uses the
/// angle bounds when they are active, else `θmax = π`), and the rating term
/// needs the branch's thermal limit and both terminals' voltage bounds. A
/// branch with zero series impedance has no impedance term, where
/// PowerModels' pseudo-inverse would state a zero limit.
fn derive_current_limits(base: &AcOpfInstance) -> Vec<Option<f64>> {
    let network = base.network();
    let constraints = base.constraints();
    let normalized = network.is_normalized();
    let base_mva = network.base_mva();
    let buses: HashMap<BusId, (f64, f64, bool)> = network
        .buses()
        .iter()
        .map(|bus| {
            let active = constraints.voltage_bounds.selects(&bus.id.to_string());
            (bus.id, (bus.vmin, bus.vmax, active))
        })
        .collect();
    network
        .branches()
        .iter()
        .enumerate()
        .map(|(row, branch)| {
            let identity = row_identity(branch.uid.as_deref(), "branches", row);
            let (&(vmin_f, vmax_f, active_f), &(vmin_t, vmax_t, active_t)) =
                (buses.get(&branch.from)?, buses.get(&branch.to)?);
            if !(active_f && active_t) {
                return None;
            }
            let mut limit: Option<f64> = None;
            let impedance = branch.r.hypot(branch.x);
            if impedance > 0.0 {
                let theta_max = if constraints.angle_bounds.selects(&identity) {
                    let (angmin, angmax) = if normalized {
                        (branch.angmin, branch.angmax)
                    } else {
                        (branch.angmin.to_radians(), branch.angmax.to_radians())
                    };
                    let (angmin, angmax) = correct_angle_difference_bounds(angmin, angmax);
                    angmin.abs().max(angmax.abs())
                } else {
                    std::f64::consts::PI
                };
                // PowerModels' arithmetic, term for term.
                let spread =
                    vmax_f * vmax_f + vmax_t * vmax_t - 2.0 * vmax_f * vmax_t * theta_max.cos();
                limit = Some(spread.max(0.0).sqrt() / impedance);
            }
            if branch.rate_a > 0.0 && constraints.thermal_limits.selects(&identity) {
                let rate = if normalized {
                    branch.rate_a
                } else {
                    branch.rate_a / base_mva
                };
                let rated = rate / vmin_f.min(vmin_t);
                limit = Some(limit.map_or(rated, |limit| limit.min(rated)));
            }
            limit.filter(|limit| limit.is_finite())
        })
        .collect()
}

/// Give every island of `network` exactly one reference bus, the choice
/// [`powerio_tx::Island::calc_reference_choice`] makes, else the island's
/// smallest bus. Nothing is de-energized: an island with no generator keeps
/// its loads. Demoted references become PV when they host an in-service
/// generator, else PQ.
fn assign_island_references(network: &mut BalancedNetwork) -> Vec<Diagnostic> {
    let partition = network.calc_islands();
    let hosts_generator: BTreeSet<BusId> = network
        .generators()
        .iter()
        .filter(|generator| generator.in_service)
        .map(|generator| generator.bus)
        .collect();
    let mut kinds: HashMap<BusId, BusType> = HashMap::new();
    let mut diagnostics = Vec::new();
    for island in &partition.islands {
        let chosen = island
            .calc_reference_choice(network.generators())
            .unwrap_or(island.buses[0]);
        if island.references.as_slice() == [chosen] {
            continue;
        }
        kinds.insert(chosen, BusType::Ref);
        for &bus in island.references.iter().filter(|&&bus| bus != chosen) {
            let kind = if hosts_generator.contains(&bus) {
                BusType::Pv
            } else {
                BusType::Pq
            };
            kinds.insert(bus, kind);
        }
        diagnostics.push(transform_assumption(&format!(
            "the final island of {} bus(es) takes bus {chosen} as its one reference bus",
            island.buses.len()
        )));
    }
    if !kinds.is_empty() {
        for bus in network.buses_mut() {
            if let Some(&kind) = kinds.get(&bus.id) {
                bus.kind = kind;
            }
        }
    }
    diagnostics
}

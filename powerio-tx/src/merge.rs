//! One explicit bus merge: closed switches and zero impedance branches.
//!
//! [`BalancedNetwork::merge_buses`] joins every set of buses connected by the
//! elements a [`BusMergeRule`] selects into one surviving bus, rewrites every
//! element reference onto it, and removes the joining elements. The result is
//! a [`BusMerge`]: the merged network, the map from each merged bus to its
//! survivor, every removed element with its source row and ratings, the
//! branch and switch row maps, and diagnostics for what the merge changed.
//!
//! The rule is always the caller's: [`ZeroImpedanceRule::Exact`] merges only
//! branches with `r == 0` and `x == 0`, and a threshold rule names its
//! threshold. Nothing here applies a tolerance the caller did not state.
//!
//! The merged network no longer has the removed elements' flows as
//! variables. [`BusMerge::calc_removed_flows`] recovers their active power
//! from a solution of the merged network.

use std::collections::{BTreeMap, HashMap};

use crate::diagnostics::{Diagnostic, codes};
use crate::network::{BalancedNetwork, BusId, BusType};
use crate::{Error, Result};

/// The zero impedance threshold PSS/E applies when a case states none
/// (`THRSHZ`, per unit). [`BusMergeRule::psse`] does not fall back to it; a
/// caller that wants it for a case that states no threshold names it.
pub const PSSE_DEFAULT_ZERO_IMPEDANCE_THRESHOLD: f64 = 0.0001;

/// Which branches a [`BusMergeRule`] treats as zero impedance connections.
///
/// Every rule considers in-service branches only: an out-of-service branch
/// does not join its buses.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ZeroImpedanceRule {
    /// `r == 0` and `x == 0` exactly. A transformer qualifies only at
    /// nominal ratio with no phase shift, since an ideal off-nominal
    /// transformer is a voltage ratio rather than a connection.
    Exact,
    /// PSS/E zero impedance line semantics: a non-transformer branch with
    /// `r == 0` and `|x| <= threshold` (per unit). A threshold of zero
    /// disables the treatment, as `THRSHZ = 0` does in PSS/E.
    PsseThreshold(f64),
    /// A non-transformer branch whose series impedance magnitude
    /// `hypot(r, x)` is at or below the threshold (per unit). Wider than the
    /// PSS/E rule: it also merges small resistive jumpers.
    ImpedanceMagnitude(f64),
}

impl ZeroImpedanceRule {
    /// Whether `branch` is a zero impedance connection under this rule.
    #[allow(clippy::float_cmp)] // the exact rule compares exactly, by definition
    fn matches(self, branch: &crate::network::Branch) -> bool {
        if !branch.in_service {
            return false;
        }
        match self {
            ZeroImpedanceRule::Exact => {
                branch.r == 0.0
                    && branch.x == 0.0
                    && branch.calc_effective_tap() == 1.0
                    && branch.shift == 0.0
            }
            ZeroImpedanceRule::PsseThreshold(threshold) => {
                threshold > 0.0
                    && !branch.is_transformer()
                    && branch.r == 0.0
                    && branch.x.abs() <= threshold
            }
            ZeroImpedanceRule::ImpedanceMagnitude(threshold) => {
                !branch.is_transformer() && branch.r.hypot(branch.x) <= threshold
            }
        }
    }

    fn threshold(self) -> Option<f64> {
        match self {
            ZeroImpedanceRule::Exact => None,
            ZeroImpedanceRule::PsseThreshold(threshold)
            | ZeroImpedanceRule::ImpedanceMagnitude(threshold) => Some(threshold),
        }
    }
}

impl std::fmt::Display for ZeroImpedanceRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZeroImpedanceRule::Exact => f.write_str("r == 0 and x == 0"),
            ZeroImpedanceRule::PsseThreshold(threshold) => {
                write!(f, "non-transformer, r == 0 and |x| <= {threshold}")
            }
            ZeroImpedanceRule::ImpedanceMagnitude(threshold) => {
                write!(f, "non-transformer, |r + jx| <= {threshold}")
            }
        }
    }
}

/// What [`BalancedNetwork::merge_buses`] merges.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct BusMergeRule {
    /// Merge the two buses of every closed switch.
    pub closed_switches: bool,
    /// Merge the two buses of every branch this rule selects; `None` merges
    /// no branch.
    pub zero_impedance: Option<ZeroImpedanceRule>,
}

impl BusMergeRule {
    /// A rule from its two parts.
    #[must_use]
    pub const fn new(closed_switches: bool, zero_impedance: Option<ZeroImpedanceRule>) -> Self {
        Self {
            closed_switches,
            zero_impedance,
        }
    }

    /// Closed switches and exact zero impedance branches: the connections
    /// that impose equal terminal voltages with no tolerance.
    #[must_use]
    pub const fn exact() -> Self {
        Self::new(true, Some(ZeroImpedanceRule::Exact))
    }

    /// Closed switches only.
    #[must_use]
    pub const fn closed_switches() -> Self {
        Self::new(true, None)
    }

    /// PSS/E semantics for `network`: closed switches plus
    /// [`ZeroImpedanceRule::PsseThreshold`] at the zero impedance threshold
    /// the case states (`GENERAL THRSHZ`, carried in
    /// [`SolverParams::zero_impedance_threshold`](crate::SolverParams::zero_impedance_threshold)).
    ///
    /// Read it from the network as parsed: normalization does not carry the
    /// solver parameters, so a normalized network states no threshold.
    ///
    /// # Errors
    /// [`Error::BusMergeRule`] when the case states no threshold. Name one
    /// with [`ZeroImpedanceRule::PsseThreshold`], for example
    /// [`PSSE_DEFAULT_ZERO_IMPEDANCE_THRESHOLD`].
    pub fn psse(network: &BalancedNetwork) -> Result<Self> {
        let threshold = network
            .solver()
            .as_ref()
            .and_then(|solver| solver.zero_impedance_threshold)
            .ok_or_else(|| Error::BusMergeRule {
                message: "the case states no zero impedance threshold (PSS/E THRSHZ); give the \
                          threshold explicitly"
                    .to_owned(),
            })?;
        let rule = Self::new(true, Some(ZeroImpedanceRule::PsseThreshold(threshold)));
        rule.check()?;
        Ok(rule)
    }

    fn check(&self) -> Result<()> {
        if let Some(threshold) = self.zero_impedance.and_then(ZeroImpedanceRule::threshold)
            && !(threshold.is_finite() && threshold >= 0.0)
        {
            return Err(Error::BusMergeRule {
                message: format!(
                    "the zero impedance threshold must be a finite nonnegative number, got {threshold}"
                ),
            });
        }
        Ok(())
    }
}

impl std::fmt::Display for BusMergeRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.closed_switches, self.zero_impedance) {
            (true, Some(rule)) => write!(f, "closed switches and branches with {rule}"),
            (false, Some(rule)) => write!(f, "branches with {rule}"),
            (true, None) => f.write_str("closed switches"),
            (false, None) => f.write_str("nothing"),
        }
    }
}

/// Why a merge removed an element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RemovalReason {
    /// A branch the rule's [`ZeroImpedanceRule`] selected.
    ZeroImpedance,
    /// A closed switch the rule merged.
    ClosedSwitch,
    /// An element the rule did not select whose two buses the merge joined
    /// through other elements: a branch in parallel with a jumper path, or an
    /// open switch across a merged group.
    Shorted,
}

/// A branch the merge removed.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RemovedBranch {
    /// Row in the source network's branch table.
    pub row: usize,
    /// The branch's `uid`, else `branches:{row}`.
    pub identity: String,
    /// The source endpoints.
    pub from: BusId,
    pub to: BusId,
    /// The bus both endpoints merged into.
    pub survivor: BusId,
    pub reason: RemovalReason,
    pub in_service: bool,
    pub r: f64,
    pub x: f64,
    /// Total line charging susceptance, per unit; the merge drops it.
    pub b: f64,
    /// Thermal ratings in MVA; `0` means unlimited.
    pub rate_a: f64,
    pub rate_b: f64,
    pub rate_c: f64,
}

/// A switch the merge removed.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RemovedSwitch {
    /// Row in the source network's switch table.
    pub row: usize,
    /// The switch's `uid`, else `switches:{row}`.
    pub identity: String,
    pub from: BusId,
    pub to: BusId,
    pub survivor: BusId,
    pub reason: RemovalReason,
    pub closed: bool,
    /// Thermal rating in MVA, when stated.
    pub thermal_rating: Option<f64>,
}

/// Buses the merge joined into one.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BusGroup {
    /// The bus that carries the group in the merged network.
    pub survivor: BusId,
    /// Every bus of the group, survivor included, in ascending id order.
    pub members: Vec<BusId>,
}

/// The result of [`BalancedNetwork::merge_buses`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BusMerge {
    /// The merged network.
    pub network: BalancedNetwork,
    /// The network the merge read; a shared handle, not a copy.
    pub source: BalancedNetwork,
    /// The rule that produced this merge.
    pub rule: BusMergeRule,
    /// Every merged bus to the bus that now carries it. Buses that survived,
    /// whether or not they absorbed others, are absent.
    pub merged_buses: BTreeMap<BusId, BusId>,
    /// Every group of two or more buses, in ascending survivor order.
    pub groups: Vec<BusGroup>,
    /// Removed branches in source row order.
    pub removed_branches: Vec<RemovedBranch>,
    /// Removed switches in source row order.
    pub removed_switches: Vec<RemovedSwitch>,
    /// For each source branch row, its row in the merged network, or `None`
    /// when the merge removed it.
    pub branch_rows: Vec<Option<usize>>,
    /// For each source switch row, its row in the merged network, or `None`
    /// when the merge removed it.
    pub switch_rows: Vec<Option<usize>>,
    /// What the merge changed or could not do.
    pub diagnostics: Vec<Diagnostic>,
}

impl BusMerge {
    /// The bus that carries `bus` in the merged network: its survivor when
    /// it merged, else `bus` itself.
    #[must_use]
    pub fn survivor(&self, bus: BusId) -> BusId {
        self.merged_buses.get(&bus).copied().unwrap_or(bus)
    }

    /// Whether the merge changed nothing.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.merged_buses.is_empty()
            && self.removed_branches.is_empty()
            && self.removed_switches.is_empty()
    }
}

/// A solution of a merged network, as [`BusMerge::calc_removed_flows`]
/// reads it. Every value is active power in the network's own units (MW for
/// a network as parsed, per unit for a normalized one).
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct MergedFlows<'a> {
    /// Power entering each branch of the merged network at its from end,
    /// in merged branch order.
    pub branch_p_from: &'a [f64],
    /// Power entering each branch at its to end. A lossless DC solution
    /// states the negation of `branch_p_from`.
    pub branch_p_to: &'a [f64],
    /// Power each generator injects, in generator order; out-of-service rows
    /// are ignored. `None` reads the stated `pg`.
    pub generator_p: Option<&'a [f64]>,
    /// Power entering each winding of each three winding transformer from
    /// its bus. Required when a merged group holds a winding bus.
    pub transformer_3w_p: &'a [[f64; 3]],
}

impl<'a> MergedFlows<'a> {
    /// Branch end flows with stated generation and no three winding
    /// transformer flows.
    #[must_use]
    pub fn new(branch_p_from: &'a [f64], branch_p_to: &'a [f64]) -> Self {
        Self {
            branch_p_from,
            branch_p_to,
            generator_p: None,
            transformer_3w_p: &[],
        }
    }

    /// Use these generator outputs rather than the stated `pg`.
    #[must_use]
    pub fn with_generator_p(mut self, generator_p: &'a [f64]) -> Self {
        self.generator_p = Some(generator_p);
        self
    }

    /// Supply the three winding transformer winding flows.
    #[must_use]
    pub fn with_transformer_3w_p(mut self, transformer_3w_p: &'a [[f64; 3]]) -> Self {
        self.transformer_3w_p = transformer_3w_p;
        self
    }
}

/// How [`BusMerge::calc_removed_flows`] determined one flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RemovedFlowMethod {
    /// The element is out of service or an open switch, so it carries none.
    OutOfService,
    /// The removed elements form a tree here, so Kirchhoff's current law
    /// alone fixes the flow.
    Tree,
    /// The element lies on a loop, split by the elements' own reactances.
    Reactance,
    /// The element lies on a loop of zero reactance elements, whose split no
    /// network quantity fixes; the minimum norm split is reported.
    MinimumNorm,
}

/// One recovered flow.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct RemovedFlow {
    /// Active power entering the element at its source from end.
    pub p_from: f64,
    pub method: RemovedFlowMethod,
}

/// The result of [`BusMerge::calc_removed_flows`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RemovedFlows {
    /// One flow per [`BusMerge::removed_branches`] entry, in that order.
    pub branches: Vec<RemovedFlow>,
    /// One flow per [`BusMerge::removed_switches`] entry, in that order.
    pub switches: Vec<RemovedFlow>,
    /// Loops split by minimum norm and groups whose injections do not
    /// balance.
    pub diagnostics: Vec<Diagnostic>,
}

/// Union-find over dense bus indices that refuses a union joining two
/// windings of one three winding transformer.
struct MergeSets {
    parent: Vec<usize>,
    /// Per root, the three winding transformers with a winding in the set.
    windings: HashMap<usize, Vec<usize>>,
}

enum Join {
    Joined,
    AlreadyJoined,
    /// The union would join two windings of this transformer.
    Refused(usize),
}

impl MergeSets {
    fn new(n: usize, windings: HashMap<usize, Vec<usize>>) -> Self {
        Self {
            parent: (0..n).collect(),
            windings,
        }
    }

    fn root(&mut self, node: usize) -> usize {
        let mut root = node;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut walk = node;
        while self.parent[walk] != root {
            let next = self.parent[walk];
            self.parent[walk] = root;
            walk = next;
        }
        root
    }

    fn join(&mut self, a: usize, b: usize) -> Join {
        let (ra, rb) = (self.root(a), self.root(b));
        if ra == rb {
            return Join::AlreadyJoined;
        }
        if let (Some(wa), Some(wb)) = (self.windings.get(&ra), self.windings.get(&rb))
            && let Some(&shared) = wa.iter().find(|t| wb.contains(t))
        {
            return Join::Refused(shared);
        }
        let (keep, gone) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[gone] = keep;
        if let Some(moved) = self.windings.remove(&gone) {
            self.windings.entry(keep).or_default().extend(moved);
        }
        Join::Joined
    }
}

/// Move the detailed connectivity's calculated bus assignments onto the
/// survivors: connectivity nodes and configured buses point at the survivor,
/// voltage levels list it once, and the calculated bus records of one survivor
/// combine. When that leaves the hierarchy inconsistent, as when a merge joins
/// buses of two voltage levels, the calculated bus assignments are dropped and
/// the finding returned; the rest of the record, such as component metadata,
/// stays.
fn remap_detailed_connectivity(
    network: &mut BalancedNetwork,
    resolve: &impl Fn(BusId) -> BusId,
) -> Option<Diagnostic> {
    let detailed = network.detailed_connectivity().as_deref()?;
    let moved = |bus: BusId| resolve(bus) != bus;
    let touches = detailed
        .voltage_levels
        .iter()
        .any(|level| level.buses.iter().copied().any(moved))
        || detailed
            .bus_breaker_buses
            .iter()
            .any(|bus| bus.calculated_bus.is_some_and(moved))
        || detailed
            .connectivity_nodes
            .iter()
            .any(|node| node.calculated_bus.is_some_and(moved))
        || detailed
            .calculated_buses
            .iter()
            .any(|calculated| moved(calculated.calculated_bus));
    if !touches {
        return None;
    }
    let mut remapped = detailed.clone();
    for level in &mut remapped.voltage_levels {
        let mut seen = std::collections::HashSet::new();
        level.buses = level
            .buses
            .iter()
            .map(|&bus| resolve(bus))
            .filter(|&bus| seen.insert(bus))
            .collect();
    }
    for bus in &mut remapped.bus_breaker_buses {
        bus.calculated_bus = bus.calculated_bus.map(resolve);
    }
    for node in &mut remapped.connectivity_nodes {
        node.calculated_bus = node.calculated_bus.map(resolve);
    }
    let mut combined: Vec<crate::network::CalculatedBus> = Vec::new();
    let mut position: HashMap<BusId, usize> = HashMap::new();
    for mut calculated in std::mem::take(&mut remapped.calculated_buses) {
        calculated.calculated_bus = resolve(calculated.calculated_bus);
        if let Some(&kept) = position.get(&calculated.calculated_bus) {
            combined[kept].nodes.extend(calculated.nodes);
        } else {
            position.insert(calculated.calculated_bus, combined.len());
            combined.push(calculated);
        }
    }
    remapped.calculated_buses = combined;
    *network.detailed_connectivity_mut() = Some(std::sync::Arc::new(remapped));
    if network.validate().is_ok() {
        return None;
    }

    let detailed = std::sync::Arc::make_mut(
        network
            .detailed_connectivity_mut()
            .as_mut()
            .expect("the record was just written"),
    );
    for level in &mut detailed.voltage_levels {
        level.buses.clear();
    }
    for bus in &mut detailed.bus_breaker_buses {
        bus.calculated_bus = None;
    }
    for node in &mut detailed.connectivity_nodes {
        node.calculated_bus = None;
    }
    detailed.calculated_buses.clear();
    Some(Diagnostic::of(
        &codes::CANONICALIZE_MERGE_DETAIL_DROPPED,
        "the detailed connectivity's calculated bus assignments were dropped: the merge joined \
         buses its hierarchy keeps apart, such as buses of two voltage levels",
    ))
}

/// Bus kind importance: the survivor of a group takes the strongest kind of
/// its members, so a merge never demotes a slack.
pub(crate) fn kind_priority(kind: BusType) -> u8 {
    match kind {
        BusType::Ref => 3,
        BusType::Pv => 2,
        BusType::Pq => 1,
        BusType::Isolated => 0,
    }
}

fn identity(uid: Option<&str>, table: &str, row: usize) -> String {
    uid.map_or_else(|| format!("{table}:{row}"), str::to_owned)
}

/// A finding whose details carry the counts its message states.
fn counted(
    info: &'static crate::diagnostics::DiagnosticInfo,
    message: String,
    counts: &[(&str, usize)],
) -> Diagnostic {
    let details = counts
        .iter()
        .map(|(key, count)| ((*key).to_owned(), serde_json::Value::from(*count)))
        .collect();
    Diagnostic::of(info, message)
        .with_details(details)
        .expect("a few short count entries are within the detail bounds")
}

impl BalancedNetwork {
    /// Merge every set of buses the elements `rule` selects connect.
    ///
    /// Each set becomes one bus. The survivor is the set's reference bus,
    /// else a bus hosting an in-service generator, else a bus an in-service
    /// generator regulates, with the smallest id breaking each tie. It takes
    /// the strongest bus kind of the set (reference, then PV, then PQ) and
    /// keeps its own other attributes. Every element reference to a merged
    /// bus moves to the survivor: loads, shunts, static var compensators,
    /// generators and their regulated buses, storage, branch, switch, and
    /// HVDC endpoints, three winding transformer windings, control buses,
    /// and area swing buses.
    ///
    /// The merge removes the selected elements and any other branch or
    /// switch whose two buses it joined; [`BusMerge::removed_branches`] and
    /// [`BusMerge::removed_switches`] list each with its source row and
    /// ratings. An HVDC line whose two ends merged stays, with a diagnostic.
    /// An element whose merge would join two windings of one three winding
    /// transformer stays in place and is reported. Detailed connectivity
    /// follows the survivors; when the merge joins buses its hierarchy keeps
    /// apart, such as buses of two voltage levels, its calculated bus
    /// assignments are dropped with a diagnostic.
    ///
    /// The input is not modified, and a merge that joins nothing returns a
    /// shared handle to it. Merging the result again under the same rule
    /// joins nothing.
    ///
    /// # Errors
    /// [`Error::BusMergeRule`] for a threshold that is negative or not
    /// finite. [`Error::UnknownBus`] when a selected element names a bus the
    /// network does not declare.
    #[allow(clippy::too_many_lines)] // one stanza per stage, in order
    pub fn merge_buses(&self, rule: &BusMergeRule) -> Result<BusMerge> {
        rule.check()?;
        let buses = self.buses();
        let index_of: HashMap<BusId, usize> = buses
            .iter()
            .enumerate()
            .map(|(index, bus)| (bus.id, index))
            .collect();
        let lookup = |bus: BusId, row: usize| {
            index_of.get(&bus).copied().ok_or(Error::UnknownBus {
                bus_id: bus,
                element_index: row,
            })
        };

        let mut windings: HashMap<usize, Vec<usize>> = HashMap::new();
        for (t, transformer) in self.transformers_3w().iter().enumerate() {
            for winding in &transformer.windings {
                if let Some(&index) = index_of.get(&winding.bus) {
                    let list = windings.entry(index).or_default();
                    if !list.contains(&t) {
                        list.push(t);
                    }
                }
            }
        }
        let mut sets = MergeSets::new(buses.len(), windings);
        let mut diagnostics = Vec::new();
        let mut refused = |what: String, from: BusId, to: BusId, t: usize| {
            let transformer = &self.transformers_3w()[t];
            let name = identity(transformer.uid.as_deref(), "transformers_3w", t);
            diagnostics.push(Diagnostic::of(
                &codes::CANONICALIZE_MERGE_WINDING_PAIR_KEPT,
                format!(
                    "{what} between buses {from} and {to} was kept: merging it would join two \
                     windings of three winding transformer `{name}`"
                ),
            ));
        };

        if rule.closed_switches {
            for (row, switch) in self.switches().iter().enumerate() {
                if !switch.closed || switch.from == switch.to {
                    continue;
                }
                let (a, b) = (lookup(switch.from, row)?, lookup(switch.to, row)?);
                if let Join::Refused(t) = sets.join(a, b) {
                    let name = identity(switch.uid.as_deref(), "switches", row);
                    refused(format!("closed switch `{name}`"), switch.from, switch.to, t);
                }
            }
        }
        let mut selected = vec![false; self.branches().len()];
        if let Some(zero_impedance) = rule.zero_impedance {
            for (row, branch) in self.branches().iter().enumerate() {
                if branch.from == branch.to || !zero_impedance.matches(branch) {
                    continue;
                }
                let (a, b) = (lookup(branch.from, row)?, lookup(branch.to, row)?);
                match sets.join(a, b) {
                    Join::Joined | Join::AlreadyJoined => selected[row] = true,
                    Join::Refused(t) => {
                        let name = identity(branch.uid.as_deref(), "branches", row);
                        refused(format!("branch `{name}`"), branch.from, branch.to, t);
                    }
                }
            }
        }

        // Group the buses by root and pick each group's survivor.
        let mut members_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for index in 0..buses.len() {
            let root = sets.root(index);
            if root != index {
                members_of.entry(root).or_default().push(index);
            }
        }
        // Only merged members so far; each group's root joins its list.
        for (&root, members) in &mut members_of {
            members.push(root);
        }
        if members_of.is_empty() {
            return Ok(self.identity_merge(*rule, diagnostics));
        }

        let mut hosts_generator = vec![false; buses.len()];
        let mut regulated = vec![false; buses.len()];
        for generator in self.generators().iter().filter(|g| g.in_service) {
            if let Some(&index) = index_of.get(&generator.bus) {
                hosts_generator[index] = true;
            }
            if let Some(index) = generator.regulated_bus.and_then(|b| index_of.get(&b)) {
                regulated[*index] = true;
            }
        }
        let rank = |index: usize| {
            let bus = &buses[index];
            (
                bus.kind == BusType::Ref,
                hosts_generator[index],
                regulated[index],
                std::cmp::Reverse(bus.id),
            )
        };

        let mut merged_buses: HashMap<BusId, BusId> = HashMap::new();
        let mut groups = Vec::with_capacity(members_of.len());
        let mut survivor_kind: HashMap<BusId, BusType> = HashMap::new();
        for (&root, members) in &members_of {
            let survivor_index = members
                .iter()
                .copied()
                .max_by_key(|&index| rank(index))
                .unwrap_or(root);
            let survivor = buses[survivor_index].id;
            let kind = members
                .iter()
                .map(|&index| buses[index].kind)
                .max_by_key(|&kind| kind_priority(kind))
                .unwrap_or(buses[survivor_index].kind);
            survivor_kind.insert(survivor, kind);
            let mut ids: Vec<BusId> = members.iter().map(|&index| buses[index].id).collect();
            ids.sort_unstable();
            for &id in &ids {
                if id != survivor {
                    merged_buses.insert(id, survivor);
                }
            }

            let base_kv = buses[survivor_index].base_kv;
            let mut other_kv: Vec<f64> = members
                .iter()
                .map(|&index| buses[index].base_kv)
                .filter(|kv| kv.to_bits() != base_kv.to_bits())
                .collect();
            if !other_kv.is_empty() {
                other_kv.sort_by(f64::total_cmp);
                other_kv.dedup_by(|a, b| a.to_bits() == b.to_bits());
                let other = other_kv
                    .iter()
                    .map(|kv| format!("{kv} kV"))
                    .collect::<Vec<_>>()
                    .join(", ");
                diagnostics.push(Diagnostic::of(
                    &codes::CANONICALIZE_MERGE_ATTRIBUTE_CONFLICT,
                    format!(
                        "buses of base {other} merged into bus {survivor} (base {base_kv} kV); \
                         the surviving base was kept"
                    ),
                ));
            }
            let references: Vec<BusId> = ids
                .iter()
                .copied()
                .filter(|&id| buses[index_of[&id]].kind == BusType::Ref)
                .collect();
            if references.len() > 1 {
                let list = references
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                diagnostics.push(Diagnostic::of(
                    &codes::CANONICALIZE_MERGE_ATTRIBUTE_CONFLICT,
                    format!(
                        "reference buses {list} merged into one; bus {survivor} remains the \
                         reference"
                    ),
                ));
            }
            groups.push(BusGroup {
                survivor,
                members: ids,
            });
        }
        groups.sort_by_key(|group| group.survivor);
        let resolve = |bus: BusId| merged_buses.get(&bus).copied().unwrap_or(bus);

        // Classify what the merge removes.
        let mut removed_branches = Vec::new();
        let mut branch_rows = Vec::with_capacity(self.branches().len());
        let mut next = 0usize;
        let (mut charging_dropped, mut rated) = (0usize, 0usize);
        for (row, branch) in self.branches().iter().enumerate() {
            let survivor = resolve(branch.from);
            if branch.from == branch.to || survivor != resolve(branch.to) {
                branch_rows.push(Some(next));
                next += 1;
                continue;
            }
            branch_rows.push(None);
            let reason = if selected[row] {
                RemovalReason::ZeroImpedance
            } else {
                RemovalReason::Shorted
            };
            let removed = RemovedBranch {
                row,
                identity: identity(branch.uid.as_deref(), "branches", row),
                from: branch.from,
                to: branch.to,
                survivor,
                reason,
                in_service: branch.in_service,
                r: branch.r,
                x: branch.x,
                b: branch.calc_total_charging_b(),
                rate_a: branch.rate_a,
                rate_b: branch.rate_b,
                rate_c: branch.rate_c,
            };
            if reason == RemovalReason::Shorted {
                let kind = if branch.is_transformer() {
                    "transformer"
                } else {
                    "branch"
                };
                diagnostics.push(Diagnostic::of(
                    &codes::CANONICALIZE_MERGE_ELEMENT_SHORTED,
                    format!(
                        "{kind} `{}` between buses {} and {} was removed: the merge joined its \
                         two buses into bus {survivor} through other elements",
                        removed.identity, removed.from, removed.to
                    ),
                ));
            } else {
                charging_dropped += usize::from(removed.b != 0.0);
                rated += usize::from(
                    removed.rate_a > 0.0 || removed.rate_b > 0.0 || removed.rate_c > 0.0,
                );
            }
            removed_branches.push(removed);
        }
        let mut removed_switches = Vec::new();
        let mut switch_rows = Vec::with_capacity(self.switches().len());
        let mut next = 0usize;
        for (row, switch) in self.switches().iter().enumerate() {
            let survivor = resolve(switch.from);
            if switch.from == switch.to || survivor != resolve(switch.to) {
                switch_rows.push(Some(next));
                next += 1;
                continue;
            }
            switch_rows.push(None);
            let reason = if switch.closed && rule.closed_switches {
                RemovalReason::ClosedSwitch
            } else {
                RemovalReason::Shorted
            };
            let removed = RemovedSwitch {
                row,
                identity: identity(switch.uid.as_deref(), "switches", row),
                from: switch.from,
                to: switch.to,
                survivor,
                reason,
                closed: switch.closed,
                thermal_rating: switch.thermal_rating,
            };
            if reason == RemovalReason::Shorted {
                let state = if switch.closed { "closed" } else { "open" };
                diagnostics.push(Diagnostic::of(
                    &codes::CANONICALIZE_MERGE_ELEMENT_SHORTED,
                    format!(
                        "{state} switch `{}` between buses {} and {} was removed: the merge \
                         joined its two buses into bus {survivor} through other elements",
                        removed.identity, removed.from, removed.to
                    ),
                ));
            }
            removed_switches.push(removed);
        }
        for (row, line) in self.hvdc().iter().enumerate() {
            if line.from != line.to && resolve(line.from) == resolve(line.to) {
                let name = identity(line.uid.as_deref(), "hvdc", row);
                diagnostics.push(Diagnostic::of(
                    &codes::CANONICALIZE_MERGE_ELEMENT_SHORTED,
                    format!(
                        "HVDC line `{name}` between buses {} and {} now has both ends on bus {}; \
                         it was kept",
                        line.from,
                        line.to,
                        resolve(line.from)
                    ),
                ));
            }
        }

        let zero_impedance = removed_branches
            .iter()
            .filter(|removed| removed.reason == RemovalReason::ZeroImpedance)
            .count();
        if let Some(branch_rule) = rule.zero_impedance
            && zero_impedance > 0
        {
            diagnostics.push(counted(
                &codes::CANONICALIZE_MERGE_ZERO_IMPEDANCE,
                format!(
                    "{zero_impedance} in-service branch(es) with {branch_rule} were merged \
                     ({rated} with a thermal rating, {charging_dropped} with line charging, \
                     which the merge drops); their flows are no longer variables of the merged \
                     network, and BusMerge::calc_removed_flows recovers them from a solution"
                ),
                &[
                    ("count", zero_impedance),
                    ("rated", rated),
                    ("charging_dropped", charging_dropped),
                ],
            ));
        }
        let closed = removed_switches
            .iter()
            .filter(|removed| removed.reason == RemovalReason::ClosedSwitch)
            .count();
        if closed > 0 {
            diagnostics.push(counted(
                &codes::CANONICALIZE_MERGE_CLOSED_SWITCH,
                format!(
                    "{closed} closed switch(es) were merged; their flows are no longer variables \
                     of the merged network, and BusMerge::calc_removed_flows recovers them from a \
                     solution"
                ),
                &[("count", closed)],
            ));
        }

        let mut network = self.clone();
        {
            let mut row = 0usize;
            network.branches_mut().retain(|_| {
                let keep = branch_rows[row].is_some();
                row += 1;
                keep
            });
            if !removed_switches.is_empty() {
                let mut row = 0usize;
                network.switches_mut().retain(|_| {
                    let keep = switch_rows[row].is_some();
                    row += 1;
                    keep
                });
            }
        }
        network.remap_buses(resolve);
        network.buses_mut().retain_mut(|bus| {
            if merged_buses.contains_key(&bus.id) {
                return false;
            }
            if let Some(&kind) = survivor_kind.get(&bus.id) {
                bus.kind = kind;
            }
            true
        });
        if let Some(diagnostic) = remap_detailed_connectivity(&mut network, &resolve) {
            diagnostics.push(diagnostic);
        }
        debug_assert!(
            network.validate().is_ok(),
            "merge_buses produced a dangling reference"
        );

        Ok(BusMerge {
            network,
            source: self.clone(),
            rule: *rule,
            merged_buses: merged_buses.into_iter().collect(),
            groups,
            removed_branches,
            removed_switches,
            branch_rows,
            switch_rows,
            diagnostics,
        })
    }

    fn identity_merge(&self, rule: BusMergeRule, diagnostics: Vec<Diagnostic>) -> BusMerge {
        BusMerge {
            network: self.clone(),
            source: self.clone(),
            rule,
            merged_buses: BTreeMap::new(),
            groups: Vec::new(),
            removed_branches: Vec::new(),
            removed_switches: Vec::new(),
            branch_rows: (0..self.branches().len()).map(Some).collect(),
            switch_rows: (0..self.switches().len()).map(Some).collect(),
            diagnostics,
        }
    }
}

/// The largest loop component [`BusMerge::calc_removed_flows`] splits with a
/// dense solve.
const MAX_DENSE_LOOP_BUSES: usize = 2000;

/// One removed element inside a merged group, in the group's local indices.
#[derive(Clone, Copy)]
struct GroupEdge {
    from: usize,
    to: usize,
    /// Series reactance; zero for a switch.
    x: f64,
    slot: Slot,
}

#[derive(Clone, Copy)]
enum Slot {
    Branch(usize),
    Switch(usize),
}

impl BusMerge {
    /// Recover the active power on every removed element from a solution of
    /// the merged network.
    ///
    /// Each merged group is one bus of the merged solution. The power its
    /// members exchange through the removed elements follows from what sits
    /// at each member: the stated in-service loads (`p`), shunts (`g`, at
    /// 1 per unit voltage), storage (`ps`), and HVDC terminals (`-pf` at the
    /// from end, `pt` at the to end), the generator outputs, and the flows
    /// `flows` states for the kept branches and three winding transformer
    /// windings at that member. Whatever the merged solution injects at a
    /// group beyond those stated values, such as a slack generator's
    /// adjustment, enters at the survivor.
    ///
    /// Where the removed elements of a group form a tree, Kirchhoff's current
    /// law fixes every flow. A loop is split by the elements' reactances,
    /// which is the DC power flow of the unmerged elements. A zero reactance
    /// element (a switch, or an exact zero impedance branch) holds its two
    /// buses at one angle, so an element in parallel with it carries nothing.
    /// A loop made only of zero reactance
    /// elements has no physical split, so the minimum norm split is reported
    /// with a diagnostic. Out-of-service branches and open switches carry
    /// none.
    ///
    /// A group whose injections do not balance gets a diagnostic: a group
    /// with no reference bus, or any group when `flows` states the generator
    /// outputs, should balance, and an imbalance means the solution modeled
    /// a device differently.
    ///
    /// # Errors
    /// [`Error::MergedFlowShape`] when a slice in `flows` does not match the
    /// merged network's table, or when a group holds a three winding
    /// transformer winding and `flows` states no winding flows.
    #[allow(clippy::too_many_lines)] // one stanza per injection source, in order
    pub fn calc_removed_flows(&self, flows: &MergedFlows<'_>) -> Result<RemovedFlows> {
        let shape = |what: &'static str, expected: usize, got: usize| {
            if expected == got {
                Ok(())
            } else {
                Err(Error::MergedFlowShape {
                    what,
                    expected,
                    got,
                })
            }
        };
        let merged_branches = self.network.branches().len();
        shape("branch_p_from", merged_branches, flows.branch_p_from.len())?;
        shape("branch_p_to", merged_branches, flows.branch_p_to.len())?;
        let source = &self.source;
        if let Some(generator_p) = flows.generator_p {
            shape("generator_p", source.generators().len(), generator_p.len())?;
        }
        if !flows.transformer_3w_p.is_empty() {
            shape(
                "transformer_3w_p",
                source.transformers_3w().len(),
                flows.transformer_3w_p.len(),
            )?;
        }

        let mut place: HashMap<BusId, (usize, usize)> = HashMap::new();
        for (group, members) in self.groups.iter().enumerate() {
            for (local, &bus) in members.members.iter().enumerate() {
                place.insert(bus, (group, local));
            }
        }
        let mut injection: Vec<Vec<f64>> = self
            .groups
            .iter()
            .map(|group| vec![0.0; group.members.len()])
            .collect();
        // Per group: the stated device injection, the power the merged
        // solution sends out through kept elements, and a magnitude scale
        // for the balance check.
        let mut device = vec![0.0; self.groups.len()];
        let mut leaving = vec![0.0; self.groups.len()];
        let mut scale = vec![0.0f64; self.groups.len()];
        let mut inject = |bus: BusId, value: f64, from_device: bool| {
            if let Some(&(group, local)) = place.get(&bus) {
                injection[group][local] += value;
                if from_device {
                    device[group] += value;
                } else {
                    leaving[group] -= value;
                }
                scale[group] += value.abs();
            }
        };

        for (row, generator) in source.generators().iter().enumerate() {
            if generator.in_service {
                let p = flows.generator_p.map_or(generator.pg, |p| p[row]);
                inject(generator.bus, p, true);
            }
        }
        for load in source.loads().iter().filter(|load| load.in_service) {
            inject(load.bus, -load.p, true);
        }
        for shunt in source.shunts().iter().filter(|shunt| shunt.in_service) {
            inject(shunt.bus, -shunt.g, true);
        }
        for storage in source.storage().iter().filter(|storage| storage.in_service) {
            inject(storage.bus, -storage.ps, true);
        }
        for line in source.hvdc().iter().filter(|line| line.in_service) {
            inject(line.from, -line.pf, true);
            inject(line.to, line.pt, true);
        }
        for (row, merged_row) in self.branch_rows.iter().enumerate() {
            if let Some(merged_row) = *merged_row {
                let branch = &source.branches()[row];
                inject(branch.from, -flows.branch_p_from[merged_row], false);
                inject(branch.to, -flows.branch_p_to[merged_row], false);
            }
        }
        for (row, transformer) in source.transformers_3w().iter().enumerate() {
            if !transformer.in_service {
                continue;
            }
            for (winding, terminal) in transformer.windings.iter().enumerate() {
                if !place.contains_key(&terminal.bus) {
                    continue;
                }
                let Some(p) = flows.transformer_3w_p.get(row) else {
                    return Err(Error::MergedFlowShape {
                        what: "transformer_3w_p",
                        expected: source.transformers_3w().len(),
                        got: flows.transformer_3w_p.len(),
                    });
                };
                inject(terminal.bus, -p[winding], false);
            }
        }

        let references: std::collections::HashSet<BusId> = source
            .buses()
            .iter()
            .filter(|bus| bus.kind == BusType::Ref)
            .map(|bus| bus.id)
            .collect();
        let mut diagnostics = Vec::new();
        for (index, group) in self.groups.iter().enumerate() {
            let residual = leaving[index] - device[index];
            let local = group
                .members
                .iter()
                .position(|&bus| bus == group.survivor)
                .unwrap_or_default();
            injection[index][local] += residual;
            let has_reference = group.members.iter().any(|bus| references.contains(bus));
            let tolerance = 1e-6 * scale[index].max(1.0);
            if residual.abs() > tolerance && (flows.generator_p.is_some() || !has_reference) {
                diagnostics.push(Diagnostic::of(
                    &codes::CANONICALIZE_MERGE_FLOW_RESIDUAL,
                    format!(
                        "the group merged into bus {} receives {residual} more from the merged \
                         solution than its stated devices inject; the difference was placed at \
                         bus {}, so its removed element flows carry it",
                        group.survivor, group.survivor
                    ),
                ));
            }
        }

        let not_carried = RemovedFlow {
            p_from: 0.0,
            method: RemovedFlowMethod::OutOfService,
        };
        let mut branches = vec![not_carried; self.removed_branches.len()];
        let mut switches = vec![not_carried; self.removed_switches.len()];
        let mut edges: Vec<Vec<GroupEdge>> = vec![Vec::new(); self.groups.len()];
        let local = |bus: BusId| place[&bus];
        for (index, removed) in self.removed_branches.iter().enumerate() {
            if removed.in_service {
                let ((group, from), (_, to)) = (local(removed.from), local(removed.to));
                edges[group].push(GroupEdge {
                    from,
                    to,
                    x: removed.x,
                    slot: Slot::Branch(index),
                });
            }
        }
        for (index, removed) in self.removed_switches.iter().enumerate() {
            if removed.closed {
                let ((group, from), (_, to)) = (local(removed.from), local(removed.to));
                edges[group].push(GroupEdge {
                    from,
                    to,
                    x: 0.0,
                    slot: Slot::Switch(index),
                });
            }
        }

        for (index, group) in self.groups.iter().enumerate() {
            if edges[index].is_empty() {
                continue;
            }
            let solved = split_group(group.members.len(), &edges[index], &injection[index]);
            for (edge, flow) in edges[index].iter().zip(&solved.flows) {
                match edge.slot {
                    Slot::Branch(row) => branches[row] = *flow,
                    Slot::Switch(row) => switches[row] = *flow,
                }
            }
            if solved.minimum_norm > 0 {
                diagnostics.push(Diagnostic::of(
                    &codes::CANONICALIZE_MERGE_FLOW_UNDETERMINED,
                    format!(
                        "{} removed element(s) in the group merged into bus {} lie on loops of \
                         zero reactance elements; no network quantity fixes their split, so the \
                         minimum norm split is reported",
                        solved.minimum_norm, group.survivor
                    ),
                ));
            }
            if solved.too_large > 0 {
                diagnostics.push(Diagnostic::of(
                    &codes::CANONICALIZE_MERGE_FLOW_UNDETERMINED,
                    format!(
                        "{} removed element(s) in the group merged into bus {} lie on a loop of \
                         more than {MAX_DENSE_LOOP_BUSES} buses, which is not split; their flows \
                         are NaN",
                        solved.too_large, group.survivor
                    ),
                ));
            }
        }

        Ok(RemovedFlows {
            branches,
            switches,
            diagnostics,
        })
    }
}

struct SplitGroup {
    flows: Vec<RemovedFlow>,
    minimum_norm: usize,
    too_large: usize,
}

/// Split one group's member injections over its removed elements.
///
/// Zero reactance elements force their buses to one angle, so they are
/// collapsed first and the reactive elements are split over the collapsed
/// graph by their reactances. Each collapsed set then routes what remains
/// at its members over its zero reactance elements.
fn split_group(members: usize, edges: &[GroupEdge], injection: &[f64]) -> SplitGroup {
    let mut flows = vec![
        RemovedFlow {
            p_from: 0.0,
            method: RemovedFlowMethod::Reactance,
        };
        edges.len()
    ];
    let mut too_large = 0;

    // Collapse the zero reactance elements.
    let mut sets = MergeSets::new(members, HashMap::new());
    for edge in edges.iter().filter(|edge| edge.x == 0.0) {
        let _ = sets.join(edge.from, edge.to);
    }
    let mut collapsed_of = vec![usize::MAX; members];
    let mut collapsed = 0usize;
    for node in 0..members {
        let root = sets.root(node);
        if collapsed_of[root] == usize::MAX {
            collapsed_of[root] = collapsed;
            collapsed += 1;
        }
        collapsed_of[node] = collapsed_of[root];
    }
    let mut collapsed_injection = vec![0.0; collapsed];
    for (node, value) in injection.iter().enumerate() {
        collapsed_injection[collapsed_of[node]] += value;
    }

    // Reactive elements between collapsed sets, by reactance. One inside a
    // set is shorted by a zero reactance path and carries nothing.
    let reactive: Vec<usize> = (0..edges.len())
        .filter(|&e| edges[e].x != 0.0 && collapsed_of[edges[e].from] != collapsed_of[edges[e].to])
        .collect();
    let mut remaining = injection.to_vec();
    if !reactive.is_empty() {
        let links: Vec<(usize, usize, f64)> = reactive
            .iter()
            .map(|&e| {
                (
                    collapsed_of[edges[e].from],
                    collapsed_of[edges[e].to],
                    edges[e].x,
                )
            })
            .collect();
        let solved = split_network(collapsed, &links, &collapsed_injection);
        for (&e, (flow, kind)) in reactive.iter().zip(solved) {
            flows[e] = RemovedFlow {
                p_from: flow,
                method: match kind {
                    Split::Tree => RemovedFlowMethod::Tree,
                    Split::Loop => RemovedFlowMethod::Reactance,
                    Split::TooLarge => {
                        too_large += 1;
                        RemovedFlowMethod::Reactance
                    }
                },
            };
            remaining[edges[e].from] -= flow;
            remaining[edges[e].to] += flow;
        }
    }

    // Zero reactance elements within each collapsed set.
    let zero: Vec<usize> = (0..edges.len()).filter(|&e| edges[e].x == 0.0).collect();
    let mut minimum_norm = 0;
    if !zero.is_empty() {
        let links: Vec<(usize, usize, f64)> = zero
            .iter()
            .map(|&e| (edges[e].from, edges[e].to, 1.0))
            .collect();
        let solved = split_network(members, &links, &remaining);
        for (&e, (flow, kind)) in zero.iter().zip(solved) {
            flows[e] = RemovedFlow {
                p_from: flow,
                method: match kind {
                    Split::Tree => RemovedFlowMethod::Tree,
                    Split::Loop => {
                        minimum_norm += 1;
                        RemovedFlowMethod::MinimumNorm
                    }
                    Split::TooLarge => {
                        too_large += 1;
                        RemovedFlowMethod::MinimumNorm
                    }
                },
            };
        }
    }
    SplitGroup {
        flows,
        minimum_norm,
        too_large,
    }
}

#[derive(Clone, Copy)]
enum Split {
    Tree,
    Loop,
    TooLarge,
}

/// Flows `from → to` on `links` (each `(from, to, reactance)`) carrying the
/// node `injection`. Leaf elimination fixes every link outside a loop; each
/// meshed core left over is split by its reactance weighted Laplacian.
fn split_network(
    nodes: usize,
    links: &[(usize, usize, f64)],
    injection: &[f64],
) -> Vec<(f64, Split)> {
    let mut result = vec![(0.0, Split::Tree); links.len()];
    let mut net = injection.to_vec();
    let mut incident = vec![Vec::new(); nodes];
    for (index, &(from, to, _)) in links.iter().enumerate() {
        incident[from].push(index);
        incident[to].push(index);
    }
    let mut degree: Vec<usize> = incident.iter().map(Vec::len).collect();
    let mut alive = vec![true; links.len()];
    let mut leaves: Vec<usize> = (0..nodes).filter(|&node| degree[node] == 1).collect();
    while let Some(leaf) = leaves.pop() {
        let Some(&index) = incident[leaf].iter().find(|&&index| alive[index]) else {
            continue;
        };
        alive[index] = false;
        let (from, to, _) = links[index];
        let other = if from == leaf { to } else { from };
        let out = net[leaf];
        result[index] = (if from == leaf { out } else { -out }, Split::Tree);
        net[other] += out;
        net[leaf] = 0.0;
        degree[leaf] = 0;
        degree[other] -= 1;
        if degree[other] == 1 {
            leaves.push(other);
        }
    }

    // What is left is a set of meshed cores, each split on its own.
    let mut sets = MergeSets::new(nodes, HashMap::new());
    for (index, &(from, to, _)) in links.iter().enumerate() {
        if alive[index] {
            let _ = sets.join(from, to);
        }
    }
    let mut cores: BTreeMap<usize, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for node in (0..nodes).filter(|&node| degree[node] > 0) {
        cores.entry(sets.root(node)).or_default().0.push(node);
    }
    for (index, &(from, _, _)) in links.iter().enumerate() {
        if alive[index] {
            cores.entry(sets.root(from)).or_default().1.push(index);
        }
    }
    for (members, link_indices) in cores.values() {
        if members.len() > MAX_DENSE_LOOP_BUSES {
            for &index in link_indices {
                result[index] = (f64::NAN, Split::TooLarge);
            }
            continue;
        }
        let solved = solve_loop(members, links, link_indices, &net).unwrap_or_else(|| {
            // A singular reactance Laplacian (reactances of opposite sign
            // cancelling around a loop): split by minimum norm.
            let unit: Vec<(usize, usize, f64)> =
                links.iter().map(|&(from, to, _)| (from, to, 1.0)).collect();
            solve_loop(members, &unit, link_indices, &net)
                .expect("a unit weight Laplacian of a connected graph is nonsingular")
        });
        for (index, flow) in solved {
            result[index] = (flow, Split::Loop);
        }
    }
    result
}

/// Solve the weighted Laplacian of a meshed component, grounded at its first
/// member, and return each link's flow. `None` when the system is singular.
fn solve_loop(
    members: &[usize],
    links: &[(usize, usize, f64)],
    link_indices: &[usize],
    injection: &[f64],
) -> Option<Vec<(usize, f64)>> {
    let position: HashMap<usize, usize> = members
        .iter()
        .enumerate()
        .map(|(position, &node)| (node, position))
        .collect();
    let size = members.len() - 1;
    let mut matrix = vec![0.0; size * size];
    let mut rhs: Vec<f64> = members[1..].iter().map(|&node| injection[node]).collect();
    for &index in link_indices {
        let (from, to, x) = links[index];
        let conductance = 1.0 / x;
        let (i, j) = (position[&from], position[&to]);
        for (a, b, value) in [
            (i, i, conductance),
            (j, j, conductance),
            (i, j, -conductance),
            (j, i, -conductance),
        ] {
            if a > 0 && b > 0 {
                matrix[(a - 1) * size + (b - 1)] += value;
            }
        }
    }
    let angle = solve_dense(&mut matrix, &mut rhs, size)?;
    let theta = |node: usize| match position[&node] {
        0 => 0.0,
        p => angle[p - 1],
    };
    Some(
        link_indices
            .iter()
            .map(|&index| {
                let (from, to, x) = links[index];
                (index, (theta(from) - theta(to)) / x)
            })
            .collect(),
    )
}

/// Gaussian elimination with partial pivoting on a dense row major system.
fn solve_dense(matrix: &mut [f64], rhs: &mut [f64], size: usize) -> Option<Vec<f64>> {
    let largest = matrix.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let floor = largest * 1e-13;
    for column in 0..size {
        let pivot = (column..size).max_by(|&a, &b| {
            matrix[a * size + column]
                .abs()
                .total_cmp(&matrix[b * size + column].abs())
        })?;
        if matrix[pivot * size + column].abs() <= floor
            || !matrix[pivot * size + column].is_finite()
        {
            return None;
        }
        if pivot != column {
            for k in 0..size {
                matrix.swap(pivot * size + k, column * size + k);
            }
            rhs.swap(pivot, column);
        }
        let diagonal = matrix[column * size + column];
        for row in column + 1..size {
            let factor = matrix[row * size + column] / diagonal;
            if factor == 0.0 {
                continue;
            }
            for k in column..size {
                matrix[row * size + k] -= factor * matrix[column * size + k];
            }
            rhs[row] -= factor * rhs[column];
        }
    }
    let mut solution = vec![0.0; size];
    for row in (0..size).rev() {
        let mut value = rhs[row];
        for k in row + 1..size {
            value -= matrix[row * size + k] * solution[k];
        }
        solution[row] = value / matrix[row * size + row];
    }
    Some(solution)
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // the recovered flows are exact in these small cases
mod tests {
    use super::*;
    use crate::network::{
        Area, Branch, Bus, Generator, Hvdc, Impedance, Load, Shunt, SolverParams, Switch,
        SwitchedShuntControl, SwitchedShuntMode, Transformer3W, TransformerControl,
        TransformerControlMode, Winding,
    };

    const PSSE: BusMergeRule =
        BusMergeRule::new(true, Some(ZeroImpedanceRule::PsseThreshold(1e-4)));

    fn net(ids: &[usize], branches: Vec<Branch>) -> BalancedNetwork {
        let buses = ids
            .iter()
            .map(|&id| Bus::new(BusId(id), BusType::Pq, 230.0))
            .collect();
        BalancedNetwork::in_memory("merge", 100.0, buses, branches)
    }

    fn line(from: usize, to: usize, r: f64, x: f64) -> Branch {
        Branch::new(BusId(from), BusId(to), r, x)
    }

    fn jumper(from: usize, to: usize) -> Branch {
        line(from, to, 0.0, 5e-5)
    }

    fn transformer_3w(buses: [usize; 3]) -> Transformer3W {
        Transformer3W::new(
            buses.map(|bus| Winding::new(BusId(bus))),
            [Impedance::new(0.0, 0.1, 100.0); 3],
        )
    }

    fn codes_of(diagnostics: &[Diagnostic]) -> Vec<&str> {
        diagnostics.iter().map(Diagnostic::code).collect()
    }

    #[test]
    fn a_ring_of_closed_switches_merges_and_an_open_switch_stays() {
        let mut network = net(&[1, 2, 3, 4, 5, 6], vec![line(5, 6, 0.01, 0.1)]);
        network.switches_mut().extend([
            Switch::new(BusId(1), BusId(2), true),
            Switch::new(BusId(2), BusId(3), true),
            Switch::new(BusId(3), BusId(4), true),
            Switch::new(BusId(4), BusId(1), true),
            Switch::new(BusId(4), BusId(5), false),
        ]);
        let merge = network
            .merge_buses(&BusMergeRule::closed_switches())
            .unwrap();

        assert_eq!(
            merge.groups,
            vec![BusGroup {
                survivor: BusId(1),
                members: vec![BusId(1), BusId(2), BusId(3), BusId(4)],
            }]
        );
        assert_eq!(merge.survivor(BusId(4)), BusId(1));
        assert_eq!(merge.survivor(BusId(5)), BusId(5));
        assert_eq!(merge.removed_switches.len(), 4);
        assert!(
            merge
                .removed_switches
                .iter()
                .all(|removed| removed.reason == RemovalReason::ClosedSwitch)
        );
        assert_eq!(
            merge.switch_rows,
            vec![None, None, None, None, Some(0)],
            "the open switch keeps its identity at merged row 0"
        );
        let open = &merge.network.switches()[0];
        assert_eq!(
            (open.from, open.to, open.closed),
            (BusId(1), BusId(5), false)
        );
        assert_eq!(merge.network.buses().len(), 3);
        assert!(codes_of(&merge.diagnostics).contains(&"CANONICALIZE.MERGE.CLOSED_SWITCH"));
        merge.network.validate().unwrap();
    }

    #[test]
    fn the_psse_threshold_merges_a_chain_of_jumpers() {
        let network = net(
            &[1, 2, 3, 4, 5],
            vec![
                jumper(1, 2),
                jumper(2, 3),
                jumper(3, 4),
                line(4, 5, 0.01, 0.1),
            ],
        );
        let merge = network.merge_buses(&PSSE).unwrap();
        assert_eq!(merge.groups.len(), 1);
        assert_eq!(merge.groups[0].members.len(), 4);
        assert_eq!(merge.removed_branches.len(), 3);
        assert_eq!(merge.branch_rows, vec![None, None, None, Some(0)]);
        let kept = &merge.network.branches()[0];
        assert_eq!((kept.from, kept.to), (BusId(1), BusId(5)));
        let summary = merge
            .diagnostics
            .iter()
            .find(|d| d.code() == "CANONICALIZE.MERGE.ZERO_IMPEDANCE")
            .unwrap();
        assert_eq!(summary.details()["count"], 3);
    }

    #[test]
    fn the_psse_threshold_skips_resistive_jumpers_and_transformers() {
        let mut transformer = jumper(3, 4);
        transformer.tap = 1.0;
        let network = net(
            &[1, 2, 3, 4],
            vec![line(1, 2, 1e-5, 5e-5), line(2, 3, 0.01, 0.1), transformer],
        );
        let merge = network.merge_buses(&PSSE).unwrap();
        assert!(
            merge.is_identity(),
            "R > 0 and transformers are not PSS/E jumpers"
        );

        let wider = BusMergeRule::new(false, Some(ZeroImpedanceRule::ImpedanceMagnitude(1e-3)));
        let merge = network.merge_buses(&wider).unwrap();
        assert_eq!(merge.merged_buses, BTreeMap::from([(BusId(2), BusId(1))]));
        assert_eq!(merge.removed_branches.len(), 1, "the transformer stays");
    }

    #[test]
    fn a_zero_threshold_disables_the_psse_rule() {
        let network = net(&[1, 2], vec![line(1, 2, 0.0, 0.0)]);
        let off = BusMergeRule::new(false, Some(ZeroImpedanceRule::PsseThreshold(0.0)));
        assert!(network.merge_buses(&off).unwrap().is_identity());
        let exact = network.merge_buses(&BusMergeRule::exact()).unwrap();
        assert_eq!(exact.removed_branches.len(), 1);
    }

    #[test]
    fn the_exact_rule_keeps_an_off_nominal_ideal_transformer() {
        let mut ideal = line(1, 2, 0.0, 0.0);
        ideal.tap = 1.05;
        let mut nominal = line(2, 3, 0.0, 0.0);
        nominal.tap = 1.0;
        let network = net(&[1, 2, 3], vec![ideal, nominal]);
        let merge = network.merge_buses(&BusMergeRule::exact()).unwrap();
        assert_eq!(merge.merged_buses, BTreeMap::from([(BusId(3), BusId(2))]));
    }

    #[test]
    fn the_survivor_is_the_reference_then_a_generator_then_a_regulated_bus() {
        // Bus 1 is plain, 2 is regulated by the generator on 5, 3 hosts a
        // generator, 4 is the reference.
        let chain = |ids: &[usize]| {
            let mut network = net(ids, ids.windows(2).map(|w| jumper(w[0], w[1])).collect());
            network.generators_mut().push(Generator::new(BusId(5)));
            network.generators_mut()[0].regulated_bus = Some(BusId(2));
            network
        };
        let mut network = chain(&[1, 2, 3, 4, 5]);
        network.generators_mut().push(Generator::new(BusId(3)));
        network.buses_mut()[3].kind = BusType::Ref;
        let merge = network.merge_buses(&PSSE).unwrap();
        assert_eq!(merge.groups[0].survivor, BusId(4));
        let survivor = &merge.network.buses()[0];
        assert_eq!((survivor.id, survivor.kind), (BusId(4), BusType::Ref));

        let mut network = chain(&[1, 2, 3, 6]);
        network.generators_mut()[0].bus = BusId(6);
        network.generators_mut().push(Generator::new(BusId(3)));
        network.generators_mut()[1].in_service = true;
        let merge = network.merge_buses(&PSSE).unwrap();
        assert_eq!(
            merge.groups[0].survivor,
            BusId(3),
            "generator over lower id"
        );

        let mut network = chain(&[1, 2, 3, 5]);
        network.generators_mut()[0].in_service = true;
        network.generators_mut()[0].bus = BusId(7);
        network
            .buses_mut()
            .push(Bus::new(BusId(7), BusType::Pv, 230.0));
        let merge = network.merge_buses(&PSSE).unwrap();
        assert_eq!(
            merge.groups[0].survivor,
            BusId(2),
            "regulated over lower id"
        );

        let network = net(&[3, 1, 2], vec![jumper(3, 1), jumper(1, 2)]);
        let merge = network.merge_buses(&PSSE).unwrap();
        assert_eq!(merge.groups[0].survivor, BusId(1), "lowest id");
    }

    #[test]
    fn every_reference_follows_the_survivor() {
        let mut network = net(
            &[1, 2, 3, 4, 5, 6],
            vec![jumper(1, 2), line(2, 3, 0.01, 0.1), line(3, 4, 0.01, 0.1)],
        );
        network.buses_mut()[0].kind = BusType::Ref;
        let mut regulating = Generator::new(BusId(3));
        regulating.regulated_bus = Some(BusId(2));
        network.generators_mut().push(regulating);
        network.loads_mut().push(Load::new(BusId(2), 10.0, 1.0));
        let mut shunt = Shunt::new(BusId(3), 0.0, 10.0);
        let mut control =
            SwitchedShuntControl::new(SwitchedShuntMode::Discrete, 1.05, 0.95, vec![]);
        control.control_bus = Some(BusId(2));
        shunt.control = Some(control);
        network.shunts_mut().push(shunt);
        let mut tap_control = TransformerControl::new(TransformerControlMode::Voltage);
        tap_control.controlled_bus = Some(BusId(2));
        network.branches_mut()[2].tap = 1.0;
        network.branches_mut()[2].control = Some(tap_control);
        network.hvdc_mut().push(Hvdc::new(BusId(2), BusId(5)));
        network
            .transformers_3w_mut()
            .push(transformer_3w([2, 5, 6]));
        let mut area = Area::new(1);
        area.slack_bus = Some(BusId(2));
        network.areas_mut().push(area);

        let merge = network.merge_buses(&PSSE).unwrap();
        let merged = &merge.network;
        assert_eq!(merge.merged_buses, BTreeMap::from([(BusId(2), BusId(1))]));
        assert_eq!(merged.generators()[0].regulated_bus, Some(BusId(1)));
        assert_eq!(merged.loads()[0].bus, BusId(1));
        let control = merged.shunts()[0].control.as_ref().unwrap();
        assert_eq!(control.control_bus, Some(BusId(1)));
        let tap = merged.branches()[1].control.as_ref().unwrap();
        assert_eq!(tap.controlled_bus, Some(BusId(1)));
        assert_eq!(merged.branches()[0].from, BusId(1));
        assert_eq!(merged.hvdc()[0].from, BusId(1));
        assert_eq!(merged.transformers_3w()[0].windings[0].bus, BusId(1));
        assert_eq!(merged.areas()[0].slack_bus, Some(BusId(1)));
        merged.validate().unwrap();
        // The input is untouched.
        assert_eq!(network.loads()[0].bus, BusId(2));
    }

    #[test]
    fn a_jumper_across_two_windings_of_one_transformer_is_kept() {
        let mut network = net(
            &[1, 2, 3, 4],
            vec![jumper(1, 2), jumper(3, 4), jumper(2, 4)],
        );
        // Windings on 1 and 3: the 1-2 and 3-4 jumpers merge, the 2-4 jumper
        // would then join both windings.
        network
            .transformers_3w_mut()
            .push(transformer_3w([1, 3, 4]));
        let merge = network.merge_buses(&PSSE).unwrap();
        assert_eq!(
            merge.groups.len(),
            1,
            "only 1-2 merges; 3-4 joins two windings"
        );
        assert_eq!(merge.network.branches().len(), 2);
        let kept: Vec<&Diagnostic> = merge
            .diagnostics
            .iter()
            .filter(|d| d.code() == "CANONICALIZE.MERGE.WINDING_PAIR_KEPT")
            .collect();
        assert_eq!(kept.len(), 2, "{:#?}", merge.diagnostics);
        merge.network.validate().unwrap();
    }

    #[test]
    fn merging_the_result_again_joins_nothing() {
        let mut network = net(
            &[1, 2, 3, 4, 5],
            vec![
                jumper(1, 2),
                jumper(2, 3),
                line(3, 4, 0.01, 0.1),
                jumper(4, 5),
            ],
        );
        network
            .switches_mut()
            .push(Switch::new(BusId(5), BusId(1), true));
        network
            .transformers_3w_mut()
            .push(transformer_3w([1, 3, 5]));
        let once = network.merge_buses(&PSSE).unwrap();
        assert!(!once.is_identity());
        let twice = once.network.merge_buses(&PSSE).unwrap();
        assert!(twice.is_identity());
        assert_eq!(twice.network.buses(), once.network.buses());
    }

    #[test]
    fn the_psse_rule_reads_the_stated_threshold() {
        let mut network = net(&[1, 2], vec![line(1, 2, 0.0, 3e-4)]);
        let error = BusMergeRule::psse(&network).unwrap_err();
        assert_eq!(error.code().code, "CANONICALIZE.MERGE.INVALID_RULE");

        let mut solver = SolverParams::new();
        solver.zero_impedance_threshold = Some(5e-4);
        *network.solver_mut() = Some(solver);
        let rule = BusMergeRule::psse(&network).unwrap();
        assert_eq!(
            rule,
            BusMergeRule::new(true, Some(ZeroImpedanceRule::PsseThreshold(5e-4)))
        );
        assert_eq!(
            network.merge_buses(&rule).unwrap().removed_branches.len(),
            1
        );

        for threshold in [-1e-4, f64::NAN, f64::INFINITY] {
            let rule = BusMergeRule::new(false, Some(ZeroImpedanceRule::PsseThreshold(threshold)));
            assert!(network.merge_buses(&rule).is_err(), "{threshold}");
        }
    }

    #[test]
    fn differing_base_voltages_and_two_references_are_reported() {
        let mut network = net(&[1, 2, 3], vec![jumper(1, 2), jumper(2, 3)]);
        network.buses_mut()[1].base_kv = 34.5;
        network.buses_mut()[0].kind = BusType::Ref;
        network.buses_mut()[2].kind = BusType::Ref;
        let merge = network.merge_buses(&PSSE).unwrap();
        let conflicts: Vec<&str> = merge
            .diagnostics
            .iter()
            .filter(|d| d.code() == "CANONICALIZE.MERGE.ATTRIBUTE_CONFLICT")
            .map(Diagnostic::message)
            .collect();
        assert_eq!(conflicts.len(), 2, "{conflicts:?}");
        assert!(conflicts[0].contains("34.5 kV"));
        assert!(conflicts[1].contains("reference buses 1, 3"));
        assert_eq!(merge.network.buses()[0].kind, BusType::Ref);
    }

    #[test]
    fn a_line_in_parallel_with_a_jumper_is_shorted_and_an_hvdc_line_stays() {
        let mut network = net(&[1, 2, 3], vec![jumper(1, 2), line(1, 2, 0.01, 0.1)]);
        network.hvdc_mut().push(Hvdc::new(BusId(1), BusId(2)));
        network
            .switches_mut()
            .push(Switch::new(BusId(2), BusId(1), false));
        let merge = network.merge_buses(&PSSE).unwrap();
        let reasons: Vec<RemovalReason> = merge
            .removed_branches
            .iter()
            .map(|removed| removed.reason)
            .collect();
        assert_eq!(
            reasons,
            [RemovalReason::ZeroImpedance, RemovalReason::Shorted]
        );
        assert_eq!(merge.removed_switches[0].reason, RemovalReason::Shorted);
        assert_eq!(merge.network.hvdc().len(), 1);
        let shorted = codes_of(&merge.diagnostics)
            .into_iter()
            .filter(|code| *code == "CANONICALIZE.MERGE.ELEMENT_SHORTED")
            .count();
        assert_eq!(shorted, 3);
    }

    #[test]
    fn detailed_connectivity_follows_the_survivor() {
        use crate::network::{DetailedConnectivity, TopologyKind, VoltageLevel};
        let mut network = net(&[1, 2, 3], vec![jumper(1, 2), line(2, 3, 0.01, 0.1)]);
        let level = |name: &str, buses: Vec<BusId>| VoltageLevel {
            component: powerio_core::ComponentId::new("voltage_level", name).unwrap(),
            substation: None,
            nominal_kv: 230.0,
            low_voltage_limit_kv: None,
            high_voltage_limit_kv: None,
            topology_kind: TopologyKind::BusBreaker,
            buses,
        };
        let detailed = DetailedConnectivity {
            voltage_levels: vec![
                level("A", vec![BusId(1), BusId(2)]),
                level("B", vec![BusId(3)]),
            ],
            ..DetailedConnectivity::default()
        };
        *network.detailed_connectivity_mut() = Some(std::sync::Arc::new(detailed));
        network.validate().unwrap();

        let merge = network.merge_buses(&PSSE).unwrap();
        let levels = &merge
            .network
            .detailed_connectivity()
            .as_deref()
            .unwrap()
            .voltage_levels;
        assert_eq!(levels[0].buses, vec![BusId(1)]);
        assert!(!codes_of(&merge.diagnostics).contains(&"CANONICALIZE.MERGE.DETAIL_DROPPED"));

        // A jumper across the two levels leaves one bus in both, so the
        // calculated bus assignments go.
        network.branches_mut()[1] = jumper(2, 3);
        let merge = network.merge_buses(&PSSE).unwrap();
        let detailed = merge.network.detailed_connectivity().as_deref().unwrap();
        assert!(
            detailed
                .voltage_levels
                .iter()
                .all(|level| level.buses.is_empty())
        );
        assert!(codes_of(&merge.diagnostics).contains(&"CANONICALIZE.MERGE.DETAIL_DROPPED"));
        merge.network.validate().unwrap();
    }

    /// Bus 1 feeds a group {2, 3, 4} through a 1-2 line; loads sit on 3
    /// and 4.
    fn fed_group(extra: Vec<Branch>) -> BalancedNetwork {
        let mut branches = vec![line(1, 2, 0.01, 0.1)];
        branches.extend(extra);
        let mut network = net(&[1, 2, 3, 4], branches);
        network.buses_mut()[0].kind = BusType::Ref;
        network.generators_mut().push(Generator::new(BusId(1)));
        network.loads_mut().push(Load::new(BusId(3), 30.0, 0.0));
        network.loads_mut().push(Load::new(BusId(4), 50.0, 0.0));
        network
    }

    #[test]
    fn removed_flows_follow_kirchhoff_on_a_tree() {
        let network = fed_group(vec![jumper(2, 3), jumper(3, 4)]);
        let merge = network.merge_buses(&PSSE).unwrap();
        // The merged 1-2 line carries the group's 80 MW.
        let flows = merge
            .calc_removed_flows(&MergedFlows::new(&[80.0], &[-80.0]))
            .unwrap();
        let p: Vec<f64> = flows.branches.iter().map(|flow| flow.p_from).collect();
        assert_eq!(p, [80.0, 50.0]);
        assert!(
            flows
                .branches
                .iter()
                .all(|flow| flow.method == RemovedFlowMethod::Tree)
        );
        assert!(flows.diagnostics.is_empty(), "{:#?}", flows.diagnostics);
    }

    #[test]
    fn removed_flows_split_a_loop_by_reactance() {
        // Two parallel jumpers 2-3 (x 1e-5 and 3e-5) and a jumper 3-4.
        let network = fed_group(vec![
            line(2, 3, 0.0, 1e-5),
            line(3, 2, 0.0, 3e-5),
            jumper(3, 4),
        ]);
        let merge = network.merge_buses(&PSSE).unwrap();
        let flows = merge
            .calc_removed_flows(&MergedFlows::new(&[80.0], &[-80.0]))
            .unwrap();
        let p: Vec<f64> = flows.branches.iter().map(|flow| flow.p_from).collect();
        assert!((p[0] - 60.0).abs() < 1e-9, "{p:?}");
        assert!(
            (p[1] + 20.0).abs() < 1e-9,
            "the reversed jumper reads negative: {p:?}"
        );
        assert!((p[2] - 50.0).abs() < 1e-9, "{p:?}");
        assert_eq!(flows.branches[0].method, RemovedFlowMethod::Reactance);
        assert_eq!(flows.branches[2].method, RemovedFlowMethod::Tree);
    }

    #[test]
    fn a_loop_of_switches_is_split_by_minimum_norm() {
        let mut network = fed_group(vec![jumper(3, 4)]);
        network.switches_mut().extend([
            Switch::new(BusId(2), BusId(3), true),
            Switch::new(BusId(2), BusId(3), true),
        ]);
        let merge = network.merge_buses(&PSSE).unwrap();
        let flows = merge
            .calc_removed_flows(&MergedFlows::new(&[80.0], &[-80.0]))
            .unwrap();
        assert_eq!(flows.branches[0].p_from, 50.0);
        for flow in &flows.switches {
            assert!((flow.p_from - 40.0).abs() < 1e-9);
            assert_eq!(flow.method, RemovedFlowMethod::MinimumNorm);
        }
        assert_eq!(
            codes_of(&flows.diagnostics),
            ["CANONICALIZE.MERGE.FLOW_UNDETERMINED"]
        );
    }

    #[test]
    fn a_zero_reactance_element_carries_its_parallel_paths_share() {
        // A closed switch 2-3 in parallel with a jumper 2-3: the switch takes
        // everything, and the jumper none.
        let mut network = fed_group(vec![jumper(2, 3), jumper(3, 4)]);
        network
            .switches_mut()
            .push(Switch::new(BusId(2), BusId(3), true));
        let merge = network.merge_buses(&PSSE).unwrap();
        let flows = merge
            .calc_removed_flows(&MergedFlows::new(&[80.0], &[-80.0]))
            .unwrap();
        assert_eq!(flows.branches[0].p_from, 0.0);
        assert_eq!(flows.switches[0].p_from, 80.0);
        assert_eq!(flows.switches[0].method, RemovedFlowMethod::Tree);
    }

    #[test]
    fn an_unbalanced_group_and_a_wrong_shape_are_reported() {
        let mut network = fed_group(vec![jumper(2, 3), jumper(3, 4)]);
        network.buses_mut()[0].kind = BusType::Pq;
        let merge = network.merge_buses(&PSSE).unwrap();
        let flows = merge
            .calc_removed_flows(&MergedFlows::new(&[70.0], &[-70.0]))
            .unwrap();
        assert_eq!(
            codes_of(&flows.diagnostics),
            ["CANONICALIZE.MERGE.FLOW_RESIDUAL"]
        );
        let error = merge
            .calc_removed_flows(&MergedFlows::new(&[], &[]))
            .unwrap_err();
        assert_eq!(error.code().code, "CANONICALIZE.MERGE.FLOW_SHAPE_MISMATCH");
    }

    #[test]
    fn out_of_service_elements_carry_no_flow() {
        let mut network = fed_group(vec![jumper(2, 3), jumper(3, 4)]);
        let mut open = line(2, 4, 0.01, 0.1);
        open.in_service = false;
        network.branches_mut().push(open);
        let merge = network.merge_buses(&PSSE).unwrap();
        let flows = merge
            .calc_removed_flows(&MergedFlows::new(&[80.0], &[-80.0]))
            .unwrap();
        assert_eq!(flows.branches[2].method, RemovedFlowMethod::OutOfService);
        assert_eq!(flows.branches[2].p_from, 0.0);
    }
}

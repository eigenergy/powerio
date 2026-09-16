//! Binding the cases of a `.con` file to the elements of a network.
//!
//! A `.con` statement names an element the way a PSS/E RAW file does: a bus
//! number, a machine id, a circuit id. A [`BalancedNetwork`] names an element
//! by its `uid`, which is a source identity or one PowerIO generated from bus
//! numbers (`bus-4`, `3-1`) and so carries no machine or circuit id.
//! [`PsseEquipmentIndex`] closes that gap by recomputing, for every element,
//! the id a RAW file written from this network would state, using the writer's
//! own allocation. A statement therefore binds to the element PSS/E would
//! address by the same words.
//!
//! [`ContingencySet::resolve`] is total: every action either binds to network
//! elements or is kept with a structured [`UnresolvedReason`]. A case holding
//! any unresolved action is counted unresolved and reported as
//! `BUILD.CON.CASE_UNRESOLVED`; the actions of that case that did bind stay
//! listed, so a caller can see how far a case got.

use std::collections::BTreeMap;

use powerio_core::ComponentId;

use crate::diagnostics::{Diagnostic, codes};
use crate::format::psse::{
    detailed_source_property, quoted_circuit_id, quoted_device_id, transformer_3w_id,
};
use crate::network::{BalancedNetwork, BusId, BusType};

use super::{ContingencyAction, ContingencySet, MAX_READER_NOTES};

/// Component type strings, the same spellings the update resolver in
/// `powerio-prob` requires of a [`ComponentId`], and the ones a
/// [`ResolvedComponent`] states for the table its row indexes.
const BUS: &str = "bus";
const LOAD: &str = "load";
const SHUNT: &str = "shunt";
const GENERATOR: &str = "generator";
const BRANCH: &str = "branch";
const TRANSFORMER_3W: &str = "transformer_3w";

/// The PSS/E id every element of a network would carry in a RAW file written
/// from it, with the lookups a `.con` statement needs.
///
/// The ids come from the RAW writer's own allocation: an element's retained id
/// when it has one and that id is still free on its key, compared trimmed, else
/// the lowest positive integer still free there. That is why an id the reader
/// dropped as a positional default (`1`) comes back, and why parallel elements
/// stay distinct.
///
/// Building the index walks every table once. Resolving many sets against one
/// network builds it once and calls [`ContingencySet::resolve_with`]. The index
/// borrows the network it was built from, so a row it states is always a row of
/// that network.
#[derive(Debug, Clone)]
pub struct PsseEquipmentIndex<'n> {
    net: &'n BalancedNetwork,
    machine_ids: Vec<String>,
    circuit_ids: Vec<String>,
    transformer_3w_ids: Vec<String>,
    bus_rows: BTreeMap<BusId, usize>,
    /// Keyed on the stored terminal order, as the writer keys it. A lookup
    /// reads both orientations.
    branch_rows: BranchRows,
    machine_rows: MachineRows,
    fixed_shunt_rows: DeviceRows,
    switched_shunt_rows: BTreeMap<BusId, Vec<usize>>,
    load_rows: DeviceRows,
    /// Keyed on the three bus ids in ascending order, so a statement naming
    /// them in any order finds the transformer.
    transformer_3w_rows: Transformer3wRows,
}

fn sorted_triple(buses: [BusId; 3]) -> [BusId; 3] {
    let mut sorted = buses;
    sorted.sort_unstable();
    sorted
}

/// Rows of one device family keyed by bus, each with the trimmed id the RAW
/// writer would state for it. PSS/E reads a quoted id by its trimmed text, as
/// the `.con` reader and the RAW reader both do, and the writer's allocation
/// gives two rows on one bus two trimmed ids, so a trimmed id names one row.
type DeviceRows = BTreeMap<BusId, Vec<(String, usize)>>;

/// Branch rows keyed by the stored terminal pair, whether the branch is a two
/// winding transformer, and the circuit id.
///
/// The RAW writer allocates the line ids and the transformer ids in separate
/// namespaces, so a line and a two winding transformer on the same terminal
/// pair both take circuit `1`; keying the two families apart keeps each row
/// reachable. Parallel branches of one family stored in opposite terminal
/// orders can still take the same circuit id, so a key holds a list.
type BranchRows = BTreeMap<(BusId, BusId, bool, String), Vec<usize>>;

/// Rows of the generators, keyed by bus and machine id.
type MachineRows = BTreeMap<(BusId, String), usize>;

/// Three winding transformer rows keyed by their three buses in ascending
/// order, each with the id the RAW writer would state.
type Transformer3wRows = BTreeMap<[BusId; 3], Vec<(String, usize)>>;

/// The machine id of every generator, in table order, and the row each
/// `(bus, id)` pair names. PSS/E requires machine ids to be unique on a bus,
/// compared by their trimmed text, and the writer's allocation preserves that,
/// so one pair names one row.
///
/// The key is the trimmed id the writer allocates, which is the id a RAW file
/// states and a `.con` statement names.
fn machine_index(net: &BalancedNetwork, sanitized: &mut usize) -> (Vec<String>, MachineRows) {
    let mut ids = Vec::with_capacity(net.generators().len());
    let mut rows = BTreeMap::new();
    let mut used = BTreeMap::new();
    for (row, generator) in net.generators().iter().enumerate() {
        let preferred =
            detailed_source_property(net, GENERATOR, generator.uid.as_deref(), "psse_eqid")
                .filter(|id| !id.is_empty());
        let id = quoted_circuit_id(preferred, generator.bus, &mut used, sanitized);
        rows.insert((generator.bus, id.trim().to_owned()), row);
        ids.push(id);
    }
    (ids, rows)
}

/// The circuit id of every branch, aligned with `net.branches()`, and the rows
/// each `(from, to, circuit)` key names.
///
/// The writer states the lines first and the two winding transformers after
/// them, each family with its own id allocation, both keyed on the stored
/// terminal pair. Walking the table twice reproduces that order while keeping
/// the ids aligned with the table.
fn branch_index(net: &BalancedNetwork, sanitized: &mut usize) -> (Vec<String>, BranchRows) {
    let mut ids = vec![String::new(); net.branches().len()];
    let mut rows: BranchRows = BTreeMap::new();
    let mut line_ids = BTreeMap::new();
    let mut transformer_ids = BTreeMap::new();
    for transformers in [false, true] {
        for (row, branch) in net.branches().iter().enumerate() {
            if branch.is_transformer() != transformers {
                continue;
            }
            let retained = transformers
                .then(|| {
                    detailed_source_property(net, "transformer", branch.uid.as_deref(), "psse_eqid")
                })
                .flatten();
            let preferred = branch
                .extras
                .get("id")
                .and_then(serde_json::Value::as_str)
                .or(retained);
            let used = if transformers {
                &mut transformer_ids
            } else {
                &mut line_ids
            };
            let id = quoted_circuit_id(preferred, (branch.from, branch.to), used, sanitized);
            rows.entry((branch.from, branch.to, transformers, id.trim().to_owned()))
                .or_default()
                .push(row);
            ids[row] = id;
        }
    }
    (ids, rows)
}

/// The rows of `net.loads()` per bus, with the id each load would carry.
fn load_index(net: &BalancedNetwork, sanitized: &mut usize) -> DeviceRows {
    let mut rows: DeviceRows = BTreeMap::new();
    let mut used = BTreeMap::new();
    for (row, load) in net.loads().iter().enumerate() {
        let id = quoted_device_id(&load.extras, load.bus, &mut used, sanitized);
        rows.entry(load.bus)
            .or_default()
            .push((id.trim().to_owned(), row));
    }
    rows
}

/// The fixed and switched shunt rows per bus.
///
/// Both families are one table here and two sections in a RAW file, with their
/// own id allocations, and only the fixed shunt record states an id. The rows
/// are positions in `net.shunts()`, the table a shunt `ComponentId` names.
fn shunt_index(
    net: &BalancedNetwork,
    sanitized: &mut usize,
) -> (DeviceRows, BTreeMap<BusId, Vec<usize>>) {
    let mut fixed: DeviceRows = BTreeMap::new();
    let mut switched: BTreeMap<BusId, Vec<usize>> = BTreeMap::new();
    let mut used = BTreeMap::new();
    for (row, shunt) in net.shunts().iter().enumerate() {
        if shunt.control.is_some() {
            switched.entry(shunt.bus).or_default().push(row);
            continue;
        }
        let id = quoted_device_id(&shunt.extras, shunt.bus, &mut used, sanitized);
        fixed
            .entry(shunt.bus)
            .or_default()
            .push((id.trim().to_owned(), row));
    }
    (fixed, switched)
}

/// The three winding transformer rows keyed on their three buses in ascending
/// order, so a statement naming the buses in any order finds the transformer.
fn transformer_3w_index(
    net: &BalancedNetwork,
    sanitized: &mut usize,
) -> (Vec<String>, Transformer3wRows) {
    let mut ids = Vec::with_capacity(net.transformers_3w().len());
    let mut rows: Transformer3wRows = BTreeMap::new();
    let mut used = BTreeMap::new();
    for (row, transformer) in net.transformers_3w().iter().enumerate() {
        let id = transformer_3w_id(net, transformer, &mut used, sanitized);
        let buses = sorted_triple([
            transformer.windings[0].bus,
            transformer.windings[1].bus,
            transformer.windings[2].bus,
        ]);
        rows.entry(buses)
            .or_default()
            .push((id.trim().to_owned(), row));
        ids.push(id);
    }
    (ids, rows)
}

impl<'n> PsseEquipmentIndex<'n> {
    /// Recompute every PSS/E id in `net` and index the rows by it.
    #[must_use]
    pub fn new(net: &'n BalancedNetwork) -> Self {
        // The writer counts the ids sanitation changed so it can warn about
        // them; the index states the same ids and discards the count.
        let mut sanitized = 0usize;
        let (machine_ids, machine_rows) = machine_index(net, &mut sanitized);
        let (circuit_ids, branch_rows) = branch_index(net, &mut sanitized);
        let (fixed_shunt_rows, switched_shunt_rows) = shunt_index(net, &mut sanitized);
        let (transformer_3w_ids, transformer_3w_rows) = transformer_3w_index(net, &mut sanitized);
        Self {
            net,
            machine_ids,
            circuit_ids,
            transformer_3w_ids,
            bus_rows: net
                .buses()
                .iter()
                .enumerate()
                .map(|(row, bus)| (bus.id, row))
                .collect(),
            branch_rows,
            machine_rows,
            fixed_shunt_rows,
            switched_shunt_rows,
            load_rows: load_index(net, &mut sanitized),
            transformer_3w_rows,
        }
    }

    /// The network the index was built from. Every row it states indexes a
    /// table of this network.
    #[must_use]
    pub fn network(&self) -> &'n BalancedNetwork {
        self.net
    }

    /// The machine id of every generator, aligned with `net.generators()`.
    #[must_use]
    pub fn machine_ids(&self) -> &[String] {
        &self.machine_ids
    }

    /// The circuit id of every branch, aligned with `net.branches()`.
    #[must_use]
    pub fn circuit_ids(&self) -> &[String] {
        &self.circuit_ids
    }

    /// The circuit id of every three winding transformer, aligned with
    /// `net.transformers_3w()`.
    #[must_use]
    pub fn transformer_3w_ids(&self) -> &[String] {
        &self.transformer_3w_ids
    }

    /// The row of `bus` in `net.buses()`.
    #[must_use]
    pub fn bus_row(&self, bus: BusId) -> Option<usize> {
        self.bus_rows.get(&bus).copied()
    }

    /// The rows of `net.branches()` joining `from` and `to` on `circuit`, in
    /// either orientation. A self-loop is counted once. More than one row
    /// means the statement names several branches and binds to none of them.
    ///
    /// The lines answer first and the two winding transformers only when no
    /// line carries the circuit id, because the RAW writer allocates the two
    /// families apart and a `.con` statement names a branch without saying
    /// which table it sits in.
    #[must_use]
    pub fn branch_rows(&self, from: BusId, to: BusId, circuit: &str) -> Vec<usize> {
        let lines = self.family_rows(from, to, circuit, false);
        if lines.is_empty() {
            self.family_rows(from, to, circuit, true)
        } else {
            lines
        }
    }

    /// The rows of one branch family joining `from` and `to` on `circuit`, in
    /// either orientation.
    fn family_rows(&self, from: BusId, to: BusId, circuit: &str, transformer: bool) -> Vec<usize> {
        let circuit = circuit.trim();
        let mut rows = Vec::new();
        if let Some(forward) = self
            .branch_rows
            .get(&(from, to, transformer, circuit.to_owned()))
        {
            rows.extend_from_slice(forward);
        }
        if from != to
            && let Some(reverse) =
                self.branch_rows
                    .get(&(to, from, transformer, circuit.to_owned()))
        {
            rows.extend_from_slice(reverse);
        }
        rows
    }

    /// The row of `net.generators()` for machine `id` at `bus`. PSS/E requires
    /// machine ids to be unique on a bus and the writer's allocation preserves
    /// that, so at most one row matches. `id` is matched trimmed, against the
    /// trimmed id the writer allocates.
    #[must_use]
    pub fn machine_row(&self, bus: BusId, id: &str) -> Option<usize> {
        self.machine_rows.get(&(bus, id.trim().to_owned())).copied()
    }

    /// The rows of `net.shunts()` holding a fixed shunt at `bus`: the one with
    /// this id, or every fixed shunt there when no id is stated.
    #[must_use]
    pub fn fixed_shunt_rows(&self, bus: BusId, id: Option<&str>) -> Vec<usize> {
        select_rows(self.fixed_shunt_rows.get(&bus), id)
    }

    /// The rows of `net.shunts()` holding a switched shunt at `bus`, every one
    /// of them. The `.con` grammar states `REMOVE SWSHUNT FROM BUS i` and
    /// carries no id, as the RAW switched shunt record itself does not, so the
    /// statement addresses every switched shunt at the bus.
    #[must_use]
    pub fn switched_shunt_rows(&self, bus: BusId) -> Vec<usize> {
        self.switched_shunt_rows
            .get(&bus)
            .cloned()
            .unwrap_or_default()
    }

    /// The rows of `net.loads()` at `bus`: the one with this id, or every load
    /// there when no id is stated.
    #[must_use]
    pub fn load_rows(&self, bus: BusId, id: Option<&str>) -> Vec<usize> {
        select_rows(self.load_rows.get(&bus), id)
    }

    /// The row of `net.transformers_3w()` on these three buses and circuit id.
    /// The buses match in any order, because a `.con` statement need not state
    /// them in winding order. When several transformers on the same three
    /// buses carry the same id, the first in table order is returned;
    /// [`PsseEquipmentIndex::transformer_3w_rows`] states all of them.
    #[must_use]
    pub fn transformer_3w_row(&self, buses: [BusId; 3], circuit: &str) -> Option<usize> {
        self.transformer_3w_rows(buses, circuit).first().copied()
    }

    /// The rows of `net.transformers_3w()` on these three buses and circuit
    /// id, in table order. The buses match in any order. More than one row
    /// means the statement names several transformers and binds to none of
    /// them, which happens when two transformers on the same three buses are
    /// stored in different winding orders and take the same id.
    #[must_use]
    pub fn transformer_3w_rows(&self, buses: [BusId; 3], circuit: &str) -> Vec<usize> {
        let circuit = circuit.trim();
        self.transformer_3w_rows
            .get(&sorted_triple(buses))
            .into_iter()
            .flatten()
            .filter(|(id, _)| id == circuit)
            .map(|(_, row)| *row)
            .collect()
    }
}

/// The rows of one bus's devices whose id matches, or all of them when the
/// statement names no id. An id matches trimmed, against the trimmed id the
/// writer allocates.
fn select_rows(at_bus: Option<&Vec<(String, usize)>>, id: Option<&str>) -> Vec<usize> {
    let Some(devices) = at_bus else {
        return Vec::new();
    };
    match id {
        None => devices.iter().map(|(_, row)| *row).collect(),
        Some(wanted) => {
            let wanted = wanted.trim();
            devices
                .iter()
                .filter(|(id, _)| id == wanted)
                .map(|(_, row)| *row)
                .collect()
        }
    }
}

/// What a whole contingency set bound to.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ContingencyResolution {
    /// One entry per case of the set, in the set's order.
    pub cases: Vec<ResolvedCase>,
    /// Cases whose every action bound.
    pub resolved: usize,
    /// Cases holding at least one action that did not bind.
    pub unresolved: usize,
    /// Actions the reader kept as text, counted over every case.
    pub unrecognized_statements: usize,
}

impl ContingencyResolution {
    /// One `BUILD.CON.CASE_UNRESOLVED` note per unresolved case, naming the
    /// case and the first action of it that did not bind.
    ///
    /// The notes stop at the reader's budget: the first case past it records
    /// one `BUILD.CON.NOTES_TRUNCATED` in place of its note and the cases
    /// after that record nothing, so a set resolved against the wrong network
    /// cannot grow the note list without limit. Every case is still counted.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut notes = Vec::new();
        for case in self.cases.iter().filter(|case| !case.is_resolved()) {
            if notes.len() == MAX_READER_NOTES {
                notes.push(Diagnostic::of(
                    &codes::BUILD_CON_NOTES_TRUNCATED,
                    "further resolution notes suppressed",
                ));
                break;
            }
            let first = &case.unresolved[0];
            notes.push(Diagnostic::of(
                &codes::BUILD_CON_CASE_UNRESOLVED,
                format!(
                    "contingency '{}': {}",
                    case.name,
                    describe(&first.action, first.reason)
                ),
            ));
        }
        notes
    }
}

/// What one case bound to.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ResolvedCase {
    pub name: String,
    /// The elements the case's actions bound to, in action order. An action
    /// naming several elements contributes all of them.
    pub components: Vec<ResolvedComponent>,
    /// The actions that bound to nothing, with the reason each one did not.
    pub unresolved: Vec<UnresolvedAction>,
}

impl ResolvedCase {
    /// Whether every action of the case bound.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.unresolved.is_empty()
    }
}

/// One network element a case's action bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedComponent {
    /// The component type naming the table `row` indexes: `bus`, `load`,
    /// `shunt`, `generator`, `branch`, or `transformer_3w`. Every component
    /// states it, including one the network states no identity for.
    pub component_type: &'static str,
    /// The element's identity, when the network states one for the row: its
    /// `uid`, under `component_type`. A row carrying no `uid`, or one
    /// [`ComponentId`] does not accept, states `None`. A caller that needs
    /// persistent identities calls `assign_missing_component_ids` on the
    /// network before building the index, which gives every row a `uid`.
    pub id: Option<ComponentId>,
    /// The element's position in the table `component_type` names.
    pub row: usize,
    /// The element's own in service flag as the network states it now, before
    /// the case is applied. For a bus it is whether the bus type is anything
    /// other than isolated.
    pub in_service: bool,
}

/// One action that bound to nothing, kept with the reason.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct UnresolvedAction {
    pub action: ContingencyAction,
    pub reason: UnresolvedReason,
}

/// Why an action bound to nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnresolvedReason {
    NoSuchBus,
    NoSuchBranch,
    /// The terminal pair and circuit id name more than one branch, so the
    /// statement does not say which one opens.
    AmbiguousBranch {
        matches: usize,
    },
    /// The three buses and circuit id name more than one three winding
    /// transformer, so the statement does not say which one opens.
    AmbiguousTransformer3w {
        matches: usize,
    },
    NoSuchMachine,
    NoSuchShunt,
    NoSuchLoad,
    NoSuchTransformer3w,
    /// The reader kept this statement as text, so it names no element.
    Unrecognized,
}

impl UnresolvedReason {
    /// The snake_case name of the reason, for reports and bindings.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::NoSuchBus => "no_such_bus",
            Self::NoSuchBranch => "no_such_branch",
            Self::AmbiguousBranch { .. } => "ambiguous_branch",
            Self::AmbiguousTransformer3w { .. } => "ambiguous_transformer_3w",
            Self::NoSuchMachine => "no_such_machine",
            Self::NoSuchShunt => "no_such_shunt",
            Self::NoSuchLoad => "no_such_load",
            Self::NoSuchTransformer3w => "no_such_transformer_3w",
            Self::Unrecognized => "unrecognized",
        }
    }
}

/// A one line account of an action that did not bind.
fn describe(action: &ContingencyAction, reason: UnresolvedReason) -> String {
    match (action, reason) {
        (ContingencyAction::OpenBranch { from, to, circuit }, UnresolvedReason::NoSuchBranch) => {
            format!("no branch {from} to {to} circuit {circuit}")
        }
        (
            ContingencyAction::OpenBranch { from, to, circuit },
            UnresolvedReason::AmbiguousBranch { matches },
        ) => format!("branch {from} to {to} circuit {circuit} names {matches} branches"),
        (
            ContingencyAction::OpenThreeWinding { buses, circuit },
            UnresolvedReason::AmbiguousTransformer3w { matches },
        ) => format!(
            "three winding transformer on buses {} {} {} circuit {circuit} names {matches} transformers",
            buses[0], buses[1], buses[2]
        ),
        (ContingencyAction::OpenThreeWinding { buses, circuit }, _) => format!(
            "no three winding transformer on buses {} {} {} circuit {circuit}",
            buses[0], buses[1], buses[2]
        ),
        (
            ContingencyAction::RemoveMachine { bus, id }
            | ContingencyAction::AddMachine { bus, id },
            _,
        ) => format!("no machine {id} at bus {bus}"),
        (ContingencyAction::RemoveShunt { bus, id }, _) => match id {
            Some(id) => format!("no fixed shunt {id} at bus {bus}"),
            None => format!("no fixed shunt at bus {bus}"),
        },
        (ContingencyAction::RemoveSwitchedShunt { bus }, _) => {
            format!("no switched shunt at bus {bus}")
        }
        (ContingencyAction::RemoveLoad { bus, id }, _) => match id {
            Some(id) => format!("no load {id} at bus {bus}"),
            None => format!("no load at bus {bus}"),
        },
        (ContingencyAction::Unrecognized { text }, _) => {
            format!("statement kept as text: {text}")
        }
        (
            ContingencyAction::DisconnectBus { bus }
            | ContingencyAction::ChangeLoad { bus, .. }
            | ContingencyAction::ChangeGeneration { bus, .. },
            _,
        ) => format!("no bus {bus}"),
        (ContingencyAction::OpenBranch { from, to, circuit }, _) => {
            format!("branch {from} to {to} circuit {circuit} did not bind")
        }
    }
}

impl ContingencySet {
    /// Bind every case to the elements of `net`, building the equipment index
    /// once.
    ///
    /// Resolution reports rather than refuses: an action that names no element
    /// is kept with its reason and the case is counted unresolved, while the
    /// actions of that case that did bind stay listed.
    ///
    /// An element the network already states out of service binds like any
    /// other, with `in_service` false, because outaging it changes nothing.
    /// A case with no actions resolves to no components.
    #[must_use]
    pub fn resolve(&self, net: &BalancedNetwork) -> ContingencyResolution {
        self.resolve_with(&PsseEquipmentIndex::new(net))
    }

    /// [`ContingencySet::resolve`] against an index built once, for a caller
    /// resolving several sets against one network.
    ///
    /// The network is the one the index borrows, so the rows it states always
    /// index that network's tables.
    #[must_use]
    pub fn resolve_with(&self, index: &PsseEquipmentIndex<'_>) -> ContingencyResolution {
        let mut out = ContingencyResolution::default();
        for case in &self.cases {
            let mut resolved = ResolvedCase {
                name: case.name.clone(),
                ..ResolvedCase::default()
            };
            for action in &case.actions {
                if matches!(action, ContingencyAction::Unrecognized { .. }) {
                    out.unrecognized_statements += 1;
                }
                match bind(index, action) {
                    Ok(components) => resolved.components.extend(components),
                    Err(reason) => resolved.unresolved.push(UnresolvedAction {
                        action: action.clone(),
                        reason,
                    }),
                }
            }
            if resolved.is_resolved() {
                out.resolved += 1;
            } else {
                out.unresolved += 1;
            }
            out.cases.push(resolved);
        }
        out
    }
}

/// The elements one action names, or why it names none.
///
/// [`ContingencyAction::DisconnectBus`] binds to the bus alone: which elements
/// at that bus leave service depends on what the consumer models, so expanding
/// the bus is the consumer's work. [`ContingencyAction::ChangeLoad`] and
/// [`ContingencyAction::ChangeGeneration`] bind to the bus alone for the same
/// reason; the amount to move rides on the action itself and is not applied
/// here.
fn bind(
    index: &PsseEquipmentIndex<'_>,
    action: &ContingencyAction,
) -> Result<Vec<ResolvedComponent>, UnresolvedReason> {
    let net = index.network();
    match action {
        ContingencyAction::OpenBranch { from, to, circuit } => {
            let rows = index.branch_rows(*from, *to, circuit);
            match rows.as_slice() {
                [] => Err(UnresolvedReason::NoSuchBranch),
                [row] => Ok(vec![branch_component(net, *row)]),
                many => Err(UnresolvedReason::AmbiguousBranch {
                    matches: many.len(),
                }),
            }
        }
        ContingencyAction::OpenThreeWinding { buses, circuit } => {
            let rows = index.transformer_3w_rows(*buses, circuit);
            match rows.as_slice() {
                [] => Err(UnresolvedReason::NoSuchTransformer3w),
                [row] => Ok(vec![transformer_3w_component(net, *row)]),
                many => Err(UnresolvedReason::AmbiguousTransformer3w {
                    matches: many.len(),
                }),
            }
        }
        ContingencyAction::RemoveMachine { bus, id }
        | ContingencyAction::AddMachine { bus, id } => index
            .machine_row(*bus, id)
            .map(|row| vec![generator_component(net, row)])
            .ok_or(UnresolvedReason::NoSuchMachine),
        ContingencyAction::RemoveShunt { bus, id } => {
            let rows = index.fixed_shunt_rows(*bus, id.as_deref());
            non_empty(rows, UnresolvedReason::NoSuchShunt).map(|rows| {
                rows.into_iter()
                    .map(|row| shunt_component(net, row))
                    .collect()
            })
        }
        ContingencyAction::RemoveSwitchedShunt { bus } => {
            let rows = index.switched_shunt_rows(*bus);
            non_empty(rows, UnresolvedReason::NoSuchShunt).map(|rows| {
                rows.into_iter()
                    .map(|row| shunt_component(net, row))
                    .collect()
            })
        }
        ContingencyAction::RemoveLoad { bus, id } => {
            let rows = index.load_rows(*bus, id.as_deref());
            non_empty(rows, UnresolvedReason::NoSuchLoad).map(|rows| {
                rows.into_iter()
                    .map(|row| load_component(net, row))
                    .collect()
            })
        }
        ContingencyAction::DisconnectBus { bus }
        | ContingencyAction::ChangeLoad { bus, .. }
        | ContingencyAction::ChangeGeneration { bus, .. } => index
            .bus_row(*bus)
            .map(|row| vec![bus_component(net, row)])
            .ok_or(UnresolvedReason::NoSuchBus),
        ContingencyAction::Unrecognized { .. } => Err(UnresolvedReason::Unrecognized),
    }
}

fn non_empty(rows: Vec<usize>, reason: UnresolvedReason) -> Result<Vec<usize>, UnresolvedReason> {
    if rows.is_empty() {
        Err(reason)
    } else {
        Ok(rows)
    }
}

/// The identity of one table row: its `uid` under `component_type`, when the
/// row carries a `uid` [`ComponentId::new`] accepts, else none. A row without
/// one has no identity to state, and a position stated in its place would name
/// an identity the network does not hold. A caller that feeds these identities
/// to an update batch calls `assign_missing_component_ids` on the network
/// first, which gives every row a `uid`.
fn component_id(component_type: &str, uid: Option<&str>) -> Option<ComponentId> {
    let local = uid.filter(|uid| !uid.is_empty())?;
    ComponentId::new(component_type, local).ok()
}

fn bus_component(net: &BalancedNetwork, row: usize) -> ResolvedComponent {
    let bus = &net.buses()[row];
    ResolvedComponent {
        component_type: BUS,
        id: component_id(BUS, bus.uid.as_deref()),
        row,
        in_service: bus.kind != BusType::Isolated,
    }
}

fn load_component(net: &BalancedNetwork, row: usize) -> ResolvedComponent {
    let load = &net.loads()[row];
    ResolvedComponent {
        component_type: LOAD,
        id: component_id(LOAD, load.uid.as_deref()),
        row,
        in_service: load.in_service,
    }
}

fn shunt_component(net: &BalancedNetwork, row: usize) -> ResolvedComponent {
    let shunt = &net.shunts()[row];
    ResolvedComponent {
        component_type: SHUNT,
        id: component_id(SHUNT, shunt.uid.as_deref()),
        row,
        in_service: shunt.in_service,
    }
}

fn generator_component(net: &BalancedNetwork, row: usize) -> ResolvedComponent {
    let generator = &net.generators()[row];
    ResolvedComponent {
        component_type: GENERATOR,
        id: component_id(GENERATOR, generator.uid.as_deref()),
        row,
        in_service: generator.in_service,
    }
}

fn branch_component(net: &BalancedNetwork, row: usize) -> ResolvedComponent {
    let branch = &net.branches()[row];
    ResolvedComponent {
        component_type: BRANCH,
        id: component_id(BRANCH, branch.uid.as_deref()),
        row,
        in_service: branch.in_service,
    }
}

fn transformer_3w_component(net: &BalancedNetwork, row: usize) -> ResolvedComponent {
    let transformer = &net.transformers_3w()[row];
    ResolvedComponent {
        component_type: TRANSFORMER_3W,
        id: component_id(TRANSFORMER_3W, transformer.uid.as_deref()),
        row,
        in_service: transformer.in_service,
    }
}

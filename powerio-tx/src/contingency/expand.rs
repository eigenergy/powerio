//! Expanding a `.con` file's automatic specifications into explicit cases.
//!
//! A `SINGLE BRANCH IN SUBSYSTEM 'WOA'` line states a rule, not a list. The
//! elements it names are whatever the network holds inside the subsystem the
//! `.sub` file states, so expanding needs all three files. Expansion produces
//! ordinary [`ContingencyCase`] values, which resolve and write like any case
//! read from a file.
//!
//! PSS/E's own generated case names are not publicly documented. The names
//! here are PowerIO's convention and are listed in `FORMAT.md`.

use std::collections::BTreeSet;

use super::sub::SubsystemSet;
use super::{
    AutomaticOrder, AutomaticSpec, AutomaticTarget, ContingencyAction, ContingencyCase,
    ContingencySet, PsseEquipmentIndex, SkipRule,
};
use crate::diagnostics::{Diagnostic, codes};
use crate::network::{BalancedNetwork, BusId, Transformer3W};

/// Output of an expansion: the set with its automatic specifications turned
/// into cases, plus the notes on the specifications that produced no case.
///
/// Two findings are noted, one note each: a specification naming a subsystem
/// the subsystem set does not state earns `BUILD.CON.SUBSYSTEM_UNKNOWN`, and a
/// specification whose subsystem is stated but holds fewer in service elements
/// than its order needs, one for `SINGLE` and two for `DOUBLE`, earns
/// `BUILD.CON.SPECIFICATION_EMPTY`. Nothing else is noted.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Expanded {
    pub set: ContingencySet,
    pub diagnostics: Vec<Diagnostic>,
}

impl ContingencySet {
    /// Expand every automatic specification against `net` and `subsystems`.
    ///
    /// The expanded set keeps the header and the statements kept as text, puts
    /// the explicit cases first and the generated cases after them, and holds
    /// no automatic specification that expanded. A specification naming a
    /// subsystem `subsystems` does not state stays in
    /// [`ContingencySet::automatic`] and earns a `BUILD.CON.SUBSYSTEM_UNKNOWN`
    /// note; the `SKIP` rules stay with it, because it still needs them. A
    /// specification whose subsystem holds fewer elements it names than its
    /// order needs expands into no case and earns a
    /// `BUILD.CON.SPECIFICATION_EMPTY` note whose message states both counts.
    /// A `DOUBLE` specification therefore needs two eligible elements, because
    /// its cases are the unordered pairs of them.
    ///
    /// Only elements the network states in service expand into cases, because
    /// outaging an element already out of service changes nothing.
    #[must_use]
    pub fn expand(&self, net: &BalancedNetwork, subsystems: &SubsystemSet) -> Expanded {
        self.expand_with(&PsseEquipmentIndex::new(net), subsystems)
    }

    /// [`ContingencySet::expand`] against an index built once, for a caller
    /// expanding several sets over one network.
    ///
    /// The network is the one the index borrows, so the rows it reads always
    /// index that network's tables.
    #[must_use]
    pub fn expand_with(
        &self,
        index: &PsseEquipmentIndex<'_>,
        subsystems: &SubsystemSet,
    ) -> Expanded {
        let net = index.network();
        let mut diagnostics = Vec::new();
        let mut kept = Vec::new();
        let mut generated = Vec::new();
        for spec in &self.automatic {
            let Some(subsystem) = subsystems.get(&spec.subsystem) else {
                diagnostics.push(Diagnostic::of(
                    &codes::BUILD_CON_SUBSYSTEM_UNKNOWN,
                    format!(
                        "{}: the subsystem set states no subsystem '{}'",
                        describe(spec),
                        spec.subsystem
                    ),
                ));
                kept.push(spec.clone());
                continue;
            };
            let buses = subsystem.select_buses(net);
            let singles = single_cases(net, index, spec, &buses, &self.skips);
            let needed = match spec.order {
                AutomaticOrder::Single => 1,
                AutomaticOrder::Double => 2,
            };
            let eligible = singles.len();
            if eligible < needed {
                let plural = if eligible == 1 { "element" } else { "elements" };
                diagnostics.push(Diagnostic::of(
                    &codes::BUILD_CON_SPECIFICATION_EMPTY,
                    format!(
                        "{}: subsystem '{}' holds {eligible} in service {plural} this specification names, and its order needs {needed}",
                        describe(spec),
                        spec.subsystem
                    ),
                ));
            }
            match spec.order {
                AutomaticOrder::Single => generated.extend(singles),
                AutomaticOrder::Double => generated.extend(double_cases(&singles)),
            }
        }
        let mut cases = self.cases.clone();
        cases.extend(generated);
        // The rules are the expansion's own input, so they are dropped only
        // once every specification that could read them has expanded. A set
        // stating rules and no specification keeps them.
        let skips = if kept.is_empty() && !self.automatic.is_empty() {
            Vec::new()
        } else {
            self.skips.clone()
        };
        Expanded {
            set: ContingencySet {
                header: self.header.clone(),
                cases,
                automatic: kept,
                skips,
                retained: self.retained.clone(),
            },
            diagnostics,
        }
    }
}

/// A specification as its own line states it, for a note that names it.
fn describe(spec: &AutomaticSpec) -> String {
    let order = match spec.order {
        AutomaticOrder::Single => "SINGLE",
        AutomaticOrder::Double => "DOUBLE",
    };
    let target = match spec.target {
        AutomaticTarget::Branch => "BRANCH",
        AutomaticTarget::Unit => "UNIT",
        AutomaticTarget::Tie => "TIE",
    };
    format!("{order} {target}")
}

/// The one element cases a specification names, in table order.
fn single_cases(
    net: &BalancedNetwork,
    index: &PsseEquipmentIndex,
    spec: &AutomaticSpec,
    buses: &BTreeSet<BusId>,
    skips: &[SkipRule],
) -> Vec<ContingencyCase> {
    let mut cases = Vec::new();
    match spec.target {
        AutomaticTarget::Branch | AutomaticTarget::Tie => {
            let tie = spec.target == AutomaticTarget::Tie;
            for (row, branch) in net.branches().iter().enumerate() {
                let inside = usize::from(buses.contains(&branch.from))
                    + usize::from(buses.contains(&branch.to));
                let wanted = if tie { inside == 1 } else { inside == 2 };
                if !branch.in_service || !wanted {
                    continue;
                }
                let circuit = index.circuit_ids()[row].trim();
                if skipped(skips, branch.from, branch.to, circuit) {
                    continue;
                }
                cases.push(ContingencyCase {
                    name: format!("L_{}_{}_{circuit}", branch.from.0, branch.to.0),
                    actions: vec![ContingencyAction::OpenBranch {
                        from: branch.from,
                        to: branch.to,
                        circuit: circuit.to_owned(),
                    }],
                });
            }
            if spec.low_voltage_3w && !tie {
                cases.extend(three_winding_cases(net, index, buses));
            }
        }
        AutomaticTarget::Unit => {
            for (row, generator) in net.generators().iter().enumerate() {
                if !generator.in_service || !buses.contains(&generator.bus) {
                    continue;
                }
                let id = index.machine_ids()[row].trim();
                cases.push(ContingencyCase {
                    name: format!("G_{}_{id}", generator.bus.0),
                    actions: vec![ContingencyAction::RemoveMachine {
                        bus: generator.bus,
                        id: id.to_owned(),
                    }],
                });
            }
        }
    }
    cases
}

/// The three winding transformer cases `3WLOWVOLTAGE` adds: one per in service
/// transformer whose lowest voltage winding sits inside the subsystem.
fn three_winding_cases(
    net: &BalancedNetwork,
    index: &PsseEquipmentIndex,
    buses: &BTreeSet<BusId>,
) -> Vec<ContingencyCase> {
    let mut cases = Vec::new();
    for (row, transformer) in net.transformers_3w().iter().enumerate() {
        if !transformer.in_service || !buses.contains(&low_voltage_bus(net, index, transformer)) {
            continue;
        }
        let circuit = index.transformer_3w_ids()[row].trim();
        let terminals = [
            transformer.windings[0].bus,
            transformer.windings[1].bus,
            transformer.windings[2].bus,
        ];
        cases.push(ContingencyCase {
            name: format!(
                "T_{}_{}_{}_{circuit}",
                terminals[0].0, terminals[1].0, terminals[2].0
            ),
            actions: vec![ContingencyAction::OpenThreeWinding {
                buses: terminals,
                circuit: circuit.to_owned(),
            }],
        });
    }
    cases
}

/// One case per unordered pair of single cases, the second case's actions
/// following the first's.
fn double_cases(singles: &[ContingencyCase]) -> Vec<ContingencyCase> {
    let mut cases = Vec::new();
    for (position, first) in singles.iter().enumerate() {
        for second in &singles[position + 1..] {
            let mut actions = first.actions.clone();
            actions.extend(second.actions.iter().cloned());
            cases.push(ContingencyCase {
                name: format!("{}+{}", first.name, second.name),
                actions,
            });
        }
    }
    cases
}

/// Whether a `SKIP` rule names this branch. A rule matches in either terminal
/// order, as a `.con` statement does.
fn skipped(skips: &[SkipRule], from: BusId, to: BusId, circuit: &str) -> bool {
    skips.iter().any(|rule| {
        rule.circuit.trim() == circuit
            && ((rule.from == from && rule.to == to) || (rule.from == to && rule.to == from))
    })
}

/// The bus of the winding with the lowest voltage. A winding stating no
/// nominal kV defers to its terminal bus base kV, which is the same rule the
/// RAW reader states. Ties take the earlier winding.
pub(super) fn low_voltage_bus(
    net: &BalancedNetwork,
    index: &PsseEquipmentIndex,
    transformer: &Transformer3W,
) -> BusId {
    let kv_of = |winding: &crate::network::Winding| {
        if winding.nominal_kv > 0.0 {
            return winding.nominal_kv;
        }
        index
            .bus_row(winding.bus)
            .map_or(0.0, |row| net.buses()[row].base_kv)
    };
    let mut lowest = &transformer.windings[0];
    for winding in &transformer.windings[1..] {
        if kv_of(winding) < kv_of(lowest) {
            lowest = winding;
        }
    }
    lowest.bus
}

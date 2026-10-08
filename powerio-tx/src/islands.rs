//! Electrical islands and one reference bus per island.
//!
//! A large case can hold several AC islands: asynchronous systems joined only
//! by HVDC, radial pockets behind an open breaker, and stranded equipment. A
//! power flow needs one reference bus in each island it solves, and an island
//! with no source cannot be solved at all. [`BalancedNetwork::calc_islands`]
//! partitions the energized buses; [`BalancedNetwork::assign_island_references`]
//! gives each supplied island one reference and de-energizes the rest, and
//! [`IslandReferencePolicy::PerIsland`] applies the same rule during
//! normalization.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use crate::diagnostics::{Diagnostic, codes};
use crate::network::{BalancedNetwork, BusId, BusType, Generator};

/// How normalization and [`BalancedNetwork::assign_island_references`] treat
/// the reference buses of a network with several islands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum IslandReferencePolicy {
    /// Keep the reference buses the case states. Normalization designates
    /// one only when none survives anywhere, and
    /// [`BalancedNetwork::assign_island_references`] changes nothing.
    #[default]
    Stated,
    /// One reference per island. An island that states none takes the bus of
    /// its in-service generator with the largest `pmax`; an island stating
    /// several keeps the one with the most generation and demotes the
    /// others; an island with no in-service generator cannot be solved and is
    /// de-energized.
    PerIsland,
}

/// One AC island: energized buses joined by in-service branches, closed
/// switches, and in-service three winding transformers.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Island {
    /// The island's buses in ascending id order.
    pub buses: Vec<BusId>,
    /// Its reference (slack) buses, in ascending id order.
    pub references: Vec<BusId>,
    /// Rows of the in-service generators on its buses.
    pub generators: Vec<usize>,
}

impl Island {
    /// Whether an in-service generator can supply the island.
    #[must_use]
    pub fn is_supplied(&self) -> bool {
        !self.generators.is_empty()
    }
}

/// The result of [`BalancedNetwork::calc_islands`].
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct IslandPartition {
    /// Every island, largest first; islands of one size come in ascending
    /// order of their smallest bus id.
    pub islands: Vec<Island>,
    /// Buses typed isolated, which belong to no island.
    pub isolated: Vec<BusId>,
    island_of: HashMap<BusId, usize>,
}

impl IslandPartition {
    /// The position in [`islands`](Self::islands) of the island holding
    /// `bus`, or `None` for an isolated or unknown bus.
    #[must_use]
    pub fn island_of(&self, bus: BusId) -> Option<usize> {
        self.island_of.get(&bus).copied()
    }
}

/// The result of [`BalancedNetwork::assign_island_references`].
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct IslandReferenceReport {
    /// The islands after the assignment.
    pub partition: IslandPartition,
    /// Buses made the reference of their island.
    pub designated: Vec<BusId>,
    /// Reference buses demoted because their island kept another.
    pub demoted: Vec<BusId>,
    /// Buses of the islands that were de-energized, now typed isolated.
    pub de_energized: Vec<BusId>,
    /// One finding per island the assignment changed.
    pub diagnostics: Vec<Diagnostic>,
}

/// The `pmax` order the reference choice uses: a NaN bound never wins, and an
/// unbounded `+Inf` wins as the largest capacity.
fn pmax_key(generator: &Generator) -> f64 {
    if generator.pmax.is_nan() {
        f64::NEG_INFINITY
    } else {
        generator.pmax
    }
}

/// The bus of the in-service generator with the largest `pmax` among `rows`,
/// the smallest row breaking ties.
pub(crate) fn largest_generator_bus(generators: &[Generator], rows: &[usize]) -> Option<BusId> {
    rows.iter()
        .copied()
        .max_by(|&a, &b| {
            pmax_key(&generators[a])
                .total_cmp(&pmax_key(&generators[b]))
                .then(b.cmp(&a))
        })
        .map(|row| generators[row].bus)
}

/// Of several reference buses, the one hosting the most in-service `pmax`,
/// the smallest id breaking ties.
pub(crate) fn kept_reference(
    references: &[BusId],
    generators: &[Generator],
    rows: &[usize],
) -> Option<BusId> {
    let capacity = |bus: BusId| -> f64 {
        rows.iter()
            .map(|&row| &generators[row])
            .filter(|generator| generator.bus == bus && !generator.pmax.is_nan())
            .map(|generator| generator.pmax)
            .sum()
    };
    references
        .iter()
        .copied()
        .max_by(|&a, &b| capacity(a).total_cmp(&capacity(b)).then(b.cmp(&a)))
}

/// A short list of bus ids for a message: the first few, then a count.
pub(crate) fn bus_list(buses: &[BusId]) -> String {
    const SHOWN: usize = 5;
    let mut text = buses
        .iter()
        .take(SHOWN)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if buses.len() > SHOWN {
        let _ = write!(text, " and {} more", buses.len() - SHOWN);
    }
    text
}

impl BalancedNetwork {
    /// Partition the energized buses into AC islands.
    ///
    /// Two buses share an island when a path of in-service branches, closed
    /// switches, and in-service three winding transformers joins them. HVDC
    /// lines do not join islands: they tie asynchronous systems. Buses typed
    /// isolated belong to no island and are listed apart, and an element that
    /// touches one does not join anything through it, except that a three
    /// winding transformer with one winding on an isolated bus still joins its
    /// other two windings, as the matrix view's star expansion does.
    #[must_use]
    pub fn calc_islands(&self) -> IslandPartition {
        self.calc_islands_where(true)
    }

    /// [`calc_islands`](BalancedNetwork::calc_islands), where
    /// `partial_transformers_3w` decides whether a three winding transformer
    /// with a winding on an isolated bus still joins its other two windings.
    /// Normalization drops such a transformer whole, so it partitions without
    /// it.
    pub(crate) fn calc_islands_where(&self, partial_transformers_3w: bool) -> IslandPartition {
        let buses = self.buses();
        let index_of: HashMap<BusId, usize> = buses
            .iter()
            .enumerate()
            .filter(|(_, bus)| bus.kind != BusType::Isolated)
            .map(|(index, bus)| (bus.id, index))
            .collect();
        let mut sets = petgraph::unionfind::UnionFind::new(buses.len());
        let mut join = |a: BusId, b: BusId| {
            if let (Some(&i), Some(&j)) = (index_of.get(&a), index_of.get(&b)) {
                sets.union(i, j);
            }
        };
        for branch in self.branches().iter().filter(|branch| branch.in_service) {
            join(branch.from, branch.to);
        }
        for switch in self.switches().iter().filter(|switch| switch.closed) {
            join(switch.from, switch.to);
        }
        for transformer in self.transformers_3w().iter().filter(|t| t.in_service) {
            let [a, b, c] = &transformer.windings;
            if partial_transformers_3w || [a, b, c].iter().all(|w| index_of.contains_key(&w.bus)) {
                join(a.bus, b.bus);
                join(b.bus, c.bus);
                join(a.bus, c.bus);
            }
        }
        let labels = sets.into_labeling();

        let mut by_root: HashMap<usize, Island> = HashMap::new();
        let mut isolated = Vec::new();
        for (index, bus) in buses.iter().enumerate() {
            if bus.kind == BusType::Isolated {
                isolated.push(bus.id);
                continue;
            }
            let island = by_root.entry(labels[index]).or_insert_with(|| Island {
                buses: Vec::new(),
                references: Vec::new(),
                generators: Vec::new(),
            });
            island.buses.push(bus.id);
            if bus.kind == BusType::Ref {
                island.references.push(bus.id);
            }
        }
        for (row, generator) in self.generators().iter().enumerate() {
            if let Some(&index) = index_of.get(&generator.bus)
                && generator.in_service
                && let Some(island) = by_root.get_mut(&labels[index])
            {
                island.generators.push(row);
            }
        }
        let mut islands: Vec<Island> = by_root.into_values().collect();
        for island in &mut islands {
            island.buses.sort_unstable();
            island.references.sort_unstable();
        }
        islands.sort_by(|a, b| {
            b.buses
                .len()
                .cmp(&a.buses.len())
                .then(a.buses[0].cmp(&b.buses[0]))
        });
        isolated.sort_unstable();
        let island_of = islands
            .iter()
            .enumerate()
            .flat_map(|(position, island)| island.buses.iter().map(move |&bus| (bus, position)))
            .collect();
        IslandPartition {
            islands,
            isolated,
            island_of,
        }
    }

    /// Give every island one reference bus under `policy`, returning what
    /// changed.
    ///
    /// Under [`IslandReferencePolicy::PerIsland`]:
    ///
    /// - An island that states one reference keeps it.
    /// - An island that states none and holds an in-service generator gets
    ///   the bus of its largest `pmax` generator as its reference.
    /// - An island that states several keeps the one hosting the most
    ///   in-service `pmax` (the smallest id breaking ties) and demotes the
    ///   others to PV when they host an in-service generator, else PQ.
    /// - An island with no in-service generator has no source to balance it,
    ///   so it is de-energized: its buses are typed isolated, and the
    ///   in-service loads, shunts, static var compensators, storage, and every
    ///   branch, three winding transformer, and HVDC line touching them are
    ///   taken out of service.
    ///
    /// Each change records one finding. [`IslandReferencePolicy::Stated`]
    /// changes nothing and reports the partition.
    #[allow(clippy::too_many_lines)] // one stanza per island outcome
    pub fn assign_island_references(
        &mut self,
        policy: IslandReferencePolicy,
    ) -> IslandReferenceReport {
        let partition = self.calc_islands();
        if policy == IslandReferencePolicy::Stated {
            return IslandReferenceReport {
                partition,
                ..IslandReferenceReport::default()
            };
        }

        let mut designated = Vec::new();
        let mut demoted: Vec<(BusId, BusType)> = Vec::new();
        let mut dead: BTreeSet<BusId> = BTreeSet::new();
        let mut diagnostics = Vec::new();
        let hosts_generator: BTreeSet<BusId> = self
            .generators()
            .iter()
            .filter(|generator| generator.in_service)
            .map(|generator| generator.bus)
            .collect();
        for island in &partition.islands {
            if !island.is_supplied() {
                let load: f64 = self
                    .loads()
                    .iter()
                    .filter(|load| load.in_service && island.buses.binary_search(&load.bus).is_ok())
                    .map(|load| load.p)
                    .sum();
                diagnostics.push(Diagnostic::of(
                    &codes::CANONICALIZE_ISLAND_DE_ENERGIZED,
                    format!(
                        "the island of {} bus(es) ({}) has no in-service generator and was \
                         de-energized; its {load} MW of in-service load is no longer served",
                        island.buses.len(),
                        bus_list(&island.buses)
                    ),
                ));
                dead.extend(island.buses.iter().copied());
                continue;
            }
            match island.references.as_slice() {
                [_] => {}
                [] => {
                    let Some(bus) = largest_generator_bus(self.generators(), &island.generators)
                    else {
                        continue;
                    };
                    diagnostics.push(Diagnostic::of(
                        &codes::CANONICALIZE_ISLAND_REFERENCE_DESIGNATED,
                        format!(
                            "the island of {} bus(es) ({}) states no reference bus; bus {bus} \
                             hosts its largest pmax in-service generator and was designated the \
                             reference",
                            island.buses.len(),
                            bus_list(&island.buses)
                        ),
                    ));
                    designated.push(bus);
                }
                references => {
                    let kept = kept_reference(references, self.generators(), &island.generators)
                        .unwrap_or(references[0]);
                    let others: Vec<BusId> = references
                        .iter()
                        .copied()
                        .filter(|&bus| bus != kept)
                        .collect();
                    diagnostics.push(Diagnostic::of(
                        &codes::CANONICALIZE_ISLAND_REFERENCE_DEMOTED,
                        format!(
                            "the island of {} bus(es) states {} reference buses; bus {kept} \
                             hosts the most generation and stays the reference, and bus(es) {} \
                             were demoted",
                            island.buses.len(),
                            references.len(),
                            bus_list(&others)
                        ),
                    ));
                    for bus in others {
                        let kind = if hosts_generator.contains(&bus) {
                            BusType::Pv
                        } else {
                            BusType::Pq
                        };
                        demoted.push((bus, kind));
                    }
                }
            }
        }

        if !designated.is_empty() || !demoted.is_empty() || !dead.is_empty() {
            let designated_set: BTreeSet<BusId> = designated.iter().copied().collect();
            let demoted_kind: HashMap<BusId, BusType> = demoted.iter().copied().collect();
            for bus in self.buses_mut() {
                if designated_set.contains(&bus.id) {
                    bus.kind = BusType::Ref;
                } else if let Some(&kind) = demoted_kind.get(&bus.id) {
                    bus.kind = kind;
                } else if dead.contains(&bus.id) {
                    bus.kind = BusType::Isolated;
                }
            }
        }
        if !dead.is_empty() {
            self.de_energize(&dead);
        }

        IslandReferenceReport {
            partition: self.calc_islands(),
            designated,
            demoted: demoted.into_iter().map(|(bus, _)| bus).collect(),
            de_energized: dead.into_iter().collect(),
            diagnostics,
        }
    }

    /// Take every element at or touching `buses` out of service. Only a table
    /// with such an element in service is copied.
    fn de_energize(&mut self, buses: &BTreeSet<BusId>) {
        let at = |bus: BusId| buses.contains(&bus);
        if self.loads().iter().any(|l| l.in_service && at(l.bus)) {
            for load in self.loads_mut().iter_mut().filter(|l| at(l.bus)) {
                load.in_service = false;
            }
        }
        if self.shunts().iter().any(|s| s.in_service && at(s.bus)) {
            for shunt in self.shunts_mut().iter_mut().filter(|s| at(s.bus)) {
                shunt.in_service = false;
            }
        }
        if self
            .static_var_compensators()
            .iter()
            .any(|s| s.in_service && at(s.bus))
        {
            for svc in self
                .static_var_compensators_mut()
                .iter_mut()
                .filter(|s| at(s.bus))
            {
                svc.in_service = false;
            }
        }
        if self.storage().iter().any(|s| s.in_service && at(s.bus)) {
            for storage in self.storage_mut().iter_mut().filter(|s| at(s.bus)) {
                storage.in_service = false;
            }
        }
        let touches = |from: BusId, to: BusId| at(from) || at(to);
        if self
            .branches()
            .iter()
            .any(|b| b.in_service && touches(b.from, b.to))
        {
            for branch in self
                .branches_mut()
                .iter_mut()
                .filter(|b| touches(b.from, b.to))
            {
                branch.in_service = false;
            }
        }
        if self
            .hvdc()
            .iter()
            .any(|d| d.in_service && touches(d.from, d.to))
        {
            for line in self.hvdc_mut().iter_mut().filter(|d| touches(d.from, d.to)) {
                line.in_service = false;
            }
        }
        let winding_at = |t: &crate::network::Transformer3W| t.windings.iter().any(|w| at(w.bus));
        if self
            .transformers_3w()
            .iter()
            .any(|t| t.in_service && winding_at(t))
        {
            for transformer in self
                .transformers_3w_mut()
                .iter_mut()
                .filter(|t| winding_at(t))
            {
                transformer.in_service = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{Branch, Bus, Hvdc, Impedance, Load, Switch, Transformer3W, Winding};
    use crate::{IslandReferencePolicy, NormalizeOptions};

    fn generator(bus: usize, pmax: f64) -> Generator {
        let mut generator = Generator::new(BusId(bus));
        generator.pmax = pmax;
        generator.pg = pmax / 2.0;
        generator
    }

    /// Island A {1, 2, 9} holds the reference bus 1; island B {3, 4, 5}
    /// holds generation and no reference, bus 5 joined by a closed switch;
    /// island C {6, 7} holds only a load and an HVDC end; bus 8 is isolated.
    fn three_islands() -> BalancedNetwork {
        let mut buses: Vec<Bus> = (1..=9)
            .map(|id| Bus::new(BusId(id), BusType::Pq, 230.0))
            .collect();
        buses[0].kind = BusType::Ref;
        buses[7].kind = BusType::Isolated;
        let line = |from, to| Branch::new(BusId(from), BusId(to), 0.01, 0.1);
        let mut net = BalancedNetwork::in_memory(
            "islands",
            100.0,
            buses,
            vec![line(1, 2), line(2, 9), line(3, 4), line(6, 7), line(7, 8)],
        );
        net.switches_mut()
            .push(Switch::new(BusId(4), BusId(5), true));
        net.generators_mut()
            .extend([generator(1, 200.0), generator(4, 50.0), generator(5, 80.0)]);
        net.loads_mut().extend([
            Load::new(BusId(2), 60.0, 0.0),
            Load::new(BusId(3), 40.0, 0.0),
            Load::new(BusId(7), 25.0, 0.0),
        ]);
        net.hvdc_mut().push(Hvdc::new(BusId(1), BusId(6)));
        net
    }

    fn kind(net: &BalancedNetwork, id: usize) -> BusType {
        net.buses().iter().find(|b| b.id == BusId(id)).unwrap().kind
    }

    fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
        diagnostics.iter().map(Diagnostic::code).collect()
    }

    #[test]
    fn islands_follow_branches_closed_switches_and_not_hvdc() {
        let net = three_islands();
        let partition = net.calc_islands();
        let buses: Vec<Vec<usize>> = partition
            .islands
            .iter()
            .map(|island| island.buses.iter().map(|b| b.0).collect())
            .collect();
        assert_eq!(buses, [vec![1, 2, 9], vec![3, 4, 5], vec![6, 7]]);
        assert_eq!(partition.isolated, [BusId(8)]);
        assert_eq!(partition.islands[0].references, [BusId(1)]);
        assert_eq!(partition.islands[1].generators, [1, 2]);
        assert!(!partition.islands[2].is_supplied());
        assert_eq!(partition.island_of(BusId(5)), Some(1));
        assert_eq!(partition.island_of(BusId(8)), None);
    }

    #[test]
    fn a_three_winding_transformer_joins_its_windings_while_in_service() {
        let buses = (1..=3)
            .map(|id| Bus::new(BusId(id), BusType::Pq, 230.0))
            .collect();
        let mut net = BalancedNetwork::in_memory("t3w", 100.0, buses, Vec::new());
        net.transformers_3w_mut().push(Transformer3W::new(
            [1, 2, 3].map(|bus| Winding::new(BusId(bus))),
            [Impedance::new(0.0, 0.1, 100.0); 3],
        ));
        assert_eq!(net.calc_islands().islands.len(), 1);
        net.transformers_3w_mut()[0].in_service = false;
        assert_eq!(net.calc_islands().islands.len(), 3);
    }

    #[test]
    fn a_winding_on_an_isolated_bus_leaves_the_other_two_joined() {
        // Normalization drops a three winding transformer with a winding on
        // an isolated bus whole, so its per island pass sees two islands.
        let mut buses: Vec<Bus> = (1..=3)
            .map(|id| Bus::new(BusId(id), BusType::Pq, 230.0))
            .collect();
        buses[0].kind = BusType::Ref;
        buses[1].kind = BusType::Isolated;
        let mut net = BalancedNetwork::in_memory("t3w", 100.0, buses, Vec::new());
        net.transformers_3w_mut().push(Transformer3W::new(
            [1, 2, 3].map(|bus| Winding::new(BusId(bus))),
            [Impedance::new(0.0, 0.1, 100.0); 3],
        ));
        net.generators_mut().push(generator(1, 100.0));
        net.loads_mut().push(Load::new(BusId(3), 10.0, 0.0));
        let partition = net.calc_islands();
        assert_eq!(partition.islands.len(), 1);
        assert_eq!(partition.islands[0].buses, [BusId(1), BusId(3)]);

        let options = NormalizeOptions {
            island_references: IslandReferencePolicy::PerIsland,
            ..NormalizeOptions::default()
        };
        let normalized = net.to_normalized_with_options(&options).unwrap();
        assert_eq!(normalized.network.buses().len(), 1);
        assert!(codes(&normalized.diagnostics).contains(&"CANONICALIZE.ISLAND.DE_ENERGIZED"));
    }

    #[test]
    fn each_island_gets_one_reference_and_an_unsupplied_one_is_de_energized() {
        let mut net = three_islands();
        let unchanged = net.assign_island_references(IslandReferencePolicy::Stated);
        assert!(unchanged.diagnostics.is_empty());
        assert_eq!(kind(&net, 5), BusType::Pq);

        let report = net.assign_island_references(IslandReferencePolicy::PerIsland);
        // Island A keeps bus 1; island B takes bus 5, its largest generator.
        assert_eq!(report.designated, [BusId(5)]);
        assert_eq!(kind(&net, 1), BusType::Ref);
        assert_eq!(kind(&net, 5), BusType::Ref);
        // Island C has no generator: its buses are isolated and everything
        // on or touching them is out of service.
        assert_eq!(report.de_energized, [BusId(6), BusId(7)]);
        assert_eq!(kind(&net, 6), BusType::Isolated);
        assert!(!net.loads()[2].in_service);
        assert!(!net.branches()[3].in_service);
        assert!(!net.branches()[4].in_service, "the branch onto bus 8");
        assert!(!net.hvdc()[0].in_service);
        assert!(net.branches()[0].in_service && net.loads()[0].in_service);
        assert_eq!(
            codes(&report.diagnostics),
            [
                "CANONICALIZE.ISLAND.REFERENCE_DESIGNATED",
                "CANONICALIZE.ISLAND.DE_ENERGIZED"
            ]
        );
        assert!(report.diagnostics[1].message().contains("25 MW"));
        assert_eq!(report.partition.islands.len(), 2);
        assert!(
            report
                .partition
                .islands
                .iter()
                .all(|island| island.references.len() == 1)
        );
        net.validate().unwrap();

        // A second pass has nothing left to do.
        let again = net.assign_island_references(IslandReferencePolicy::PerIsland);
        assert!(again.diagnostics.is_empty());
    }

    #[test]
    fn of_two_references_in_one_island_the_larger_source_stays() {
        let buses = (1..=3)
            .map(|id| Bus::new(BusId(id), BusType::Ref, 230.0))
            .collect();
        let line = |from, to| Branch::new(BusId(from), BusId(to), 0.01, 0.1);
        let mut net =
            BalancedNetwork::in_memory("refs", 100.0, buses, vec![line(1, 2), line(2, 3)]);
        net.generators_mut()
            .extend([generator(1, 100.0), generator(2, 300.0)]);
        let report = net.assign_island_references(IslandReferencePolicy::PerIsland);
        assert_eq!(report.demoted, [BusId(1), BusId(3)]);
        assert_eq!(kind(&net, 2), BusType::Ref);
        assert_eq!(kind(&net, 1), BusType::Pv, "it hosts a generator");
        assert_eq!(kind(&net, 3), BusType::Pq);
        assert_eq!(
            codes(&report.diagnostics),
            ["CANONICALIZE.ISLAND.REFERENCE_DEMOTED"]
        );
    }

    #[test]
    fn normalization_per_island_references_every_island_and_drops_the_unsupplied() {
        let net = three_islands();
        let stated = net
            .to_normalized_with_options(&NormalizeOptions::default())
            .unwrap();
        assert_eq!(
            stated.network.buses().len(),
            8,
            "only the typed isolated bus goes"
        );
        let references: Vec<BusId> = stated
            .network
            .buses()
            .iter()
            .filter(|bus| bus.kind == BusType::Ref)
            .map(|bus| bus.id)
            .collect();
        assert_eq!(references, [BusId(1)]);
        assert!(
            codes(&stated.diagnostics).contains(&"CANONICALIZE.NORMALIZE.GENERATOR_BUS_RETYPED")
        );

        let options = NormalizeOptions {
            island_references: IslandReferencePolicy::PerIsland,
            ..NormalizeOptions::default()
        };
        let per_island = net.to_normalized_with_options(&options).unwrap();
        let ids: Vec<usize> = per_island.network.buses().iter().map(|b| b.id.0).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 9]);
        let references: Vec<BusId> = per_island
            .network
            .buses()
            .iter()
            .filter(|bus| bus.kind == BusType::Ref)
            .map(|bus| bus.id)
            .collect();
        assert_eq!(references, [BusId(1), BusId(5)]);
        assert!(per_island.network.hvdc().is_empty());
        let found = codes(&per_island.diagnostics);
        assert!(found.contains(&"CANONICALIZE.ISLAND.DE_ENERGIZED"));
        assert!(found.contains(&"CANONICALIZE.ISLAND.REFERENCE_DESIGNATED"));
        crate::IndexedNetwork::new(&per_island.network)
            .check_reference_coverage()
            .unwrap();
    }

    #[test]
    fn one_island_carves_out_with_its_elements() {
        let net = three_islands();
        let partition = net.calc_islands();
        let island: BTreeSet<BusId> = partition.islands[1].buses.iter().copied().collect();
        let sub = net.subset_buses(&island);
        assert_eq!(sub.buses().len(), 3);
        assert_eq!(sub.branches().len(), 1);
        assert_eq!(sub.switches().len(), 1);
        assert_eq!(sub.generators().len(), 2);
        assert_eq!(sub.loads().len(), 1);
        assert!(sub.hvdc().is_empty());
        sub.validate().unwrap();
    }
}

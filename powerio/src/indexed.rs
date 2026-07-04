//! [`IndexedNetwork`]: the dense-indexed analysis view over a [`Network`].
//!
//! [`Network`] is the canonical data record — format-neutral tables with no
//! analysis behavior. The matrix builders, connectivity diagnostics, and the
//! DC-OPF instance need things a plain table doesn't carry: a dense `[0, n)` bus
//! index, demand and shunts aggregated per bus, the in-service subsets, and the
//! reference bus. [`IndexCore`] derives those once from a borrowed `&Network`;
//! [`IndexedNetwork`] pairs that core with the network and answers the queries.
//! Keeping this off `Network` is what stops `Network` from turning into a god
//! type: data on one side, derived analysis on the other.
//!
//! The derived core is one `HashMap` and four `Vec<f64>`. One-shot callers use
//! [`IndexedNetwork::new`], which builds and owns a throwaway core. A long-lived
//! handle (the Python and C ABI wrappers) builds an [`IndexCore`] once at parse
//! time and rebinds a borrowing view per query with
//! [`IndexedNetwork::with_core`], so repeated queries never re-fold the loads
//! and shunts.

use std::borrow::Cow;
use std::collections::HashMap;

use petgraph::graph::UnGraph;

use crate::network::{Branch, BusId, BusType, Generator, Network};
use crate::{Error, Result};

/// The owned, network-independent derivation behind [`IndexedNetwork`]: the
/// dense bus-id map plus the per-bus demand/shunt aggregates. Build it once with
/// [`IndexCore::build`] and reuse it across many [`IndexedNetwork::with_core`]
/// views of the same [`Network`].
#[derive(Debug, Clone)]
pub struct IndexCore {
    /// Stable bus id → dense index in `[0, n)`.
    bus_id_to_idx: HashMap<BusId, usize>,
    /// Active demand summed per bus (dense index order, MW).
    pd: Vec<f64>,
    /// Reactive demand summed per bus (MVAr).
    qd: Vec<f64>,
    /// Shunt conductance summed per bus (MW at V = 1 p.u.).
    gs: Vec<f64>,
    /// Shunt susceptance summed per bus (MVAr at V = 1 p.u.).
    bs: Vec<f64>,
}

impl IndexCore {
    /// Index `net`: map bus ids to dense indices and fold every load/shunt onto
    /// its bus. Loads and shunts are summed regardless of their `in_service`
    /// flag, matching the folded `pd/qd/gs/bs` the MATPOWER-shaped model carried
    /// on the bus row (the matrices key off topology and these aggregates, not
    /// per-element service status).
    ///
    /// # Correctness
    /// Bus ids must be unique; a duplicate collapses two buses onto one dense
    /// index and silently corrupts every aggregate. The format readers and
    /// [`Network::from_json`](crate::Network::from_json) run
    /// [`Network::validate`](crate::Network::validate) before this, so a parsed
    /// or JSON-sourced network always satisfies it; a hand-built [`Network`] must
    /// call `validate` itself. Backstopped here by a `debug_assert`.
    #[must_use]
    pub fn build(net: &Network) -> Self {
        let n = net.buses.len();
        let bus_id_to_idx: HashMap<BusId, usize> = net
            .buses
            .iter()
            .enumerate()
            .map(|(idx, b)| (b.id, idx))
            .collect();
        debug_assert_eq!(
            bus_id_to_idx.len(),
            n,
            "duplicate bus id in network (run Network::check_references first)"
        );
        let mut pd = vec![0.0; n];
        let mut qd = vec![0.0; n];
        for l in &net.loads {
            if let Some(&idx) = bus_id_to_idx.get(&l.bus) {
                pd[idx] += l.p;
                qd[idx] += l.q;
            }
        }
        let mut gs = vec![0.0; n];
        let mut bs = vec![0.0; n];
        for s in &net.shunts {
            if let Some(&idx) = bus_id_to_idx.get(&s.bus) {
                gs[idx] += s.g;
                bs[idx] += s.b;
            }
        }
        Self {
            bus_id_to_idx,
            pd,
            qd,
            gs,
            bs,
        }
    }
}

/// A `Network` paired with its derived [`IndexCore`]. The network is borrowed for
/// the common case, but owned when it had to be star-lowered (a 3-winding
/// transformer expanded into a star bus plus three branches via
/// `Network::expand_transformers_3w`); the core is owned ([`IndexedNetwork::new`])
/// or borrowed from a cached [`IndexCore`] ([`IndexedNetwork::with_core`]).
#[derive(Debug)]
pub struct IndexedNetwork<'n> {
    net: Cow<'n, Network>,
    core: Cow<'n, IndexCore>,
}

impl<'n> IndexedNetwork<'n> {
    /// Build a one-shot view that owns a freshly derived [`IndexCore`]. For
    /// repeated queries on a long-lived handle, cache an [`IndexCore`] and use
    /// [`with_core`](Self::with_core) so the derivation isn't rebuilt per call.
    #[must_use]
    pub fn new(net: &'n Network) -> Self {
        let net = net.expand_transformers_3w();
        let core = IndexCore::build(&net);
        Self {
            net,
            core: Cow::Owned(core),
        }
    }

    /// Pair `net` with an already-built [`IndexCore`] — no allocation when `net`
    /// has no 3-winding transformer (the core must have been built from this same
    /// `net`). When `net` does carry a 3-winding transformer, the view star-lowers
    /// it and rebuilds the core over the expanded form, since the cached core was
    /// derived from the unexpanded network.
    #[must_use]
    pub fn with_core(net: &'n Network, core: &'n IndexCore) -> Self {
        match net.expand_transformers_3w() {
            Cow::Borrowed(net) => Self {
                net: Cow::Borrowed(net),
                core: Cow::Borrowed(core),
            },
            Cow::Owned(net) => {
                let core = IndexCore::build(&net);
                Self {
                    net: Cow::Owned(net),
                    core: Cow::Owned(core),
                }
            }
        }
    }

    /// The underlying network (the star-lowered form when a 3-winding transformer
    /// was expanded).
    #[inline]
    pub fn network(&self) -> &Network {
        &self.net
    }

    #[inline]
    pub fn n(&self) -> usize {
        self.net.buses.len()
    }

    #[inline]
    pub fn base_mva(&self) -> f64 {
        self.net.base_mva
    }

    /// The divisor for turning a power quantity (shunt, load, generation) into
    /// per unit. `base_mva` for a raw network; `1.0` for a normalized one, whose
    /// powers are already per unit (so a second division would scale them twice).
    /// `base_mva` itself stays at the system base — for MW recovery and
    /// write-back — so use this, not `base_mva`, wherever the intent is "÷ base
    /// to get per unit". The effect is that a per-unit matrix builder yields the
    /// same matrix for a network and its [`to_normalized`](Network::to_normalized)
    /// form.
    #[inline]
    pub fn per_unit_base(&self) -> f64 {
        if self.net.is_normalized() {
            1.0
        } else {
            self.net.base_mva
        }
    }

    /// A branch/bus angle field (`shift`, `va`) in radians. The raw model stores
    /// angles in degrees; a normalized network already stores radians, so for it
    /// this is the identity. The angle analogue of
    /// [`per_unit_base`](Self::per_unit_base): a builder gets the same radians
    /// whether it is handed a network or its
    /// [`to_normalized`](Network::to_normalized) form, so the matrix comes out
    /// the same.
    #[inline]
    pub fn angle_radians(&self, angle: f64) -> f64 {
        if self.net.is_normalized() {
            angle
        } else {
            angle.to_radians()
        }
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.net.name
    }

    /// All branches, in source order (column order for incidence-based builds).
    #[inline]
    pub fn branches(&self) -> &[Branch] {
        &self.net.branches
    }

    /// All generators, in source order.
    #[inline]
    pub fn generators(&self) -> &[Generator] {
        &self.net.generators
    }

    /// Resolve a bus id to its dense `[0, n)` index.
    #[inline]
    pub fn bus_index(&self, bus_id: BusId) -> Option<usize> {
        self.core.bus_id_to_idx.get(&bus_id).copied()
    }

    /// The bus id at dense index `idx` — the inverse of
    /// [`bus_index`](Self::bus_index).
    ///
    /// # Panics
    /// Panics if `idx >= n`. Pass a dense index (e.g. from [`bus_index`](Self::bus_index) or a
    /// matrix row), not a raw bus id.
    #[inline]
    pub fn bus_id(&self, idx: usize) -> BusId {
        self.net.buses[idx].id
    }

    /// Nodal active demand, length `n`.
    #[inline]
    pub fn pd(&self) -> &[f64] {
        &self.core.pd
    }

    /// Nodal reactive demand, length `n`.
    #[inline]
    pub fn qd(&self) -> &[f64] {
        &self.core.qd
    }

    /// Nodal shunt conductance, length `n`.
    #[inline]
    pub fn gs(&self) -> &[f64] {
        &self.core.gs
    }

    /// Nodal shunt susceptance, length `n`.
    #[inline]
    pub fn bs(&self) -> &[f64] {
        &self.core.bs
    }

    /// In-service branches with their index into [`branches`](Self::branches).
    pub fn in_service_branches(&self) -> impl Iterator<Item = (usize, &Branch)> {
        self.net
            .branches
            .iter()
            .enumerate()
            .filter(|(_, b)| b.in_service)
    }

    /// In-service generators with their index into [`generators`](Self::generators).
    pub fn in_service_gens(&self) -> impl Iterator<Item = (usize, &Generator)> {
        self.net
            .generators
            .iter()
            .enumerate()
            .filter(|(_, g)| g.in_service)
    }

    /// Dense indices of every reference (slack) bus, in ascending order. A
    /// network may carry more than one [`BusType::Ref`] (a slack per island, or
    /// several the source file marked) — the matrix layer grounds one row/column
    /// per entry. Empty when the network has no reference bus.
    pub fn reference_bus_indices(&self) -> Vec<usize> {
        self.net
            .buses
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == BusType::Ref)
            .map(|(i, _)| i)
            .collect()
    }

    /// Dense index of the single reference (slack) bus. Errors unless exactly
    /// one [`BusType::Ref`] exists; for the multi-reference case use
    /// [`reference_bus_indices`](Self::reference_bus_indices).
    pub fn reference_bus_index(&self) -> Result<usize> {
        match self.reference_bus_indices().as_slice() {
            [r] => Ok(*r),
            other => Err(Error::ReferenceBusCount { found: other.len() }),
        }
    }

    /// Undirected graph view: node weight = dense bus index, edge weight =
    /// index into [`branches`](Self::branches). Out-of-service branches are
    /// skipped; parallel branches are kept as separate edges.
    pub fn to_petgraph(&self) -> UnGraph<usize, usize> {
        let mut g = UnGraph::with_capacity(self.n(), self.net.branches.len());
        let nodes: Vec<_> = (0..self.n()).map(|i| g.add_node(i)).collect();
        for (idx, br) in self.in_service_branches() {
            if let (Some(i), Some(j)) = (self.bus_index(br.from), self.bus_index(br.to)) {
                g.add_edge(nodes[i], nodes[j], idx);
            }
        }
        g
    }

    /// Number of connected components in the in-service topology.
    pub fn n_connected_components(&self) -> usize {
        petgraph::algo::connected_components(&self.to_petgraph())
    }

    /// Connected-component label per dense bus index (in-service topology): two
    /// buses share a label iff an in-service branch path joins them, and an
    /// isolated bus is its own component. Labels are representative indices in
    /// `[0, n)`, not a dense `[0, k)` range — use them for equality grouping
    /// (e.g. checking every island carries a reference bus to ground).
    pub fn connected_component_labels(&self) -> Vec<usize> {
        let mut uf = petgraph::unionfind::UnionFind::new(self.n());
        for (_, br) in self.in_service_branches() {
            if let (Some(i), Some(j)) = (self.bus_index(br.from), self.bus_index(br.to)) {
                uf.union(i, j);
            }
        }
        uf.into_labeling()
    }

    /// Error unless every connected component of the in-service topology carries
    /// at least one reference (slack) bus. This is the grounding precondition
    /// for the DC Laplacian: an island with no reference leaves its all-ones
    /// null vector in the system, so the reference-grounded Laplacian stays
    /// singular. With one reference in a single island it reduces to the
    /// single slack requirement. Reports the count of ungrounded components.
    pub fn check_reference_coverage(&self) -> Result<()> {
        let labels = self.connected_component_labels();
        // Mark each component (by its representative index in `[0, n)`) that holds
        // a reference. A Vec<bool> keyed by label avoids hashing and uses one
        // flat allocation.
        let mut grounded = vec![false; labels.len()];
        for r in self.reference_bus_indices() {
            grounded[labels[r]] = true;
        }
        // A component shows up once at its root (`labels[i] == i`); count the
        // roots whose component was never grounded.
        let ungrounded = (0..labels.len())
            .filter(|&i| labels[i] == i && !grounded[i])
            .count();
        if ungrounded > 0 {
            return Err(Error::UngroundedComponent {
                components: ungrounded,
            });
        }
        Ok(())
    }

    /// True iff the in-service topology is a forest (`|E| = |V| - components`).
    pub fn is_radial(&self) -> bool {
        let g = self.to_petgraph();
        let n_components = petgraph::algo::connected_components(&g);
        g.edge_count() == g.node_count().saturating_sub(n_components)
    }

    /// One-shot topological diagnostic.
    pub fn connectivity_report(&self) -> ConnectivityReport {
        let g = self.to_petgraph();
        let n_components = petgraph::algo::connected_components(&g);
        let isolated: Vec<usize> = g
            .node_indices()
            .filter(|n| g.neighbors(*n).next().is_none())
            .map(|n| g[n])
            .collect();
        ConnectivityReport {
            n_buses: self.n(),
            n_branches_in_service: self.net.branches.iter().filter(|b| b.in_service).count(),
            n_components,
            isolated_buses: isolated,
        }
    }
}

/// Topological invariants the TUI Inspect screen and downstream solvers care
/// about.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct ConnectivityReport {
    pub n_buses: usize,
    pub n_branches_in_service: usize,
    pub n_components: usize,
    /// Dense bus indices with no incident in-service branch.
    pub isolated_buses: Vec<usize>,
}

impl ConnectivityReport {
    #[inline]
    pub fn is_single_island(&self) -> bool {
        self.n_components == 1 && self.isolated_buses.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{IndexCore, IndexedNetwork};
    use crate::network::{
        Bus, BusId, BusType, Extras, Impedance, Load, Network, Shunt, Transformer3W, Winding,
    };

    fn bus(id: usize, kind: BusType) -> Bus {
        Bus {
            id: BusId(id),
            kind,
            vm: 1.0,
            va: 0.0,
            base_kv: 1.0,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: None,
            uid: None,
            location: None,
            extras: Extras::new(),
        }
    }

    fn agg_net() -> Network {
        let mut net = Network::in_memory(
            "agg",
            100.0,
            vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
            Vec::new(),
        );
        net.loads.push(Load {
            bus: BusId(1),
            p: 10.0,
            q: 5.0,
            voltage_model: None,
            in_service: true,
            uid: None,
            extras: Extras::new(),
        });
        net.loads.push(Load {
            bus: BusId(1),
            p: 3.0,
            q: 1.0,
            voltage_model: None,
            in_service: true,
            uid: None,
            extras: Extras::new(),
        });
        net.shunts.push(Shunt {
            bus: BusId(1),
            g: 0.2,
            b: 0.4,
            in_service: true,
            control: None,
            uid: None,
            extras: Extras::new(),
        });
        net.shunts.push(Shunt {
            bus: BusId(1),
            g: 0.1,
            b: 0.3,
            in_service: true,
            control: None,
            uid: None,
            extras: Extras::new(),
        });
        net
    }

    fn assert_aggregates(view: &IndexedNetwork) {
        let i = view.bus_index(BusId(1)).unwrap();
        assert!((view.pd()[i] - 13.0).abs() < 1e-12);
        assert!((view.qd()[i] - 6.0).abs() < 1e-12);
        assert!((view.gs()[i] - 0.3).abs() < 1e-12);
        assert!((view.bs()[i] - 0.7).abs() < 1e-12);
        let j = view.bus_index(BusId(2)).unwrap();
        assert!(view.pd()[j].abs() < 1e-12);
        assert!(view.gs()[j].abs() < 1e-12);
    }

    fn three_winding(a: usize, b: usize, c: usize) -> Transformer3W {
        let winding = |bus| Winding {
            bus: BusId(bus),
            tap: 1.0,
            shift: 0.0,
            nominal_kv: 0.0,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
        };
        let imp = Impedance {
            r: 0.0,
            x: 0.1,
            base_mva: 100.0,
        };
        Transformer3W {
            windings: [winding(a), winding(b), winding(c)],
            z: [imp, imp, imp],
            star_vm: 1.0,
            star_va: 0.0,
            mag_g: 0.0,
            mag_b: 0.0,
            in_service: true,
            name: None,
            uid: None,
            extras: Extras::new(),
        }
    }

    #[test]
    fn three_winding_star_lowering_adds_a_grounded_star_bus_with_its_magnetizing_shunt() {
        // Three buses joined only by a 3-winding transformer; bus 1 is the
        // reference. The view star-lowers it into one grounded component plus a
        // synthetic star bus that carries the magnetizing shunt.
        let mut net = Network::in_memory(
            "t3w",
            100.0,
            vec![
                bus(1, BusType::Ref),
                bus(2, BusType::Pq),
                bus(3, BusType::Pq),
            ],
            Vec::new(),
        );
        let mut t = three_winding(1, 2, 3);
        t.mag_b = 0.05; // p.u. on the system base
        net.transformers_3w.push(t);

        let view = IndexedNetwork::new(&net);
        assert_eq!(view.n(), 4, "three buses plus the synthetic star point");
        assert_eq!(view.n_connected_components(), 1);
        view.check_reference_coverage().unwrap();

        // The star bus is the last dense index; its susceptance is the magnetizing
        // b scaled to the system base (raw network: × base_mva).
        let star = view.n() - 1;
        assert!((view.bs()[star] - 0.05 * 100.0).abs() < 1e-9);
        for i in 0..3 {
            assert!(view.bs()[i].abs() < 1e-12, "original buses carry no shunt");
        }

        // The canonical model keeps the typed record and gains no buses/branches.
        assert_eq!(net.buses.len(), 3);
        assert!(net.branches.is_empty());
        assert_eq!(net.transformers_3w.len(), 1);
    }

    #[test]
    fn out_of_service_three_winding_is_not_expanded() {
        let mut net = Network::in_memory(
            "t3w",
            100.0,
            vec![
                bus(1, BusType::Ref),
                bus(2, BusType::Pq),
                bus(3, BusType::Pq),
            ],
            Vec::new(),
        );
        let mut t = three_winding(1, 2, 3);
        t.in_service = false;
        net.transformers_3w.push(t);

        let view = IndexedNetwork::new(&net);
        // No star bus is synthesized for an out-of-service transformer, so the
        // three buses stay as three islands (no spurious star point).
        assert_eq!(view.n(), 3);
        assert_eq!(view.n_connected_components(), 3);
    }

    #[test]
    fn aggregates_sum_multiple_loads_and_shunts_per_bus() {
        // PSS/E and PowerModels admit several loads/shunts on one bus; the
        // per-bus fold must add them, not overwrite (last-writer-wins would pass
        // every MATPOWER fixture, which folds one load per bus).
        let net = agg_net();
        assert_aggregates(&IndexedNetwork::new(&net));
    }

    #[test]
    fn with_core_matches_one_shot_view() {
        // A view over a cached core sees the same aggregates as a fresh one.
        let net = agg_net();
        let core = IndexCore::build(&net);
        assert_aggregates(&IndexedNetwork::with_core(&net, &core));
    }
}

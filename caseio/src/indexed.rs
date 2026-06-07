//! [`IndexedNetwork`]: the dense-indexed analysis view over a [`Network`].
//!
//! [`Network`] is the canonical data record — format-neutral tables with no
//! analysis behavior. The matrix builders, connectivity diagnostics, and the
//! DC-OPF instance need things a plain table doesn't carry: a dense `[0, n)`
//! bus index, demand and shunts aggregated per bus, the in-service subsets, and
//! the reference bus. `IndexedNetwork` computes those once from a borrowed
//! `&Network` and hands them to the numerics. Keeping this on a separate view
//! (rather than on `Network`) is what stops `Network` from turning into a god
//! type: data on one side, derived analysis on the other.
//!
//! It is cheap — one `HashMap` and four `Vec<f64>` — and built lazily, only
//! when a matrix or a connectivity query is asked for, so the parse path never
//! pays for it.

use std::collections::HashMap;

use petgraph::graph::UnGraph;

use crate::network::{Branch, BusType, Generator, Network};
use crate::{Error, Result};

/// A `Network` plus the dense bus index and per-bus aggregates the numerics
/// need. Borrows the network; build with [`IndexedNetwork::new`].
#[derive(Debug)]
pub struct IndexedNetwork<'n> {
    net: &'n Network,
    /// Stable bus id → dense index in `[0, n)`.
    bus_id_to_idx: HashMap<usize, usize>,
    /// Active demand summed per bus (dense index order, MW).
    pd: Vec<f64>,
    /// Reactive demand summed per bus (MVAr).
    qd: Vec<f64>,
    /// Shunt conductance summed per bus (MW at V = 1 p.u.).
    gs: Vec<f64>,
    /// Shunt susceptance summed per bus (MVAr at V = 1 p.u.).
    bs: Vec<f64>,
}

impl<'n> IndexedNetwork<'n> {
    /// Index `net`: map bus ids to dense indices and fold every load/shunt onto
    /// its bus. Loads and shunts are summed regardless of their `in_service`
    /// flag, matching the folded `pd/qd/gs/bs` the MATPOWER-shaped model carried
    /// on the bus row (the matrices key off topology and these aggregates, not
    /// per-element service status).
    #[must_use]
    pub fn new(net: &'n Network) -> Self {
        let n = net.buses.len();
        let bus_id_to_idx: HashMap<usize, usize> =
            net.buses.iter().enumerate().map(|(idx, b)| (b.id, idx)).collect();
        // A duplicate bus id would collapse two buses onto one dense index and
        // silently corrupt every aggregate. The format readers run
        // `check_references`; the MATPOWER reader and in-memory networks don't,
        // so guard it in debug builds.
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
        Self { net, bus_id_to_idx, pd, qd, gs, bs }
    }

    /// The underlying network.
    #[inline]
    pub fn network(&self) -> &Network {
        self.net
    }

    #[inline]
    pub fn n(&self) -> usize {
        self.net.buses.len()
    }

    #[inline]
    pub fn base_mva(&self) -> f64 {
        self.net.base_mva
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
    pub fn bus_index(&self, bus_id: usize) -> Option<usize> {
        self.bus_id_to_idx.get(&bus_id).copied()
    }

    /// The bus id at dense index `idx` — the inverse of
    /// [`bus_index`](Self::bus_index).
    ///
    /// # Panics
    /// Panics if `idx >= n`. Pass a dense index (e.g. from [`bus_index`] or a
    /// matrix row), not a raw bus id.
    #[inline]
    pub fn bus_id(&self, idx: usize) -> usize {
        self.net.buses[idx].id
    }

    /// Nodal active demand, length `n`.
    #[inline]
    pub fn pd(&self) -> &[f64] {
        &self.pd
    }

    /// Nodal reactive demand, length `n`.
    #[inline]
    pub fn qd(&self) -> &[f64] {
        &self.qd
    }

    /// Nodal shunt conductance, length `n`.
    #[inline]
    pub fn gs(&self) -> &[f64] {
        &self.gs
    }

    /// Nodal shunt susceptance, length `n`.
    #[inline]
    pub fn bs(&self) -> &[f64] {
        &self.bs
    }

    /// In-service branches with their index into [`branches`](Self::branches).
    pub fn in_service_branches(&self) -> impl Iterator<Item = (usize, &Branch)> {
        self.net.branches.iter().enumerate().filter(|(_, b)| b.in_service)
    }

    /// In-service generators with their index into [`generators`](Self::generators).
    pub fn in_service_gens(&self) -> impl Iterator<Item = (usize, &Generator)> {
        self.net.generators.iter().enumerate().filter(|(_, g)| g.in_service)
    }

    /// Dense index of the single reference (slack) bus. Errors unless exactly
    /// one [`BusType::Ref`] exists.
    pub fn reference_bus_index(&self) -> Result<usize> {
        let refs: Vec<usize> = self
            .net
            .buses
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == BusType::Ref)
            .map(|(i, _)| i)
            .collect();
        match refs.as_slice() {
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

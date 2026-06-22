//! LinDist3Flow path impedance matrices for a connected radial case.
//!
//! The returned `incidence` is the reduced branch-by-bus incidence `A`: rows
//! follow in-service branch order and are oriented away from the reference bus,
//! columns are all non-reference buses in dense index order, and each row has
//! `+1` at the downstream bus and `-1` at the upstream bus when that upstream
//! bus is not the reference. The `r` and `x` matrices are `A^-1 diag(r) A^-T` and
//! `A^-1 diag(x) A^-T`, the closed-form inverse blocks of the grounded LACPF
//! matrix on a radial network with no shunts.

use std::cmp::Ordering;
use std::collections::VecDeque;

use petgraph::algo::min_spanning_tree;
use petgraph::data::Element;
use petgraph::graph::UnGraph;
use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::{Error, Result};

use super::BuildOptions;
use super::triplet::CooBuilder;

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LinDist3FlowMatrices {
    pub r: CsMat<f64>,
    pub x: CsMat<f64>,
    pub incidence: CsMat<f64>,
}

#[derive(Clone, Copy, Debug)]
struct TreeEdge {
    edge: usize,
    parent: usize,
    child: usize,
}

struct TreeData {
    edges: Vec<TreeEdge>,
    edge_r: Vec<f64>,
    edge_x: Vec<f64>,
    parent: Vec<Option<usize>>,
    edge_to_bus: Vec<Option<usize>>,
}

#[derive(Clone, Copy, Debug)]
struct BranchEdge {
    from: usize,
    to: usize,
    r: f64,
    x: f64,
}

#[derive(Clone, Copy, Debug)]
struct MstWeight {
    impedance: f64,
    branch: usize,
}

impl PartialEq for MstWeight {
    fn eq(&self, other: &Self) -> bool {
        self.impedance.total_cmp(&other.impedance) == Ordering::Equal && self.branch == other.branch
    }
}

impl Eq for MstWeight {}

impl PartialOrd for MstWeight {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MstWeight {
    fn cmp(&self, other: &Self) -> Ordering {
        self.impedance
            .total_cmp(&other.impedance)
            .then(self.branch.cmp(&other.branch))
    }
}

/// Build LinDist3Flow path impedance matrices for a connected radial network.
///
/// `opts` is accepted for API symmetry with the other builders. LinDist3Flow is
/// a branch impedance path model, so taps, shifts, branch charging, and shunts
/// do not enter these matrices.
pub fn build_lindist3flow(
    case: &IndexedNetwork,
    _opts: &BuildOptions,
) -> Result<LinDist3FlowMatrices> {
    let report = case.connectivity_report();
    if report.n_components != 1 || !case.is_radial() {
        return Err(not_radial(
            report.n_buses,
            report.n_branches_in_service,
            report.n_components,
        ));
    }
    let root = case.reference_bus_index()?;
    let bus_count = case.n();
    let branch_count = report.n_branches_in_service;
    if branch_count != bus_count.saturating_sub(1) {
        return Err(not_radial(bus_count, branch_count, report.n_components));
    }

    let branch_edges = collect_branch_edges(case, bus_count, branch_count, report.n_components)?;
    build_from_branch_edges(&branch_edges, root, bus_count, report.n_components)
}

/// Build LinDist3Flow matrices after projecting a connected topology to a
/// minimum spanning tree.
///
/// This is an explicit topology reduction for slightly meshed cases. Edge
/// weights are branch impedance magnitudes `sqrt(r² + x²)`, with source branch
/// order as a deterministic tie-breaker. The returned incidence rows follow
/// the retained in-service branch order.
pub fn build_lindist3flow_spanning_tree(
    case: &IndexedNetwork,
    _opts: &BuildOptions,
) -> Result<LinDist3FlowMatrices> {
    let report = case.connectivity_report();
    if report.n_components != 1 {
        return Err(not_radial(
            report.n_buses,
            report.n_branches_in_service,
            report.n_components,
        ));
    }
    let root = case.reference_bus_index()?;
    let bus_count = case.n();
    let branch_count = report.n_branches_in_service;
    let branch_edges = collect_branch_edges(case, bus_count, branch_count, report.n_components)?;
    let selected = minimum_spanning_tree_edges(&branch_edges, bus_count);
    if selected.len() != bus_count.saturating_sub(1) {
        return Err(not_radial(bus_count, branch_count, report.n_components));
    }
    let projected: Vec<_> = selected.into_iter().map(|idx| branch_edges[idx]).collect();
    build_from_branch_edges(&projected, root, bus_count, report.n_components)
}

fn build_from_branch_edges(
    branch_edges: &[BranchEdge],
    root: usize,
    bus_count: usize,
    component_count: usize,
) -> Result<LinDist3FlowMatrices> {
    let branch_count = branch_edges.len();
    if branch_count != bus_count.saturating_sub(1) {
        return Err(not_radial(bus_count, branch_count, component_count));
    }
    let tree = radial_tree(branch_edges, root, bus_count, component_count)?;
    let reduced = reduced_index(bus_count, root);
    let incidence = branch_bus_incidence(
        &tree.edges,
        &reduced,
        bus_count,
        branch_count,
        component_count,
    )?;
    let paths = paths_to_root(
        &reduced,
        &tree.parent,
        &tree.edge_to_bus,
        root,
        bus_count,
        branch_count,
        component_count,
    )?;
    let (r, x) = path_impedance_matrices(&paths, &tree.edge_r, &tree.edge_x, bus_count);

    Ok(LinDist3FlowMatrices { r, x, incidence })
}

fn not_radial(bus_count: usize, branch_count: usize, component_count: usize) -> Error {
    Error::NotRadial {
        buses: bus_count,
        branches: branch_count,
        components: component_count,
    }
}

fn collect_branch_edges(
    case: &IndexedNetwork,
    bus_count: usize,
    branch_count: usize,
    component_count: usize,
) -> Result<Vec<BranchEdge>> {
    let mut edges = Vec::with_capacity(branch_count);
    for (row, br) in case.in_service_branches() {
        let from = case.bus_index(br.from).ok_or(Error::UnknownBus {
            bus_id: br.from,
            element_index: row,
        })?;
        let to = case.bus_index(br.to).ok_or(Error::UnknownBus {
            bus_id: br.to,
            element_index: row,
        })?;
        if from == to {
            return Err(not_radial(bus_count, branch_count, component_count));
        }
        if !br.r.is_finite() || !br.x.is_finite() {
            return Err(Error::NonFiniteSusceptance { row });
        }
        edges.push(BranchEdge {
            from,
            to,
            r: br.r,
            x: br.x,
        });
    }
    Ok(edges)
}

fn minimum_spanning_tree_edges(branch_edges: &[BranchEdge], bus_count: usize) -> Vec<usize> {
    let mut graph = UnGraph::<usize, MstWeight>::with_capacity(bus_count, branch_edges.len());
    let nodes: Vec<_> = (0..bus_count).map(|idx| graph.add_node(idx)).collect();
    for (branch, edge) in branch_edges.iter().enumerate() {
        graph.add_edge(
            nodes[edge.from],
            nodes[edge.to],
            MstWeight {
                impedance: edge.r.hypot(edge.x),
                branch,
            },
        );
    }
    let mut selected: Vec<_> = min_spanning_tree(&graph)
        .filter_map(|element| match element {
            Element::Edge { weight, .. } => Some(weight.branch),
            Element::Node { .. } => None,
        })
        .collect();
    selected.sort_unstable();
    selected
}

fn radial_tree(
    branch_edges: &[BranchEdge],
    root: usize,
    bus_count: usize,
    component_count: usize,
) -> Result<TreeData> {
    let branch_count = branch_edges.len();
    let mut adjacency = vec![Vec::<(usize, usize)>::new(); bus_count];
    let mut edge_r = Vec::with_capacity(branch_count);
    let mut edge_x = Vec::with_capacity(branch_count);
    for (edge, br) in branch_edges.iter().enumerate() {
        adjacency[br.from].push((br.to, edge));
        adjacency[br.to].push((br.from, edge));
        edge_r.push(br.r);
        edge_x.push(br.x);
    }

    let mut visited = vec![false; bus_count];
    let mut parent = vec![None::<usize>; bus_count];
    let mut edge_to_bus = vec![None::<usize>; bus_count];
    let mut edges = Vec::<TreeEdge>::with_capacity(branch_count);
    let mut queue = VecDeque::from([root]);
    visited[root] = true;
    while let Some(bus) = queue.pop_front() {
        for &(next, edge) in &adjacency[bus] {
            if Some(next) == parent[bus] {
                continue;
            }
            if visited[next] {
                return Err(not_radial(bus_count, branch_count, component_count));
            }
            edges.push(TreeEdge {
                edge,
                parent: bus,
                child: next,
            });
            visited[next] = true;
            parent[next] = Some(bus);
            edge_to_bus[next] = Some(edge);
            queue.push_back(next);
        }
    }
    if visited.iter().any(|v| !v) || edges.len() != branch_count {
        return Err(not_radial(bus_count, branch_count, component_count));
    }

    Ok(TreeData {
        edges,
        edge_r,
        edge_x,
        parent,
        edge_to_bus,
    })
}

fn branch_bus_incidence(
    edges: &[TreeEdge],
    reduced: &[Option<usize>],
    bus_count: usize,
    branch_count: usize,
    component_count: usize,
) -> Result<CsMat<f64>> {
    let mut incidence =
        CooBuilder::with_capacity_rect(branch_count, bus_count - 1, 2 * branch_count);
    for e in edges {
        let child_col =
            reduced[e.child].ok_or(not_radial(bus_count, branch_count, component_count))?;
        incidence.add(e.edge, child_col, 1.0);
        if let Some(parent_col) = reduced[e.parent] {
            incidence.add(e.edge, parent_col, -1.0);
        }
    }
    Ok(incidence.finish_csr())
}

fn paths_to_root(
    reduced: &[Option<usize>],
    parent: &[Option<usize>],
    edge_to_bus: &[Option<usize>],
    root: usize,
    bus_count: usize,
    branch_count: usize,
    component_count: usize,
) -> Result<Vec<Vec<usize>>> {
    let mut paths = vec![Vec::<usize>::new(); bus_count - 1];
    for (bus, &maybe_col) in reduced.iter().enumerate() {
        let Some(col) = maybe_col else { continue };
        let mut path = Vec::new();
        let mut at = bus;
        while at != root {
            let edge =
                edge_to_bus[at].ok_or(not_radial(bus_count, branch_count, component_count))?;
            path.push(edge);
            at = parent[at].ok_or(not_radial(bus_count, branch_count, component_count))?;
        }
        path.reverse();
        paths[col] = path;
    }
    Ok(paths)
}

fn path_impedance_matrices(
    paths: &[Vec<usize>],
    edge_r: &[f64],
    edge_x: &[f64],
    bus_count: usize,
) -> (CsMat<f64>, CsMat<f64>) {
    let mut r_path = CooBuilder::with_capacity(bus_count - 1, (bus_count - 1) * (bus_count - 1));
    let mut x_path = CooBuilder::with_capacity(bus_count - 1, (bus_count - 1) * (bus_count - 1));
    for i in 0..paths.len() {
        for j in i..paths.len() {
            let mut rij = 0.0;
            let mut xij = 0.0;
            for (&ei, &ej) in paths[i].iter().zip(&paths[j]) {
                if ei != ej {
                    break;
                }
                rij += edge_r[ei];
                xij += edge_x[ei];
            }
            r_path.add_sym(i, j, rij);
            x_path.add_sym(i, j, xij);
        }
    }
    (r_path.finish_csr(), x_path.finish_csr())
}

fn reduced_index(n: usize, root: usize) -> Vec<Option<usize>> {
    let mut next = 0usize;
    let mut out = vec![None; n];
    for (idx, slot) in out.iter_mut().enumerate() {
        if idx != root {
            *slot = Some(next);
            next += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use approx::assert_relative_eq;

    use super::*;
    use crate::indexed::IndexedNetwork;
    use crate::matrix::{BuildOptions, build_lacpf};
    use crate::network::{Branch, Bus, BusId, BusType, Extras, Network};

    fn bus(id: usize, kind: BusType) -> Bus {
        Bus {
            id: BusId(id),
            kind,
            vm: 1.0,
            va: 0.0,
            base_kv: 12.47,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: None,
            extras: Extras::new(),
        }
    }

    fn br(from: usize, to: usize, r: f64, x: f64) -> Branch {
        Branch {
            from: BusId(from),
            to: BusId(to),
            r,
            x,
            b: 0.0,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
            tap: 0.0,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            extras: Extras::new(),
        }
    }

    fn radial_three_bus() -> Network {
        Network::in_memory(
            "radial",
            100.0,
            vec![
                bus(1, BusType::Ref),
                bus(2, BusType::Pq),
                bus(3, BusType::Pq),
            ],
            vec![br(1, 2, 0.01, 0.10), br(2, 3, 0.02, 0.20)],
        )
    }

    #[test]
    fn three_bus_path_impedances_are_hand_verified() {
        let net = radial_three_bus();
        let view = IndexedNetwork::new(&net);
        let mats = build_lindist3flow(&view, &BuildOptions::default()).unwrap();
        let r = mats.r.to_dense();
        let x = mats.x.to_dense();
        let a = mats.incidence.to_dense();

        assert_relative_eq!(r[[0, 0]], 0.01, epsilon = 1e-12);
        assert_relative_eq!(r[[0, 1]], 0.01, epsilon = 1e-12);
        assert_relative_eq!(r[[1, 0]], 0.01, epsilon = 1e-12);
        assert_relative_eq!(r[[1, 1]], 0.03, epsilon = 1e-12);
        assert_relative_eq!(x[[0, 0]], 0.10, epsilon = 1e-12);
        assert_relative_eq!(x[[0, 1]], 0.10, epsilon = 1e-12);
        assert_relative_eq!(x[[1, 0]], 0.10, epsilon = 1e-12);
        assert_relative_eq!(x[[1, 1]], 0.30, epsilon = 1e-12);

        assert_relative_eq!(a[[0, 0]], 1.0, epsilon = 1e-12);
        assert_relative_eq!(a[[0, 1]], 0.0, epsilon = 1e-12);
        assert_relative_eq!(a[[1, 0]], -1.0, epsilon = 1e-12);
        assert_relative_eq!(a[[1, 1]], 1.0, epsilon = 1e-12);
    }

    #[test]
    fn inverse_matches_grounded_lacpf_block() {
        let net = radial_three_bus();
        let view = IndexedNetwork::new(&net);
        let mats = build_lindist3flow(&view, &BuildOptions::default()).unwrap();
        let j = build_lacpf(&view, &BuildOptions::default())
            .unwrap()
            .to_dense();
        let r = mats.r.to_dense();
        let x = mats.x.to_dense();
        let n = view.n();
        let root = view.reference_bus_index().unwrap();
        let keep: Vec<usize> = (0..n).filter(|&i| i != root).collect();
        let dim = 2 * keep.len();
        let mut product = vec![vec![0.0; dim]; dim];

        for row in 0..dim {
            for col in 0..dim {
                let mut sum = 0.0;
                for k in 0..dim {
                    let jr = if row < keep.len() {
                        keep[row]
                    } else {
                        n + keep[row - keep.len()]
                    };
                    let jk = if k < keep.len() {
                        keep[k]
                    } else {
                        n + keep[k - keep.len()]
                    };
                    let inv = if k < keep.len() && col < keep.len() {
                        r[[k, col]]
                    } else if k < keep.len() {
                        x[[k, col - keep.len()]]
                    } else if col < keep.len() {
                        x[[k - keep.len(), col]]
                    } else {
                        -r[[k - keep.len(), col - keep.len()]]
                    };
                    sum += j[[jr, jk]] * inv;
                }
                product[row][col] = sum;
            }
        }

        for (i, row) in product.iter().enumerate() {
            for (j, &v) in row.iter().enumerate() {
                assert_relative_eq!(v, if i == j { 1.0 } else { 0.0 }, epsilon = 1e-9);
            }
        }
    }

    #[test]
    fn incidence_rows_follow_in_service_branch_order() {
        let net = Network::in_memory(
            "reversed",
            100.0,
            vec![
                bus(1, BusType::Ref),
                bus(2, BusType::Pq),
                bus(3, BusType::Pq),
            ],
            vec![br(2, 3, 0.02, 0.20), br(1, 2, 0.01, 0.10)],
        );
        let mats = build_lindist3flow(&IndexedNetwork::new(&net), &BuildOptions::default())
            .unwrap()
            .incidence
            .to_dense();

        assert_relative_eq!(mats[[0, 0]], -1.0, epsilon = 1e-12);
        assert_relative_eq!(mats[[0, 1]], 1.0, epsilon = 1e-12);
        assert_relative_eq!(mats[[1, 0]], 1.0, epsilon = 1e-12);
        assert_relative_eq!(mats[[1, 1]], 0.0, epsilon = 1e-12);
    }

    #[test]
    fn meshed_case_returns_not_radial() {
        let net = Network::in_memory(
            "mesh",
            100.0,
            vec![
                bus(1, BusType::Ref),
                bus(2, BusType::Pq),
                bus(3, BusType::Pq),
            ],
            vec![
                br(1, 2, 0.01, 0.10),
                br(2, 3, 0.02, 0.20),
                br(1, 3, 0.03, 0.30),
            ],
        );
        let err =
            build_lindist3flow(&IndexedNetwork::new(&net), &BuildOptions::default()).unwrap_err();
        assert!(matches!(err, Error::NotRadial { .. }));
    }

    #[test]
    fn spanning_tree_projection_uses_minimum_impedance_edges() {
        let net = Network::in_memory(
            "mesh",
            100.0,
            vec![
                bus(1, BusType::Ref),
                bus(2, BusType::Pq),
                bus(3, BusType::Pq),
            ],
            vec![
                br(1, 2, 0.01, 0.10),
                br(2, 3, 0.02, 0.20),
                br(1, 3, 1.00, 1.00),
            ],
        );
        let mats =
            build_lindist3flow_spanning_tree(&IndexedNetwork::new(&net), &BuildOptions::default())
                .unwrap();
        let r = mats.r.to_dense();
        let a = mats.incidence.to_dense();

        assert_eq!(mats.incidence.rows(), 2);
        assert_relative_eq!(r[[0, 0]], 0.01, epsilon = 1e-12);
        assert_relative_eq!(r[[0, 1]], 0.01, epsilon = 1e-12);
        assert_relative_eq!(r[[1, 1]], 0.03, epsilon = 1e-12);
        assert_relative_eq!(a[[0, 0]], 1.0, epsilon = 1e-12);
        assert_relative_eq!(a[[1, 0]], -1.0, epsilon = 1e-12);
        assert_relative_eq!(a[[1, 1]], 1.0, epsilon = 1e-12);
    }

    #[test]
    fn ieee13_opendss_fixture_builds_spanning_tree_matrices() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss");
        let dist = powerio_dist::dss::parse_dss_file(fixture).unwrap();
        let network = dist_topology_as_network(&dist);
        let view = IndexedNetwork::new(&network);
        let mats = build_lindist3flow_spanning_tree(&view, &BuildOptions::default()).unwrap();

        assert!(view.n() > 10);
        assert_eq!(mats.r.rows(), view.n() - 1);
        assert_eq!(mats.r.cols(), view.n() - 1);
        assert_eq!(mats.x.rows(), view.n() - 1);
        assert_eq!(mats.incidence.rows(), view.n() - 1);
        assert_eq!(mats.incidence.cols(), view.n() - 1);
        assert!(mats.r.nnz() >= view.n() - 1);
        assert!(mats.x.nnz() >= view.n() - 1);
    }

    fn dist_topology_as_network(dist: &powerio_dist::DistNetwork) -> Network {
        let source = dist.sources.first().map(|s| s.bus.as_str());
        let mut endpoints = BTreeSet::<String>::new();
        for line in &dist.lines {
            endpoints.insert(line.bus_from.clone());
            endpoints.insert(line.bus_to.clone());
        }
        for switch in &dist.switches {
            if !switch.open {
                endpoints.insert(switch.bus_from.clone());
                endpoints.insert(switch.bus_to.clone());
            }
        }
        for transformer in &dist.transformers {
            if let [from, to, ..] = transformer.windings.as_slice() {
                endpoints.insert(from.bus.clone());
                endpoints.insert(to.bus.clone());
            }
        }
        if let Some(source) = source {
            endpoints.insert(source.to_string());
        }

        let ref_bus = source
            .filter(|bus| endpoints.contains(*bus))
            .or_else(|| endpoints.iter().next().map(String::as_str));
        let mut ids = BTreeMap::new();
        let buses: Vec<_> = endpoints
            .iter()
            .enumerate()
            .map(|(idx, name)| {
                let id = BusId(idx + 1);
                ids.insert(name.clone(), id);
                Bus {
                    id,
                    kind: if Some(name.as_str()) == ref_bus {
                        BusType::Ref
                    } else {
                        BusType::Pq
                    },
                    vm: 1.0,
                    va: 0.0,
                    base_kv: 12.47,
                    vmax: 1.1,
                    vmin: 0.9,
                    evhi: None,
                    evlo: None,
                    area: 1,
                    zone: 1,
                    name: Some(name.clone()),
                    extras: Extras::new(),
                }
            })
            .collect();

        let mut branches = Vec::new();
        for line in &dist.lines {
            if let (Some(&from), Some(&to)) = (ids.get(&line.bus_from), ids.get(&line.bus_to)) {
                let (r, x) = line_impedance(dist, line);
                branches.push(br(from.0, to.0, r, x));
            }
        }
        for switch in &dist.switches {
            if !switch.open
                && let (Some(&from), Some(&to)) =
                    (ids.get(&switch.bus_from), ids.get(&switch.bus_to))
            {
                branches.push(br(from.0, to.0, 1e-6, 1e-6));
            }
        }
        for transformer in &dist.transformers {
            if let [from_winding, to_winding, ..] = transformer.windings.as_slice()
                && let (Some(&from), Some(&to)) =
                    (ids.get(&from_winding.bus), ids.get(&to_winding.bus))
            {
                let r = ((from_winding.r_pct + to_winding.r_pct) / 100.0).max(1e-6);
                let x = transformer
                    .xsc_pct
                    .first()
                    .map_or(1e-6, |x| (x / 100.0).max(1e-6));
                branches.push(br(from.0, to.0, r, x));
            }
        }

        Network::in_memory("ieee13 topology", 100.0, buses, branches)
    }

    fn line_impedance(
        dist: &powerio_dist::DistNetwork,
        line: &powerio_dist::DistLine,
    ) -> (f64, f64) {
        let Some(code) = dist.linecode(&line.linecode) else {
            return (1e-6, 1e-6);
        };
        let r = diagonal_value(&code.r_series) * line.length;
        let x = diagonal_value(&code.x_series) * line.length;
        (r.max(1e-6), x.max(1e-6))
    }

    fn diagonal_value(matrix: &[Vec<f64>]) -> f64 {
        matrix
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| row.get(idx))
            .copied()
            .find(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(1e-6)
    }
}

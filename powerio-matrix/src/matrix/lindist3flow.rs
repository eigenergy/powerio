//! LinDist3Flow path impedance matrices for a connected radial case.
//!
//! The returned `incidence` is the reduced branch-by-bus incidence `A`: rows
//! follow in-service branch order and are oriented away from the reference bus,
//! columns are all non-reference buses in dense index order, and each row has
//! `+1` at the downstream bus and `-1` at the upstream bus when that upstream
//! bus is not the reference. The `r` and `x` matrices are `A^-1 diag(r) A^-T` and
//! `A^-1 diag(x) A^-T`, the closed-form inverse blocks of the grounded LACPF
//! matrix on a radial network with no shunts.

use std::collections::VecDeque;

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
        return Err(Error::NotRadial {
            buses: report.n_buses,
            branches: report.n_branches_in_service,
            components: report.n_components,
        });
    }
    let root = case.reference_bus_index()?;
    let bus_count = case.n();
    let branch_count = report.n_branches_in_service;
    if branch_count != bus_count.saturating_sub(1) {
        return Err(Error::NotRadial {
            buses: bus_count,
            branches: branch_count,
            components: report.n_components,
        });
    }

    let tree = radial_tree(case, root, bus_count, branch_count, report.n_components)?;
    let reduced = reduced_index(bus_count, root);
    let incidence = branch_bus_incidence(
        &tree.edges,
        &reduced,
        bus_count,
        branch_count,
        report.n_components,
    )?;
    let paths = paths_to_root(
        &reduced,
        &tree.parent,
        &tree.edge_to_bus,
        root,
        bus_count,
        branch_count,
        report.n_components,
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

fn radial_tree(
    case: &IndexedNetwork,
    root: usize,
    bus_count: usize,
    branch_count: usize,
    component_count: usize,
) -> Result<TreeData> {
    let mut adjacency = vec![Vec::<(usize, usize)>::new(); bus_count];
    let mut edge_r = Vec::with_capacity(branch_count);
    let mut edge_x = Vec::with_capacity(branch_count);
    for (edge, (row, br)) in case.in_service_branches().enumerate() {
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
        adjacency[from].push((to, edge));
        adjacency[to].push((from, edge));
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
}

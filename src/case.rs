//! Domain types for a parsed power network case.

use std::collections::HashMap;

use petgraph::graph::UnGraph;

/// Bus type per MATPOWER convention: 1=PQ, 2=PV, 3=ref/slack, 4=isolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BusType {
    Pq = 1,
    Pv = 2,
    Ref = 3,
    Isolated = 4,
}

impl BusType {
    fn from_f64(v: f64) -> Self {
        match v as i32 {
            2 => Self::Pv,
            3 => Self::Ref,
            4 => Self::Isolated,
            _ => Self::Pq,
        }
    }
}

/// A single bus in MATPOWER form. Values stored in the natural MATPOWER
/// units (MW, MVAr, p.u.). Conversion to per unit happens at consumption.
#[derive(Debug, Clone)]
pub struct Bus {
    /// Original (1-based) MATPOWER bus id, possibly non-contiguous.
    pub id: usize,
    pub kind: BusType,
    /// Active load demand (MW).
    pub pd: f64,
    /// Reactive load demand (MVAr).
    pub qd: f64,
    /// Shunt conductance (MW at V = 1.0 p.u.).
    pub gs: f64,
    /// Shunt susceptance (MVAr injected at V = 1.0 p.u.).
    pub bs: f64,
    pub area: usize,
    /// Voltage magnitude (p.u.).
    pub vm: f64,
    /// Voltage angle (degrees).
    pub va: f64,
    pub base_kv: f64,
    pub zone: usize,
    pub vmax: f64,
    pub vmin: f64,
}

/// A single transmission element (line or transformer) in MATPOWER form.
#[derive(Debug, Clone)]
pub struct Branch {
    pub from_id: usize,
    pub to_id: usize,
    /// Series resistance (p.u.).
    pub r: f64,
    /// Series reactance (p.u.).
    pub x: f64,
    /// Total line charging susceptance (p.u.). Half goes to each end.
    pub b: f64,
    pub rate_a: f64,
    pub rate_b: f64,
    pub rate_c: f64,
    /// Tap ratio. MATPOWER convention: 0 means "no tap" (treat as 1).
    pub tap: f64,
    /// Phase shift (degrees).
    pub shift: f64,
    /// 1 = in service, 0 = out.
    pub status: f64,
    pub angmin: f64,
    pub angmax: f64,
}

impl Branch {
    /// Effective tap ratio (0 → 1 per MATPOWER convention).
    #[inline]
    pub fn effective_tap(&self) -> f64 {
        if self.tap == 0.0 { 1.0 } else { self.tap }
    }

    #[inline]
    pub fn is_in_service(&self) -> bool {
        self.status == 1.0
    }

    /// Series admittance `y = 1 / (r + j x)`. Returns `None` if zero impedance.
    #[inline]
    pub fn series_admittance(&self) -> Option<num_complex::Complex<f64>> {
        let denom = self.r * self.r + self.x * self.x;
        if denom == 0.0 {
            None
        } else {
            Some(num_complex::Complex::new(self.r / denom, -self.x / denom))
        }
    }
}

/// Parsed MATPOWER case with a stable mapping from MATPOWER bus ids to
/// dense `[0, n)` indices.
#[derive(Debug, Clone)]
pub struct MpcCase {
    pub name: String,
    pub base_mva: f64,
    pub buses: Vec<Bus>,
    pub branches: Vec<Branch>,
    /// MATPOWER bus id → dense index in [0, n).
    bus_id_to_idx: HashMap<usize, usize>,
}

impl MpcCase {
    pub fn new(
        name: impl Into<String>,
        base_mva: f64,
        buses: Vec<Bus>,
        branches: Vec<Branch>,
    ) -> Self {
        let bus_id_to_idx = buses
            .iter()
            .enumerate()
            .map(|(idx, bus)| (bus.id, idx))
            .collect();
        Self {
            name: name.into(),
            base_mva,
            buses,
            branches,
            bus_id_to_idx,
        }
    }

    #[inline]
    pub fn n(&self) -> usize {
        self.buses.len()
    }

    /// Resolve a 1-based MATPOWER bus id to a dense `[0, n)` index.
    #[inline]
    pub fn bus_index(&self, bus_id: usize) -> Option<usize> {
        self.bus_id_to_idx.get(&bus_id).copied()
    }

    pub fn in_service_branches(&self) -> impl Iterator<Item = (usize, &Branch)> {
        self.branches
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_in_service())
    }

    /// Build an undirected graph view of the case where:
    /// - **node weights** are dense bus indices `[0, n)`,
    /// - **edge weights** are branch indices into `self.branches` (so the
    ///   caller can recover series impedance, charging, taps, etc.).
    ///
    /// Out of service branches are skipped. Parallel branches between the
    /// same pair of buses are preserved as separate edges.
    pub fn to_petgraph(&self) -> UnGraph<usize, usize> {
        let mut g = UnGraph::with_capacity(self.n(), self.branches.len());
        let nodes: Vec<_> = (0..self.n()).map(|i| g.add_node(i)).collect();
        for (idx, br) in self.in_service_branches() {
            if let (Some(i), Some(j)) =
                (self.bus_index(br.from_id), self.bus_index(br.to_id))
            {
                g.add_edge(nodes[i], nodes[j], idx);
            }
        }
        g
    }

    /// Number of connected components in the in-service topology.
    /// `1` for a healthy single island case; `> 1` indicates electrical
    /// islands the user may not have intended.
    pub fn n_connected_components(&self) -> usize {
        petgraph::algo::connected_components(&self.to_petgraph())
    }

    /// True iff the in-service topology is a forest (no cycles, possibly
    /// with multiple disconnected trees / isolated nodes). LinDist3Flow
    /// and the closed form Talkington inverse `[[R, X], [X, -R]]` require
    /// radiality. Uses the forest invariant `|E| = |V| - n_components`.
    pub fn is_radial(&self) -> bool {
        let g = self.to_petgraph();
        let n_components = petgraph::algo::connected_components(&g);
        g.edge_count() == g.node_count().saturating_sub(n_components)
    }

    /// One shot diagnostic report covering the topological invariants the
    /// TUI Inspect screen and downstream solvers care about.
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
            n_branches_in_service: self.branches.iter().filter(|b| b.is_in_service()).count(),
            n_components,
            isolated_buses: isolated,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConnectivityReport {
    pub n_buses: usize,
    pub n_branches_in_service: usize,
    pub n_components: usize,
    /// Dense bus indices that have no incident in-service branches.
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
    use super::*;

    fn br(from: usize, to: usize) -> Branch {
        Branch {
            from_id: from,
            to_id: to,
            r: 0.0,
            x: 0.1,
            b: 0.0,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
            tap: 0.0,
            shift: 0.0,
            status: 1.0,
            angmin: -360.0,
            angmax: 360.0,
        }
    }

    fn bus(id: usize) -> Bus {
        Bus {
            id,
            kind: BusType::Pq,
            pd: 0.0,
            qd: 0.0,
            gs: 0.0,
            bs: 0.0,
            area: 1,
            vm: 1.0,
            va: 0.0,
            base_kv: 345.0,
            zone: 1,
            vmax: 1.1,
            vmin: 0.9,
        }
    }

    #[test]
    fn radial_3bus_tree() {
        let case = MpcCase::new(
            "tree",
            100.0,
            vec![bus(1), bus(2), bus(3)],
            vec![br(1, 2), br(2, 3)],
        );
        assert!(case.is_radial());
        assert_eq!(case.n_connected_components(), 1);
        let report = case.connectivity_report();
        assert!(report.is_single_island());
    }

    #[test]
    fn meshed_3bus_triangle_is_not_radial() {
        let case = MpcCase::new(
            "triangle",
            100.0,
            vec![bus(1), bus(2), bus(3)],
            vec![br(1, 2), br(2, 3), br(3, 1)],
        );
        assert!(!case.is_radial());
        assert_eq!(case.n_connected_components(), 1);
    }

    #[test]
    fn islanded_pair_detected() {
        // Buses 1-2 connected, bus 3 isolated.
        let case = MpcCase::new(
            "islanded",
            100.0,
            vec![bus(1), bus(2), bus(3)],
            vec![br(1, 2)],
        );
        assert_eq!(case.n_connected_components(), 2);
        let report = case.connectivity_report();
        assert!(!report.is_single_island());
        assert_eq!(report.isolated_buses, vec![2]); // dense index of bus 3
    }
}

/// Bus matrix column indices per the MATPOWER manual (0-based).
pub mod bus_col {
    pub const BUS_I: usize = 0;
    pub const BUS_TYPE: usize = 1;
    pub const PD: usize = 2;
    pub const QD: usize = 3;
    pub const GS: usize = 4;
    pub const BS: usize = 5;
    pub const BUS_AREA: usize = 6;
    pub const VM: usize = 7;
    pub const VA: usize = 8;
    pub const BASE_KV: usize = 9;
    pub const ZONE: usize = 10;
    pub const VMAX: usize = 11;
    pub const VMIN: usize = 12;
    pub const REQUIRED: usize = 13;
}

/// Branch matrix column indices per the MATPOWER manual (0-based).
pub mod branch_col {
    pub const F_BUS: usize = 0;
    pub const T_BUS: usize = 1;
    pub const BR_R: usize = 2;
    pub const BR_X: usize = 3;
    pub const BR_B: usize = 4;
    pub const RATE_A: usize = 5;
    pub const RATE_B: usize = 6;
    pub const RATE_C: usize = 7;
    pub const TAP: usize = 8;
    pub const SHIFT: usize = 9;
    pub const BR_STATUS: usize = 10;
    pub const ANGMIN: usize = 11;
    pub const ANGMAX: usize = 12;
    pub const REQUIRED: usize = 13;
}

impl Bus {
    pub fn from_row(row: &[f64], row_idx: usize) -> crate::Result<Self> {
        if row.len() < bus_col::REQUIRED {
            return Err(crate::Error::ShortRow {
                field: "bus",
                row: row_idx,
                expected: bus_col::REQUIRED,
                got: row.len(),
            });
        }
        Ok(Self {
            id: row[bus_col::BUS_I] as usize,
            kind: BusType::from_f64(row[bus_col::BUS_TYPE]),
            pd: row[bus_col::PD],
            qd: row[bus_col::QD],
            gs: row[bus_col::GS],
            bs: row[bus_col::BS],
            area: row[bus_col::BUS_AREA] as usize,
            vm: row[bus_col::VM],
            va: row[bus_col::VA],
            base_kv: row[bus_col::BASE_KV],
            zone: row[bus_col::ZONE] as usize,
            vmax: row[bus_col::VMAX],
            vmin: row[bus_col::VMIN],
        })
    }
}

impl Branch {
    pub fn from_row(row: &[f64], row_idx: usize) -> crate::Result<Self> {
        if row.len() < branch_col::REQUIRED {
            return Err(crate::Error::ShortRow {
                field: "branch",
                row: row_idx,
                expected: branch_col::REQUIRED,
                got: row.len(),
            });
        }
        Ok(Self {
            from_id: row[branch_col::F_BUS] as usize,
            to_id: row[branch_col::T_BUS] as usize,
            r: row[branch_col::BR_R],
            x: row[branch_col::BR_X],
            b: row[branch_col::BR_B],
            rate_a: row[branch_col::RATE_A],
            rate_b: row[branch_col::RATE_B],
            rate_c: row[branch_col::RATE_C],
            tap: row[branch_col::TAP],
            shift: row[branch_col::SHIFT],
            status: row[branch_col::BR_STATUS],
            angmin: row[branch_col::ANGMIN],
            angmax: row[branch_col::ANGMAX],
        })
    }
}

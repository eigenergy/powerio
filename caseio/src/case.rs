//! Domain types for a parsed power network case.

use std::collections::HashMap;
use std::sync::Arc;

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
    /// Human-readable label from `mpc.bus_name`, by position. `None` when the
    /// case has no `bus_name` block or its length doesn't match the bus count.
    pub name: Option<String>,
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

/// A two-terminal HVDC line (`mpc.dcline` row, MATPOWER 17-column layout).
#[derive(Debug, Clone)]
pub struct DcLine {
    pub from_id: usize,
    pub to_id: usize,
    /// 1 = in service, 0 = out.
    pub status: f64,
    /// Real power injected at the from / to ends (MW).
    pub pf: f64,
    pub pt: f64,
    /// Reactive power at the from / to ends (MVAr).
    pub qf: f64,
    pub qt: f64,
    /// Voltage set points at the from / to ends (p.u.).
    pub vf: f64,
    pub vt: f64,
    pub pmin: f64,
    pub pmax: f64,
    pub qminf: f64,
    pub qmaxf: f64,
    pub qmint: f64,
    pub qmaxt: f64,
    /// Loss model `loss = loss0 + loss1·Pf`.
    pub loss0: f64,
    pub loss1: f64,
    /// Columns past the 17-column layout (e.g. `mu_*` shadow prices), verbatim.
    pub extra: Vec<f64>,
}

impl DcLine {
    #[inline]
    pub fn is_in_service(&self) -> bool {
        self.status == 1.0
    }

    pub fn from_row(row: &[f64], row_idx: usize) -> crate::Result<Self> {
        if row.len() < dcline_col::REQUIRED {
            return Err(crate::Error::ShortRow {
                field: "dcline",
                row: row_idx,
                expected: dcline_col::REQUIRED,
                got: row.len(),
            });
        }
        Ok(Self {
            from_id: row[dcline_col::F_BUS] as usize,
            to_id: row[dcline_col::T_BUS] as usize,
            status: row[dcline_col::BR_STATUS],
            pf: row[dcline_col::PF],
            pt: row[dcline_col::PT],
            qf: row[dcline_col::QF],
            qt: row[dcline_col::QT],
            vf: row[dcline_col::VF],
            vt: row[dcline_col::VT],
            pmin: row[dcline_col::PMIN],
            pmax: row[dcline_col::PMAX],
            qminf: row[dcline_col::QMINF],
            qmaxf: row[dcline_col::QMAXF],
            qmint: row[dcline_col::QMINT],
            qmaxt: row[dcline_col::QMAXT],
            loss0: row[dcline_col::LOSS0],
            loss1: row[dcline_col::LOSS1],
            extra: row[dcline_col::REQUIRED..].to_vec(),
        })
    }
}

/// A generator (`mpc.gen` row) with its cost curve (`mpc.gencost` row)
/// folded in. MATPOWER guarantees one gencost row per gen row in the same
/// order, so the cost rides along instead of living in a parallel vector.
#[derive(Debug, Clone)]
pub struct Generator {
    /// Bus the generator sits on (1-based MATPOWER id).
    pub bus_id: usize,
    /// Real power dispatch set point (MW).
    pub pg: f64,
    /// Reactive power dispatch set point (MVAr).
    pub qg: f64,
    pub qmax: f64,
    pub qmin: f64,
    /// Voltage magnitude set point (p.u.).
    pub vg: f64,
    pub mbase: f64,
    /// 1 = in service, 0 = out.
    pub status: f64,
    /// Real power upper bound (MW).
    pub pmax: f64,
    /// Real power lower bound (MW).
    pub pmin: f64,
    pub cost: Option<GenCost>,
    /// Columns past `PMIN` (`Pc1, Pc2, Qc1min, Qc1max, Qc2min, Qc2max,
    /// ramp_agc, ramp_10, ramp_30, ramp_q, apf`), kept verbatim so unit
    /// commitment / AGC data isn't silently dropped.
    pub extra: Vec<f64>,
}

impl Generator {
    #[inline]
    pub fn is_in_service(&self) -> bool {
        self.status == 1.0
    }

    pub fn from_row(row: &[f64], row_idx: usize) -> crate::Result<Self> {
        if row.len() < gen_col::REQUIRED {
            return Err(crate::Error::ShortRow {
                field: "gen",
                row: row_idx,
                expected: gen_col::REQUIRED,
                got: row.len(),
            });
        }
        Ok(Self {
            bus_id: row[gen_col::GEN_BUS] as usize,
            pg: row[gen_col::PG],
            qg: row[gen_col::QG],
            qmax: row[gen_col::QMAX],
            qmin: row[gen_col::QMIN],
            vg: row[gen_col::VG],
            mbase: row[gen_col::MBASE],
            status: row[gen_col::GEN_STATUS],
            pmax: row[gen_col::PMAX],
            pmin: row[gen_col::PMIN],
            cost: None,
            extra: row[gen_col::REQUIRED..].to_vec(),
        })
    }
}

/// A generator cost curve (`mpc.gencost` row).
#[derive(Debug, Clone)]
pub struct GenCost {
    /// 1 = piecewise linear, 2 = polynomial.
    pub model: u8,
    pub startup: f64,
    pub shutdown: f64,
    /// Number of cost coefficients (polynomial) or breakpoints (piecewise).
    pub ncost: usize,
    /// Raw coefficients, highest order first for the polynomial model:
    /// `[c_{k-1}, …, c1, c0]`.
    pub coeffs: Vec<f64>,
}

impl GenCost {
    pub fn from_row(row: &[f64], row_idx: usize) -> crate::Result<Self> {
        if row.len() < gencost_col::REQUIRED {
            return Err(crate::Error::ShortRow {
                field: "gencost",
                row: row_idx,
                expected: gencost_col::REQUIRED,
                got: row.len(),
            });
        }
        Ok(Self {
            model: row[gencost_col::MODEL] as u8,
            startup: row[gencost_col::STARTUP],
            shutdown: row[gencost_col::SHUTDOWN],
            ncost: row[gencost_col::NCOST] as usize,
            coeffs: row[gencost_col::COEFF0..].to_vec(),
        })
    }

    /// `(q, c)` for the quadratic cost `½ q p² + c p` from a polynomial
    /// (model 2) row. MATPOWER stores `c2 p² + c1 p + c0`, so `q = 2·c2` and
    /// `c = c1`. Linear rows (`ncost == 2`) give `q = 0`. Piecewise (model 1)
    /// or cubic and higher return `None`.
    pub fn quadratic(&self) -> Option<(f64, f64)> {
        if self.model != 2 {
            return None;
        }
        // Reject a row whose coefficient slice is shorter than `ncost` claims,
        // rather than reading the wrong powers by position. The guard makes the
        // indexing below infallible.
        if self.coeffs.len() < self.ncost {
            return None;
        }
        match self.ncost {
            3 => Some((2.0 * self.coeffs[0], self.coeffs[1])),
            2 => Some((0.0, self.coeffs[0])),
            1 => Some((0.0, 0.0)),
            _ => None,
        }
    }
}

/// A storage unit (`mpc.storage` row), PowerModels / pglib 17-column layout.
/// Power values are in MATPOWER units (MW, MVAr), converted to per unit at
/// consumption like every other quantity here.
#[derive(Debug, Clone)]
pub struct Storage {
    /// Bus the unit sits on (1-based MATPOWER id).
    pub bus_id: usize,
    /// Real power output set point (MW).
    pub ps: f64,
    /// Reactive power output set point (MVAr).
    pub qs: f64,
    /// Stored energy (MWh).
    pub energy: f64,
    pub energy_rating: f64,
    pub charge_rating: f64,
    pub discharge_rating: f64,
    pub charge_efficiency: f64,
    pub discharge_efficiency: f64,
    pub thermal_rating: f64,
    pub qmin: f64,
    pub qmax: f64,
    /// Series resistance (p.u.).
    pub r: f64,
    /// Series reactance (p.u.).
    pub x: f64,
    /// Standby real power loss (MW).
    pub p_loss: f64,
    /// Standby reactive power loss (MVAr).
    pub q_loss: f64,
    /// 1 = in service, 0 = out.
    pub status: f64,
}

impl Storage {
    #[inline]
    pub fn is_in_service(&self) -> bool {
        self.status == 1.0
    }

    pub fn from_row(row: &[f64], row_idx: usize) -> crate::Result<Self> {
        if row.len() < storage_col::REQUIRED {
            return Err(crate::Error::ShortRow {
                field: "storage",
                row: row_idx,
                expected: storage_col::REQUIRED,
                got: row.len(),
            });
        }
        Ok(Self {
            bus_id: row[storage_col::STORAGE_BUS] as usize,
            ps: row[storage_col::PS],
            qs: row[storage_col::QS],
            energy: row[storage_col::ENERGY],
            energy_rating: row[storage_col::ENERGY_RATING],
            charge_rating: row[storage_col::CHARGE_RATING],
            discharge_rating: row[storage_col::DISCHARGE_RATING],
            charge_efficiency: row[storage_col::CHARGE_EFFICIENCY],
            discharge_efficiency: row[storage_col::DISCHARGE_EFFICIENCY],
            thermal_rating: row[storage_col::THERMAL_RATING],
            qmin: row[storage_col::QMIN],
            qmax: row[storage_col::QMAX],
            r: row[storage_col::R],
            x: row[storage_col::X],
            p_loss: row[storage_col::P_LOSS],
            q_loss: row[storage_col::Q_LOSS],
            status: row[storage_col::STATUS],
        })
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
    /// Generators (`mpc.gen` + `mpc.gencost`). Empty for a power-flow-only
    /// case; the DC-OPF builders error if they need generators and find none.
    pub gens: Vec<Generator>,
    /// Storage units (`mpc.storage`). Empty when the case has no storage block.
    pub storage: Vec<Storage>,
    /// Two-terminal HVDC lines (`mpc.dcline`). Empty when absent.
    pub dclines: Vec<DcLine>,
    /// MATPOWER bus id → dense index in [0, n).
    bus_id_to_idx: HashMap<usize, usize>,
    /// Original `.m` source text, present when parsed from text. The writer
    /// echoes it for a byte-exact round-trip; `None` for cases built in memory
    /// (e.g. `synth`), which write canonical output. `Arc<str>` so cloning an
    /// `MpcCase` shares the source instead of copying the whole file.
    source: Option<Arc<str>>,
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
            gens: Vec::new(),
            storage: Vec::new(),
            dclines: Vec::new(),
            bus_id_to_idx,
            source: None,
        }
    }

    /// Attach generators (builder style) so `new` stays a 4-argument
    /// constructor for the existing synth and test call sites.
    #[must_use]
    pub fn with_gens(mut self, gens: Vec<Generator>) -> Self {
        self.gens = gens;
        self
    }

    /// Attach storage units (builder style).
    #[must_use]
    pub fn with_storage(mut self, storage: Vec<Storage>) -> Self {
        self.storage = storage;
        self
    }

    /// Attach HVDC lines (builder style).
    #[must_use]
    pub fn with_dclines(mut self, dclines: Vec<DcLine>) -> Self {
        self.dclines = dclines;
        self
    }

    /// Attach the original source text (builder style). Set by the parser so the
    /// case can round-trip; `write_matpower` echoes it verbatim.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<Arc<str>>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// The original source text, if this case was parsed from `.m` text.
    #[must_use]
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// Dense index of the single reference (slack) bus. Errors unless exactly
    /// one `BusType::Ref` exists.
    pub fn reference_bus_index(&self) -> crate::Result<usize> {
        let refs: Vec<usize> = self
            .buses
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == BusType::Ref)
            .map(|(i, _)| i)
            .collect();
        match refs.as_slice() {
            [r] => Ok(*r),
            other => Err(crate::Error::ReferenceBusCount { found: other.len() }),
        }
    }

    /// Generators that are in service (`GEN_STATUS == 1`), with their index
    /// into `self.gens`.
    pub fn in_service_gens(&self) -> impl Iterator<Item = (usize, &Generator)> {
        self.gens.iter().enumerate().filter(|(_, g)| g.is_in_service())
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
    /// and the closed form radial inverse `[[R, X], [X, -R]]` require
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
            name: None,
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

/// DC line matrix column indices per the MATPOWER manual (0-based).
pub mod dcline_col {
    pub const F_BUS: usize = 0;
    pub const T_BUS: usize = 1;
    pub const BR_STATUS: usize = 2;
    pub const PF: usize = 3;
    pub const PT: usize = 4;
    pub const QF: usize = 5;
    pub const QT: usize = 6;
    pub const VF: usize = 7;
    pub const VT: usize = 8;
    pub const PMIN: usize = 9;
    pub const PMAX: usize = 10;
    pub const QMINF: usize = 11;
    pub const QMAXF: usize = 12;
    pub const QMINT: usize = 13;
    pub const QMAXT: usize = 14;
    pub const LOSS0: usize = 15;
    pub const LOSS1: usize = 16;
    pub const REQUIRED: usize = 17;
}

/// Generator matrix column indices per the MATPOWER manual (0-based).
pub mod gen_col {
    pub const GEN_BUS: usize = 0;
    pub const PG: usize = 1;
    pub const QG: usize = 2;
    pub const QMAX: usize = 3;
    pub const QMIN: usize = 4;
    pub const VG: usize = 5;
    pub const MBASE: usize = 6;
    pub const GEN_STATUS: usize = 7;
    pub const PMAX: usize = 8;
    pub const PMIN: usize = 9;
    /// Need columns through PMIN.
    pub const REQUIRED: usize = 10;
}

/// Storage matrix column indices (PowerModels / pglib 17-column layout, 0-based).
pub mod storage_col {
    pub const STORAGE_BUS: usize = 0;
    pub const PS: usize = 1;
    pub const QS: usize = 2;
    pub const ENERGY: usize = 3;
    pub const ENERGY_RATING: usize = 4;
    pub const CHARGE_RATING: usize = 5;
    pub const DISCHARGE_RATING: usize = 6;
    pub const CHARGE_EFFICIENCY: usize = 7;
    pub const DISCHARGE_EFFICIENCY: usize = 8;
    pub const THERMAL_RATING: usize = 9;
    pub const QMIN: usize = 10;
    pub const QMAX: usize = 11;
    pub const R: usize = 12;
    pub const X: usize = 13;
    pub const P_LOSS: usize = 14;
    pub const Q_LOSS: usize = 15;
    pub const STATUS: usize = 16;
    pub const REQUIRED: usize = 17;
}

/// Generator cost matrix column indices per the MATPOWER manual (0-based).
pub mod gencost_col {
    pub const MODEL: usize = 0;
    pub const STARTUP: usize = 1;
    pub const SHUTDOWN: usize = 2;
    pub const NCOST: usize = 3;
    /// First cost coefficient; the rest follow contiguously.
    pub const COEFF0: usize = 4;
    /// Need at least the header columns before the coefficients.
    pub const REQUIRED: usize = 4;
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
            name: None, // populated from `mpc.bus_name` by the parser
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

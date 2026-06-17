//! Network operations: deriving or rewriting a [`Network`].
//!
//! These are model-level transforms, distinct from the format readers/writers and
//! from the per-unit [`to_normalized`](Network::to_normalized) view.
//! [`subset`](Network::subset) carves a study footprint out of a larger case;
//! [`merge_bus`](Network::merge_bus) collapses two buses into one (re-homing the
//! incident elements), and [`reduce_zero_impedance`](Network::reduce_zero_impedance)
//! builds on it to remove jumper branches.

use std::collections::HashSet;

use serde_json::Value;

use crate::network::{Branch, Bus, BusId, BusType, Network, Shunt, SourceFormat};

/// Bus-kind importance, so a [`merge_bus`](Network::merge_bus) keeps the stronger
/// designation (a slack outranks a PV bus, which outranks PQ, which outranks an
/// isolated stub).
fn kind_priority(kind: BusType) -> u8 {
    match kind {
        BusType::Ref => 3,
        BusType::Pv => 2,
        BusType::Pq => 1,
        BusType::Isolated => 0,
    }
}

/// Which buses a [`subset`](Network::subset) keeps: inclusive ranges over area,
/// zone, base kV, and bus number, ANDed together. An unset (`None`) filter
/// matches every bus, so [`Selector::default`] selects the whole network.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selector {
    /// Inclusive `(low, high)` area-number range.
    pub area: Option<(usize, usize)>,
    /// Inclusive `(low, high)` zone-number range.
    pub zone: Option<(usize, usize)>,
    /// Inclusive `(low, high)` base-kV range.
    pub base_kv: Option<(f64, f64)>,
    /// Inclusive `(low, high)` bus-number range.
    pub bus: Option<(usize, usize)>,
}

impl Selector {
    /// Whether `bus` satisfies every set filter.
    fn matches(&self, bus: &Bus) -> bool {
        fn in_usize(range: Option<(usize, usize)>, v: usize) -> bool {
            range.is_none_or(|(lo, hi)| lo <= v && v <= hi)
        }
        fn in_f64(range: Option<(f64, f64)>, v: f64) -> bool {
            range.is_none_or(|(lo, hi)| lo <= v && v <= hi)
        }
        in_usize(self.area, bus.area)
            && in_usize(self.zone, bus.zone)
            && in_f64(self.base_kv, bus.base_kv)
            && in_usize(self.bus, bus.id.0)
    }
}

impl Network {
    /// Carve out the sub-network whose buses match `sel`.
    ///
    /// In-scope buses keep their loads, shunts, generators, and storage; a branch,
    /// HVDC line, or 3-winding transformer is kept when every bus it touches is
    /// kept. With `keep_boundary`, a branch or HVDC line straddling the selection
    /// edge pulls its out-of-scope endpoint in as a *tie bus* (tagged
    /// `extras["tie_bus"] = true`) so the carved island has no dangling branch
    /// ends; without it, a straddling branch is dropped. A tie bus is a stub: its
    /// own loads/generators are not pulled in. A control reference (regulated bus)
    /// that falls outside the kept set is cleared so the result is
    /// reference-consistent.
    ///
    /// The result is a fresh [`SourceFormat::InMemory`] network (no retained
    /// source); an empty `Selector` returns a clone-equivalent of the whole case,
    /// and a selector matching no bus returns an empty network.
    #[must_use]
    // A flat filter pipeline, one stanza per element table; splitting it would add
    // indirection without clarity.
    #[expect(clippy::too_many_lines)]
    pub fn subset(&self, sel: &Selector, keep_boundary: bool) -> Network {
        let in_scope: HashSet<BusId> = self
            .buses
            .iter()
            .filter(|b| sel.matches(b))
            .map(|b| b.id)
            .collect();

        // Boundary: the out-of-scope endpoint of any branch/HVDC with exactly one
        // endpoint in scope.
        let mut boundary: HashSet<BusId> = HashSet::new();
        if keep_boundary {
            let mut edge = |a: BusId, b: BusId| match (in_scope.contains(&a), in_scope.contains(&b))
            {
                (true, false) => {
                    boundary.insert(b);
                }
                (false, true) => {
                    boundary.insert(a);
                }
                _ => {}
            };
            for br in &self.branches {
                edge(br.from, br.to);
            }
            for d in &self.hvdc {
                edge(d.from, d.to);
            }
        }
        let kept: HashSet<BusId> = in_scope.union(&boundary).copied().collect();

        let buses: Vec<Bus> = self
            .buses
            .iter()
            .filter(|b| kept.contains(&b.id))
            .map(|b| {
                let mut b = b.clone();
                if boundary.contains(&b.id) {
                    b.extras.insert("tie_bus".into(), Value::Bool(true));
                }
                b
            })
            .collect();

        // Injection elements live only on in-scope buses; tie buses are stubs.
        let loads = self
            .loads
            .iter()
            .filter(|l| in_scope.contains(&l.bus))
            .cloned()
            .collect();
        let mut shunts: Vec<Shunt> = self
            .shunts
            .iter()
            .filter(|s| in_scope.contains(&s.bus))
            .cloned()
            .collect();
        let generators = self
            .generators
            .iter()
            .filter(|g| in_scope.contains(&g.bus))
            .cloned()
            .collect();
        let storage = self
            .storage
            .iter()
            .filter(|s| in_scope.contains(&s.bus))
            .cloned()
            .collect();

        let mut branches: Vec<Branch> = self
            .branches
            .iter()
            .filter(|br| kept.contains(&br.from) && kept.contains(&br.to))
            .cloned()
            .collect();
        let hvdc = self
            .hvdc
            .iter()
            .filter(|d| kept.contains(&d.from) && kept.contains(&d.to))
            .cloned()
            .collect();
        let transformers_3w = self
            .transformers_3w
            .iter()
            .filter(|t| t.windings.iter().all(|w| kept.contains(&w.bus)))
            .cloned()
            .collect();

        // Clear control references that point outside the kept set.
        for br in &mut branches {
            if let Some(c) = &mut br.control {
                if c.controlled_bus.is_some_and(|b| !kept.contains(&b)) {
                    c.controlled_bus = None;
                }
            }
        }
        for sh in &mut shunts {
            if let Some(c) = &mut sh.control {
                if c.control_bus.is_some_and(|b| !kept.contains(&b)) {
                    c.control_bus = None;
                }
            }
        }

        let net = Network {
            name: format!("{} (subset)", self.name),
            base_mva: self.base_mva,
            base_frequency: self.base_frequency,
            buses,
            loads,
            shunts,
            branches,
            generators,
            storage,
            hvdc,
            transformers_3w,
            areas: Vec::new(),
            source_format: SourceFormat::InMemory,
            source: None,
        };
        debug_assert!(
            net.validate().is_ok(),
            "subset produced a dangling reference"
        );
        net
    }

    /// Merge bus `from` into bus `into`: re-home every element on `from` (loads,
    /// shunts, generators, storage, branch/HVDC/transformer endpoints, and control
    /// references) onto `into`, drop the branches and HVDC lines that ran directly
    /// between the two (now self-loops), and remove the `from` bus. The surviving
    /// bus keeps the stronger of the two bus kinds (a slack is not demoted).
    ///
    /// A no-op when `into == from`. The other attributes of `from` (its voltage,
    /// limits, name) are discarded; the topology and injections are what move.
    pub fn merge_bus(&mut self, into: BusId, from: BusId) {
        if into == from {
            return;
        }
        let remap = |b: &mut BusId| {
            if *b == from {
                *b = into;
            }
        };

        for l in &mut self.loads {
            remap(&mut l.bus);
        }
        for s in &mut self.shunts {
            remap(&mut s.bus);
            if let Some(cb) = s.control.as_mut().and_then(|c| c.control_bus.as_mut()) {
                remap(cb);
            }
        }
        for g in &mut self.generators {
            remap(&mut g.bus);
        }
        for st in &mut self.storage {
            remap(&mut st.bus);
        }
        for br in &mut self.branches {
            remap(&mut br.from);
            remap(&mut br.to);
            if let Some(cb) = br.control.as_mut().and_then(|c| c.controlled_bus.as_mut()) {
                remap(cb);
            }
        }
        self.branches.retain(|b| b.from != b.to);
        for d in &mut self.hvdc {
            remap(&mut d.from);
            remap(&mut d.to);
        }
        self.hvdc.retain(|d| d.from != d.to);
        for t in &mut self.transformers_3w {
            for w in &mut t.windings {
                remap(&mut w.bus);
            }
        }

        // Promote the surviving bus kind, then drop the merged bus.
        let from_kind = self.buses.iter().find(|b| b.id == from).map(|b| b.kind);
        self.buses.retain(|b| b.id != from);
        if let (Some(fk), Some(into_bus)) =
            (from_kind, self.buses.iter_mut().find(|b| b.id == into))
        {
            if kind_priority(fk) > kind_priority(into_bus.kind) {
                into_bus.kind = fk;
            }
        }
    }

    /// Collapse every non-transformer branch whose series impedance magnitude is
    /// at or below `threshold` by merging its endpoints (the to-bus into the
    /// from-bus), returning the number of branches removed. Parallel jumpers
    /// between the same pair go in the same step.
    ///
    /// Zero-impedance branches (bus ties, breakers modeled as jumpers) carry no
    /// power-flow drop, so collapsing them shrinks the network without changing
    /// its electrical behavior. Transformers are never collapsed (a unity-ratio
    /// transformer is a real device, not a jumper).
    pub fn reduce_zero_impedance(&mut self, threshold: f64) -> usize {
        let before = self.branches.len();
        // Re-scan after each merge: bus ids and the branch list both change.
        while let Some((into, from)) = self.branches.iter().find_map(|b| {
            (!b.is_transformer() && b.from != b.to && b.r.hypot(b.x) <= threshold)
                .then_some((b.from, b.to))
        }) {
            self.merge_bus(into, from);
        }
        before - self.branches.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{BusType, Extras, Load};

    fn bus(id: usize, area: usize, base_kv: f64) -> Bus {
        Bus {
            id: BusId(id),
            kind: BusType::Pq,
            vm: 1.0,
            va: 0.0,
            base_kv,
            vmax: 1.1,
            vmin: 0.9,
            area,
            zone: 1,
            name: None,
            extras: Extras::new(),
        }
    }

    fn line(from: usize, to: usize) -> Branch {
        Branch {
            from: BusId(from),
            to: BusId(to),
            r: 0.0,
            x: 0.1,
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

    fn load(bus: usize) -> Load {
        Load {
            bus: BusId(bus),
            p: 10.0,
            q: 5.0,
            in_service: true,
            extras: Extras::new(),
        }
    }

    /// Two area-1 buses (1, 2) and one area-2 bus (3); a line within area 1 and a
    /// line crossing into area 2.
    fn two_area_net() -> Network {
        let mut net = Network::in_memory(
            "net",
            100.0,
            vec![bus(1, 1, 230.0), bus(2, 1, 230.0), bus(3, 2, 230.0)],
            vec![line(1, 2), line(2, 3)],
        );
        net.loads.push(load(1));
        net.loads.push(load(3));
        net
    }

    #[test]
    fn subset_by_area_drops_out_of_scope_buses_and_cut_branches() {
        let net = two_area_net();
        let sel = Selector {
            area: Some((1, 1)),
            ..Selector::default()
        };
        let sub = net.subset(&sel, false);

        // Buses 1, 2 kept; bus 3 (area 2) dropped along with the crossing line.
        assert_eq!(sub.buses.len(), 2);
        assert!(sub.buses.iter().all(|b| b.area == 1));
        assert_eq!(sub.branches.len(), 1, "only the intra-area line survives");
        assert_eq!(sub.loads.len(), 1, "the area-2 load is dropped");
        sub.validate().unwrap();
    }

    #[test]
    fn subset_keep_boundary_pulls_in_the_tie_bus() {
        let net = two_area_net();
        let sel = Selector {
            area: Some((1, 1)),
            ..Selector::default()
        };
        let sub = net.subset(&sel, true);

        // Bus 3 is pulled in as a tie bus so the crossing line keeps both ends.
        assert_eq!(sub.buses.len(), 3);
        assert_eq!(sub.branches.len(), 2);
        let tie = sub.buses.iter().find(|b| b.id == BusId(3)).unwrap();
        assert_eq!(tie.extras.get("tie_bus"), Some(&Value::Bool(true)));
        // The tie bus is a stub: its load is not pulled in.
        assert_eq!(sub.loads.len(), 1);
        sub.validate().unwrap();
    }

    #[test]
    fn empty_selector_keeps_everything() {
        let net = two_area_net();
        let sub = net.subset(&Selector::default(), false);
        assert_eq!(sub.buses.len(), net.buses.len());
        assert_eq!(sub.branches.len(), net.branches.len());
    }

    #[test]
    fn base_kv_range_filters_by_voltage() {
        let mut net = two_area_net();
        net.buses[2].base_kv = 115.0; // bus 3 to a different voltage class
        let sel = Selector {
            base_kv: Some((200.0, 300.0)),
            ..Selector::default()
        };
        let sub = net.subset(&sel, false);
        assert_eq!(sub.buses.len(), 2, "only the 230 kV buses match");
    }

    #[test]
    fn merge_bus_rehomes_elements_and_drops_the_connecting_branch() {
        let mut net = two_area_net(); // buses 1,2,3; lines 1-2, 2-3; loads on 1, 3
        net.merge_bus(BusId(2), BusId(3));

        assert_eq!(net.buses.len(), 2, "bus 3 removed");
        assert!(net.buses.iter().all(|b| b.id != BusId(3)));
        assert_eq!(
            net.branches.len(),
            1,
            "the 2-3 line collapsed to a self-loop"
        );
        assert_eq!(net.branches[0].from, BusId(1));
        assert_eq!(net.branches[0].to, BusId(2));
        // Both loads survive; the one on bus 3 moved to bus 2.
        assert_eq!(net.loads.len(), 2);
        assert!(net.loads.iter().any(|l| l.bus == BusId(2)));
        net.validate().unwrap();
    }

    #[test]
    fn merge_bus_keeps_the_stronger_bus_kind() {
        let mut net = two_area_net();
        net.buses[2].kind = BusType::Ref; // bus 3 is the slack
        net.merge_bus(BusId(2), BusId(3)); // merge the slack into the PQ bus 2
        let two = net.buses.iter().find(|b| b.id == BusId(2)).unwrap();
        assert_eq!(two.kind, BusType::Ref, "the slack designation is not lost");
    }

    #[test]
    fn reduce_zero_impedance_collapses_jumpers_only() {
        // Buses 1-2 a real line, 2-3 a zero-impedance jumper.
        let mut jumper = line(2, 3);
        jumper.x = 0.0;
        let mut net = Network::in_memory(
            "net",
            100.0,
            vec![bus(1, 1, 230.0), bus(2, 1, 230.0), bus(3, 1, 230.0)],
            vec![line(1, 2), jumper],
        );
        net.loads.push(load(3));

        let removed = net.reduce_zero_impedance(1e-9);
        assert_eq!(removed, 1, "only the jumper is collapsed");
        assert_eq!(net.buses.len(), 2);
        assert_eq!(net.branches.len(), 1, "the real 1-2 line remains");
        assert!(net.loads.iter().any(|l| l.bus == BusId(2)), "load re-homed");
        net.validate().unwrap();
    }
}

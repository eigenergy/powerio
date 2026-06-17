//! Network operations: deriving a new [`Network`] from an existing one.
//!
//! These are model-level transforms, distinct from the format readers/writers and
//! from the per-unit [`to_normalized`](Network::to_normalized) view. The first is
//! [`subset`](Network::subset): carve a study footprint out of a larger case by
//! area, zone, base kV, or bus number, optionally completing the cut branches with
//! their out-of-scope boundary buses.

use std::collections::HashSet;

use serde_json::Value;

use crate::network::{Branch, Bus, BusId, Network, Shunt, SourceFormat};

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
            source_format: SourceFormat::InMemory,
            source: None,
        };
        debug_assert!(
            net.validate().is_ok(),
            "subset produced a dangling reference"
        );
        net
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
}

//! Network operations: deriving or rewriting a [`Network`].
//!
//! These are model-level transforms, distinct from the format readers/writers and
//! from the per-unit [`to_normalized`](Network::to_normalized) view.
//! [`subset`](Network::subset) carves a study footprint out of a larger case;
//! [`merge_bus`](Network::merge_bus) collapses two buses into one (re-homing the
//! incident elements), and [`reduce_zero_impedance`](Network::reduce_zero_impedance)
//! builds on it to remove jumper branches.
//! [`reduce_passthrough_buses`](Network::reduce_passthrough_buses) folds dummy-bus
//! line sections back into one equivalent branch.

use std::collections::HashSet;

use serde_json::Value;

use crate::network::{
    Branch, Bus, BusId, BusType, Extras, Generator, Network, Shunt, SourceFormat,
};

/// The endpoint of `b` other than `m` (assumes `m` is an endpoint).
fn other_end(b: &Branch, m: BusId) -> BusId {
    if b.from == m { b.to } else { b.from }
}

/// Combine two thermal ratings into the equivalent for a series pair. `0` means
/// "no limit" in the MATPOWER convention, so it yields to a finite rating; two
/// finite ratings give the more limiting (smaller) one.
fn combine_rate(a: f64, b: f64) -> f64 {
    match (a == 0.0, b == 0.0) {
        (true, _) => b,
        (_, true) => a,
        _ => a.min(b),
    }
}

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
        let mut generators: Vec<Generator> = self
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
        for g in &mut generators {
            if g.regulated_bus.is_some_and(|b| !kept.contains(&b)) {
                g.regulated_bus = None;
            }
        }

        // Keep the area records still referenced by a kept bus (clearing a dangling
        // area-slack), plus the global solver settings. The bus `area` numbers alone
        // can't carry the interchange schedule or the solver tolerances, so dropping
        // them would silently lose data a PSS/E/PSLF write of the subset emits.
        let kept_area_numbers: HashSet<usize> = buses.iter().map(|b| b.area).collect();
        let areas = self
            .areas
            .iter()
            .filter(|a| kept_area_numbers.contains(&a.number))
            .cloned()
            .map(|mut a| {
                if a.slack_bus.is_some_and(|b| !kept.contains(&b)) {
                    a.slack_bus = None;
                }
                a
            })
            .collect::<Vec<_>>();

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
            areas,
            solver: self.solver.clone(),
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
            if let Some(rb) = g.regulated_bus.as_mut() {
                remap(rb);
            }
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
        for a in &mut self.areas {
            if let Some(slack) = a.slack_bus.as_mut() {
                remap(slack);
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

    /// Collapse degree-2 passthrough buses, returning the number removed. A
    /// passthrough bus carries nothing but two in-service line sections, so it is
    /// an electrically inert junction: the two sections fold into one equivalent
    /// branch between their outer endpoints and the middle bus is deleted.
    ///
    /// This is the multi-section-line reduction. Exporters often split one circuit
    /// into segments joined at dummy buses; folding them back recovers the single
    /// branch. A bus qualifies only when it carries no load, generator, shunt, or
    /// storage, is not a control reference, area swing, HVDC endpoint, or 3-winding
    /// winding bus, is not the system slack, and is touched by exactly two ordinary
    /// branches (never transformers) that are both in service and run to two
    /// distinct other buses. The equivalent branch sums the series impedance and
    /// line charging, takes the more limiting thermal rating of the two sections,
    /// and intersects their angle limits. Chains of dummy buses collapse fully, one
    /// bus per step.
    pub fn reduce_passthrough_buses(&mut self) -> usize {
        let mut collapsed = 0;
        // Re-scan after each fold: the equivalent branch becomes a section for the
        // next bus in a dummy chain, and the bus list shrinks.
        while let Some(mid) = self
            .buses
            .iter()
            .map(|b| b.id)
            .find(|&m| self.is_passthrough(m))
        {
            self.collapse_passthrough(mid);
            collapsed += 1;
        }
        collapsed
    }

    /// Whether `m` is a collapsible degree-2 passthrough bus (see
    /// [`reduce_passthrough_buses`](Network::reduce_passthrough_buses)).
    fn is_passthrough(&self, m: BusId) -> bool {
        let Some(bus) = self.buses.iter().find(|b| b.id == m) else {
            return false;
        };
        if bus.kind == BusType::Ref {
            return false;
        }
        if self.loads.iter().any(|l| l.bus == m)
            || self.generators.iter().any(|g| g.bus == m)
            || self.shunts.iter().any(|s| s.bus == m)
            || self.storage.iter().any(|s| s.bus == m)
            || self.hvdc.iter().any(|d| d.from == m || d.to == m)
        {
            return false;
        }
        if self
            .transformers_3w
            .iter()
            .any(|t| t.windings.iter().any(|w| w.bus == m))
        {
            return false;
        }
        if self.areas.iter().any(|a| a.slack_bus == Some(m)) {
            return false;
        }
        let controlled = self
            .branches
            .iter()
            .any(|b| b.control.as_ref().and_then(|c| c.controlled_bus) == Some(m));
        let regulated = self
            .shunts
            .iter()
            .any(|s| s.control.as_ref().and_then(|c| c.control_bus) == Some(m));
        let gen_regulated = self.generators.iter().any(|g| g.regulated_bus == Some(m));
        if controlled || regulated || gen_regulated {
            return false;
        }
        let incident: Vec<&Branch> = self
            .branches
            .iter()
            .filter(|b| b.from == m || b.to == m)
            .collect();
        if incident.len() != 2 {
            return false;
        }
        let a = other_end(incident[0], m);
        let c = other_end(incident[1], m);
        incident.iter().all(|b| !b.is_transformer() && b.in_service) && a != m && c != m && a != c
    }

    /// Fold the two line sections at passthrough bus `m` into one equivalent branch
    /// and remove `m`. The caller has already checked [`is_passthrough`].
    fn collapse_passthrough(&mut self, m: BusId) {
        let mut sections: Vec<Branch> = Vec::new();
        self.branches.retain(|b| {
            if b.from == m || b.to == m {
                sections.push(b.clone());
                false
            } else {
                true
            }
        });
        debug_assert_eq!(sections.len(), 2, "passthrough bus must have two sections");
        let (s1, s2) = (&sections[0], &sections[1]);
        // Intersect the two sections' angle windows, but never emit an inverted
        // (empty) limit: two disjoint windows give angmin > angmax, which an OPF
        // angle-difference constraint reads as infeasible. Disjoint windows fall
        // back to the union so folding a multi-section line never turns a feasible
        // case infeasible. (Whether series sections should intersect vs sum their
        // windows is a modeling choice; this only fixes the invalid-range case.)
        let mut angmin = s1.angmin.max(s2.angmin);
        let mut angmax = s1.angmax.min(s2.angmax);
        if angmin > angmax {
            angmin = s1.angmin.min(s2.angmin);
            angmax = s1.angmax.max(s2.angmax);
        }
        self.branches.push(Branch {
            from: other_end(s1, m),
            to: other_end(s2, m),
            r: s1.r + s2.r,
            x: s1.x + s2.x,
            b: s1.b + s2.b,
            rate_a: combine_rate(s1.rate_a, s2.rate_a),
            rate_b: combine_rate(s1.rate_b, s2.rate_b),
            rate_c: combine_rate(s1.rate_c, s2.rate_c),
            tap: 0.0,
            shift: 0.0,
            in_service: true,
            angmin,
            angmax,
            control: None,
            extras: Extras::new(),
        });
        self.buses.retain(|b| b.id != m);
    }

    /// Retype to [`BusType::Isolated`] every bus with no in-service electrical
    /// connection — no in-service incident branch, HVDC line, or 3-winding
    /// transformer — returning the number retyped.
    ///
    /// A stranded bus (retired or not-yet-built equipment, or the residue of a
    /// topology edit) otherwise keeps a PQ/PV/slack kind that tells a solver to
    /// include it, leaving an ungrounded singleton in the system. This only
    /// *demotes* a disconnected bus; it never promotes a connected one, and a bus
    /// the source already marks isolated is left untouched. Connectivity is judged
    /// on in-service equipment only, so opening the last branch into a bus makes it
    /// eligible.
    pub fn retype_isolated_buses(&mut self) -> usize {
        let mut connected: HashSet<BusId> = HashSet::new();
        for br in self.branches.iter().filter(|b| b.in_service) {
            connected.insert(br.from);
            connected.insert(br.to);
        }
        for d in self.hvdc.iter().filter(|d| d.in_service) {
            connected.insert(d.from);
            connected.insert(d.to);
        }
        for t in self.transformers_3w.iter().filter(|t| t.in_service) {
            for w in &t.windings {
                connected.insert(w.bus);
            }
        }
        let mut retyped = 0;
        for b in &mut self.buses {
            if b.kind != BusType::Isolated && !connected.contains(&b.id) {
                b.kind = BusType::Isolated;
                retyped += 1;
            }
        }
        retyped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{Area, BusType, Extras, Generator, Load};

    fn bus(id: usize, area: usize, base_kv: f64) -> Bus {
        Bus {
            id: BusId(id),
            kind: BusType::Pq,
            vm: 1.0,
            va: 0.0,
            base_kv,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
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

    fn gen_regulating(bus: usize, regulated: usize) -> Generator {
        Generator {
            bus: BusId(bus),
            pg: 10.0,
            qg: 0.0,
            pmax: 100.0,
            pmin: 0.0,
            qmax: 50.0,
            qmin: -50.0,
            vg: 1.0,
            mbase: 100.0,
            in_service: true,
            cost: None,
            caps: Default::default(),
            regulated_bus: Some(BusId(regulated)),
        }
    }

    #[test]
    fn subset_clears_a_regulated_bus_outside_the_kept_set() {
        // A generator on in-scope bus 1 regulates bus 3, which the area filter drops.
        let mut net = two_area_net();
        net.generators.push(gen_regulating(1, 3));
        let sel = Selector {
            area: Some((1, 1)),
            ..Selector::default()
        };
        let sub = net.subset(&sel, false);
        assert_eq!(sub.generators.len(), 1);
        assert_eq!(
            sub.generators[0].regulated_bus, None,
            "the dropped remote regulated bus is cleared, not left dangling"
        );
        sub.validate().unwrap();
    }

    #[test]
    fn merge_bus_remaps_regulated_bus_and_area_slack() {
        let mut net = two_area_net();
        net.generators.push(gen_regulating(1, 3)); // gen on bus 1 regulates bus 3
        net.areas.push(Area {
            number: 1,
            slack_bus: Some(BusId(3)),
            net_interchange: 0.0,
            tolerance: 0.0,
            name: None,
        });
        net.merge_bus(BusId(2), BusId(3)); // bus 3 merges into bus 2
        assert_eq!(
            net.generators[0].regulated_bus,
            Some(BusId(2)),
            "the regulated bus follows the merge"
        );
        assert_eq!(
            net.areas[0].slack_bus,
            Some(BusId(2)),
            "the area swing follows the merge"
        );
        net.validate().unwrap();
    }

    #[test]
    fn reduce_passthrough_keeps_a_generator_regulated_bus() {
        // Bus 2 is a degree-2 junction with no injection, but a generator on bus 1
        // regulates it, so it is not an inert passthrough.
        let mut net = Network::in_memory(
            "net",
            100.0,
            vec![bus(1, 1, 230.0), bus(2, 1, 230.0), bus(3, 1, 230.0)],
            vec![line(1, 2), line(2, 3)],
        );
        net.generators.push(gen_regulating(1, 2));
        assert_eq!(net.reduce_passthrough_buses(), 0);
        assert_eq!(net.buses.len(), 3);
        net.validate().unwrap();
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
    fn reduce_passthrough_folds_a_multi_section_line() {
        // A 1-2-3-4 chain where 2 and 3 are dummy junctions; ratings 100 / 80 /
        // unlimited along the sections.
        let mut s1 = line(1, 2);
        s1.rate_a = 100.0;
        let mut s2 = line(2, 3);
        s2.rate_a = 80.0;
        let s3 = line(3, 4); // rate_a 0 == no limit
        let mut net = Network::in_memory(
            "net",
            100.0,
            vec![
                bus(1, 1, 230.0),
                bus(2, 1, 230.0),
                bus(3, 1, 230.0),
                bus(4, 1, 230.0),
            ],
            vec![s1, s2, s3],
        );

        let removed = net.reduce_passthrough_buses();
        assert_eq!(removed, 2, "both dummy buses collapse");
        assert_eq!(net.buses.len(), 2);
        assert!(
            net.buses
                .iter()
                .all(|b| b.id == BusId(1) || b.id == BusId(4))
        );
        assert_eq!(net.branches.len(), 1, "one equivalent branch");
        let eq = &net.branches[0];
        assert_eq!(
            [eq.from, eq.to].iter().copied().collect::<HashSet<_>>(),
            [BusId(1), BusId(4)].into_iter().collect::<HashSet<_>>(),
        );
        assert!((eq.x - 0.3).abs() < 1e-9, "series reactance sums");
        assert!(
            (eq.rate_a - 80.0).abs() < 1e-9,
            "the more limiting finite rating wins"
        );
        net.validate().unwrap();
    }

    #[test]
    fn reduce_passthrough_keeps_a_bus_with_injection() {
        // Bus 2 is degree 2 but carries a load, so it is not inert.
        let mut net = Network::in_memory(
            "net",
            100.0,
            vec![bus(1, 1, 230.0), bus(2, 1, 230.0), bus(3, 1, 230.0)],
            vec![line(1, 2), line(2, 3)],
        );
        net.loads.push(load(2));
        assert_eq!(net.reduce_passthrough_buses(), 0);
        assert_eq!(net.buses.len(), 3);
    }

    #[test]
    fn reduce_passthrough_does_not_fold_across_a_transformer() {
        // Section 2-3 is a transformer, so bus 2 is a real terminal, not a junction.
        let mut xfmr = line(2, 3);
        xfmr.tap = 1.0;
        let mut net = Network::in_memory(
            "net",
            100.0,
            vec![bus(1, 1, 230.0), bus(2, 1, 230.0), bus(3, 1, 230.0)],
            vec![line(1, 2), xfmr],
        );
        assert_eq!(net.reduce_passthrough_buses(), 0);
        assert_eq!(net.buses.len(), 3);
    }

    #[test]
    fn retype_isolated_marks_stranded_buses() {
        // Bus 3 has no incident branch.
        let mut net = Network::in_memory(
            "net",
            100.0,
            vec![bus(1, 1, 230.0), bus(2, 1, 230.0), bus(3, 1, 230.0)],
            vec![line(1, 2)],
        );
        assert_eq!(net.retype_isolated_buses(), 1);
        let three = net.buses.iter().find(|b| b.id == BusId(3)).unwrap();
        assert_eq!(three.kind, BusType::Isolated);
        // The connected buses keep their kind.
        let one = net.buses.iter().find(|b| b.id == BusId(1)).unwrap();
        assert_eq!(one.kind, BusType::Pq);
        net.validate().unwrap();
    }

    #[test]
    fn retype_isolated_judges_in_service_equipment_only() {
        // The only branch is out of service, so both of its ends are stranded.
        let mut br = line(1, 2);
        br.in_service = false;
        let mut net = Network::in_memory(
            "net",
            100.0,
            vec![bus(1, 1, 230.0), bus(2, 1, 230.0)],
            vec![br],
        );
        assert_eq!(net.retype_isolated_buses(), 2);
        assert!(net.buses.iter().all(|b| b.kind == BusType::Isolated));
    }

    #[test]
    fn retype_isolated_is_idempotent() {
        let mut net = Network::in_memory(
            "net",
            100.0,
            vec![bus(1, 1, 230.0), bus(2, 1, 230.0), bus(3, 1, 230.0)],
            vec![line(1, 2)],
        );
        assert_eq!(net.retype_isolated_buses(), 1);
        assert_eq!(net.retype_isolated_buses(), 0, "second pass is a no-op");
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

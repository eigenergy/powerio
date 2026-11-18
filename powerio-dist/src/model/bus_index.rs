//! Bus position by name for [`MulticonductorNetwork::bus`], built on demand.
//!
//! The handle holds the index beside its tables. `tables_mut`, the one
//! mutation door, gives the handle an empty index of its own before it
//! hands out the tables, and no lookup can run while that mutable borrow
//! lives, so an index that exists was built from the bus table as it is now.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::DistBus;

/// Lookups a bus table answers by scan before the index is built. Building
/// costs about this many scans, so code that looks a bus up between edits of
/// the tables never pays for more than about twice its scans.
const SCANS_BEFORE_INDEX: usize = 32;

#[derive(Debug, Default)]
pub(super) struct BusIndex {
    scans: AtomicUsize,
    /// ASCII lowercase id to the first position that carries it.
    positions: OnceLock<HashMap<Box<str>, usize>>,
}

impl BusIndex {
    /// Back to the state before any lookup.
    pub(super) fn clear(&mut self) {
        *self.scans.get_mut() = 0;
        self.positions.take();
    }

    /// The first bus whose id matches `id` without regard to ASCII case:
    /// the answer of [`find_by_scan`], which the first lookups run.
    pub(super) fn find<'a>(&self, buses: &'a [DistBus], id: &str) -> Option<&'a DistBus> {
        let positions = match self.positions.get() {
            Some(positions) => positions,
            None if self.scans.fetch_add(1, Ordering::Relaxed) < SCANS_BEFORE_INDEX => {
                return find_by_scan(buses, id);
            }
            None => self.positions.get_or_init(|| calc_positions(buses)),
        };
        let key = if id.bytes().any(|byte| byte.is_ascii_uppercase()) {
            Cow::Owned(id.to_ascii_lowercase())
        } else {
            Cow::Borrowed(id)
        };
        positions.get(&*key).map(|&position| &buses[position])
    }
}

fn find_by_scan<'a>(buses: &'a [DistBus], id: &str) -> Option<&'a DistBus> {
    buses.iter().find(|bus| bus.id.eq_ignore_ascii_case(id))
}

/// Two ids match without regard to ASCII case exactly when their ASCII
/// lowercase forms are equal, and the first position wins as in the scan.
fn calc_positions(buses: &[DistBus]) -> HashMap<Box<str>, usize> {
    let mut positions = HashMap::with_capacity(buses.len());
    for (position, bus) in buses.iter().enumerate() {
        positions
            .entry(bus.id.to_ascii_lowercase().into_boxed_str())
            .or_insert(position);
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MulticonductorNetwork;

    fn bus(id: &str, terminal: &str) -> DistBus {
        DistBus::new(id, vec![terminal.to_owned()])
    }

    /// Every lookup agrees with the scan, before and after the index exists.
    fn assert_agrees(net: &MulticonductorNetwork, ids: &[&str]) {
        for round in 0..=SCANS_BEFORE_INDEX {
            for id in ids {
                let found = net.bus(id).map(std::ptr::from_ref);
                let scanned = find_by_scan(net.buses(), id).map(std::ptr::from_ref);
                assert_eq!(found, scanned, "`{id}` in round {round}");
            }
        }
        assert!(net.bus_index.positions.get().is_some());
    }

    const QUERIES: &[&str] = &[
        "a", "A", "Bus1", "bus1", "BUS1", "bUs1", "dup", "DUP", "Dup", "é", "É", "", "missing",
        "bus", "bus10",
    ];

    #[test]
    fn the_index_and_the_scan_find_the_same_bus() {
        let mut net = MulticonductorNetwork::new();
        net.buses_mut().extend([
            bus("A", "1"),
            bus("Bus1", "1"),
            bus("dup", "first"),
            bus("DUP", "second"),
            bus("bus1", "shadowed"),
            bus("é", "1"),
            bus("", "1"),
        ]);
        assert_agrees(&net, QUERIES);
        assert_eq!(net.bus("Dup").unwrap().terminals, ["first"]);
        assert_eq!(net.bus("BUS1").unwrap().terminals, ["1"]);
        // ASCII case only: `É` is not `é`.
        assert!(net.bus("É").is_none());
    }

    #[test]
    fn every_edit_of_the_bus_table_reaches_the_next_lookup() {
        let mut net = MulticonductorNetwork::new();
        net.buses_mut()
            .extend([bus("a", "1"), bus("Bus1", "1"), bus("dup", "first")]);
        assert_agrees(&net, QUERIES);

        net.buses_mut().push(bus("DUP", "second"));
        net.buses_mut().push(bus("bus10", "1"));
        assert_agrees(&net, QUERIES);
        assert_eq!(net.bus("DUP").unwrap().terminals, ["first"]);

        net.buses_mut()[1].id = "Renamed".into();
        assert_agrees(&net, QUERIES);
        assert!(net.bus("bus1").is_none());
        assert_eq!(net.bus("RENAMED").unwrap().terminals, ["1"]);

        net.buses_mut().retain(|b| b.terminals != ["first"]);
        assert_agrees(&net, QUERIES);
        assert_eq!(net.bus("dup").unwrap().terminals, ["second"]);

        net.buses_mut().swap(0, 1);
        net.buses_mut().insert(0, bus("A", "front"));
        assert_agrees(&net, QUERIES);
        assert_eq!(net.bus("a").unwrap().terminals, ["front"]);

        net.buses_mut().clear();
        assert_agrees(&net, QUERIES);
        assert!(net.bus("a").is_none());
    }

    #[test]
    fn a_clone_keeps_its_own_buses_when_the_other_handle_changes() {
        let mut net = MulticonductorNetwork::new();
        net.buses_mut().extend([bus("a", "1"), bus("b", "1")]);
        assert_agrees(&net, QUERIES);

        let mut edited = net.clone();
        assert_agrees(&edited, QUERIES);
        edited.buses_mut()[0].id = "c".into();
        edited.buses_mut().push(bus("A", "new"));
        assert_agrees(&edited, QUERIES);
        assert_agrees(&net, QUERIES);
        assert_eq!(net.bus("A").unwrap().terminals, ["1"]);
        assert_eq!(edited.bus("a").unwrap().terminals, ["new"]);
        assert!(net.bus("c").is_none());

        // Handles that split before any lookup: neither reads the other's.
        let mut left = net.clone();
        let right = left.clone();
        left.buses_mut()[0].id = "z".into();
        assert_agrees(&right, QUERIES);
        assert_agrees(&left, QUERIES);
        assert!(left.bus("a").is_none());
        assert_eq!(left.bus("Z").unwrap().terminals, ["1"]);
        let mut right = right;
        right.buses_mut().remove(0);
        assert_agrees(&left, QUERIES);
        assert_agrees(&right, QUERIES);
        assert!(right.bus("a").is_none());

        // A table that is not the bus table goes through the same door.
        let mut renamed = edited.clone();
        *renamed.name_mut() = Some("other".into());
        assert_agrees(&renamed, QUERIES);
    }

    #[test]
    fn a_serde_round_trip_and_the_debug_text_carry_no_index() {
        let mut net = MulticonductorNetwork::named("n");
        net.buses_mut().extend([bus("a", "1"), bus("A", "second")]);
        let before = (serde_json::to_string(&net).unwrap(), format!("{net:?}"));
        assert_agrees(&net, QUERIES);
        let after = (serde_json::to_string(&net).unwrap(), format!("{net:?}"));
        assert_eq!(before, after);
        assert!(before.1.starts_with("MulticonductorNetwork { tables: "));
        assert!(!before.1.contains("bus_index"));

        let read: MulticonductorNetwork = serde_json::from_str(&after.0).unwrap();
        assert_agrees(&read, QUERIES);
        assert_eq!(read.bus("A").unwrap().terminals, ["1"]);
    }
}

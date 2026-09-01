//! The checked, explicit zero impedance resolution.
//!
//! Networks and instances preserve zero impedance branches; a finite matrix
//! or problem projection refuses them rather than silently skipping rows.
//! [`merge_zero_impedance_buses`] is the explicit resolution: buses joined by
//! an in service branch with zero series impedance merge into one electrical
//! node, and the transformation returns the complete mapping plus diagnostics
//! for the branch behavior the merge removes.

use std::collections::BTreeMap;

use powerio_core::{Diagnostic, Error};
use powerio_tx::{BalancedNetwork, BusId};

use crate::diagnostics::codes;
use crate::operating::row_identity;

/// What one merge did: which buses now name which surviving bus, and which
/// branches the merge removed.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ZeroImpedanceMerge {
    /// Every merged bus to the bus that now carries it. Buses that survived
    /// unchanged are absent.
    pub merged_buses: BTreeMap<BusId, BusId>,
    /// The removed zero impedance branches, by stable identity
    /// (`uid`, else `branches:{row}` of the source network).
    pub removed_branches: Vec<String>,
}

/// Merge every group of buses joined by in service branches with zero series
/// impedance (`r == 0` and `x == 0`, self loops excluded) into that group's
/// smallest bus id, rewriting every element reference and dropping the merged
/// buses and the zero impedance branches.
///
/// The flow through a removed branch is no longer a variable of any derived
/// calculation, and merged buses may have stated different attributes; both
/// are reported as diagnostics. The input network is never mutated.
///
/// # Errors
/// A zero impedance branch naming a bus the network does not declare.
fn find(parent: &mut [usize], node: usize) -> usize {
    let mut root = node;
    while parent[root] != root {
        root = parent[root];
    }
    let mut walk = node;
    while parent[walk] != root {
        let next = parent[walk];
        parent[walk] = root;
        walk = next;
    }
    root
}

#[allow(clippy::too_many_lines)] // one pass per element table, stated in full
pub fn merge_zero_impedance_buses(
    network: &BalancedNetwork,
) -> Result<(BalancedNetwork, ZeroImpedanceMerge, Vec<Diagnostic>), Error> {
    let mut diagnostics = Vec::new();

    // Union-find over bus ids, keyed by table index.
    let index_of: BTreeMap<BusId, usize> = network
        .buses()
        .iter()
        .enumerate()
        .map(|(index, bus)| (bus.id, index))
        .collect();
    let mut parent: Vec<usize> = (0..network.buses().len()).collect();

    let mut removed_rows = Vec::new();
    for (row, branch) in network.branches().iter().enumerate() {
        let zero = branch.r == 0.0 && branch.x == 0.0;
        if !zero || !branch.in_service || branch.from == branch.to {
            continue;
        }
        let (Some(&from), Some(&to)) = (index_of.get(&branch.from), index_of.get(&branch.to))
        else {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                format!(
                    "zero impedance branch row {row} names bus {} or {} the network does not declare",
                    branch.from, branch.to
                ),
            ));
        };
        let (from_root, to_root) = (find(&mut parent, from), find(&mut parent, to));
        if from_root != to_root {
            parent[from_root.max(to_root)] = from_root.min(to_root);
        }
        removed_rows.push(row);
        let identity = row_identity(branch.uid.as_deref(), "branches", row);
        diagnostics.push(Diagnostic::of(
            &codes::CANONICALIZE_MERGE_ZERO_IMPEDANCE,
            format!(
                "zero impedance branch `{identity}` between buses {} and {} was merged; its flow is not a variable of any derived calculation",
                branch.from, branch.to
            ),
        ));
    }

    if removed_rows.is_empty() {
        return Ok((network.clone(), ZeroImpedanceMerge::default(), diagnostics));
    }

    // The surviving bus of each group is the smallest bus id in it, which is
    // the root after unioning toward the smaller table index of an id-sorted
    // bus table; resolve ids directly so the rule holds for any table order.
    let mut survivor_of_root: BTreeMap<usize, BusId> = BTreeMap::new();
    for index in 0..network.buses().len() {
        let root = find(&mut parent, index);
        let id = network.buses()[index].id;
        let entry = survivor_of_root.entry(root).or_insert(id);
        if id < *entry {
            *entry = id;
        }
    }
    let mut merged_buses = BTreeMap::new();
    for index in 0..network.buses().len() {
        let root = find(&mut parent, index);
        let id = network.buses()[index].id;
        let survivor = survivor_of_root[&root];
        if id != survivor {
            merged_buses.insert(id, survivor);
        }
    }

    let resolve = |bus: BusId| merged_buses.get(&bus).copied().unwrap_or(bus);

    let mut merged = network.clone();
    // Attribute conflicts between a merged bus and its survivor are reported;
    // the survivor's values are kept.
    for (&gone, &kept) in &merged_buses {
        let gone_bus = &network.buses()[index_of[&gone]];
        let kept_bus = &network.buses()[index_of[&kept]];
        // Bit inequality on purpose: any stated difference is worth a note,
        // and the values come from one document, so equal bases agree
        // exactly.
        if gone_bus.base_kv.to_bits() != kept_bus.base_kv.to_bits() {
            diagnostics.push(Diagnostic::of(
                &codes::CANONICALIZE_MERGE_ATTRIBUTE_CONFLICT,
                format!(
                    "bus {gone} (base {} kV) merged into bus {kept} (base {} kV); the surviving base was kept",
                    gone_bus.base_kv, kept_bus.base_kv
                ),
            ));
        }
        if gone_bus.kind != kept_bus.kind && gone_bus.kind == powerio_tx::BusType::Ref {
            // A reference designation must survive the merge.
            let survivor = &mut merged.buses_mut()[index_of[&kept]];
            survivor.kind = powerio_tx::BusType::Ref;
        }
    }

    let removed: std::collections::BTreeSet<usize> = removed_rows.iter().copied().collect();
    let removed_branches = removed_rows
        .iter()
        .map(|&row| row_identity(network.branches()[row].uid.as_deref(), "branches", row))
        .collect();

    merged
        .buses_mut()
        .retain(|bus| !merged_buses.contains_key(&bus.id));
    let mut row = 0usize;
    merged.branches_mut().retain(|_| {
        let keep = !removed.contains(&row);
        row += 1;
        keep
    });
    for branch in merged.branches_mut() {
        branch.from = resolve(branch.from);
        branch.to = resolve(branch.to);
    }
    for load in merged.loads_mut() {
        load.bus = resolve(load.bus);
    }
    for generator in merged.generators_mut() {
        generator.bus = resolve(generator.bus);
    }
    for shunt in merged.shunts_mut() {
        shunt.bus = resolve(shunt.bus);
    }

    Ok((
        merged,
        ZeroImpedanceMerge {
            merged_buses,
            removed_branches,
        },
        diagnostics,
    ))
}

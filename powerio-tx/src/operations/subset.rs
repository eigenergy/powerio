//! Row provenance and detailed topology for electrical cuts.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use powerio_core::ComponentId;

use crate::network::{
    BalancedNetwork, CalculatedBus, DetailedConnectivity, TerminalReference, TopologyEndpoint,
};

/// A bus cut and its source-to-result row maps.
///
/// Each map has one entry per row of the corresponding **source** table.
/// `Some(row)` is a zero-based row in `network`; `None` means the source row
/// was removed. Bus IDs and component identities are preserved, not renumbered.
/// A dead winding's retained stub has a bus row, but its excluded injections
/// have no result row. Service status does not otherwise filter the cut.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BusSubset {
    pub network: BalancedNetwork,
    pub bus_rows: Vec<Option<usize>>,
    pub load_rows: Vec<Option<usize>>,
    pub shunt_rows: Vec<Option<usize>>,
    pub static_var_compensator_rows: Vec<Option<usize>>,
    pub branch_rows: Vec<Option<usize>>,
    pub switch_rows: Vec<Option<usize>>,
    pub generator_rows: Vec<Option<usize>>,
    pub storage_rows: Vec<Option<usize>>,
    pub hvdc_rows: Vec<Option<usize>>,
    pub transformer_3w_rows: Vec<Option<usize>>,
    pub area_rows: Vec<Option<usize>>,
}

pub(super) fn filter_rows<T: Clone>(
    source: &[T],
    keep: impl Fn(&T) -> bool,
) -> (Vec<T>, Vec<Option<usize>>) {
    let mut result = Vec::new();
    let rows = source
        .iter()
        .map(|value| {
            keep(value).then(|| {
                let row = result.len();
                result.push(value.clone());
                row
            })
        })
        .collect();
    (result, rows)
}

/// Remove topology tied to removed calculation records and nodes. Unassigned
/// topology and its containers remain: absence of a calculated bus is valid
/// source data, not evidence that the record belongs to an excluded bus.
#[expect(clippy::too_many_lines)]
pub(crate) fn filter_connectivity(source: &BalancedNetwork, cut: &mut BalancedNetwork) {
    let Some(original) = source.detailed_connectivity().as_deref() else {
        return;
    };
    let mut detail = original.clone();
    let kept: HashSet<_> = cut.buses().iter().map(|bus| bus.id).collect();
    let mut removed = HashSet::<ComponentId>::new();
    macro_rules! removed_records {
        ($table:ident, $kind:literal) => {{
            let ids: HashSet<_> = cut
                .$table()
                .iter()
                .filter_map(|v| v.uid.as_deref())
                .collect();
            removed.extend(
                source
                    .$table()
                    .iter()
                    .filter_map(|v| v.uid.as_deref())
                    .filter(|uid| !ids.contains(uid))
                    .filter_map(|uid| ComponentId::new($kind, uid).ok()),
            );
        }};
    }
    removed_records!(buses, "bus");
    removed_records!(loads, "load");
    removed_records!(shunts, "shunt");
    removed_records!(static_var_compensators, "static_var_compensator");
    removed_records!(branches, "branch");
    removed_records!(switches, "switch");
    removed_records!(generators, "generator");
    removed_records!(storage, "storage");
    removed_records!(hvdc, "hvdc");
    removed_records!(transformers_3w, "transformer_3w");

    for level in &mut detail.voltage_levels {
        level.buses.retain(|bus| kept.contains(bus));
    }
    for bus in &detail.calculated_buses {
        if !kept.contains(&bus.calculated_bus) {
            removed.extend(bus.nodes.iter().cloned());
        }
    }
    detail
        .calculated_buses
        .retain(|bus| kept.contains(&bus.calculated_bus));
    detail.bus_breaker_buses.retain(|bus| {
        let keep = bus.calculated_bus.is_none_or(|id| kept.contains(&id));
        if !keep {
            removed.insert(bus.component.clone());
        }
        keep
    });
    detail.connectivity_nodes.retain(|node| {
        let keep = !removed.contains(&node.component)
            && node.calculated_bus.is_none_or(|id| kept.contains(&id));
        if !keep {
            removed.insert(node.component.clone());
        }
        keep
    });
    for bus in &mut detail.calculated_buses {
        bus.nodes.retain(|node| !removed.contains(node));
    }
    detail.busbar_sections.retain(|busbar| {
        let keep = !removed.contains(&busbar.node);
        if !keep {
            removed.insert(busbar.component.clone());
        }
        keep
    });
    let endpoint = |value: &TopologyEndpoint| match value {
        TopologyEndpoint::Bus(id) | TopologyEndpoint::Node(id) => id.clone(),
    };
    detail.switches.retain(|switch| {
        let keep = !removed.contains(&switch.component)
            && !removed.contains(&endpoint(&switch.endpoint1))
            && !removed.contains(&endpoint(&switch.endpoint2));
        if !keep {
            removed.insert(switch.component.clone());
        }
        keep
    });
    detail
        .internal_connections
        .retain(|edge| !removed.contains(&edge.node1) && !removed.contains(&edge.node2));
    // A retained calculation record can have a connectable bus outside the cut.
    // Keep its terminal, clearing only the out-of-scope connection reference.
    let source_equipment: HashSet<_> = original
        .terminals
        .iter()
        .filter(|t| t.bus.is_some() || t.node.is_some() || t.connectable_bus.is_some())
        .map(|t| t.equipment.clone())
        .collect();
    detail.terminals.retain_mut(|terminal| {
        if removed.contains(&terminal.equipment) {
            return false;
        }
        let mut lost_connection = false;
        for connection in [&mut terminal.bus, &mut terminal.node] {
            if connection.as_ref().is_some_and(|id| removed.contains(id)) {
                *connection = None;
                lost_connection = true;
            }
        }
        if terminal
            .connectable_bus
            .as_ref()
            .is_some_and(|id| removed.contains(id))
        {
            terminal.connectable_bus = None;
        }
        if lost_connection {
            terminal.connected = false;
        }
        true
    });
    // Detailed-only equipment whose entire AC connection was cut must not be
    // emitted again as part of the selected electrical network.
    let attached: HashSet<_> = detail
        .terminals
        .iter()
        .filter(|t| t.bus.is_some() || t.node.is_some() || t.connectable_bus.is_some())
        .map(|t| t.equipment.clone())
        .collect();
    macro_rules! detailed_equipment {
        ($table:ident) => {
            detail.$table.retain(|value| {
                let keep = !removed.contains(&value.component)
                    && (!source_equipment.contains(&value.component)
                        || attached.contains(&value.component));
                if !keep {
                    removed.insert(value.component.clone());
                }
                keep
            });
        };
    }
    detailed_equipment!(junctions);
    detailed_equipment!(boundary_lines);
    detailed_equipment!(voltage_source_converters);
    detailed_equipment!(line_commutated_converters);
    detail.boundary_lines.retain(|line| {
        let keep = !line
            .calculation_load
            .as_ref()
            .is_some_and(|id| removed.contains(id))
            && !line
                .calculation_generator
                .as_ref()
                .is_some_and(|id| removed.contains(id));
        if !keep {
            removed.insert(line.component.clone());
        }
        keep
    });
    detail.tie_lines.retain(|line| {
        let keep = !removed.contains(&line.boundary_line1)
            && !removed.contains(&line.boundary_line2)
            && !line
                .calculation_branch
                .as_ref()
                .is_some_and(|id| removed.contains(id));
        if !keep {
            removed.insert(line.component.clone());
        }
        keep
    });
    detail.terminals.retain(|t| !removed.contains(&t.equipment));
    for terminal in &original.terminals {
        if removed.contains(&terminal.equipment)
            && let Some(id) = &terminal.component
        {
            removed.insert(id.clone());
        }
    }
    let terminals: HashSet<_> = detail
        .terminals
        .iter()
        .map(|t| (t.equipment.clone(), t.terminal))
        .collect();
    let clear_reference = |reference: &mut Option<TerminalReference>| {
        if reference
            .as_ref()
            .is_some_and(|r| !terminals.contains(&(r.equipment.clone(), r.terminal)))
        {
            *reference = None;
        }
    };
    detail.tap_changers.retain(|tap| {
        let keep = !removed.contains(&tap.transformer);
        if !keep && let Some(id) = &tap.component {
            removed.insert(id.clone());
        }
        keep
    });
    for tap in &mut detail.tap_changers {
        clear_reference(&mut tap.regulation_terminal);
    }
    for converter in &mut detail.voltage_source_converters {
        clear_reference(&mut converter.pcc_terminal);
    }
    for converter in &mut detail.line_commutated_converters {
        clear_reference(&mut converter.pcc_terminal);
    }
    // Do not emit emptied voltage levels: XIIDM requires topology in each
    // emitted level. Keep levels still used by disconnected source records.
    let used_levels: HashSet<_> = detail
        .bus_breaker_buses
        .iter()
        .map(|b| b.voltage_level.clone())
        .chain(
            detail
                .connectivity_nodes
                .iter()
                .map(|n| n.voltage_level.clone()),
        )
        .chain(detail.terminals.iter().map(|t| t.voltage_level.clone()))
        .chain(
            detail
                .boundary_lines
                .iter()
                .map(|b| b.voltage_level.clone()),
        )
        .collect();
    let originally_empty: HashSet<_> = original
        .voltage_levels
        .iter()
        .filter(|level| level.buses.is_empty())
        .map(|level| &level.component)
        .collect();
    detail.voltage_levels.retain(|level| {
        let keep = !level.buses.is_empty()
            || used_levels.contains(&level.component)
            || originally_empty.contains(&level.component);
        if !keep {
            removed.insert(level.component.clone());
        }
        keep
    });
    detail
        .operational_limit_groups
        .retain(|g| !removed.contains(&g.equipment));
    detail
        .equipment_reactive_limits
        .retain(|g| !removed.contains(&g.equipment));
    detail
        .omitted_fields
        .retain(|f| !removed.contains(&f.component));
    detail
        .component_metadata
        .retain(|m| !removed.contains(&m.component));
    for metadata in &mut detail.component_metadata {
        if metadata
            .equipment_container
            .as_ref()
            .is_some_and(|id| removed.contains(id))
        {
            metadata.equipment_container = None;
        }
    }
    macro_rules! clear_removed_containers {
        ($($table:ident),+) => {$(
            for record in &mut detail.$table {
                if record.equipment_container.as_ref().is_some_and(|id| removed.contains(id)) {
                    record.equipment_container = None;
                }
            }
        )+};
    }
    clear_removed_containers!(
        dc_grounds,
        dc_busbars,
        dc_lines,
        dc_series_devices,
        dc_switches
    );
    for subnetwork in &mut detail.subnetworks {
        subnetwork.components.retain(|id| !removed.contains(id));
    }
    if !removed.is_empty() || cut.buses().len() != source.buses().len() {
        preserve_orphaned_buses(cut, &mut detail);
    }
    *cut.detailed_connectivity_mut() = Some(Arc::new(detail));
}

// Cutting the last switch/terminal off a node must not erase its calculation
// bus on fresh XIIDM emission. An explicit calculated-bus record can carry
// that otherwise unanchored node without inventing electrical equipment.
fn preserve_orphaned_buses(cut: &BalancedNetwork, detail: &mut DetailedConnectivity) {
    let mut anchored: HashSet<_> = detail
        .calculated_buses
        .iter()
        .flat_map(|b| b.nodes.iter().cloned())
        .chain(detail.busbar_sections.iter().map(|b| b.node.clone()))
        .chain(detail.terminals.iter().filter_map(|t| t.node.clone()))
        .chain(
            detail
                .internal_connections
                .iter()
                .flat_map(|e| [e.node1.clone(), e.node2.clone()]),
        )
        .collect();
    for switch in &detail.switches {
        for end in [&switch.endpoint1, &switch.endpoint2] {
            if let TopologyEndpoint::Node(node) = end {
                anchored.insert(node.clone());
            }
        }
    }
    let mut nodes_by_bus: HashMap<_, Vec<_>> = HashMap::new();
    for node in &detail.connectivity_nodes {
        if let Some(bus) = node.calculated_bus {
            nodes_by_bus.entry(bus).or_default().push(node);
        }
    }
    for bus in cut.buses() {
        let Some(nodes) = nodes_by_bus.get(&bus.id) else {
            continue;
        };
        if nodes.iter().any(|node| anchored.contains(&node.component))
            || nodes
                .iter()
                .any(|node| node.voltage_level != nodes[0].voltage_level)
        {
            continue;
        }
        detail.calculated_buses.push(CalculatedBus {
            voltage_level: nodes[0].voltage_level.clone(),
            calculated_bus: bus.id,
            nodes: nodes.iter().map(|node| node.component.clone()).collect(),
            voltage_kv: Some(bus.vm * bus.base_kv),
            angle_degrees: Some(if cut.is_normalized() {
                bus.va.to_degrees()
            } else {
                bus.va
            }),
        });
    }
}

//! Voltage base zones: the buses that share one voltage base, and the flood
//! of zone bases across transformers.

use std::collections::BTreeMap;

use crate::MulticonductorNetwork;

/// The voltage zone of each network bus, in bus table order: buses joined by
/// a line or a closed switch share a label (a representative bus index),
/// with bus identities compared ASCII case insensitively. Transformers
/// separate zones. The one network zone partition: the multiconductor to
/// balanced lowering bases its zones on it and the load nominal voltage
/// inference anchors its zones on it.
///
/// # Errors
/// A line or closed switch naming a bus the network does not declare.
pub fn calc_voltage_zone_labels(network: &MulticonductorNetwork) -> Result<Vec<usize>, String> {
    let position: BTreeMap<String, usize> = network
        .buses()
        .iter()
        .enumerate()
        .map(|(index, bus)| (bus.id.to_ascii_lowercase(), index))
        .collect();
    let find = |bus: &str, label: &str| {
        position
            .get(&bus.to_ascii_lowercase())
            .copied()
            .ok_or_else(|| format!("{label} references unknown bus `{bus}`"))
    };
    let mut edges = Vec::new();
    for line in network.lines() {
        let label = format!("line `{}`", line.name);
        edges.push((find(&line.bus_from, &label)?, find(&line.bus_to, &label)?));
    }
    for switch in network.switches().iter().filter(|switch| !switch.open) {
        let label = format!("switch `{}`", switch.name);
        edges.push((
            find(&switch.bus_from, &label)?,
            find(&switch.bus_to, &label)?,
        ));
    }
    Ok(powerio_core::union_find::calc_component_labels(
        network.buses().len(),
        edges,
    ))
}

/// Flood zone voltage bases across transformer edges, depth first from the
/// seeded zones (`seeds`, in the order given; the last seed is explored
/// first): crossing edge `k` from `edges[k].0` to `edges[k].1` gives the far
/// zone `carry(k, true, base)`, and the reverse crossing
/// `carry(k, false, base)`, where `base` is the near zone's. A zone keeps the
/// first base that reaches it; `carry` returning `None` does not cross. The
/// one zone base flood: the multiconductor to balanced lowering (the far
/// winding's rated voltage) and the PMD MATHEMATICAL writer (`tm_nom`, as
/// PMD's `calc_voltage_bases` scales) state only their crossing rule.
pub fn calc_flooded_zone_bases(
    seeds: impl IntoIterator<Item = (usize, f64)>,
    edges: &[(usize, usize)],
    carry: impl Fn(usize, bool, f64) -> Option<f64>,
) -> BTreeMap<usize, f64> {
    let mut adjacent: BTreeMap<usize, Vec<(usize, usize, bool)>> = BTreeMap::new();
    for (k, &(from, to)) in edges.iter().enumerate() {
        adjacent.entry(from).or_default().push((to, k, true));
        adjacent.entry(to).or_default().push((from, k, false));
    }
    let mut base: BTreeMap<usize, f64> = BTreeMap::new();
    let mut stack = Vec::new();
    for (zone, value) in seeds {
        if let std::collections::btree_map::Entry::Vacant(entry) = base.entry(zone) {
            entry.insert(value);
            stack.push(zone);
        }
    }
    while let Some(near) = stack.pop() {
        let near_base = base[&near];
        for &(far, k, forward) in adjacent.get(&near).map_or(&[][..], Vec::as_slice) {
            if base.contains_key(&far) {
                continue;
            }
            if let Some(value) = carry(k, forward, near_base) {
                base.insert(far, value);
                stack.push(far);
            }
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bases_flood_from_the_seeds_across_each_edge_once() {
        // zones 0 -(x10)- 1 -(x0.5)- 2, seeded at 1.
        let edges = [(0, 1), (1, 2)];
        let scale = [10.0, 0.5];
        let base = calc_flooded_zone_bases([(1, 4.0)], &edges, |k, forward, value| {
            Some(if forward {
                value * scale[k]
            } else {
                value / scale[k]
            })
        });
        assert_eq!(base, BTreeMap::from([(0, 0.4), (1, 4.0), (2, 2.0)]));
    }
}

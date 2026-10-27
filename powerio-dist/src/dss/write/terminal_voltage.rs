//! Nominal two-terminal voltage hints for single-phase generation.
//! Center-tap winding polarity distinguishes a 240 V hot pair from 120 V legs.
use crate::{DistWindingConn, MulticonductorNetwork};
use num_complex::Complex64;
use std::collections::{BTreeMap, VecDeque};

type Key = (String, String, String);
pub(super) type TerminalVoltages = BTreeMap<Key, f64>;
fn key(bus: &str, a: &str, b: &str) -> Key {
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    (bus.to_ascii_lowercase(), a.to_owned(), b.to_owned())
}
pub(super) fn get(values: &TerminalVoltages, bus: &str, terminals: &[String]) -> Option<f64> {
    let [a, b] = terminals else {
        return None;
    };
    values.get(&key(bus, a, b)).copied()
}
fn insert(values: &mut TerminalVoltages, bus: &str, a: &str, b: &str, voltage: f64) {
    if a != b && voltage.is_finite() && voltage > 0.0 {
        values.entry(key(bus, a, b)).or_insert(voltage / 1000.0);
    }
}
pub(super) fn estimate(net: &MulticonductorNetwork) -> TerminalVoltages {
    let mut values = TerminalVoltages::new();
    // Winding ratings define nominal bases even when the source is off nominal.
    for transformer in net.transformers().iter().filter(|t| t.phases == 1) {
        let windings = transformer
            .windings
            .iter()
            .filter(|w| w.conn == DistWindingConn::Wye && w.terminal_map.len() == 2)
            .collect::<Vec<_>>();
        for (i, a) in windings.iter().enumerate() {
            insert(
                &mut values,
                &a.bus,
                &a.terminal_map[0],
                &a.terminal_map[1],
                a.v_ref,
            );
            for b in windings
                .iter()
                .skip(i + 1)
                .filter(|b| b.bus.eq_ignore_ascii_case(&a.bus))
            {
                for (ai, common) in a.terminal_map.iter().enumerate() {
                    if let Some(bi) = b.terminal_map.iter().position(|v| v == common) {
                        let sign_a = if ai == 1 { 1.0 } else { -1.0 };
                        let sign_b = if bi == 1 { 1.0 } else { -1.0 };
                        insert(
                            &mut values,
                            &a.bus,
                            &a.terminal_map[1 - ai],
                            &b.terminal_map[1 - bi],
                            (sign_a * a.v_ref - sign_b * b.v_ref).abs(),
                        );
                    }
                }
            }
        }
    }
    for source in net.sources() {
        let mut terminals = source
            .terminal_map
            .iter()
            .cloned()
            .zip(
                source
                    .v_magnitude
                    .iter()
                    .zip(&source.v_angle)
                    .map(|(&v, &a)| Complex64::from_polar(v, a)),
            )
            .collect::<BTreeMap<_, _>>();
        if let Some(reference) = &source.reference_terminal {
            terminals.entry(reference.clone()).or_default();
        }
        if let Some(bus) = net
            .buses()
            .iter()
            .find(|b| b.id.eq_ignore_ascii_case(&source.bus))
        {
            for terminal in &bus.grounded {
                terminals.entry(terminal.clone()).or_default();
            }
        }
        let terminals = terminals.into_iter().collect::<Vec<_>>();
        for (i, (a, va)) in terminals.iter().enumerate() {
            for (b, vb) in terminals.iter().skip(i + 1) {
                insert(&mut values, &source.bus, a, b, (*va - *vb).norm());
            }
        }
    }
    propagate(net, &mut values);
    values
}
fn propagate(net: &MulticonductorNetwork, values: &mut TerminalVoltages) {
    let mut edges = BTreeMap::<String, Vec<(String, BTreeMap<String, String>)>>::new();
    for (from, to, fmap, tmap) in net
        .lines()
        .iter()
        .map(|l| {
            (
                &l.bus_from,
                &l.bus_to,
                &l.terminal_map_from,
                &l.terminal_map_to,
            )
        })
        .chain(net.switches().iter().map(|s| {
            (
                &s.bus_from,
                &s.bus_to,
                &s.terminal_map_from,
                &s.terminal_map_to,
            )
        }))
    {
        for (a, b, amap, bmap) in [(from, to, fmap, tmap), (to, from, tmap, fmap)] {
            edges.entry(a.to_ascii_lowercase()).or_default().push((
                b.to_ascii_lowercase(),
                amap.iter().cloned().zip(bmap.iter().cloned()).collect(),
            ));
        }
    }
    let mut pending = values
        .iter()
        .map(|(k, &v)| (k.clone(), v))
        .collect::<VecDeque<_>>();
    while let Some(((bus, a, b), voltage)) = pending.pop_front() {
        for (other, map) in edges.get(&bus).into_iter().flatten() {
            if let (Some(a), Some(b)) = (map.get(&a), map.get(&b)) {
                let target = key(other, a, b);
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    values.entry(target.clone())
                {
                    entry.insert(voltage);
                    pending.push_back((target, voltage));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DistLine, DistTransformer, DistWinding, VoltageSource};
    fn map(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).into()).collect()
    }
    #[test]
    fn center_tap_polarity_and_renumbered_conductors_define_pair_voltage() {
        let mut net = MulticonductorNetwork::new();
        let winding = |terminals: &[&str], v| {
            DistWinding::new(
                "secondary",
                map(terminals),
                DistWindingConn::Wye,
                v,
                15000.0,
            )
        };
        net.transformers_mut().push(DistTransformer::new(
            "service",
            vec![winding(&["h1", "n"], 120.0), winding(&["n", "h2"], 120.0)],
            vec![1.0],
            1,
        ));
        net.lines_mut().push(DistLine::new(
            "cable",
            "secondary",
            "house",
            map(&["h2", "n", "h1"]),
            map(&["9", "0", "7"]),
            "wire",
            1.0,
        ));
        let values = estimate(&net);
        assert!((get(&values, "house", &map(&["7", "9"])).unwrap() - 0.24).abs() < 1e-12);
        assert!((get(&values, "house", &map(&["0", "9"])).unwrap() - 0.12).abs() < 1e-12);
        assert_eq!(get(&values, "house", &map(&["7", "missing"])), None);
    }
    #[test]
    fn source_phase_angles_distinguish_phase_pairs_from_phase_to_ground() {
        let mut net = MulticonductorNetwork::new();
        net.sources_mut().push(VoltageSource::new(
            "source",
            "bus",
            map(&["a", "b", "c"]),
            vec![240.0; 3],
            vec![
                0.0,
                -2.0 * std::f64::consts::PI / 3.0,
                2.0 * std::f64::consts::PI / 3.0,
            ],
        ));
        let values = estimate(&net);
        assert!(
            (get(&values, "bus", &map(&["a", "b"])).unwrap() - 0.24 * 3.0_f64.sqrt()).abs() < 1e-12
        );
        assert_eq!(get(&values, "bus", &map(&["a"])), None);
    }
}

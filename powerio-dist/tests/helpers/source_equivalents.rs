//! Test-only electrical normalization of ideal-source series equivalents.
//! No generated names or retained extras are trusted: cancellation requires
//! two one-conductor, shunt-free branches, exclusive internal nodes, and
//! exactly opposite complex series impedances.

use std::collections::{BTreeMap, BTreeSet};

use num_complex::Complex64;
use powerio_dist::{DistLine, MulticonductorNetwork};

type Node = (String, String);

fn node(bus: &str, terminal: &str) -> Node {
    (bus.to_ascii_lowercase(), terminal.to_owned())
}

fn uses(net: &MulticonductorNetwork) -> BTreeMap<Node, usize> {
    let mut uses = BTreeMap::new();
    let mut add = |bus: &str, terminals: &[String]| {
        for terminal in terminals {
            *uses.entry(node(bus, terminal)).or_default() += 1;
        }
    };
    for line in net.lines() {
        add(&line.bus_from, &line.terminal_map_from);
        add(&line.bus_to, &line.terminal_map_to);
    }
    for switch in net.switches() {
        add(&switch.bus_from, &switch.terminal_map_from);
        add(&switch.bus_to, &switch.terminal_map_to);
    }
    for load in net.loads() {
        add(&load.bus, &load.terminal_map);
    }
    for generator in net.generators() {
        add(&generator.bus, &generator.terminal_map);
    }
    for shunt in net.shunts() {
        add(&shunt.bus, &shunt.terminal_map);
    }
    for capacitor in net.capacitors() {
        add(&capacitor.bus, &capacitor.terminal_map);
    }
    for ibr in net.ibrs() {
        add(&ibr.bus, &ibr.terminal_map);
    }
    for transformer in net.transformers() {
        for winding in &transformer.windings {
            add(&winding.bus, &winding.terminal_map);
        }
    }
    for source in net.sources() {
        add(&source.bus, &source.terminal_map);
        if let Some(reference) = &source.reference_terminal {
            add(&source.bus, std::slice::from_ref(reference));
        }
    }
    uses
}

fn series(net: &MulticonductorNetwork, line: &DistLine) -> Option<Complex64> {
    let code = net.linecode(&line.linecode)?;
    if code.n_conductors != 1
        || line.terminal_map_from.len() != 1
        || line.terminal_map_to.len() != 1
        || [&code.g_from, &code.g_to, &code.b_from, &code.b_to]
            .iter()
            .any(|m| m.iter().flatten().any(|&v| v != 0.0))
    {
        return None;
    }
    let z = Complex64::new(code.r_series[0][0], code.x_series[0][0]) * line.length;
    (z.re.is_finite() && z.im.is_finite() && z != Complex64::default()).then_some(z)
}

fn grounded(net: &MulticonductorNetwork, port: &Node) -> bool {
    net.bus(&port.0).unwrap().grounded.contains(&port.1)
}

/// Eliminate only series pairs whose KCL and KVL prove an ideal constraint
/// at the far terminal. An additional device connection prevents elimination.
pub fn contract_source_pairs(network: &MulticonductorNetwork) -> MulticonductorNetwork {
    let mut net = network.clone();
    loop {
        let before = net.lines().len();
        for index in 0..net.sources().len() {
            let source = &net.sources()[index];
            if source.terminal_map.len() != 1 {
                continue;
            }
            let start = node(&source.bus, &source.terminal_map[0]);
            let uses = uses(&net);
            if uses.get(&start) != Some(&2) || grounded(&net, &start) {
                continue;
            }
            let Some((first_idx, first)) = net.lines().iter().enumerate().find(|(_, l)| {
                l.terminal_map_from.len() == 1
                    && node(&l.bus_from, &l.terminal_map_from[0]) == start
            }) else {
                continue;
            };
            let Some(z1) = series(&net, first) else {
                continue;
            };
            let middle = node(&first.bus_to, &first.terminal_map_to[0]);
            if middle == start || uses.get(&middle) != Some(&2) || grounded(&net, &middle) {
                continue;
            }
            let Some((second_idx, second)) = net.lines().iter().enumerate().find(|(i, l)| {
                *i != first_idx
                    && l.terminal_map_from.len() == 1
                    && node(&l.bus_from, &l.terminal_map_from[0]) == middle
            }) else {
                continue;
            };
            if series(&net, second) != Some(-z1) {
                continue;
            }
            let end = node(&second.bus_to, &second.terminal_map_to[0]);
            if end == start
                || end == middle
                || source.reference_terminal.is_some() && !source.bus.eq_ignore_ascii_case(&end.0)
            {
                continue;
            }
            let removed_codes = [first.linecode.clone(), second.linecode.clone()];
            let source = &mut net.sources_mut()[index];
            source.bus.clone_from(&end.0);
            source.terminal_map = vec![end.1];
            net.lines_mut().remove(first_idx.max(second_idx));
            net.lines_mut().remove(first_idx.min(second_idx));
            let used_codes: BTreeSet<_> = net
                .lines()
                .iter()
                .map(|l| l.linecode.to_ascii_lowercase())
                .collect();
            net.line_codes_mut().retain(|c| {
                !removed_codes
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&c.name))
                    || used_codes.contains(&c.name.to_ascii_lowercase())
            });
            let remaining = self::uses(&net);
            let internal_buses = [start.0.clone(), middle.0.clone()];
            for internal in [start, middle] {
                let bus = net
                    .buses_mut()
                    .iter_mut()
                    .find(|b| b.id.eq_ignore_ascii_case(&internal.0))
                    .unwrap();
                if !remaining.contains_key(&internal) {
                    bus.terminals.retain(|t| t != &internal.1);
                }
            }
            net.buses_mut().retain(|b| {
                !internal_buses.contains(&b.id.to_ascii_lowercase())
                    || b.location.is_some()
                    || remaining
                        .keys()
                        .any(|(bus, _)| b.id.eq_ignore_ascii_case(bus))
            });
        }
        if net.lines().len() == before {
            break;
        }
    }
    net
}

fn electrical_node(net: &MulticonductorNetwork, bus: &str, terminal: &str) -> Node {
    let bus = net.bus(bus).unwrap();
    if bus.grounded.iter().any(|g| g == terminal) {
        return (String::new(), "earth".into());
    }
    let terminal = terminal.parse::<u32>().map_or_else(
        |_| (bus.terminals.iter().position(|t| t == terminal).unwrap() + 1).to_string(),
        |n| n.to_string(),
    );
    node(&bus.id, &terminal)
}

fn constraints(net: &MulticonductorNetwork) -> BTreeMap<(Node, Node), Complex64> {
    let mut result = BTreeMap::new();
    for source in net.sources() {
        assert_eq!(source.terminal_map.len(), source.v_magnitude.len());
        assert_eq!(source.terminal_map.len(), source.v_angle.len());
        let reference = source.reference_terminal.as_ref().map_or_else(
            || (String::new(), "earth".into()),
            |t| electrical_node(net, &source.bus, t),
        );
        for ((terminal, &magnitude), &angle) in source
            .terminal_map
            .iter()
            .zip(&source.v_magnitude)
            .zip(&source.v_angle)
        {
            let phase = electrical_node(net, &source.bus, terminal);
            let emf = Complex64::from_polar(magnitude, angle);
            if phase == reference {
                assert_eq!(
                    emf,
                    Complex64::default(),
                    "nonzero voltage across identical terminals"
                );
                continue;
            }
            if let Some(prior) = result.insert((phase.clone(), reference.clone()), emf) {
                assert!(
                    (prior - emf).norm() <= 1e-9,
                    "inconsistent duplicate source constraints: source {} at bus {} terminal {} \
                     ({phase:?} against {reference:?}) states {emf} where another states {prior}",
                    source.name,
                    source.bus,
                    terminal,
                );
            }
        }
    }
    result
}

pub fn assert_source_constraints(a: &MulticonductorNetwork, b: &MulticonductorNetwork, what: &str) {
    let a = constraints(a);
    let b = constraints(b);
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "{what}: source terminal constraints"
    );
    for (key, emf) in a {
        assert!(
            (emf - b[&key]).norm() <= 1e-9 * emf.norm().max(1.0),
            "{what}: source phasor at {key:?}: expected {emf:?}, actual {:?}, error {}",
            b[&key],
            (emf - b[&key]).norm()
        );
    }
}

/// Eliminate a transformer leakage/cancelling-line pair only after proving its
/// differential impedance is zero and its internal ports have no other use.
/// For two floating conductors, equal opposite currents make the differential
/// impedance z11 + z22 - z12 - z21; no ground constraint is introduced.
#[allow(dead_code)] // The CLI suite includes this file for source constraints only.
pub fn contract_transformer_pairs(network: &MulticonductorNetwork) -> MulticonductorNetwork {
    let mut net = network.clone();
    for index in 0..net.transformers().len() {
        let t = &net.transformers()[index];
        if t.phases != 1
            || t.windings.len() != 2
            || t.xsc_pct != [0.0]
            || t.windings[0].r_pct != 0.0
            || t.windings.iter().any(|w| {
                w.terminal_map.len() != 2
                    || w.conn != powerio_dist::DistWindingConn::Wye
                    || w.r_neutral.is_some_and(|r| r != 0.0)
                    || w.x_neutral.is_some_and(|x| x != 0.0)
            })
            || t.extras.iter().any(|(key, value)| match key.as_str() {
                "ppm_antifloat" => {
                    value.as_f64().or_else(|| value.as_str()?.parse().ok()) != Some(0.0)
                }
                _ => true,
            })
        {
            continue;
        }
        let secondary = &t.windings[1];
        let r = secondary.r_pct / 100.0 * (secondary.v_ref * secondary.tap).powi(2)
            / secondary.s_rating;
        if !r.is_finite() || r <= 0.0 {
            continue;
        }
        let grounded_return = grounded(&net, &node(&secondary.bus, &secondary.terminal_map[1]));
        let count = if grounded_return { 1 } else { 2 };
        let usage = uses(&net);
        if secondary.terminal_map[..count].iter().any(|terminal| {
            let port = node(&secondary.bus, terminal);
            usage.get(&port) != Some(&2) || grounded(&net, &port)
        }) {
            continue;
        }
        let Some((line_index, line)) = net.lines().iter().enumerate().find(|(_, line)| {
            line.bus_from.eq_ignore_ascii_case(&secondary.bus)
                && line.terminal_map_from == secondary.terminal_map[..count]
                && line.terminal_map_to.len() == count
                && !line.bus_to.eq_ignore_ascii_case(&secondary.bus)
        }) else {
            continue;
        };
        let code = net.linecode(&line.linecode).unwrap();
        if code.n_conductors != count
            || [
                &code.x_series,
                &code.g_from,
                &code.g_to,
                &code.b_from,
                &code.b_to,
            ]
            .iter()
            .any(|m| m.iter().flatten().any(|&v| v != 0.0))
        {
            continue;
        }
        let differential = if count == 1 {
            code.r_series[0][0]
        } else {
            code.r_series[0][0] + code.r_series[1][1] - code.r_series[0][1] - code.r_series[1][0]
        } * line.length;
        if (differential + r).abs() > 1e-12 * r {
            continue;
        }
        let mut terminals = line.terminal_map_to.clone();
        if grounded_return {
            let Some(ground) = net.bus(&line.bus_to).unwrap().grounded.first() else {
                continue;
            };
            terminals.push(ground.clone());
        }
        let internal_bus = secondary.bus.clone();
        let destination = line.bus_to.clone();
        let code_name = line.linecode.clone();
        let secondary = &mut net.transformers_mut()[index].windings[1];
        secondary.bus = destination;
        secondary.terminal_map = terminals;
        secondary.r_pct = 0.0;
        net.lines_mut().remove(line_index);
        if !net
            .lines()
            .iter()
            .any(|line| line.linecode.eq_ignore_ascii_case(&code_name))
        {
            net.line_codes_mut()
                .retain(|code| !code.name.eq_ignore_ascii_case(&code_name));
        }
        let remaining = uses(&net);
        net.buses_mut().retain(|bus| {
            !bus.id.eq_ignore_ascii_case(&internal_bus)
                || bus.location.is_some()
                || remaining
                    .keys()
                    .any(|(name, _)| name.eq_ignore_ascii_case(&bus.id))
        });
    }
    net
}

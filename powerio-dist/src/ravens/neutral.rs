//! Materialize implicit wye earth returns without grounding unrelated conductors.
use super::terminal_phase;
use crate::{Configuration, DistBus, DistSwitch, DistWindingConn, MulticonductorNetwork};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

enum Owner {
    Load(usize),
    Generator(usize),
    Winding(usize, usize),
}

fn has_neutral(map: &[String]) -> bool {
    map.iter().any(|t| terminal_phase(t) == Some("N"))
}

fn return_requests(net: &MulticonductorNetwork) -> Vec<(Owner, String, bool, bool)> {
    let implicit = |configuration: Configuration, map: &[String]| {
        !map.is_empty()
            && map
                .iter()
                .map(|t| terminal_phase(t))
                .collect::<Option<BTreeSet<_>>>()
                .is_some_and(|set| set.len() == map.len())
            && !has_neutral(map)
            && (configuration == Configuration::Wye
                || configuration == Configuration::SinglePhase && map.len() == 1)
    };
    let mut requests = Vec::new();
    for (i, l) in net.loads().iter().enumerate() {
        if implicit(l.configuration, &l.terminal_map) {
            requests.push((Owner::Load(i), l.bus.clone(), true, false));
        }
    }
    for (i, g) in net.generators().iter().enumerate() {
        if implicit(g.configuration, &g.terminal_map) {
            requests.push((Owner::Generator(i), g.bus.clone(), true, false));
        }
    }
    for (i, t) in net.transformers().iter().enumerate() {
        if t.windings.len() < 2 {
            continue;
        }
        for (j, w) in t.windings.iter().enumerate() {
            if w.conn != DistWindingConn::Wye {
                continue;
            }
            let grounded = w.r_neutral.unwrap_or(0.0) == 0.0 && w.x_neutral.unwrap_or(0.0) == 0.0;
            let missing_return = !has_neutral(&w.terminal_map) && w.terminal_map.len() == t.phases;
            let missing_ground = grounded
                && net
                    .bus(&w.bus)
                    .is_some_and(|b| !b.grounded.iter().any(|n| terminal_phase(n) == Some("N")));
            if missing_return || missing_ground {
                requests.push((
                    Owner::Winding(i, j),
                    w.bus.clone(),
                    grounded,
                    has_neutral(&w.terminal_map),
                ));
            }
        }
    }
    requests
}

pub(super) fn prepare(
    net: &MulticonductorNetwork,
) -> Result<Cow<'_, MulticonductorNetwork>, powerio_core::Error> {
    let requests = return_requests(net);
    let mut prepared = Cow::Borrowed(net);
    let mut isolated: BTreeMap<String, String> = BTreeMap::new();
    for (owner, bus, ground, explicit) in requests {
        let network = prepared.to_mut();
        let b = network
            .buses_mut()
            .iter_mut()
            .find(|b| b.id.eq_ignore_ascii_case(&bus))
            .ok_or_else(|| {
                powerio_core::Error::new(
                    &crate::diagnostics::codes::BUILD_DIST_ELECTRICAL_INCOMPLETE,
                    format!("wye return names undeclared bus {bus}"),
                )
            })?;
        let existing = b
            .terminals
            .iter()
            .find(|t| terminal_phase(t) == Some("N"))
            .cloned();
        if ground && !explicit && existing.as_ref().is_some_and(|n| !b.grounded.contains(n)) {
            // The bus's N is a different, floating conductor. Give the
            // implicit earth return a private grounded node; ideal closed-switch
            // phase connections preserve KCL/KVL without grounding that N.
            let neutral = existing.expect("checked neutral");
            let key = bus.to_ascii_lowercase();
            let target = if let Some(target) = isolated.get(&key).cloned() {
                target
            } else {
                let name = isolated_ground_bus(network, &bus, &neutral);
                isolated.insert(key, name.clone());
                name
            };
            let map = match owner {
                Owner::Load(i) => {
                    network.loads_mut()[i].bus = target;
                    &mut network.loads_mut()[i].terminal_map
                }
                Owner::Generator(i) => {
                    network.generators_mut()[i].bus = target;
                    &mut network.generators_mut()[i].terminal_map
                }
                Owner::Winding(i, j) => {
                    network.transformers_mut()[i].windings[j].bus = target;
                    &mut network.transformers_mut()[i].windings[j].terminal_map
                }
            };
            if !has_neutral(map) {
                map.push(neutral);
            }
            continue;
        }
        let neutral = existing.unwrap_or_else(|| {
            b.terminals.push("4".into());
            "4".into()
        });
        if ground && !b.grounded.contains(&neutral) {
            b.grounded.push(neutral.clone());
        }
        let map = match owner {
            Owner::Load(i) => &mut network.loads_mut()[i].terminal_map,
            Owner::Generator(i) => &mut network.generators_mut()[i].terminal_map,
            Owner::Winding(i, j) => &mut network.transformers_mut()[i].windings[j].terminal_map,
        };
        if !has_neutral(map) {
            map.push(neutral);
        }
    }
    Ok(prepared)
}

fn isolated_ground_bus(network: &mut MulticonductorNetwork, bus: &str, neutral: &str) -> String {
    let hot: Vec<String> = network
        .bus(bus)
        .expect("declared parent bus")
        .terminals
        .iter()
        .filter(|t| terminal_phase(t).is_some_and(|p| p != "N"))
        .cloned()
        .collect();
    let stem = format!("_pio_ravens_ground_{bus}");
    let mut name = stem.clone();
    let mut suffix = 1;
    while network
        .buses()
        .iter()
        .any(|b| b.id.eq_ignore_ascii_case(&name))
        || network
            .lines()
            .iter()
            .any(|l| l.name.eq_ignore_ascii_case(&name))
        || network
            .switches()
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&name))
    {
        name = format!("{stem}_{suffix}");
        suffix += 1;
    }
    let mut terminals = hot.clone();
    terminals.push(neutral.to_owned());
    let mut derived = DistBus::new(&name, terminals);
    derived.grounded.push(neutral.to_owned());
    network.buses_mut().push(derived);
    network
        .switches_mut()
        .push(DistSwitch::new(&name, &name, bus, hot.clone(), hot, false));
    name
}

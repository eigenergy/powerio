//! Format-independent checks of the physical data a distribution conversion
//! carries. Metadata findings never waive these comparisons. Callers must make
//! any documented physical projection explicitly, before comparing networks.

use std::collections::{BTreeMap, BTreeSet};

use powerio_dist::{Configuration, MulticonductorNetwork};

/// CIM has precisely four conductor identities. Project only a device that
/// cannot name its own conductors, requiring that device's explicit finding.
/// This is not permission to ignore the rest of an unsupported-conductor case.
pub fn ravens_projection(
    network: &MulticonductorNetwork,
    diagnostics: &[powerio_core::Diagnostic],
) -> (MulticonductorNetwork, Vec<String>) {
    let mut held = network.clone();
    let supported = |terminals: &[String]| {
        let phases: Option<BTreeSet<_>> = terminals
            .iter()
            .map(|terminal| powerio_core::cim::terminal_phase(terminal))
            .collect();
        phases.is_some_and(|phases| phases.len() == terminals.len())
    };
    let mut removed = Vec::new();
    let mut affected_buses = BTreeSet::new();
    held.lines_mut().retain(|line| {
        let keep = supported(&line.terminal_map_from) && supported(&line.terminal_map_to);
        if !keep {
            removed.push(format!("line {}:", line.name));
            affected_buses.insert(line.bus_from.to_ascii_lowercase());
            affected_buses.insert(line.bus_to.to_ascii_lowercase());
        }
        keep
    });
    held.loads_mut().retain(|load| {
        let keep = supported(&load.terminal_map);
        if !keep {
            removed.push(format!("load {}:", load.name));
            affected_buses.insert(load.bus.to_ascii_lowercase());
        }
        keep
    });
    // A ConnectivityNode with no equipment carries no conductor evidence.
    // Its reader retains it as an untyped object, not an invalid active bus
    // with an empty terminal set. Only nodes made unused by the explicitly
    // projected devices above receive this allowance.
    let mut used = BTreeSet::new();
    let mut use_bus = |bus: &str| {
        used.insert(bus.to_ascii_lowercase());
    };
    for line in held.lines() {
        use_bus(&line.bus_from);
        use_bus(&line.bus_to);
    }
    for switch in held.switches() {
        use_bus(&switch.bus_from);
        use_bus(&switch.bus_to);
    }
    for load in held.loads() {
        use_bus(&load.bus);
    }
    for generator in held.generators() {
        use_bus(&generator.bus);
    }
    for shunt in held.shunts() {
        use_bus(&shunt.bus);
    }
    for capacitor in held.capacitors() {
        use_bus(&capacitor.bus);
    }
    for ibr in held.ibrs() {
        use_bus(&ibr.bus);
    }
    for source in held.sources() {
        use_bus(&source.bus);
    }
    for transformer in held.transformers() {
        for winding in &transformer.windings {
            use_bus(&winding.bus);
        }
    }
    held.buses_mut().retain(|bus| {
        let name = bus.id.to_ascii_lowercase();
        !affected_buses.contains(&name) || used.contains(&name)
    });
    let problems = removed
        .into_iter()
        .filter(|component| {
            !diagnostics.iter().any(|finding| {
                finding.code() == "EMIT.RAVENS_DIST.RECORD_DROPPED"
                    && finding.message().starts_with(component)
            })
        })
        .map(|component| {
            format!("unsupported {component} needs its own EMIT.RAVENS_DIST.RECORD_DROPPED finding")
        })
        .collect();
    (held, problems)
}

/// The two documented RAVENS transformer projections. A particular field
/// changes in the expected circuit only when its own finding names it;
/// all other winding ports, ratios, resistances and leakage values stay checked.
#[allow(dead_code)] // Used by the distribution corpus and CIM interoperability suites.
pub fn ravens_transformer_projection(
    network: &MulticonductorNetwork,
    diagnostics: &[powerio_core::Diagnostic],
) -> (MulticonductorNetwork, Vec<String>) {
    let mut expected = network.clone();
    let mut problems = Vec::new();
    let mut require = |code: &str, prefix: &str| {
        if !diagnostics
            .iter()
            .any(|d| d.code() == code && d.message().starts_with(prefix))
        {
            problems.push(format!("{prefix}: missing {code} finding"));
        }
    };
    for transformer in expected.transformers_mut() {
        if transformer.xsc_pct.len() > 1 && transformer.xsc_pct[1..].iter().any(|x| *x != 0.0) {
            require(
                "EMIT.RAVENS_DIST.VALUE_COLLAPSED",
                &format!(
                    "transformer {}: only the first winding pair's short-circuit reactance",
                    transformer.name
                ),
            );
            transformer.xsc_pct[1..].fill(0.0);
        }
        for (index, winding) in transformer.windings.iter_mut().enumerate() {
            if winding.tap.total_cmp(&1.0).is_ne() {
                require(
                    "EMIT.RAVENS_DIST.FIELD_DROPPED",
                    &format!(
                        "transformer {}: winding {} tap {} is dropped",
                        transformer.name,
                        index + 1,
                        winding.tap
                    ),
                );
                winding.tap = 1.0;
            }
        }
    }
    (expected, problems)
}

fn close(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1e-12)
}

fn generated_source_identity(bus: &str) -> Option<String> {
    let id = bus.to_ascii_lowercase();
    if let Some(source) = id.strip_prefix("_pio_source_emf_") {
        return Some(source.to_owned());
    }
    let rest = id
        .strip_prefix("_pio_ravens_")
        .or_else(|| id.strip_prefix("_pio_vsource_"))?;
    let (source, suffix) = rest.rsplit_once("_emf")?;
    let collision_suffix = suffix.is_empty()
        || suffix
            .strip_prefix('_')
            .is_some_and(|n| n.parse::<usize>().is_ok());
    (collision_suffix && !source.is_empty()).then(|| source.to_owned())
}

fn generated_name_matches(name: &str, base: &str) -> bool {
    name == base
        || name.strip_prefix(base).is_some_and(|suffix| {
            suffix
                .strip_prefix('_')
                .is_some_and(|n| n.parse::<usize>().is_ok())
        })
}

fn source_impedance_line_identity(line: &powerio_dist::DistLine) -> Option<String> {
    if line.terminal_map_from.len() != 3 || line.terminal_map_to.len() != 3 {
        return None;
    }
    let source = generated_source_identity(&line.bus_from)?;
    let name = line.name.to_ascii_lowercase();
    let generated_names = [
        format!("_pio_source_impedance_{source}"),
        format!("_pio_ravens_{source}_impedance"),
        format!("_pio_vsource_{source}_impedance"),
    ];
    generated_names
        .iter()
        .any(|base| generated_name_matches(&name, base))
        .then(|| format!("_pio_source_impedance_{source}"))
}

fn canonical_bus(bus: &str) -> String {
    generated_source_identity(bus).map_or_else(
        || bus.to_ascii_lowercase(),
        |source| format!("_pio_source_emf_{source}"),
    )
}

fn port(net: &MulticonductorNetwork, bus: &str, terminal: &str) -> String {
    let Some(node) = net.bus(bus) else {
        return format!("undeclared bus {bus}:{terminal}");
    };
    // OpenDSS writes any explicitly grounded conductor as node zero. Numeric
    // conductor identities otherwise survive, including a phase permutation.
    let terminal = if terminal == "0" || node.grounded.iter().any(|t| t == terminal) {
        "ground".to_owned()
    } else if let Some(phase) = powerio_core::cim::terminal_phase(terminal) {
        powerio_core::cim::phase_terminal(phase).unwrap().to_owned()
    } else {
        terminal.parse::<usize>().map_or_else(
            |_| {
                node.terminals
                    .iter()
                    .position(|t| t == terminal)
                    .map_or_else(|| terminal.to_owned(), |i| (i + 1).to_string())
            },
            |i| i.to_string(),
        )
    };
    format!("{}:{terminal}", canonical_bus(bus))
}

fn compare_values(
    before: &BTreeMap<String, f64>,
    after: &BTreeMap<String, f64>,
    problems: &mut Vec<String>,
) {
    for (key, expected) in before {
        match after.get(key) {
            Some(actual) if close(*expected, *actual) => {}
            Some(actual) => problems.push(format!("{key}: {expected} -> {actual}")),
            None => problems.push(format!("{key}: missing (expected {expected})")),
        }
    }
    for (key, value) in after {
        if !before.contains_key(key) {
            problems.push(format!("{key}: unexpected {value}"));
        }
    }
}

fn line_values(net: &MulticonductorNetwork) -> Result<BTreeMap<String, f64>, String> {
    let mut values = BTreeMap::new();
    for line in net.lines() {
        let code = net
            .linecode(&line.linecode)
            .ok_or_else(|| format!("line {} has no linecode {}", line.name, line.linecode))?;
        let n = code.n_conductors;
        if line.terminal_map_from.len() != n || line.terminal_map_to.len() != n {
            return Err(format!(
                "line {} has inconsistent conductor maps",
                line.name
            ));
        }
        let identity =
            source_impedance_line_identity(line).unwrap_or_else(|| line.name.to_ascii_lowercase());
        let conductors: Vec<_> = line
            .terminal_map_from
            .iter()
            .zip(&line.terminal_map_to)
            .map(|(a, b)| {
                format!(
                    "{} -> {}",
                    port(net, &line.bus_from, a),
                    port(net, &line.bus_to, b)
                )
            })
            .collect();
        for (field, matrix) in [
            ("R (ohm)", &code.r_series),
            ("X (ohm)", &code.x_series),
            ("G from (S)", &code.g_from),
            ("B from (S)", &code.b_from),
            ("G to (S)", &code.g_to),
            ("B to (S)", &code.b_to),
        ] {
            if matrix.len() != n || matrix.iter().any(|row| row.len() != n) {
                return Err(format!("line {} has malformed {field} matrix", line.name));
            }
            for (i, row) in matrix.iter().enumerate() {
                for (j, value) in row.iter().enumerate() {
                    let key = format!(
                        "line {identity} {field} [{}, {}]",
                        conductors[i], conductors[j]
                    );
                    if values.insert(key.clone(), value * line.length).is_some() {
                        return Err(format!("duplicate electrical identity: {key}"));
                    }
                }
            }
        }
    }
    Ok(values)
}

fn switch_values(net: &MulticonductorNetwork) -> Result<BTreeMap<String, f64>, String> {
    let mut values = BTreeMap::new();
    for switch in net.switches() {
        if switch.terminal_map_from.len() != switch.terminal_map_to.len() {
            return Err(format!(
                "switch {} has inconsistent conductor maps",
                switch.name
            ));
        }
        for (from, to) in switch.terminal_map_from.iter().zip(&switch.terminal_map_to) {
            let key = format!(
                "switch {} [{} -> {}] open",
                switch.name.to_ascii_lowercase(),
                port(net, &switch.bus_from, from),
                port(net, &switch.bus_to, to)
            );
            if values.insert(key.clone(), f64::from(switch.open)).is_some() {
                return Err(format!("duplicate electrical identity: {key}"));
            }
        }
    }
    Ok(values)
}

fn injection_values(net: &MulticonductorNetwork) -> Result<BTreeMap<String, f64>, String> {
    let mut values = BTreeMap::new();
    let devices = net
        .loads()
        .iter()
        .map(|l| {
            (
                "load",
                &l.bus,
                &l.terminal_map,
                l.configuration,
                &l.p_nom,
                &l.q_nom,
            )
        })
        .chain(net.generators().iter().map(|g| {
            (
                "generator",
                &g.bus,
                &g.terminal_map,
                g.configuration,
                &g.p_nom,
                &g.q_nom,
            )
        }));
    for (kind, bus, terminals, configuration, p, q) in devices {
        // Dispatchable generators can state only bounds, without a nominal
        // operating point. They inject no stated nominal P/Q; a writer may
        // materialize zero, but a nonzero invented setpoint still differs.
        if kind == "generator" && p.is_empty() && q.is_empty() {
            continue;
        }
        let zeros = vec![0.0; p.len().max(q.len())];
        let p = if kind == "generator" && p.is_empty() {
            &zeros
        } else {
            p
        };
        let q = if kind == "generator" && q.is_empty() {
            &zeros
        } else {
            q
        };
        if p.len() != q.len() || p.is_empty() {
            return Err(format!("{kind} at {bus} has inconsistent powers"));
        }
        let count = p.len();
        for (phase, (p, q)) in p.iter().zip(q).enumerate() {
            let pair = match configuration {
                Configuration::SinglePhase if terminals.len() == 2 && count == 1 => {
                    (&terminals[0], Some(&terminals[1]))
                }
                Configuration::SinglePhase if terminals.len() == 1 && count == 1 => {
                    (&terminals[0], None)
                }
                Configuration::Wye if terminals.len() == count + 1 => {
                    (&terminals[phase], terminals.last())
                }
                Configuration::Wye if terminals.len() == count => (&terminals[phase], None),
                Configuration::Delta
                    if terminals.len() == count && count > 1
                        || terminals.len() == 2 && count == 1 =>
                {
                    (
                        &terminals[phase],
                        Some(&terminals[(phase + 1) % terminals.len()]),
                    )
                }
                _ => {
                    return Err(format!(
                        "{kind} at {bus} has unsupported terminal/power shape"
                    ));
                }
            };
            let from = port(net, bus, pair.0);
            let to = pair.1.map_or_else(
                || format!("{}:ground", bus.to_ascii_lowercase()),
                |terminal| port(net, bus, terminal),
            );
            // Per-phase OpenDSS splitting changes names and object counts, not
            // the power attached to a physical branch. Compare those branches.
            let (a, b) = if from <= to { (from, to) } else { (to, from) };
            *values
                .entry(format!("{kind} [{a}, {b}] P (W)"))
                .or_default() += p;
            *values
                .entry(format!("{kind} [{a}, {b}] Q (var)"))
                .or_default() += q;
        }
    }
    values.retain(|_, value| *value != 0.0);
    Ok(values)
}

fn shunt_values(net: &MulticonductorNetwork) -> Result<BTreeMap<String, f64>, String> {
    let mut values = BTreeMap::new();
    for shunt in net.shunts() {
        let n = shunt.terminal_map.len();
        for (field, matrix) in [("G (S)", &shunt.g), ("B (S)", &shunt.b)] {
            if matrix.len() != n || matrix.iter().any(|row| row.len() != n) {
                return Err(format!("shunt {} has malformed {field}", shunt.name));
            }
            for (i, row) in matrix.iter().enumerate() {
                for (j, value) in row.iter().enumerate() {
                    let a = port(net, &shunt.bus, &shunt.terminal_map[i]);
                    let b = port(net, &shunt.bus, &shunt.terminal_map[j]);
                    *values
                        .entry(format!("shunt {field} [{a}, {b}]"))
                        .or_default() += value;
                }
            }
        }
    }
    values.retain(|_, value| *value != 0.0);
    Ok(values)
}

fn transformer_values(net: &MulticonductorNetwork) -> Result<BTreeMap<String, f64>, String> {
    use powerio_dist::DistWindingConn;

    let mut values = BTreeMap::new();
    for transformer in net.transformers() {
        let n = transformer.windings.len();
        if n < 2 || transformer.phases == 0 || transformer.xsc_pct.len() != n * (n - 1) / 2 {
            return Err(format!(
                "transformer {} has inconsistent winding data",
                transformer.name
            ));
        }
        for phase in 0..transformer.phases {
            let mut connections = Vec::new();
            let mut volts = Vec::new();
            let mut bases = Vec::new();
            for winding in &transformer.windings {
                let terminals = &winding.terminal_map;
                let ground = "0".to_owned();
                let (from, to) = match winding.conn {
                    DistWindingConn::Wye if terminals.len() == transformer.phases + 1 => {
                        (&terminals[phase], terminals.last().unwrap())
                    }
                    DistWindingConn::Wye if terminals.len() == transformer.phases => {
                        (&terminals[phase], &ground)
                    }
                    DistWindingConn::Delta if terminals.len() == transformer.phases => {
                        (&terminals[phase], &terminals[(phase + 1) % terminals.len()])
                    }
                    DistWindingConn::Delta if transformer.phases == 1 && terminals.len() == 2 => {
                        (&terminals[0], &terminals[1])
                    }
                    _ => {
                        return Err(format!(
                            "transformer {} has unsupported winding terminals {:?}",
                            transformer.name, terminals
                        ));
                    }
                };
                connections.push(format!(
                    "{} -> {}",
                    port(net, &winding.bus, from),
                    port(net, &winding.bus, to)
                ));
                let branch_voltage =
                    if winding.conn == DistWindingConn::Wye && transformer.phases > 1 {
                        winding.v_ref / 3.0_f64.sqrt()
                    } else {
                        winding.v_ref
                    };
                volts.push(branch_voltage * winding.tap);
                bases.push(branch_voltage.powi(2) / (winding.s_rating / transformer.phases as f64));
            }
            // Decomposing one three-phase wye bank into three single-phase
            // units has the same physical winding ports, volts and ohms.
            // Emergency-rating rebasing likewise leaves these quantities fixed.
            let identity = format!("transformer [{}]", connections.join("; "));
            for (index, winding) in transformer.windings.iter().enumerate() {
                for (field, value) in [
                    ("voltage ratio to winding 0", volts[index] / volts[0]),
                    ("resistance (ohm)", winding.r_pct / 100.0 * bases[index]),
                ] {
                    let key = format!("{identity} winding {index} {field}");
                    if values.insert(key.clone(), value).is_some() {
                        return Err(format!("duplicate electrical identity: {key}"));
                    }
                }
            }
            for (pair, xsc) in transformer.xsc_pct.iter().enumerate() {
                values.insert(
                    format!("{identity} leakage pair {pair} (ohm)"),
                    xsc / 100.0 * bases[0],
                );
            }
        }
    }
    Ok(values)
}

pub fn differences(before: &MulticonductorNetwork, after: &MulticonductorNetwork) -> Vec<String> {
    let mut problems = Vec::new();
    for comparison in [
        line_values,
        switch_values,
        injection_values,
        shunt_values,
        transformer_values,
    ] {
        match (comparison(before), comparison(after)) {
            (Ok(a), Ok(b)) => compare_values(&a, &b, &mut problems),
            (Err(error), _) => problems.push(format!("source electrical check: {error}")),
            (_, Err(error)) => problems.push(format!("result electrical check: {error}")),
        }
    }
    problems
}

#[cfg(test)]
mod source_impedance_identity_tests {
    use super::*;
    use powerio_dist::{DistBus, DistLine, DistLineCode, VoltageSource};

    fn network(
        source_name: &str,
        source_bus: &str,
        line_name: &str,
        linecode: &str,
        r_scale: f64,
    ) -> MulticonductorNetwork {
        let mut net = MulticonductorNetwork::default();
        let phases = vec!["1".to_owned(), "2".to_owned(), "3".to_owned()];
        net.buses_mut()
            .push(DistBus::new(source_bus, phases.clone()));
        net.buses_mut()
            .push(DistBus::new("sourcebus", phases.clone()));
        net.sources_mut().push(VoltageSource::new(
            source_name,
            source_bus,
            phases.clone(),
            vec![230.0; 3],
            vec![
                0.0,
                -2.0 * std::f64::consts::PI / 3.0,
                2.0 * std::f64::consts::PI / 3.0,
            ],
        ));
        let r = vec![
            vec![0.1 * r_scale, 0.02 * r_scale, 0.02 * r_scale],
            vec![0.02 * r_scale, 0.1 * r_scale, 0.02 * r_scale],
            vec![0.02 * r_scale, 0.02 * r_scale, 0.1 * r_scale],
        ];
        let x = vec![
            vec![0.3, 0.04, 0.04],
            vec![0.04, 0.3, 0.04],
            vec![0.04, 0.04, 0.3],
        ];
        net.line_codes_mut().push(DistLineCode::new(linecode, r, x));
        net.lines_mut().push(DistLine::new(
            line_name,
            source_bus,
            "sourcebus",
            phases.clone(),
            phases,
            linecode,
            1.0,
        ));
        net
    }

    #[test]
    fn generated_source_impedance_names_compare_across_ravens_and_dss() {
        let ravens = network(
            "source",
            "_pio_ravens_source_emf",
            "_pio_ravens_source_impedance",
            "ravens_source_z",
            1.0,
        );
        let dss = network(
            "source",
            "_pio_vsource_source_emf",
            "_pio_vsource_source_impedance",
            "dss_source_z",
            1.0,
        );
        assert!(
            differences(&ravens, &dss).is_empty(),
            "same source impedance must compare equal across generated names"
        );
    }

    #[test]
    fn generated_source_impedance_names_with_delimiters_and_collision_suffixes_compare_equal() {
        let source = "source_impedance_emf";
        let ravens = network(
            source,
            "_pio_ravens_source_impedance_emf_emf_1",
            "_pio_ravens_source_impedance_emf_impedance_1",
            "ravens_source_z",
            1.0,
        );
        let dss = network(
            source,
            "_pio_vsource_source_impedance_emf_emf_2",
            "_pio_vsource_source_impedance_emf_impedance_2",
            "dss_source_z",
            1.0,
        );
        assert!(
            differences(&ravens, &dss).is_empty(),
            "source names containing generated delimiters and collision suffixes must compare equal"
        );
    }

    #[test]
    fn generated_source_impedance_comparison_still_detects_model_changes() {
        let ravens = network(
            "source",
            "_pio_ravens_source_emf",
            "_pio_ravens_source_impedance",
            "ravens_source_z",
            1.0,
        );
        let changed = network(
            "source",
            "_pio_vsource_source_emf",
            "_pio_vsource_source_impedance",
            "dss_source_z",
            1.01,
        );
        assert!(
            differences(&ravens, &changed)
                .iter()
                .any(|problem| problem.contains("R (ohm)")),
            "normalization must not hide impedance changes"
        );
    }
}

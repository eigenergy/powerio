//! Typed Thevenin decomposition, staged behind the private reader oracle.
//!
//! Each internal EMF terminal lives on the return bus so the existing ideal
//! source can constrain its voltage relative to the actual return terminal.
//! One coupled series branch connects those terminals to bus1. Its KVL is
//! `V(bus1) - V(bus2) = E - Z I`, with no added grounding or small impedance.

use super::*;
use crate::dss::source::{ResolvedSource, resolve};

type SourceResult<T> = std::result::Result<T, String>;

struct Ports {
    source: ResolvedSource,
    name: String,
    start: BusSpec,
    end: BusSpec,
    from: Vec<String>,
    to: Vec<String>,
}

impl Reader<'_> {
    fn resolved_sources(&mut self, raw: &RawDss) -> SourceResult<()> {
        // Resolve every source before modifying the reader. Then register
        // every original port before any source can allocate a virtual node.
        let resolved = raw
            .of_class("vsource")
            .map(|object| {
                let source = resolve(raw, object, self.net.base_frequency())?;
                let start = source_bus(&source.bus1)?;
                let end = source_bus(&source.bus2)?;
                Ok((object.name.clone(), source, start, end))
            })
            .collect::<SourceResult<Vec<_>>>()?;
        let ports: Vec<_> = resolved
            .into_iter()
            .map(|(name, source, start, end)| {
                let n = source.emf.len();
                let from = self.terminals(&start, n, n, n);
                let to = self.terminals(&end, n, n, n);
                Ports {
                    source,
                    name,
                    start,
                    end,
                    from,
                    to,
                }
            })
            .collect();
        let mut codes = names(
            self.net
                .line_codes()
                .iter()
                .map(|v| v.name.as_str())
                .chain(self.net.lines().iter().map(|v| v.linecode.as_str())),
        );
        let mut lines = names(raw.of_class("line").map(|v| v.name.as_str()));
        let mut sources = names(ports.iter().map(|v| v.name.as_str()));
        let mut cursors = BTreeMap::new();
        for port in ports {
            let n = port.source.emf.len();
            let key = port.end.name.to_ascii_lowercase();
            let bus = self.buses.get_mut(&key).unwrap();
            let cursor = cursors.entry(key).or_insert(1);
            let virtuals = allocate_terminals(bus, cursor, n)?;
            let base = format!("_pio_vsource_{}_impedance", port.name);
            let code_name = unique_name(&base, &mut codes);
            let line_name = unique_name(&base, &mut lines);
            let mut code = DistLineCode::new(
                &code_name,
                port.source
                    .impedance
                    .iter()
                    .map(|row| row.iter().map(|v| v.re).collect())
                    .collect(),
                port.source
                    .impedance
                    .iter()
                    .map(|row| row.iter().map(|v| v.im).collect())
                    .collect(),
            );
            code.source = Some(format!("OpenDSS Vsource.{} Thevenin impedance", port.name));
            self.net.line_codes_mut().push(code);
            self.net.lines_mut().push(DistLine::new(
                line_name,
                &port.end.name,
                &port.start.name,
                virtuals.clone(),
                port.from,
                code_name,
                1.0,
            ));
            // Different return conductors require different constraints even
            // though their series impedances belong to one coupled matrix.
            let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            for (i, reference) in port.to.iter().enumerate() {
                groups.entry(reference.clone()).or_default().push(i);
            }
            for (group, (reference, indexes)) in groups.into_iter().enumerate() {
                let name = if group == 0 {
                    port.name.clone()
                } else {
                    unique_name(
                        &format!("_pio_vsource_{}_return_{group}", port.name),
                        &mut sources,
                    )
                };
                let mut source = VoltageSource::new(
                    name,
                    &port.end.name,
                    indexes.iter().map(|&i| virtuals[i].clone()).collect(),
                    indexes.iter().map(|&i| port.source.emf[i].norm()).collect(),
                    indexes.iter().map(|&i| port.source.emf[i].arg()).collect(),
                );
                // Keep the explicit ground marker until finish_buses rewrites
                // both sides of every voltage constraint consistently.
                source.reference_terminal = Some(reference);
                self.net.sources_mut().push(source);
            }
        }
        Ok(())
    }
}

fn allocate_terminals(
    bus: &mut BusState,
    cursor: &mut i32,
    count: usize,
) -> SourceResult<Vec<String>> {
    // Numeric identifiers survive DSS terminal mapping. Sweep free identifiers
    // once per bus, without restarting for each source or incrementing a node
    // supplied by the user (which can be near the integer limit).
    let mut result = Vec::with_capacity(count);
    while result.len() < count {
        if bus.nodes.insert(*cursor) {
            result.push(cursor.to_string());
        }
        *cursor = cursor
            .checked_add(1)
            .ok_or("source terminal space exhausted")?;
    }
    Ok(result)
}

fn source_bus(text: &str) -> SourceResult<BusSpec> {
    let bus = Value::new(text).to_bus_spec();
    if bus.name.is_empty() || bus.nodes.iter().any(|&n| n < 0 || n == i32::MAX) {
        return Err(format!("invalid or unmaterializable source bus {text:?}"));
    }
    Ok(bus)
}

fn names<'a>(values: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    values.map(str::to_ascii_lowercase).collect()
}

fn unique_name(base: &str, used: &mut BTreeSet<String>) -> String {
    let mut name = base.to_owned();
    let mut suffix = 0;
    while !used.insert(name.to_ascii_lowercase()) {
        suffix += 1;
        name = format!("{base}_{suffix}");
    }
    name
}

fn staged_read(text: &str) -> MulticonductorNetwork {
    let raw = crate::dss::raw::__parse_raw_with(text, "source.dss", &mut |_: &std::path::Path| {
        Err(std::io::Error::other("no includes"))
    });
    let (net, _) = to_network_with_source_reader(
        &raw,
        Some(|rd, raw| {
            rd.resolved_sources(raw).unwrap();
        }),
    );
    net
}

#[test]
fn coupled_sources_keep_each_return_and_register_all_original_terminals_first() {
    let net = staged_read(
        "New Circuit.c phases=3 bus1=feed.3.1.2 bus2=return.0.4.7 basekv=1 z1=[1 2] z0=[4 5]\nNew Vsource.later phases=1 bus1=return.1 bus2=other.0 r1=2 x1=0",
    );
    let line = &net.lines()[0];
    assert_eq!(line.terminal_map_to, ["3", "1", "2"]);
    assert_eq!(line.terminal_map_from, ["2", "3", "5"]);
    let bus = net.buses().iter().find(|b| b.id == "return").unwrap();
    assert_eq!(bus.grounded, ["8"]);
    let constraints: Vec<_> = net.sources().iter().filter(|s| s.bus == "return").collect();
    assert_eq!(constraints.len(), 3);
    for (source, reference) in constraints.iter().zip(["8", "4", "7"]) {
        assert_eq!(source.reference_terminal.as_deref(), Some(reference));
        assert!(source.extras.is_empty());
    }
    assert_eq!(
        net.line_codes()[0].r_series,
        vec![
            vec![2.0, 1.0, 1.0],
            vec![1.0, 2.0, 1.0],
            vec![1.0, 1.0, 2.0]
        ]
    );
    assert!(
        net.buses()
            .iter()
            .find(|b| b.id == "feed")
            .unwrap()
            .grounded
            .is_empty()
    );
}

#[test]
fn finite_return_is_not_grounded_and_synthetic_names_do_not_collide() {
    let net = staged_read(
        "New Circuit.c phases=1 bus1=feed.1 bus2=return.4 r1=3 x1=2\nNew Linecode._pio_vsource_source_impedance nphases=1 r1=1 x1=0\nNew Line._pio_vsource_source_impedance phases=1 bus1=return.4 bus2=return.0 r1=5 x1=0 c1=0 c0=0",
    );
    let line = net.lines().last().unwrap();
    assert_eq!(line.name, "_pio_vsource_source_impedance_1");
    assert_eq!(line.linecode, "_pio_vsource_source_impedance_1");
    let bus = net.buses().iter().find(|b| b.id == "return").unwrap();
    assert!(!bus.grounded.contains(&"4".to_owned()));
    assert_eq!(net.sources()[0].reference_terminal.as_deref(), Some("4"));
    assert_eq!(net.line_codes().last().unwrap().r_series, vec![vec![3.0]]);
}

#[test]
fn source_decomposition_does_not_resolve_a_dangling_linecode_or_ground_a_return() {
    let net = staged_read(
        "New Circuit.c phases=1 bus1=feed.1 bus2=return.4 r1=3 x1=2\nNew Line.unresolved phases=1 bus1=feed.1 bus2=load.1 linecode=_pio_vsource_source_impedance",
    );
    let original = &net.lines()[0];
    assert!(!net.line_codes().iter().any(|c| c.name == original.linecode));
    assert_eq!(net.lines()[1].linecode, "_pio_vsource_source_impedance_1");
    assert!(net.buses().iter().all(|b| b.grounded.is_empty()));
}

#[test]
#[ignore = "exports staged typed networks for the pinned source circuit oracle"]
fn export_staged_source_networks() {
    let manifest = std::env::var("POWERIO_SOURCE_LOWERING_INPUT").expect("input manifest");
    let paths: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    let mut networks = BTreeMap::new();
    for (name, path) in paths {
        let text = std::fs::read_to_string(&path).unwrap();
        let is_glm = std::path::Path::new(&path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("glm"));
        let net = if is_glm {
            crate::glm::parse(
                powerio_core::Source::from_memory("case.glm", text.as_bytes()).unwrap(),
                crate::glm::PreprocessOptions::default(),
            )
            .unwrap()
            .value()
            .clone()
        } else {
            staged_read(&text)
        };
        if let Ok(root) = std::env::var("POWERIO_SOURCE_CANONICAL_DIR") {
            let directory = std::path::Path::new(&root).join(&name);
            std::fs::create_dir_all(&directory).unwrap();
            write_staged_network(&net, &directory, "case");
            if is_glm {
                let mut edited = net.clone();
                edited.sources_mut()[0].v_magnitude[0] *= 1.03;
                edited.lines_mut()[0].length *= 1.1;
                for load in edited.loads_mut() {
                    for value in load.p_nom.iter_mut().chain(load.q_nom.iter_mut()) {
                        *value *= 1.25;
                    }
                }
                edited
                    .buses_mut()
                    .iter_mut()
                    .find(|b| b.id == "source")
                    .unwrap()
                    .location = Some(DistLocation {
                    x: 0.0,
                    y: -27.5,
                    kind: None,
                });
                write_staged_network(&edited, &directory, "edited");
            }
        }
        let encoded = serde_json::to_value(&net).unwrap();
        let restored: MulticonductorNetwork = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(encoded, serde_json::to_value(&restored).unwrap());
        networks.insert(name, encoded);
    }
    let output = std::env::var("POWERIO_SOURCE_LOWERING_OUTPUT").expect("output path");
    std::fs::write(output, serde_json::to_vec_pretty(&networks).unwrap()).unwrap();
}

fn write_staged_network(net: &MulticonductorNetwork, directory: &std::path::Path, stem: &str) {
    let emitted = crate::dss::write::emit_staged_sources(net).unwrap();
    let typed = serde_json::to_value(net).unwrap();
    let restored: MulticonductorNetwork = serde_json::from_value(typed.clone()).unwrap();
    assert_eq!(
        emitted.text,
        crate::dss::write::emit_staged_sources(&restored)
            .unwrap()
            .text
    );
    let path = directory.join(format!("{stem}.dss"));
    std::fs::write(&path, &emitted.text).unwrap();
    std::fs::write(
        directory.join(format!("{stem}.typed.json")),
        serde_json::to_vec_pretty(&typed).unwrap(),
    )
    .unwrap();
    for sidecar in emitted.sidecars {
        std::fs::write(directory.join(sidecar.path), sidecar.text).unwrap();
    }
    let raw = crate::dss::raw::__parse_raw_file(&path).unwrap();
    let (reimported, diagnostics) =
        to_network_with_source_reader(&raw, Some(|rd, raw| rd.resolved_sources(raw).unwrap()));
    assert!(
        diagnostics
            .iter()
            .all(|d| d.severity() != powerio_core::DiagnosticSeverity::Error)
    );
    let target = if stem == "case" {
        "reimport.json".to_owned()
    } else {
        format!("{stem}.reimport.json")
    };
    std::fs::write(
        directory.join(target),
        serde_json::to_vec_pretty(&reimported).unwrap(),
    )
    .unwrap();
    let second = crate::dss::write::emit_staged_sources(&reimported).unwrap();
    std::fs::write(directory.join(format!("{stem}.reimport.dss")), second.text).unwrap();
    for sidecar in second.sidecars {
        assert_eq!(
            std::fs::read_to_string(directory.join(sidecar.path)).unwrap(),
            sidecar.text
        );
    }
}

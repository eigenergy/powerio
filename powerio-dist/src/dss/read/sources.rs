//! Typed Thevenin decomposition for static snapshots.
//!
//! Floating EMF constraints share the return bus; earth-referenced sources
//! use a distinct internal bus, preserving physical bus conductor identities.
//! One coupled series branch connects those terminals to bus1. Its KVL is
//! `V(bus1) - V(bus2) = E - Z I`, with no added grounding or small impedance.

use super::{
    BTreeMap, BTreeSet, BusSpec, BusState, C, DistLine, DistLineCode, RawDss, Reader,
    UntypedObject, Value, VoltageSource,
};
#[cfg(test)]
use super::{DistLocation, MulticonductorNetwork};
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
    fn retain_disabled_sources(&mut self, raw: &RawDss) {
        for object in raw
            .of_class("vsource")
            .filter(|object| !raw.source_replay(object).enabled)
        {
            let mut retained = UntypedObject::from(object);
            retained.props.push((Some("enabled".into()), "no".into()));
            self.net.untyped_objects_mut().push(retained);
            self.warn(&C::READ_DSS_SOURCE_DISABLED, format!(
                "Vsource.{} is disabled: retained as inactive source data without electrical branches or voltage constraints", object.name,
            ));
        }
    }

    fn report_source_behavior(&mut self, raw: &RawDss) {
        for object in raw.of_class("vsource") {
            for field in ["phases", "basekv", "pu", "angle", "bus1"] {
                if object.get(field).is_none() {
                    self.defaulted("vsource", &object.name, field);
                }
            }
            let settings: Vec<_> = ["daily", "yearly", "duty", "spectrum", "scantype"]
                .into_iter()
                .filter(|key| object.get(key).is_some_and(|v| !v.text.is_empty()))
                .collect();
            if !settings.is_empty() && raw.source_replay(object).enabled {
                self.warn(&C::READ_DSS_RETAINED_SOURCE_ONLY, format!(
                    "Vsource.{} settings {} do not change snapshot source voltages; their non-snapshot behavior remains in retained source data and is omitted from the typed equivalent",
                    object.name, settings.join(", "),
                ));
            }
        }
    }

    fn emf_bus(&mut self, port: &Ports, grounded: bool, names: &mut BTreeSet<String>) -> String {
        if !grounded {
            return port.end.name.clone();
        }
        let name = unique_name(&format!("_pio_vsource_{}_emf", port.name), names);
        let spec = BusSpec {
            name: name.clone(),
            nodes: vec![0],
        };
        self.terminals(&spec, 1, 1, 1);
        name
    }

    #[expect(
        clippy::too_many_lines,
        reason = "keep source port allocation and both parts of its equivalent in one ordered pass"
    )]
    pub(super) fn resolved_sources(&mut self, raw: &RawDss) -> SourceResult<()> {
        require_snapshot_profile(raw)?;
        // Resolve every source before modifying the reader. Then register
        // every original port before any source can allocate a virtual node.
        let mut resolved = Vec::new();
        for object in raw.of_class("vsource") {
            // Native construction and primitive checks still apply to disabled
            // sources. Only their electrical contribution is omitted.
            let source = resolve(raw, object, self.net.base_frequency())
                .map_err(|error| format!("Vsource.{}: {error}", object.name))?;
            if raw.source_replay(object).enabled {
                let start = source_bus(&source.bus1)?;
                let end = source_bus(&source.bus2)?;
                resolved.push((object.name.clone(), source, start, end));
            }
        }
        self.retain_disabled_sources(raw);
        self.report_source_behavior(raw);
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
                .chain(self.net.lines().iter().map(|v| v.linecode.as_str()))
                .chain(raw.of_class("linecode").map(|v| v.name.as_str())),
        );
        let mut lines = names(raw.of_class("line").map(|v| v.name.as_str()));
        let mut sources = names(ports.iter().map(|v| v.name.as_str()));
        let mut cursors = BTreeMap::new();
        let mut bus_names = self.buses.keys().cloned().collect();
        for port in ports {
            let n = port.source.emf.len();
            // Earth-referenced sources can put the internal EMF on a distinct
            // bus. This keeps original bus conductor identities unchanged and
            // lets ordinary three-phase consumers see a separate source branch.
            let grounded = port.to.iter().all(|t| t == "0");
            let emf_bus = self.emf_bus(&port, grounded, &mut bus_names);
            let key = emf_bus.to_ascii_lowercase();
            let bus = self.buses.get_mut(&key).unwrap();
            let cursor = cursors.entry(key).or_insert(1);
            let virtuals = allocate_terminals(bus, cursor, n)?;
            let base = format!("_pio_vsource_{}_impedance", port.name);
            let code_name = unique_name(&base, &mut codes);
            let line_name = unique_name(&base, &mut lines);
            let (resistance, reactance): (Vec<Vec<_>>, Vec<Vec<_>>) = port
                .source
                .impedance
                .iter()
                .map(|row| row.iter().map(|v| (v.re, v.im)).unzip())
                .unzip();
            let mut code = DistLineCode::new(&code_name, resistance, reactance);
            code.source = Some(format!("OpenDSS Vsource.{} Thevenin impedance", port.name));
            self.net.line_codes_mut().push(code);
            self.net.lines_mut().push(DistLine::new(
                line_name,
                &emf_bus,
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
                    &emf_bus,
                    indexes.iter().map(|&i| virtuals[i].clone()).collect(),
                    indexes.iter().map(|&i| port.source.emf[i].norm()).collect(),
                    indexes.iter().map(|&i| port.source.emf[i].arg()).collect(),
                );
                // Keep the explicit ground marker until finish_buses rewrites
                // both sides of every voltage constraint consistently.
                source.reference_terminal = (!grounded).then_some(reference);
                // Nominal bases are unit metadata, independent of edited EMF
                // magnitudes. All electrical impedance remains in the line.
                source.extras.insert(
                    "nominal_phase_kv".into(),
                    port.source.nominal_phase_kv.into(),
                );
                source
                    .extras
                    .insert("basemva".into(), port.source.base_mva.into());
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

fn require_snapshot_profile(raw: &RawDss) -> SourceResult<()> {
    if let Some(mode) = raw.unresolved_mode_transition() {
        return Err(format!(
            "solution mode transition {mode} requires native solved state; it cannot be reconstructed by static parsing, including after a return to snapshot mode",
        ));
    }
    if raw.solution_mode() != "snapshot" {
        return Err(format!(
            "solution mode {} requires a separately materialized static state; its source/load behavior is not qualified by the snapshot reader",
            raw.solution_mode(),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn staged_read(text: &str) -> MulticonductorNetwork {
    let raw = crate::dss::raw::__parse_raw_with(text, "source.dss", &mut |_: &std::path::Path| {
        Err(std::io::Error::other("no includes"))
    });
    staged_from_raw(&raw).unwrap()
}

#[cfg(test)]
pub(super) fn staged_from_raw(
    raw: &RawDss,
) -> std::result::Result<MulticonductorNetwork, Vec<crate::diagnostics::Diagnostic>> {
    let (net, diagnostics) = super::snapshot_network(raw);
    if diagnostics
        .iter()
        .any(|d| d.severity() == powerio_core::DiagnosticSeverity::Error)
    {
        Err(diagnostics)
    } else {
        Ok(net)
    }
}

#[test]
fn invalid_sources_never_return_a_partial_network_even_when_disabled() {
    for enabled in ["yes", "no"] {
        for properties in [
            "phases=1 z1=[0 0]",
            "phases=3 z1=[1 0] z0=[0 0]",
            "phases=1 model=ideal puzideal=[0 0]",
            "phases=1 z1=[0 0]\nEdit Vsource.bad z1=[1 0]",
        ] {
            let script =
                format!("New Circuit.c z1=[1 0]\nNew Vsource.bad enabled={enabled} {properties}");
            let raw = crate::dss::raw::__parse_raw_with(
                &script,
                "invalid.dss",
                &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
            );
            let diagnostics = staged_from_raw(&raw).unwrap_err();
            assert!(
                diagnostics.iter().any(|diagnostic| {
                    diagnostic.code() == C::READ_DSS_SOURCE_UNRESOLVED.code
                        && diagnostic.severity() == powerio_core::DiagnosticSeverity::Error
                        && format!("{diagnostic:?}").contains("Vsource.bad")
                }),
                "{script}: {diagnostics:?}"
            );
        }
    }
}

#[test]
#[ignore = "requires the pinned local OpenDSS source-validity oracle"]
fn pinned_source_validity() {
    #[derive(serde::Deserialize)]
    struct Case {
        script: String,
        accepted: bool,
        expected_error: Option<u32>,
    }
    #[derive(serde::Deserialize)]
    struct Report {
        cases: BTreeMap<String, Case>,
        failures: Vec<String>,
    }
    let path = std::env::var("POWERIO_SOURCE_VALIDITY_ORACLE").expect("oracle report path");
    let report: Report = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(report.failures.is_empty());
    assert!(!report.cases.is_empty());
    let mut findings = BTreeMap::new();
    for (name, case) in report.cases {
        let raw = crate::dss::raw::__parse_raw_with(
            &case.script,
            "validity.dss",
            &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
        );
        let result = staged_from_raw(&raw);
        assert_eq!(result.is_ok(), case.accepted, "{name}: {result:?}");
        if let Err(diagnostics) = result {
            assert!(case.expected_error.is_some(), "{name}");
            assert!(
                diagnostics.iter().any(|d| {
                    d.code() == C::READ_DSS_SOURCE_UNRESOLVED.code
                        && d.severity() == powerio_core::DiagnosticSeverity::Error
                }),
                "{name}: {diagnostics:?}"
            );
        }
        findings.insert(name, case.accepted);
    }
    let output = std::env::var("POWERIO_SOURCE_VALIDITY_RESULT").expect("comparison path");
    std::fs::write(output, serde_json::to_vec_pretty(&findings).unwrap()).unwrap();
}

#[test]
fn snapshot_settings_validate_references_and_do_not_apply_shapes() {
    let base = "New Circuit.c phases=1 basekv=0.24 pu=0.9 z1=[1 2]\n";
    let plain = staged_read(base);
    let shaped = staged_read(&format!(
        "{base}New Loadshape.day npts=1 interval=1 mult=[0.4]\nEdit Vsource.source daily=day scantype=neg"
    ));
    assert_eq!(plain.sources(), shaped.sources());
    for body in [
        "Edit Vsource.source daily=later\nNew Loadshape.later npts=1 mult=[1]",
        "Edit Vsource.source daily=missing daily=default",
        "Edit Vsource.source enabled=no spectrum=missing",
        "New Loadshape.none npts=1 mult=[1]\nEdit Vsource.source duty=none",
        "Edit Vsource.source scantype=nonsense",
        "Set mode=daily",
    ] {
        let raw = crate::dss::raw::__parse_raw_with(
            &format!("{base}{body}"),
            "profile.dss",
            &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
        );
        assert!(staged_from_raw(&raw).is_err(), "{body}");
    }
    assert!(
        staged_read(&format!("{base}Set mode=daily\nSolve mode=snapshot"))
            .sources()
            .len()
            == 1
    );
}

#[test]
#[ignore = "requires pinned native source-profile observations"]
fn pinned_source_profiles() {
    #[derive(serde::Deserialize)]
    struct Case {
        script: String,
        mode: Option<String>,
        profile_accepted: bool,
        emf: Option<Vec<[f64; 2]>>,
    }
    #[derive(serde::Deserialize)]
    struct Report {
        cases: BTreeMap<String, Case>,
        failures: Vec<String>,
    }
    let path = std::env::var("POWERIO_SOURCE_PROFILE_ORACLE").expect("oracle report path");
    let report: Report = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(report.failures.is_empty());
    assert!(!report.cases.is_empty());
    let mut findings = BTreeMap::new();
    for (name, case) in report.cases {
        let raw = crate::dss::raw::__parse_raw_with(
            &case.script,
            "profile.dss",
            &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
        );
        if let Some(mode) = case.mode {
            assert_eq!(raw.solution_mode(), mode, "{name}");
        }
        let result = staged_from_raw(&raw);
        assert_eq!(result.is_ok(), case.profile_accepted, "{name}: {result:?}");
        if case.profile_accepted {
            let expected = case.emf.expect("native snapshot EMF");
            let source = resolve(
                &raw,
                raw.find("vsource", "source").unwrap(),
                raw.snapshot_frequency(),
            )
            .unwrap();
            assert_eq!(expected.len(), source.emf.len());
            for (expected, actual) in expected.iter().zip(source.emf) {
                let expected = num_complex::Complex64::new(expected[0], expected[1]);
                assert!(
                    (actual - expected).norm() <= 1e-6 + 1e-9 * expected.norm(),
                    "{name}: {actual} != {expected}"
                );
            }
        } else {
            assert!(
                result.unwrap_err().iter().any(|d| {
                    d.code() == C::READ_DSS_SOURCE_UNRESOLVED.code
                        && d.severity() == powerio_core::DiagnosticSeverity::Error
                }),
                "{name}"
            );
        }
        findings.insert(name, case.profile_accepted);
    }
    let output = std::env::var("POWERIO_SOURCE_PROFILE_RESULT").expect("comparison path");
    std::fs::write(output, serde_json::to_vec_pretty(&findings).unwrap()).unwrap();
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
        assert_eq!(source.extras.len(), 2);
        assert_eq!(source.extras["basemva"], serde_json::json!(100.0));
        assert!(
            (source.extras["nominal_phase_kv"].as_f64().unwrap() - 1.0 / 3_f64.sqrt()).abs()
                < 1e-15
        );
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
fn source_decomposition_does_not_resolve_a_dangling_linecode() {
    let raw = crate::dss::raw::__parse_raw_with(
        "New Circuit.c phases=1 bus1=feed.1 bus2=return.4 r1=3 x1=2\nNew Line.unresolved phases=1 bus1=feed.1 bus2=load.1 linecode=_pio_vsource_source_impedance",
        "source.dss",
        &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
    );
    assert!(
        staged_from_raw(&raw)
            .unwrap_err()
            .iter()
            .any(|d| d.code() == C::READ_DSS_LINE_UNRESOLVED.code)
    );
}

#[test]
fn disabled_sources_contribute_no_terminals_or_impedance_and_retain_the_final_state() {
    let net = staged_read("New Circuit.c bus1=unused.1.2.3\nDisable Vsource.source");
    assert!(net.sources().is_empty());
    assert!(net.lines().is_empty());
    assert!(net.line_codes().is_empty());
    assert!(net.buses().is_empty());
    let retained = &net.untyped_objects()[0];
    assert_eq!(retained.class, "vsource");
    assert_eq!(
        retained.props.last(),
        Some(&(Some("enabled".into()), "no".into()))
    );
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

#[cfg(test)]
pub(super) fn write_staged_network(
    net: &MulticonductorNetwork,
    directory: &std::path::Path,
    stem: &str,
) {
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
    let reimported = staged_from_raw(&raw).unwrap();
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

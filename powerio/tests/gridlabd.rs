use std::collections::BTreeMap;

use powerio::{Destination, EmittedOutput, Fidelity, ParseOptions, PioValue, Source};

const HEAD: &str = "module powerflow { solver_method NR; }\nobject node { name source; phases AN; bustype SWING; nominal_voltage 240; }\nobject line_configuration { name cfg; z11 1+.5j; }\nobject overhead_line { name line; phases AN; from source; to sink; length 5280 ft; configuration cfg; }\n";
const LOAD: &str =
    "object load { name sink; phases AN; nominal_voltage 240; constant_impedance_A 20+5j; }\n";

fn memory(result: powerio::EmitResult) -> BTreeMap<String, Vec<u8>> {
    let EmittedOutput::Memory { artifacts } = result.into_output() else {
        panic!("expected memory output")
    };
    artifacts
        .into_iter()
        .map(|a| (a.name().as_str().to_owned(), a.into_bytes()))
        .collect()
}

fn network(module: &powerio::PioModule<PioValue>) -> &powerio::MulticonductorNetwork {
    let PioValue::MulticonductorNetwork(net) = module.value() else {
        panic!("wrong network family")
    };
    net
}

#[test]
fn extension_and_aliases_route_to_a_typed_distribution_network() {
    let text = format!("{HEAD}{LOAD}");
    for token in [
        None,
        Some("glm"),
        Some("gridlabd"),
        Some("gridlabd-pnnl"),
        Some("gridlabd-arras"),
    ] {
        let source = Source::from_memory(
            if token.is_some() {
                "unknown.bin"
            } else {
                "case.GLM"
            },
            text.as_bytes(),
        )
        .unwrap();
        let options = token.map_or_else(ParseOptions::default, |name| {
            ParseOptions::default().format(name).unwrap()
        });
        let module = powerio::parse_with_options(source, &options).unwrap();
        assert_eq!(network(&module).lines().len(), 1);
        assert_eq!(network(&module).loads().len(), 1);
        let expected = if token == Some("gridlabd-arras") {
            "gridlabd-arras"
        } else {
            "gridlabd"
        };
        assert!(network(&module).source_format().is_none());
        assert_eq!(
            module.extensions()["org.powerio.gridlabd"]["dialect"],
            expected
        );
        assert_eq!(
            module.source().unwrap().format().unwrap().as_str(),
            expected
        );
        let info = powerio::resolve_format(expected).unwrap();
        assert!(info.is_directory && info.can_emit);
        assert_eq!(info.extension, Some("glm"));
    }
}

#[test]
fn unchanged_project_replays_all_retained_bytes_without_reopening_files() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("parts")).unwrap();
    let main = format!("\u{feff}{HEAD}#include \"parts/load.glm\"\n");
    std::fs::write(temp.path().join("main.glm"), &main).unwrap();
    std::fs::write(temp.path().join("parts/load.glm"), LOAD).unwrap();
    let module = powerio::parse(temp.path().join("main.glm")).unwrap();
    drop(temp);
    let emitted = powerio::emit(&module, "glm", Destination::memory("replay").unwrap()).unwrap();
    assert_eq!(emitted.fidelity(), Fidelity::ExactSameFormat);
    let files = memory(emitted);
    assert_eq!(files.len(), 2);
    assert_eq!(files["replay/main.glm"], main.as_bytes());
    assert_eq!(files["replay/parts/load.glm"], LOAD.as_bytes());
}

#[test]
fn memory_projects_replay_only_referenced_files_and_keep_all_supplied_bytes() {
    let main = format!("{HEAD}#include \"parts/load.glm\"\n");
    let source = Source::from_memory("main.glm", main.as_bytes())
        .unwrap()
        .with_named_buffer("parts/load.glm", LOAD.as_bytes())
        .unwrap()
        .with_named_buffer("unused.txt", b"uninterpreted retained bytes\xff".as_slice())
        .unwrap();
    let module = powerio::parse(source).unwrap();
    let retained = module.source().unwrap().acquired_buffers();
    assert_eq!(retained.len(), 3);
    assert!(
        retained
            .iter()
            .any(|b| b.bytes() == b"uninterpreted retained bytes\xff")
    );
    let files =
        memory(powerio::emit(&module, "gridlabd", Destination::memory("replay").unwrap()).unwrap());
    assert_eq!(files.len(), 2);
    assert_eq!(files["replay/main.glm"], main.as_bytes());
    assert_eq!(files["replay/parts/load.glm"], LOAD.as_bytes());
}

#[test]
fn edited_and_ir_restored_modules_emit_current_typed_values() {
    let text = format!("{HEAD}{LOAD}");
    let mut module =
        powerio::parse(Source::from_memory("original.glm", text.as_bytes()).unwrap()).unwrap();
    let ir =
        memory(powerio::serialize(&module, Destination::memory("network.json").unwrap()).unwrap());
    let restored = powerio::deserialize(
        Source::from_memory("network.json", ir["network.json"].clone()).unwrap(),
    )
    .unwrap();
    let result = powerio::emit(
        &restored,
        "gridlabd",
        Destination::memory("restored").unwrap(),
    )
    .unwrap();
    assert_eq!(result.fidelity(), Fidelity::Canonical);
    let files = memory(result);
    assert_ne!(files["restored/case.glm"], text.as_bytes());
    let PioValue::MulticonductorNetwork(net) = module.value_mut() else {
        unreachable!()
    };
    net.loads_mut()[0].p_nom[0] *= 2.0;
    let wanted = net.loads()[0].p_nom[0];
    let result = powerio::emit(&module, "glm", Destination::memory("edited").unwrap()).unwrap();
    assert_eq!(result.fidelity(), Fidelity::Canonical);
    let files = memory(result);
    let reparsed =
        powerio::parse(Source::from_memory("case.glm", files["edited/case.glm"].clone()).unwrap())
            .unwrap();
    assert!((network(&reparsed).loads()[0].p_nom[0] - wanted).abs() < 1e-9);
}

#[test]
fn explicit_fork_profiles_preserve_native_conditional_meaning() {
    let text = format!(
        "{HEAD}#define SELECT=100\n#if SELECT<20\n{LOAD}#else\n{}#endif\n",
        LOAD.replace("20+5j", "40+5j")
    );
    let pnnl = powerio::parse(Source::from_memory("case.glm", text.as_bytes()).unwrap()).unwrap();
    let arras = powerio::parse_with_options(
        Source::from_memory("case.glm", text.as_bytes()).unwrap(),
        &ParseOptions::default().format("gridlabd-arras").unwrap(),
    )
    .unwrap();
    assert!(
        (network(&pnnl).loads()[0].p_nom[0] - network(&arras).loads()[0].p_nom[0]).abs() > 100.0
    );
    let result = powerio::emit(
        &arras,
        "gridlabd-arras",
        Destination::memory("same").unwrap(),
    )
    .unwrap();
    assert_eq!(result.fidelity(), Fidelity::ExactSameFormat);
    assert_eq!(memory(result)["same/case.glm"], text.as_bytes());
    let result = powerio::emit(&arras, "gridlabd", Destination::memory("other").unwrap()).unwrap();
    assert_eq!(result.fidelity(), Fidelity::Canonical);
    let output = memory(result);
    let reparsed =
        powerio::parse(Source::from_memory("case.glm", output["other/case.glm"].clone()).unwrap())
            .unwrap();
    assert!(
        (network(&reparsed).loads()[0].p_nom[0] - network(&arras).loads()[0].p_nom[0]).abs() < 1e-9
    );
}

#[test]
fn widened_root_replay_keeps_relative_include_paths_and_entrypoint_location() {
    let input = tempfile::tempdir().unwrap();
    std::fs::create_dir(input.path().join("case")).unwrap();
    let main = format!("{HEAD}#include \"../load.glm\"\n");
    std::fs::write(input.path().join("case/main.glm"), &main).unwrap();
    std::fs::write(input.path().join("load.glm"), LOAD).unwrap();
    let module = powerio::parse_with_options(
        input.path().join("case/main.glm"),
        &ParseOptions::default().acquisition_root(input.path()),
    )
    .unwrap();
    let output = tempfile::tempdir().unwrap();
    let root = output.path().join("replay");
    powerio::emit(&module, "glm", Destination::path(&root)).unwrap();
    assert_eq!(
        std::fs::read(root.join("case/main.glm")).unwrap(),
        main.as_bytes()
    );
    drop(input);
    let restored = powerio::parse_with_options(
        root.join("case/main.glm"),
        &ParseOptions::default().acquisition_root(&root),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(network(&module)).unwrap(),
        serde_json::to_value(network(&restored)).unwrap()
    );
}

#[test]
fn nonportable_absolute_include_fails_before_creating_output() {
    let input = tempfile::tempdir().unwrap();
    std::fs::write(input.path().join("load.glm"), LOAD).unwrap();
    let absolute = std::fs::canonicalize(input.path().join("load.glm")).unwrap();
    let main = format!("{HEAD}#include \"{}\"\n", absolute.display());
    std::fs::write(input.path().join("main.glm"), main).unwrap();
    let module = powerio::parse_with_options(
        input.path().join("main.glm"),
        &ParseOptions::default().acquisition_root(input.path()),
    )
    .unwrap();
    let output = input.path().join("replay");
    let failure = powerio::emit(&module, "glm", Destination::path(&output)).unwrap_err();
    assert_eq!(
        failure.diagnostics()[0].code(),
        "EMIT.GLM.PROJECT_UNREPRESENTABLE"
    );
    assert!(!output.exists());
}

#[test]
fn explicit_preprocessing_inputs_survive_ir_and_require_replay_context() {
    let text = format!("{HEAD}{}", LOAD.replace("20+5j", "${IMPEDANCE}"));
    let typed = powerio_dist::glm::parse(
        Source::from_memory("main.glm", text.as_bytes()).unwrap(),
        powerio_dist::glm::PreprocessOptions {
            defines: BTreeMap::from([("IMPEDANCE".into(), "20+5j".into())]),
            ..Default::default()
        },
    )
    .unwrap();
    let result = powerio_dist::emit(
        &typed,
        powerio_dist::DistTargetFormat::Gridlabd,
        Destination::memory("copy").unwrap(),
    )
    .unwrap();
    assert_eq!(result.fidelity(), Fidelity::ExactSameFormat);
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|d| d.code() == "EMIT.GLM.REPLAY_CONTEXT_REQUIRED")
    );
    assert_eq!(memory(result)["copy/main.glm"], text.as_bytes());
    let ir = memory(powerio::serialize(&typed, Destination::memory("case.json").unwrap()).unwrap());
    let restored =
        powerio::deserialize(Source::from_memory("case.json", ir["case.json"].clone()).unwrap())
            .unwrap();
    assert_eq!(
        restored.extensions()["org.powerio.gridlabd"]["preprocessing"]["defines"]["IMPEDANCE"],
        "20+5j"
    );
    let result = powerio::emit(&restored, "glm", Destination::memory("restored").unwrap()).unwrap();
    assert_eq!(result.fidelity(), Fidelity::Canonical);
    let files = memory(result);
    let reparsed = powerio::parse(
        Source::from_memory("case.glm", files["restored/case.glm"].clone()).unwrap(),
    )
    .unwrap();
    assert!(
        (network(&reparsed).loads()[0].p_nom[0] - typed.value().loads()[0].p_nom[0]).abs() < 1e-9
    );
}

#[test]
fn unrelated_memory_buffers_do_not_create_portable_replay_collisions() {
    let main = format!("{HEAD}#include \"parts/load.glm\"\n");
    let source = Source::from_memory("main.glm", main.as_bytes())
        .unwrap()
        .with_named_buffer("parts/load.glm", LOAD.as_bytes())
        .unwrap()
        .with_named_buffer("MAIN.GLM", b"unrelated deck".as_slice())
        .unwrap();
    let module = powerio::parse(source).unwrap();
    let files =
        memory(powerio::emit(&module, "gridlabd", Destination::memory("replay").unwrap()).unwrap());
    assert_eq!(files.len(), 2);
    assert_eq!(files["replay/main.glm"], main.as_bytes());
}

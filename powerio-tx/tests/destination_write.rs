//! The module write through a destination: single file echo, memory
//! artifacts, PyPSA folder inventories, and the no-replace collision refusal.

mod helpers;

use powerio_core::{Destination, WrittenOutput};
use powerio_tx::TargetFormat;

fn case9() -> powerio_core::PioModule<powerio_tx::BalancedNetwork> {
    helpers::parse_module(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m"),
        None,
    )
    .expect("case9 parses")
}

#[test]
fn a_path_write_echoes_the_source_into_the_named_file() {
    let module = case9();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("out.m");
    let result = powerio_tx::write(&module, TargetFormat::Matpower, Destination::path(&target))
        .expect("write commits");
    let WrittenOutput::Path { root, artifacts } = result.output() else {
        panic!("path destination returns path output");
    };
    assert_eq!(root, &target);
    assert_eq!(artifacts, &vec![target.clone()]);
    let written = std::fs::read(&target).unwrap();
    let original = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/case9.m"
    ))
    .unwrap();
    assert_eq!(written, original, "same format write echoes the source");
}

#[test]
fn an_existing_target_is_refused_and_kept() {
    let module = case9();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("out.m");
    std::fs::write(&target, b"keep me").unwrap();
    let error = powerio_tx::write(&module, TargetFormat::Matpower, Destination::path(&target))
        .expect_err("collision refuses");
    assert!(error.to_string().contains("exists"), "{error}");
    assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
}

#[test]
fn a_memory_write_returns_the_named_artifact() {
    let module = case9();
    let result = powerio_tx::write(
        &module,
        TargetFormat::Matpower,
        Destination::memory("case9.m").unwrap(),
    )
    .expect("memory write commits");
    let WrittenOutput::Memory { artifacts } = result.output() else {
        panic!("memory destination returns memory output");
    };
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].name().as_str(), "case9.m");
    assert!(!artifacts[0].bytes().is_empty());
}

#[test]
fn a_pypsa_folder_write_commits_the_whole_inventory() {
    let module = case9();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("case9-pypsa");
    let result = powerio_tx::write_pypsa_csv(&module, Destination::path(&root))
        .expect("folder write commits");
    let WrittenOutput::Path { artifacts, .. } = result.output() else {
        panic!("path destination returns path output");
    };
    assert!(artifacts.iter().any(|p| p.ends_with("network.csv")));
    assert!(artifacts.iter().any(|p| p.ends_with("buses.csv")));
    for artifact in artifacts {
        assert!(artifact.starts_with(&root));
        assert!(artifact.is_file());
    }
    // The folder inventory matches the streaming writer's, file for file.
    let stream_dir = dir.path().join("streamed");
    let streamed =
        powerio_tx::write_pypsa_csv_folder(module.value(), &stream_dir).expect("streaming write");
    let mut committed: Vec<_> = artifacts
        .iter()
        .map(|p| p.file_name().unwrap().to_owned())
        .collect();
    let mut streamed: Vec<_> = streamed
        .files
        .iter()
        .map(|p| p.file_name().unwrap().to_owned())
        .collect();
    committed.sort();
    streamed.sort();
    assert_eq!(committed, streamed);
}

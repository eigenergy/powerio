//! Distribution module emission through a destination: single file JSON,
//! the dss directory inventory with its Buscoords sidecar, and the memory
//! form.

mod helpers;

use powerio_core::{Destination, EmittedOutput};
use powerio_dist::DistTargetFormat;

const LOCATED: &str = "New Circuit.c basekv=12.47 pu=1 phases=3 bus1=a\n\
New Line.l1 bus1=a.1.2.3 bus2=b.1.2.3 phases=3 r1=0.1 x1=0.2 length=1 units=km\n\
SetBusXY bus=a x=-80 y=35\n\
SetBusXY b -80.5 35.25\n";

fn module(text: &str) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    let source = powerio_core::Source::from_memory("<memory>", text.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("dss").unwrap());
    powerio_dist::parse(source).expect("dss parses")
}

/// A located module whose dss emission is canonical (no same format echo), so
/// the coordinates leave through the Buscoords sidecar.
fn located_canonical() -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    module(LOCATED).sever_source()
}

#[test]
fn a_json_emission_commits_the_named_file() {
    let module = module(LOCATED);
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("case.json");
    let result = powerio_dist::emit(
        &module,
        DistTargetFormat::BmopfJson,
        Destination::path(&target),
    )
    .expect("emission commits");
    let EmittedOutput::Path { artifacts, .. } = result.output() else {
        panic!("path destination returns path output");
    };
    assert_eq!(artifacts, &vec![target.clone()]);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert!(doc.get("bus").is_some());
}

#[test]
fn a_dss_emission_commits_the_case_and_its_sidecar_under_the_root() {
    let module = located_canonical();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("out");
    let result = powerio_dist::emit(&module, DistTargetFormat::Dss, Destination::path(&root))
        .expect("emission commits");
    let EmittedOutput::Path { artifacts, .. } = result.output() else {
        panic!("path destination returns path output");
    };
    assert!(artifacts.iter().any(|p| p.ends_with("case.dss")));
    // The coordinates ride the Buscoords sidecar the case text refers to.
    assert!(
        artifacts.iter().any(|p| p
            .file_name()
            .is_some_and(|n| n.to_string_lossy().contains("buscoords"))),
        "{artifacts:?}"
    );
    for artifact in artifacts {
        assert!(artifact.starts_with(&root));
        assert!(artifact.is_file());
    }
}

#[test]
fn a_memory_dss_emission_returns_the_inventory() {
    let module = located_canonical();
    let result = powerio_dist::emit(
        &module,
        DistTargetFormat::Dss,
        Destination::memory("out").unwrap(),
    )
    .expect("memory emission commits");
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("memory destination returns memory output");
    };
    assert!(
        artifacts
            .iter()
            .any(|a| a.name().as_str() == "out/case.dss")
    );
    assert!(artifacts.len() >= 2, "sidecar rides the inventory");
}

#[test]
fn a_retained_dss_project_replays_dependencies_and_companions() {
    use std::sync::Arc;
    let files: [(&str, &[u8]); 3] = [
        (
            "one/Master.dss",
            b"Clear\nNew Circuit.c basekv=12.47 bus1=source\nRedirect loads.inc\n",
        ),
        (
            "one/loads.inc",
            b"New Load.ld bus1=source phases=3 kv=12.47 kw=10 kvar=2\n",
        ),
        ("one/notes.asset", b"opaque companion bytes\x00\xff"),
    ];
    let source = powerio_core::Source::from_memory_tree(
        "project",
        files.iter().map(|(name, bytes)| {
            (
                powerio_core::ArtifactPath::new(*name).unwrap(),
                Arc::<[u8]>::from(*bytes),
            )
        }),
        Some(powerio_core::ArtifactPath::new("one/Master.dss").unwrap()),
    )
    .unwrap()
    .with_format(powerio_core::FormatId::new("dss").unwrap());
    let module = powerio_dist::parse(source).expect("project parses");
    let result = powerio_dist::emit(
        &module,
        DistTargetFormat::Dss,
        Destination::memory("out").unwrap(),
    )
    .expect("retained project emits");
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("memory destination returns memory output");
    };
    for (name, bytes) in files {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.name().as_str() == format!("out/source/{name}"))
            .unwrap_or_else(|| panic!("{name} is missing from the replay"));
        assert_eq!(artifact.bytes(), bytes, "{name}");
    }
    assert!(
        artifacts
            .iter()
            .any(|artifact| artifact.name().as_str() == "out/case.dss")
    );
    // #685: the companion is kept on purpose, and one remark names it (and
    // only it), so the copy is never silent.
    let remarks: Vec<_> = result
        .diagnostics()
        .iter()
        .filter(|d| d.code() == "EMIT.DSS.PROJECT_COMPANION_KEPT")
        .map(|d| (d.severity(), d.message().to_owned()))
        .collect();
    assert_eq!(
        remarks,
        [(
            powerio_core::DiagnosticSeverity::Remark,
            "copied 1 supplied file the deck never read under source/: one/notes.asset".to_owned()
        )]
    );
}

#[test]
fn a_file_project_that_reads_every_acquired_file_reports_no_companion() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist/opendss/ieee123/IEEE123Master.dss");
    let source = powerio_core::Source::open(path)
        .unwrap()
        .with_format(powerio_core::FormatId::new("dss").unwrap());
    let module = powerio_dist::parse(source).expect("IEEE 123 parses");
    let result = powerio_dist::emit(
        &module,
        DistTargetFormat::Dss,
        Destination::memory("out").unwrap(),
    )
    .expect("retained project emits");
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("memory destination returns memory output");
    };
    assert!(artifacts.len() > 2, "a multi-file project replays");
    let companions: Vec<_> = result
        .diagnostics()
        .iter()
        .filter(|d| d.code() == "EMIT.DSS.PROJECT_COMPANION_KEPT")
        .map(powerio_core::Diagnostic::message)
        .collect();
    assert_eq!(companions, [] as [&str; 0]);
}

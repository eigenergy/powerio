//! The distribution module write through a destination: single file JSON,
//! the dss directory inventory with its Buscoords sidecar, and the memory
//! form.

mod helpers;

use powerio_core::{Destination, WrittenOutput};
use powerio_dist::DistTargetFormat;

const LOCATED: &str = "New Circuit.c basekv=12.47 pu=1 phases=3 bus1=a\n\
New Line.l1 bus1=a.1.2.3 bus2=b.1.2.3 phases=3 r1=0.1 x1=0.2 length=1 units=km\n\
SetBusXY bus=a x=-80 y=35\n\
SetBusXY b -80.5 35.25\n";

fn module(text: &str) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    let source = powerio_core::Source::from_bytes("<memory>", text.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("dss").unwrap());
    powerio_dist::parse(source).expect("dss parses")
}

/// A located module whose dss write is canonical (no same format echo), so
/// the coordinates leave through the Buscoords sidecar.
fn located_canonical() -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    module(LOCATED).sever_source()
}

#[test]
fn a_json_write_commits_the_named_file() {
    let module = module(LOCATED);
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("case.json");
    let result = powerio_dist::write(
        &module,
        DistTargetFormat::BmopfJson,
        Destination::path(&target),
    )
    .expect("write commits");
    let WrittenOutput::Path { artifacts, .. } = result.output() else {
        panic!("path destination returns path output");
    };
    assert_eq!(artifacts, &vec![target.clone()]);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert!(doc.get("bus").is_some());
}

#[test]
fn a_dss_write_commits_the_case_and_its_sidecar_under_the_root() {
    let module = located_canonical();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("out");
    let result = powerio_dist::write(&module, DistTargetFormat::Dss, Destination::path(&root))
        .expect("write commits");
    let WrittenOutput::Path { artifacts, .. } = result.output() else {
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
fn a_memory_dss_write_returns_the_inventory() {
    let module = located_canonical();
    let result = powerio_dist::write(
        &module,
        DistTargetFormat::Dss,
        Destination::memory("out").unwrap(),
    )
    .expect("memory write commits");
    let WrittenOutput::Memory { artifacts } = result.output() else {
        panic!("memory destination returns memory output");
    };
    assert!(
        artifacts
            .iter()
            .any(|a| a.name().as_str() == "out/case.dss")
    );
    assert!(artifacts.len() >= 2, "sidecar rides the inventory");
}

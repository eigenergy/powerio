use powerio::collection::{CandidateStatus, CollectionOperation};
use powerio::{
    ArtifactPath, Collection, CollectionLimits, CollectionSelection, ParseOptions, PioValue,
};
use std::io::{Cursor, Write};
use std::sync::Arc;

fn files() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("masters/day.dss", b"New Circuit.feeder basekv=12.47 bus1=source\nRedirect ../shared/topology.inc\nNew Load.day bus1=load phases=3 conn=wye kv=12.47 kw=30 kvar=6\n"),
        ("masters/night.dss", b"New Circuit.feeder basekv=12.47 bus1=source\nRedirect ../shared/topology.inc\nNew Load.night bus1=load phases=3 conn=wye kv=12.47 kw=15 kvar=3\n"),
        ("shared/topology.inc", b"New Line.l bus1=source bus2=load phases=3 r1=0.1 x1=0.2 r0=0.3 x0=0.4 length=1\n"),
        ("comparison/case.m", include_bytes!("../../tests/data/case9.m")),
        ("missing.dss", b"New Circuit.broken basekv=12.47\nRedirect absent.inc\n"),
        ("opaque.bin", &[0, 255, 13, 10]),
    ]
}
fn memory() -> Collection {
    Collection::from_files(
        "study",
        files()
            .iter()
            .map(|(p, b)| (ArtifactPath::new(*p).unwrap(), Arc::<[u8]>::from(*b))),
        CollectionLimits::default(),
    )
    .unwrap()
}
fn zip() -> Collection {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in files() {
        zip.start_file(path, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    Collection::from_zip(
        "study.zip",
        zip.finish().unwrap().into_inner().into(),
        CollectionLimits::default(),
    )
    .unwrap()
}
#[test]
fn overlapping_masters_and_mixed_cases_match_uncompressed_results() {
    let tree = memory();
    let zip = zip();
    let candidates = zip.inspect().unwrap();
    assert_eq!(candidates.len(), 4);
    assert!(
        candidates
            .iter()
            .any(|c| c.selection.primary.as_deref() == Some("comparison/case.m"))
    );
    for order in [["day", "night"], ["night", "day"]] {
        for master in order {
            let selection =
                CollectionSelection::file(format!("masters/{master}.dss")).format("dss");
            let from_zip = zip.parse(&selection, &ParseOptions::default()).unwrap();
            let from_tree = tree.parse(&selection, &ParseOptions::default()).unwrap();
            assert_eq!(from_zip.dependencies, ["shared/topology.inc"]);
            assert!(!from_zip.incomplete);
            assert_eq!(
                serde_json::to_value(from_zip.module.diagnostics()).unwrap(),
                serde_json::to_value(from_tree.module.diagnostics()).unwrap()
            );
            let PioValue::MulticonductorNetwork(a) = from_zip.module.value() else {
                panic!()
            };
            let PioValue::MulticonductorNetwork(b) = from_tree.module.value() else {
                panic!()
            };
            assert_eq!(
                serde_json::to_value(a).unwrap(),
                serde_json::to_value(b).unwrap()
            );
            assert_eq!(a.loads().len(), 1);
            assert_eq!(a.loads()[0].name, master);
        }
    }
    let selection = CollectionSelection::file("comparison/case.m");
    let source = zip.source(&selection).unwrap();
    assert_eq!(source.acquired_buffers().len(), 1);
    assert_eq!(
        source.primary_buffer().unwrap().bytes(),
        include_bytes!("../../tests/data/case9.m")
    );
    assert!(zip.parse(&selection, &ParseOptions::default()).is_ok());
}
#[test]
fn incomplete_or_invalid_selections_do_not_poison_neighbors() {
    let collection = memory();
    let partial = collection
        .parse(
            &CollectionSelection::file("missing.dss"),
            &ParseOptions::default(),
        )
        .unwrap();
    assert!(partial.incomplete);
    assert!(
        collection
            .source(&CollectionSelection::file("../escape.dss"))
            .is_err()
    );
    assert!(collection.source(&CollectionSelection::default()).is_err());
    let mut confined = CollectionSelection::file("day.dss").format("dss");
    confined.root = Some("masters".into());
    assert!(
        collection
            .parse(&confined, &ParseOptions::default())
            .is_err()
    );
    assert!(
        collection
            .parse(
                &CollectionSelection::file("comparison/case.m"),
                &ParseOptions::default()
            )
            .is_ok()
    );
}
#[test]
fn inspection_budget_and_ir_dispatch_are_explicit() {
    let module = memory()
        .parse(
            &CollectionSelection::file("comparison/case.m"),
            &ParseOptions::default(),
        )
        .unwrap()
        .module;
    let emitted = powerio::serialize(&module, powerio::Destination::memory("ir").unwrap()).unwrap();
    let powerio::EmittedOutput::Memory { artifacts } = emitted.output() else {
        panic!()
    };
    let ir = artifacts[0].bytes();
    let collection = Collection::from_files(
        "ir",
        [(
            ArtifactPath::new("case.json").unwrap(),
            Arc::<[u8]>::from(ir),
        )],
        CollectionLimits::default(),
    )
    .unwrap();
    let candidate = collection.inspect().unwrap().remove(0);
    assert_eq!(
        candidate.selection.operation,
        CollectionOperation::Deserialize
    );
    assert!(
        collection
            .parse(&candidate.selection, &ParseOptions::default())
            .is_ok()
    );
    let mut limits = CollectionLimits::default();
    limits.probe_bytes = 1;
    let limited = Collection::from_files(
        "ir",
        [(
            ArtifactPath::new("case.json").unwrap(),
            Arc::<[u8]>::from(ir),
        )],
        limits,
    )
    .unwrap();
    assert_eq!(
        limited.inspect().unwrap()[0].status,
        CandidateStatus::ProbeLimit
    );
}
#[test]
fn folder_reads_are_bounded_and_selections_share_owned_bytes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("case.m"),
        include_bytes!("../../tests/data/case9.m"),
    )
    .unwrap();
    let collection = Collection::from_directory(dir.path(), CollectionLimits::default()).unwrap();
    let selection = CollectionSelection::file("case.m");
    let a = collection
        .source(&selection)
        .unwrap()
        .primary_buffer()
        .unwrap();
    let b = collection
        .source(&selection)
        .unwrap()
        .primary_buffer()
        .unwrap();
    assert!(Arc::ptr_eq(&a.shared_bytes(), &b.shared_bytes()));
    let mut limits = CollectionLimits::default();
    limits.tree = powerio::SourceTreeLimits::new(1, 1, 1).unwrap();
    assert!(Collection::from_directory(dir.path(), limits).is_err());
}

#[test]
fn aliases_and_uppercase_extensions_preserve_companions() {
    let collection = Collection::from_files(
        "case",
        [
            (
                ArtifactPath::new("Master.DSS").unwrap(),
                Arc::<[u8]>::from(&b"New Circuit.case basekv=12.47\nRedirect line.inc\n"[..]),
            ),
            (
                ArtifactPath::new("line.inc").unwrap(),
                Arc::<[u8]>::from(files()[2].1),
            ),
        ],
        CollectionLimits::default(),
    )
    .unwrap();
    for format in [None, Some("dss"), Some("opendss"), Some("OpenDSS")] {
        let mut selection = CollectionSelection::file("Master.DSS");
        selection.format = format.map(str::to_owned);
        let result = collection
            .parse(&selection, &ParseOptions::default())
            .unwrap();
        assert!(!result.incomplete);
        assert_eq!(result.dependencies, ["line.inc"]);
        let PioValue::MulticonductorNetwork(net) = result.module.value() else {
            panic!()
        };
        assert!(net.lines().iter().any(|line| line.name == "l"));
    }
}

#[test]
fn corrupt_empty_directory_crc_is_not_certified_as_verified() {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_directory("empty/", zip::write::SimpleFileOptions::default())
        .unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    let central = bytes.windows(4).position(|s| s == b"PK\x01\x02").unwrap();
    bytes[14..18].copy_from_slice(&1u32.to_le_bytes());
    bytes[central + 16..central + 20].copy_from_slice(&1u32.to_le_bytes());
    assert!(Collection::from_zip("bad", bytes.into(), CollectionLimits::default()).is_err());
}

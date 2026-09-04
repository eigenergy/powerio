use powerio::{
    Destination, EmittedOutput, Fidelity, OutputLayout, PioValue, Source, deserialize, emit, parse,
    serialize,
};
use std::io::{Cursor, Write};

#[test]
fn facade_emits_and_parses_a_cgmes_profile_directory() {
    let matpower = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = parse(Source::open(matpower).unwrap()).expect("case9 parses");
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("case9-cgmes");
    let result = emit(&module, "cgmes", Destination::path(&directory))
        .expect("a balanced network emits CGMES");
    assert_eq!(result.layout(), OutputLayout::Directory);
    assert_eq!(result.fidelity(), Fidelity::Canonical);
    let EmittedOutput::Path { artifacts, .. } = result.output() else {
        panic!("a path destination returned memory artifacts");
    };
    assert_eq!(artifacts.len(), 4);

    let parsed = parse(Source::open(&directory).unwrap())
        .expect("the emitted CGMES profile directory parses");
    let PioValue::BalancedNetwork(network) = &parsed.value() else {
        panic!("CGMES must produce a balanced network");
    };
    assert_eq!(network.buses().len(), 9);
    assert_eq!(network.branches().len(), 9);
    assert_eq!(network.source_format().name(), "cgmes");

    let exact = emit(&parsed, "cgmes", Destination::memory("copy").unwrap())
        .expect("an unchanged CGMES module emits its retained directory");
    assert_eq!(exact.layout(), OutputLayout::Directory);
    assert_eq!(exact.fidelity(), Fidelity::ExactSameFormat);
    let EmittedOutput::Memory { artifacts } = exact.output() else {
        panic!("a memory destination returned paths");
    };
    assert_eq!(artifacts.len(), 4);
}

#[test]
fn facade_calculates_node_breaker_buses_without_a_tp_profile() {
    let directory = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/cgmes/node-breaker"
    );
    let module = parse(Source::open(directory).unwrap())
        .expect("a node breaker EQ and SSH set parses without TP");
    let PioValue::BalancedNetwork(network) = &module.value() else {
        panic!("CGMES must produce a balanced network");
    };
    assert_eq!(network.buses().len(), 3);
    let remark = module
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code() == "READ.CGMES.TOPOLOGY_CALCULATED")
        .expect("the calculated topology is reported");
    assert!(remark.message().contains("3 calculated bus(es)"));
    let uids: Vec<Option<String>> = network.buses().iter().map(|bus| bus.uid.clone()).collect();
    assert!(uids.iter().all(Option::is_some));

    // PowerIO IR carries no retained source, so emission from it is fresh.
    let stored = serialize(&module, Destination::memory("module").unwrap())
        .expect("the calculated topology serializes");
    let EmittedOutput::Memory { artifacts: stored } = stored.into_output() else {
        panic!("a memory destination returned paths")
    };
    let restored =
        deserialize(Source::from_memory("module.pio.json", stored[0].bytes().to_vec()).unwrap())
            .expect("the stored module deserializes");
    let fresh = emit(&restored, "cgmes", Destination::memory("fresh").unwrap())
        .expect("a calculated topology emits fresh CGMES");
    assert_eq!(fresh.fidelity(), Fidelity::Canonical);
    let EmittedOutput::Memory { artifacts } = fresh.into_output() else {
        panic!("a memory destination returned paths")
    };
    assert_eq!(artifacts.len(), 4);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for artifact in artifacts {
        writer
            .start_file(
                artifact.name().as_str(),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(artifact.bytes()).unwrap();
    }
    let zip_bytes = writer.finish().unwrap().into_inner();
    let reparsed = parse(Source::from_memory("fresh.zip", zip_bytes).unwrap())
        .expect("the fresh profile set, now with TP, parses");
    let PioValue::BalancedNetwork(reparsed_network) = &reparsed.value() else {
        panic!("CGMES must produce a balanced network");
    };
    assert_eq!(reparsed_network.buses().len(), 3);
    let reparsed_uids: Vec<Option<String>> = reparsed_network
        .buses()
        .iter()
        .map(|bus| bus.uid.clone())
        .collect();
    assert_eq!(reparsed_uids, uids);
    assert!(
        reparsed
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.code() != "READ.CGMES.TOPOLOGY_CALCULATED")
    );
}

#[test]
fn facade_echoes_one_cgmes_zip_without_repacking_it() {
    let matpower = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = parse(Source::open(matpower).unwrap()).expect("case9 parses");
    let fresh = emit(&module, "cgmes", Destination::memory("profiles").unwrap())
        .expect("case9 emits CGMES profiles");
    let EmittedOutput::Memory { artifacts } = fresh.into_output() else {
        panic!("a memory destination returned paths")
    };

    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for artifact in artifacts {
        writer
            .start_file(
                artifact.name().as_str(),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(artifact.bytes()).unwrap();
    }
    let zip_bytes = writer.finish().unwrap().into_inner();
    let parsed = parse(Source::from_memory("profiles.zip", zip_bytes.clone()).unwrap())
        .expect("the CGMES ZIP parses");
    let exact = emit(&parsed, "cgmes", Destination::memory("copy.zip").unwrap())
        .expect("the unchanged CGMES ZIP emits");
    assert_eq!(exact.layout(), OutputLayout::File);
    assert_eq!(exact.fidelity(), Fidelity::ExactSameFormat);
    let EmittedOutput::Memory { artifacts } = exact.output() else {
        panic!("a memory destination returned paths")
    };
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].bytes(), zip_bytes);
}

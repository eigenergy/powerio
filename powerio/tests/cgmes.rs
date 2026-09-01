use powerio::{Destination, EmittedOutput, Fidelity, OutputLayout, PioValue, Source, emit, parse};
use std::io::{Cursor, Write};

#[test]
fn facade_emits_and_parses_a_cgmes_profile_directory() {
    let matpower = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = parse(Source::open(matpower).unwrap(), None).expect("case9 parses");
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

    let parsed = parse(Source::open(&directory).unwrap(), None)
        .expect("the emitted CGMES profile directory parses");
    let PioValue::BalancedNetwork(network) = &parsed.value else {
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
fn facade_echoes_one_cgmes_zip_without_repacking_it() {
    let matpower = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = parse(Source::open(matpower).unwrap(), None).expect("case9 parses");
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
    let parsed = parse(
        Source::from_memory("profiles.zip", zip_bytes.clone()).unwrap(),
        None,
    )
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

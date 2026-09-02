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

fn two_authority_set() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/cgmes/two-authority")
}

#[test]
fn facade_assembles_two_authorities_and_keeps_their_document_records() {
    let module = parse(Source::open(two_authority_set()).unwrap(), None)
        .expect("the two authority set assembles");
    let PioValue::BalancedNetwork(network) = &module.value else {
        panic!("CGMES must produce a balanced network");
    };
    assert_eq!(network.buses().len(), 4);
    assert_eq!(network.branches().len(), 3);
    let detailed = network.detailed_connectivity().as_deref().unwrap();
    assert_eq!(detailed.tie_lines.len(), 1);
    assert_eq!(detailed.boundary_lines.len(), 2);

    let records = module
        .extensions()
        .get(powerio::CGMES_DOCUMENT_EXTENSION)
        .expect("the module keeps the profile document records");
    let documents = records["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 8);
    let topology_b = documents
        .iter()
        .find(|document| document["source"] == "authority-b_TP.xml")
        .unwrap();
    assert_eq!(
        topology_b["modeling_authority_set"],
        "http://example.org/cgmes/authority-b"
    );
    assert_eq!(
        topology_b["depends_on"],
        serde_json::json!([
            "urn:uuid:bbbbbbbb-1111-4000-8000-000000000001",
            "urn:uuid:00000000-1111-4000-8000-000000000002"
        ])
    );
    assert_eq!(
        topology_b["model"],
        "urn:uuid:bbbbbbbb-1111-4000-8000-000000000002"
    );

    let stored = powerio::serialize(&module, Destination::memory("case.pio.json").unwrap())
        .expect("the module serializes");
    let EmittedOutput::Memory { artifacts } = stored.into_output() else {
        panic!("a memory destination returned paths")
    };
    let decoded = powerio::deserialize(
        Source::from_memory("case.pio.json", artifacts[0].bytes().to_vec()).unwrap(),
    )
    .expect("PowerIO IR keeps the document records");
    assert_eq!(
        decoded.extensions().get(powerio::CGMES_DOCUMENT_EXTENSION),
        Some(records)
    );
    assert!(module.diagnostics.iter().any(|diagnostic| {
        diagnostic.code() == "READ.CGMES.VALUE_APPROXIMATED"
            && diagnostic.message().contains("tie line branch")
    }));
}

#[test]
fn facade_emit_takes_cgmes_options_as_a_format_suffix() {
    let matpower = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = parse(Source::open(matpower).unwrap(), None).expect("case9 parses");
    let result = emit(
        &module,
        "cgmes?cgmes_version=2.4.15&cgmes_profiles=EQ,TP",
        Destination::memory("profiles").unwrap(),
    )
    .expect("the option suffix selects CIM16 and two profiles");
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("a memory destination returned paths")
    };
    assert_eq!(artifacts.len(), 2);
    assert!(artifacts[0].name().as_str().ends_with("_EQ.xml"));
    assert!(artifacts[1].name().as_str().ends_with("_TP.xml"));
    assert!(
        std::str::from_utf8(artifacts[0].bytes())
            .unwrap()
            .contains("http://iec.ch/TC57/2013/CIM-schema-cim16#")
    );

    let assembled = parse(Source::open(two_authority_set()).unwrap(), None).unwrap();
    let fresh = emit(
        &assembled,
        "cgmes?cgmes_version=2.4.15",
        Destination::memory("profiles").unwrap(),
    )
    .expect("an assembled set writes fresh CIM16 instead of its retained files");
    assert_eq!(fresh.fidelity(), Fidelity::Canonical);
    let EmittedOutput::Memory { artifacts } = fresh.output() else {
        panic!("a memory destination returned paths")
    };
    assert_eq!(artifacts.len(), 4);

    for format in [
        "matpower?cgmes_version=2.4.15",
        "cgmes?cgmes_version=16",
        "cgmes?naming_strategy=identity",
        "cgmes?cgmes_profiles",
    ] {
        let error = emit(&module, format, Destination::memory("out").unwrap()).unwrap_err();
        assert_eq!(
            error.info().map(|info| info.code),
            Some("REQUEST.EMIT.OPTION_INVALID"),
            "{format}"
        );
    }
}

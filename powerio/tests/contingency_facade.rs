//! The three PSS/E contingency analysis files are values at the facade:
//! `parse` routes each one, `emit` writes it back, PowerIO IR carries it, and
//! `resolve_format` states its artifact shape.

use powerio::{Destination, PioValue, Source};

fn data(name: &str) -> std::path::PathBuf {
    std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/psse/contingency"
    ))
    .join(name)
}

fn emitted_bytes(module: &powerio::PioModule<PioValue>, format: &str) -> Vec<u8> {
    let destination = Destination::memory("out").expect("memory destination");
    let result = powerio::emit(module, format, destination).expect("emission");
    let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("a memory destination returns memory artifacts");
    };
    let artifact = artifacts.pop().expect("one artifact");
    assert!(artifacts.is_empty(), "one artifact");
    artifact.into_bytes()
}

#[test]
fn each_file_parses_to_its_own_value_with_the_reader_notes() {
    let con = powerio::parse(data("tara_extensions.con")).expect("read the .con");
    let PioValue::ContingencySet(set) = con.value() else {
        panic!(
            "a .con parses to powerio.ContingencySet, found {}",
            con.value().type_name()
        );
    };
    assert!(!set.cases.is_empty());
    assert_eq!(
        con.source()
            .and_then(|s| s.format())
            .map(|f| f.as_str().to_owned()),
        Some("psse-con".to_owned())
    );
    assert!(
        con.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code().starts_with("READ.CON.")),
        "the reader's notes reach the module: {:?}",
        con.diagnostics()
    );

    let sub = powerio::parse(data("selectors.sub")).expect("read the .sub");
    let PioValue::SubsystemSet(set) = sub.value() else {
        panic!(
            "a .sub parses to powerio.SubsystemSet, found {}",
            sub.value().type_name()
        );
    };
    assert!(set.get("A1").is_some());

    let mon = powerio::parse(data("blocks.mon")).expect("read the .mon");
    let PioValue::MonitoredSet(set) = mon.value() else {
        panic!(
            "a .mon parses to powerio.MonitoredSet, found {}",
            mon.value().type_name()
        );
    };
    assert!(!set.statements.is_empty());
    assert!(
        mon.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code().starts_with("READ.MON.")),
        "the reader's notes reach the module: {:?}",
        mon.diagnostics()
    );
}

#[test]
fn a_declared_token_parses_content_that_carries_no_extension() {
    for (name, token, type_name) in [
        ("tara_extensions.con", "psse-con", "powerio.ContingencySet"),
        ("selectors.sub", "psse-sub", "powerio.SubsystemSet"),
        ("blocks.mon", "psse-mon", "powerio.MonitoredSet"),
    ] {
        let bytes = std::fs::read(data(name)).expect("fixture");
        let options = powerio::ParseOptions::default()
            .format(token)
            .expect("token");
        let module = powerio::parse_with_options(bytes, &options).expect("declared parse");
        assert_eq!(module.value().type_name(), type_name);
    }
}

#[test]
fn input_that_is_not_text_is_refused_by_file() {
    for (name, token, code) in [
        ("case.con", "psse-con", "READ.CON.NOT_TEXT"),
        ("case.sub", "psse-sub", "READ.SUB.NOT_TEXT"),
        ("case.mon", "psse-mon", "READ.MON.NOT_TEXT"),
    ] {
        let source = Source::from_memory(name, vec![0xff, 0xfe, 0x00]).expect("memory source");
        let options = powerio::ParseOptions::default()
            .format(token)
            .expect("token");
        let error = powerio::parse_with_options(source, &options).unwrap_err();
        assert_eq!(error.info().map(|info| info.code), Some(code), "{name}");
    }
}

#[test]
fn same_format_emission_returns_the_parsed_file() {
    for (name, token) in [
        ("tara_extensions.con", "psse-con"),
        ("selectors.sub", "psse-sub"),
        ("blocks.mon", "psse-mon"),
    ] {
        let path = data(name);
        let module = powerio::parse(&path).expect("parse");
        let source_bytes = std::fs::read(&path).expect("fixture");
        assert_eq!(emitted_bytes(&module, token), source_bytes, "{name}");
    }
}

#[test]
fn powerio_ir_carries_each_file_and_emission_writes_canonical_text() {
    for (name, token) in [
        ("tara_extensions.con", "psse-con"),
        ("selectors.sub", "psse-sub"),
        ("blocks.mon", "psse-mon"),
    ] {
        let module = powerio::parse(data(name)).expect("parse");
        let stored = powerio::serialize(&module, Destination::memory("module.pio.json").unwrap())
            .expect("serialize");
        let powerio_core::EmittedOutput::Memory { mut artifacts } = stored.into_output() else {
            panic!("a memory destination returns memory artifacts");
        };
        let document = artifacts.pop().expect("one document").into_bytes();
        let back = powerio::deserialize(Source::from_memory("module.pio.json", document).unwrap())
            .expect("deserialize");
        assert_eq!(
            back.value().type_name(),
            module.value().type_name(),
            "{name}"
        );
        assert_values_equal(module.value(), back.value(), name);

        // A deserialized module retains no source, so emission writes the
        // canonical text. Reading that text back gives a set that writes the
        // same text: a statement kept from the middle of the source is
        // written after the cases and so reads back from a later line.
        let canonical = emitted_bytes(&back, token);
        let options = powerio::ParseOptions::default()
            .format(token)
            .expect("token");
        let again = powerio::parse_with_options(
            Source::from_memory(name, canonical.clone()).unwrap(),
            &options,
        )
        .expect("the canonical text parses");
        assert_eq!(
            canonical_text(again.value()),
            String::from_utf8(canonical).expect("canonical text is UTF-8"),
            "{name}"
        );
    }
}

fn canonical_text(value: &PioValue) -> String {
    match value {
        PioValue::ContingencySet(set) => set.to_con(),
        PioValue::SubsystemSet(set) => set.to_sub(),
        PioValue::MonitoredSet(set) => set.to_mon(),
        other => panic!("{} is not a contingency analysis file", other.type_name()),
    }
}

fn assert_values_equal(left: &PioValue, right: &PioValue, label: &str) {
    match (left, right) {
        (PioValue::ContingencySet(a), PioValue::ContingencySet(b)) => assert_eq!(a, b, "{label}"),
        (PioValue::SubsystemSet(a), PioValue::SubsystemSet(b)) => assert_eq!(a, b, "{label}"),
        (PioValue::MonitoredSet(a), PioValue::MonitoredSet(b)) => assert_eq!(a, b, "{label}"),
        _ => panic!("{label}: the two values are not the same contingency analysis file"),
    }
}

#[test]
fn a_grid_case_target_and_the_other_two_files_are_refused() {
    let module = powerio::parse(data("tara_extensions.con")).expect("parse");
    for (format, code) in [
        ("psse", "REQUEST.EMIT.UNSUPPORTED_VALUE_TYPE"),
        ("matpower", "REQUEST.EMIT.UNSUPPORTED_VALUE_TYPE"),
        ("psse-sub", "REQUEST.EMIT.UNSUPPORTED_VALUE_TYPE"),
        ("psse-mon", "REQUEST.EMIT.UNSUPPORTED_VALUE_TYPE"),
        ("not-a-format", "REQUEST.EMIT.UNKNOWN_FORMAT"),
    ] {
        let destination = Destination::memory("out").expect("memory destination");
        let error = powerio::emit(&module, format, destination).unwrap_err();
        assert_eq!(error.info().map(|info| info.code), Some(code), "{format}");
    }
}

#[test]
fn geo_application_refuses_a_contingency_analysis_file() {
    let layer = powerio::GeoLayer {
        space: powerio::CoordinateSpace::Unknown,
        kind: None,
        features: Vec::new(),
    };
    for name in ["tara_extensions.con", "selectors.sub", "blocks.mon"] {
        let module = powerio::parse(data(name)).expect("parse");
        let error = powerio::apply_geo_layer(&module, &layer).unwrap_err();
        assert_eq!(
            error.info().map(|info| info.code),
            Some(powerio::codes::REQUEST_MODULE_WRONG_MODEL_KIND.code),
            "{name}"
        );
    }
}

#[test]
fn each_token_reports_a_single_text_file_that_emits() {
    for (token, extension) in [
        ("psse-con", "con"),
        ("psse-sub", "sub"),
        ("psse-mon", "mon"),
    ] {
        let info = powerio::resolve_format(token).unwrap();
        assert_eq!(info.token, token);
        assert_eq!(info.extension, Some(extension));
        assert!(!info.is_directory, "{token}");
        assert!(info.can_emit, "{token}");
    }
}

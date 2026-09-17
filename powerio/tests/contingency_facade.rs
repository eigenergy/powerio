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

/// The value with every kept statement's line number cleared. The writer
/// states a kept line where the block it belongs to states it, which is a
/// different line of the file when the writer's order differs from the
/// source's; the text and its place must survive unchanged.
fn without_kept_lines(value: &PioValue) -> PioValue {
    let clear = |statements: &mut Vec<powerio::RetainedStatement>| {
        for statement in statements {
            statement.line = 0;
        }
    };
    match value.clone() {
        PioValue::ContingencySet(mut set) => {
            clear(&mut set.retained);
            PioValue::ContingencySet(set)
        }
        PioValue::SubsystemSet(mut set) => {
            clear(&mut set.retained);
            for subsystem in &mut set.subsystems {
                clear(&mut subsystem.retained);
                for group in &mut subsystem.groups {
                    clear(&mut group.retained);
                }
            }
            PioValue::SubsystemSet(set)
        }
        PioValue::MonitoredSet(mut set) => {
            clear(&mut set.retained);
            for statement in &mut set.statements {
                if let powerio::MonitorStatement::Branches { retained, .. }
                | powerio::MonitorStatement::Interface { retained, .. } = statement
                {
                    clear(retained);
                }
            }
            PioValue::MonitoredSet(set)
        }
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

// ---------------------------------------------------------------------------
// What the readers state and the facade accepts
// ---------------------------------------------------------------------------

fn parse_text(name: &str, token: &str, text: &str) -> powerio::PioModule<PioValue> {
    let options = powerio::ParseOptions::default()
        .format(token)
        .expect("a declared token");
    powerio::parse_with_options(
        Source::from_memory(name, text.as_bytes().to_vec()).expect("memory source"),
        &options,
    )
    .expect("the text parses")
}

fn serialized(module: &powerio::PioModule<PioValue>) -> serde_json::Value {
    let stored = powerio::serialize(module, Destination::memory("module.pio.json").unwrap())
        .expect("serialize");
    let powerio_core::EmittedOutput::Memory { mut artifacts } = stored.into_output() else {
        panic!("a memory destination returns memory artifacts");
    };
    serde_json::from_slice(&artifacts.pop().expect("one document").into_bytes())
        .expect("the document is JSON")
}

fn refusal(document: &serde_json::Value) -> String {
    let text = serde_json::to_vec(document).expect("JSON");
    let error = powerio::deserialize(Source::from_memory("module.pio.json", text).unwrap())
        .expect_err("the document is refused");
    error.to_string()
}

/// The readers keep a band whose ends run the wrong way round as text, so no
/// set the readers produce states one and every parsed module serializes.
#[test]
fn a_band_stated_the_wrong_way_round_stays_text_and_the_module_serializes() {
    let sub = parse_text(
        "bands.sub",
        "psse-sub",
        "SUBSYSTEM 'A'\n   KVRANGE 240.0 100.0\n   AREA 1\nEND\nEND\n",
    );
    assert!(
        sub.diagnostics()
            .iter()
            .any(|note| note.code() == "READ.SUB.SOURCE_MALFORMED"),
        "{:?}",
        sub.diagnostics()
    );
    let PioValue::SubsystemSet(set) = sub.value() else {
        panic!("a .sub parses to powerio.SubsystemSet");
    };
    let subsystem = set.get("A").expect("subsystem A");
    assert_eq!(subsystem.retained[0].text, "KVRANGE 240.0 100.0");
    assert_eq!(subsystem.groups.len(), 1, "only the AREA selector reads");

    let mon = parse_text(
        "bands.mon",
        "psse-mon",
        "MONITOR VOLTAGE RANGE ALL BUSES 1.05 0.95\nMONITOR TIES FROM SUBSYSTEM 'A'\nEND\n",
    );
    assert!(
        mon.diagnostics()
            .iter()
            .any(|note| note.code() == "READ.MON.STATEMENT_UNRECOGNIZED"),
        "{:?}",
        mon.diagnostics()
    );
    let PioValue::MonitoredSet(set) = mon.value() else {
        panic!("a .mon parses to powerio.MonitoredSet");
    };
    assert_eq!(
        set.retained[0].text,
        "MONITOR VOLTAGE RANGE ALL BUSES 1.05 0.95"
    );
    assert_eq!(set.statements.len(), 1);

    // Every module the readers produced carries the PowerIO IR document rules.
    serialized(&sub);
    serialized(&mon);
}

/// A document stating what no reader could have read is refused rather than
/// decoded into a set that writes a file the reader would keep as text.
#[test]
fn deserialize_refuses_a_document_no_reader_could_have_stated() {
    let mut document = serialized(&parse_text(
        "lines.sub",
        "psse-sub",
        "SUBSYSTEM 'A'\n   KVRANGE 100.0 240.0\n   PARTICIPATE\nEND\nEND\nBUSNAMES\n",
    ));
    let data = &mut document["value"]["data"];
    assert_eq!(data["retained"][0]["line"], 6);
    assert_eq!(data["subsystems"][0]["retained"][0]["line"], 3);

    let mut set_level = document.clone();
    set_level["value"]["data"]["retained"][0]["line"] = serde_json::json!(0);
    assert!(refusal(&set_level).contains("lines are 1 based"));

    let mut subsystem_level = document.clone();
    subsystem_level["value"]["data"]["subsystems"][0]["retained"][0]["line"] = serde_json::json!(0);
    let message = refusal(&subsystem_level);
    assert!(message.contains("subsystem `A`"), "{message}");

    let mut band = document.clone();
    band["value"]["data"]["subsystems"][0]["groups"][0]["selectors"][0]["lo"] =
        serde_json::json!(400.0);
    assert!(refusal(&band).contains("does not run low to high"));

    let mut monitored = serialized(&parse_text(
        "lines.mon",
        "psse-mon",
        "MONITOR VOLTAGE RANGE SUBSYSTEM 'A' 0.95 1.05\nMONITOR FLOWS\nEND\n",
    ));
    let mut line = monitored.clone();
    line["value"]["data"]["retained"][0]["line"] = serde_json::json!(0);
    assert!(refusal(&line).contains("lines are 1 based"));

    monitored["value"]["data"]["statements"][0]["vmax"] = serde_json::json!(0.9);
    assert!(refusal(&monitored).contains("does not run low to high"));

    let mut scope = serialized(&parse_text(
        "scope.mon",
        "psse-mon",
        "MONITOR VOLTAGE DEVIATION SUBSYSTEM 'A' 0.05\nEND\n",
    ));
    scope["value"]["data"]["statements"][0]["scope"]["name"] = serde_json::json!("  ");
    let message = refusal(&scope);
    assert!(message.contains("names no subsystem"), "{message}");
}

/// A line kept inside a `JOIN` group or inside a monitored block states its own
/// line, and a document stating line 0 there is refused like one stating it on
/// the set.
#[test]
fn deserialize_refuses_a_kept_line_of_zero_inside_a_block() {
    let group = serialized(&parse_text(
        "join.sub",
        "psse-sub",
        "SUBSYSTEM 'A'\n   JOIN 'G'\n      PARTICIPATE\n   END\nEND\nEND\n",
    ));
    let kept = &group["value"]["data"]["subsystems"][0]["groups"][0]["retained"][0];
    assert_eq!(kept["text"], "PARTICIPATE");
    assert_eq!(kept["line"], 3);
    let mut zero = group.clone();
    zero["value"]["data"]["subsystems"][0]["groups"][0]["retained"][0]["line"] =
        serde_json::json!(0);
    let message = refusal(&zero);
    assert!(
        message.contains("selector group of subsystem `A`"),
        "{message}"
    );
    assert!(message.contains("lines are 1 based"), "{message}");

    for (name, text) in [
        (
            "branches.mon",
            "MONITOR BRANCHES\n101 102 1\nALL TIES\nEND\nEND\n",
        ),
        (
            "interface.mon",
            "MONITOR INTERFACE 'W'\n101 102 1\nALL TIES\nEND\nEND\n",
        ),
    ] {
        let block = serialized(&parse_text(name, "psse-mon", text));
        let kept = &block["value"]["data"]["statements"][0]["retained"][0];
        assert_eq!(kept["text"], "ALL TIES", "{name}");
        assert_eq!(kept["line"], 3, "{name}");
        let mut zero = block.clone();
        zero["value"]["data"]["statements"][0]["retained"][0]["line"] = serde_json::json!(0);
        let message = refusal(&zero);
        assert!(message.contains("monitor statement 0"), "{name}: {message}");
        assert!(message.contains("lines are 1 based"), "{name}: {message}");
    }
}

/// A line the reader keeps after the file `END`, or inside a block, is written
/// back where it was read, so the facade reads the emitted text as the same
/// value rather than as grammar.
#[test]
fn a_kept_line_holds_its_place_through_the_facade() {
    for (name, token, text) in [
        (
            "after_end.con",
            "psse-con",
            "CONTINGENCY 'C'\nOPEN LINE FROM BUS 101 TO BUS 102 CIRCUIT 1\nEND\nEND\nSKIP\n",
        ),
        (
            "after_end.sub",
            "psse-sub",
            "END\nSUBSYSTEM 'A'\nAREA 1\nEND\n",
        ),
        (
            "after_end.mon",
            "psse-mon",
            "END\nMONITOR VOLTAGE RANGE ALL BUSES 0.9 1.1\n",
        ),
        (
            "in_join.sub",
            "psse-sub",
            "SUBSYSTEM 'A'\n   JOIN 'G'\n      JOIN 'H'\n      AREA 1\n   END\nEND\nEND\n",
        ),
        (
            "in_block.mon",
            "psse-mon",
            "MONITOR BRANCHES\n101 102 1\nMONITOR VOLTAGE RANGE ALL BUSES 0.9 1.1\nEND\nEND\n",
        ),
    ] {
        let module = parse_text(name, token, text);
        // Emission replays the source a parsed module retains, so the module
        // goes through a document first: what emits then is the writer's own
        // text.
        let document = serde_json::to_vec(&serialized(&module)).expect("JSON");
        let back = powerio::deserialize(Source::from_memory("module.pio.json", document).unwrap())
            .expect("the document deserializes");
        let written = emitted_bytes(&back, token);
        let again = parse_text(
            name,
            token,
            std::str::from_utf8(&written).expect("the emitted text is UTF-8"),
        );
        assert_values_equal(
            &without_kept_lines(module.value()),
            &without_kept_lines(again.value()),
            name,
        );
        assert_eq!(
            canonical_text(again.value()),
            String::from_utf8(written).expect("the emitted text is UTF-8"),
            "{name}"
        );
    }
}

/// A value built in Rust states what the readers never state: a limit that is
/// not a number. Writing a document from one is refused.
#[test]
fn serialize_refuses_a_limit_that_is_not_finite() {
    use powerio::{MonitorScope, MonitorStatement, MonitoredSet};

    let refuse = |set: MonitoredSet, what: &str| {
        let module = powerio::PioModule::new(PioValue::MonitoredSet(set));
        let error = powerio::serialize(&module, Destination::memory("m.pio.json").unwrap())
            .expect_err(what);
        assert!(error.to_string().contains("not finite"), "{error}");
    };

    refuse(
        MonitoredSet {
            statements: vec![MonitorStatement::Interface {
                name: "W".to_owned(),
                rating_mw: Some(f64::INFINITY),
                branches: Vec::new(),
                retained: Vec::new(),
            }],
            ..MonitoredSet::default()
        },
        "an interface rating that is not finite",
    );
    refuse(
        MonitoredSet {
            statements: vec![MonitorStatement::VoltageDeviation {
                scope: MonitorScope::AllBuses,
                down: 0.05,
                up: Some(f64::NAN),
            }],
            ..MonitoredSet::default()
        },
        "a deviation that is not finite",
    );
    refuse(
        MonitoredSet {
            statements: vec![MonitorStatement::VoltageDeviation {
                scope: MonitorScope::Kv { kv: f64::NAN },
                down: 0.05,
                up: None,
            }],
            ..MonitoredSet::default()
        },
        "a scope base kV that is not finite",
    );

    let set = powerio::ContingencySet {
        cases: vec![powerio::ContingencyCase {
            name: "C".to_owned(),
            actions: vec![powerio::ContingencyAction::ChangeLoad {
                bus: powerio::BusId(1),
                change: powerio::Change {
                    op: powerio::ChangeOp::Increase,
                    amount: f64::INFINITY,
                    unit: powerio::ChangeUnit::Mw,
                },
            }],
        }],
        ..powerio::ContingencySet::default()
    };
    let module = powerio::PioModule::new(PioValue::ContingencySet(set));
    let error = powerio::serialize(&module, Destination::memory("m.pio.json").unwrap())
        .expect_err("a change amount that is not finite");
    assert!(error.to_string().contains("not finite"), "{error}");
}

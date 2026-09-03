//! The input and output conversions the four operations accept, and the
//! diagnostic code each stage of a failed read reports.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use powerio::{ParseOptions, PioValue};
use powerio_core::ErrorCategory;

fn data(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(relative)
}

#[test]
fn every_input_kind_reaches_parse() {
    let path = data("case9.m");
    let expected = powerio::parse(&path)
        .unwrap()
        .value()
        .type_name()
        .to_owned();

    // Names of a file, in each spelling a caller holds one.
    let owned: PathBuf = path.clone();
    let text: String = path.to_str().unwrap().to_owned();
    assert_eq!(
        powerio::parse(path.as_path()).unwrap().value().type_name(),
        expected
    );
    assert_eq!(
        powerio::parse(&owned).unwrap().value().type_name(),
        expected
    );
    assert_eq!(
        powerio::parse(owned.clone()).unwrap().value().type_name(),
        expected
    );
    assert_eq!(
        powerio::parse(text.as_str()).unwrap().value().type_name(),
        expected
    );
    assert_eq!(powerio::parse(&text).unwrap().value().type_name(), expected);
    assert_eq!(
        powerio::parse(text.clone()).unwrap().value().type_name(),
        expected
    );

    // A source the caller built keeps working through the same operation.
    let source = powerio::Source::open(&path).unwrap();
    assert_eq!(
        powerio::parse(&source).unwrap().value().type_name(),
        expected
    );
    assert_eq!(
        powerio::parse(source).unwrap().value().type_name(),
        expected
    );

    // Content in memory, in each spelling. MATPOWER is detected from the file
    // extension, so content declares its format.
    let options = ParseOptions::default().format("matpower").unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let shared: Arc<[u8]> = Arc::from(bytes.clone());
    for value in [
        powerio::parse_with_options(bytes.as_slice(), &options).unwrap(),
        powerio::parse_with_options(bytes.clone(), &options).unwrap(),
        powerio::parse_with_options(shared, &options).unwrap(),
        powerio::parse_with_options(include_bytes!("../../tests/data/case9.m"), &options)
            .unwrap_or_else(|error| panic!("byte string literal input: {error}")),
    ] {
        assert_eq!(value.value().type_name(), expected);
    }
}

#[test]
fn a_directory_reaches_parse_by_name() {
    let module = powerio::parse(data("pypsa/example")).unwrap();
    assert!(
        matches!(module.value(), PioValue::BalancedNetwork(_)),
        "{}",
        module.value().type_name()
    );
}

#[test]
fn a_declared_format_overrides_detection() {
    // The name states MATPOWER; the declared format states what the content
    // actually is, and the module records the declaration.
    let bytes = std::fs::read(data("egret/case9.json")).unwrap();
    let source = powerio::Source::from_memory("case9.m", bytes).unwrap();
    let module = powerio::parse_with_options(
        source,
        &ParseOptions::default().format("egret-json").unwrap(),
    )
    .unwrap();
    assert!(matches!(module.value(), PioValue::BalancedNetwork(_)));
    assert_eq!(
        module
            .source()
            .unwrap()
            .format()
            .map(powerio::FormatId::as_str),
        Some("egret-json")
    );
}

#[test]
fn an_acquisition_root_widens_where_includes_may_live() {
    let root = data("dist/opendss/include_root");
    let master = root.join("nested/master.dss");
    // Without the wider root the include above the master's own directory is
    // refused; with it the case reads whole.
    let refused = powerio::parse(&master).unwrap();
    let allowed =
        powerio::parse_with_options(&master, &ParseOptions::default().acquisition_root(&root));
    assert!(allowed.is_ok(), "{:?}", allowed.err());
    assert!(
        refused
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == "READ.DSS.INCLUDE_REFUSED"),
        "{:?}",
        refused.diagnostics
    );
}

#[test]
fn each_stage_of_a_failed_read_keeps_its_own_code() {
    // Acquisition.
    let missing = powerio::parse(data("no-such-case.m")).unwrap_err();
    assert_eq!(missing.diagnostics()[0].code(), "READ.IO.OPEN");
    assert_eq!(missing.category(), ErrorCategory::Io);

    // Format selection.
    let invalid = ParseOptions::default().format("Not A Token").unwrap_err();
    assert_eq!(invalid.diagnostics()[0].code(), "REQUEST.FORMAT.INVALID_ID");
    assert_eq!(invalid.category(), ErrorCategory::Request);

    let unknown = powerio::parse(b"not a case file").unwrap_err();
    assert_eq!(unknown.diagnostics()[0].code(), "REQUEST.FORMAT.UNKNOWN");
    assert_eq!(unknown.category(), ErrorCategory::Request);

    // Parsing.
    let malformed = powerio::parse_with_options(
        b"function mpc = broken\nmpc.bus = [\n\t1\t3\n",
        &ParseOptions::default().format("matpower").unwrap(),
    )
    .unwrap_err();
    assert!(
        malformed.diagnostics()[0].code().starts_with("PARSE.")
            || malformed.diagnostics()[0].code().starts_with("READ."),
        "{}",
        malformed.diagnostics()[0].code()
    );
    assert_eq!(malformed.category(), ErrorCategory::Parse);
}

#[test]
fn every_output_kind_reaches_emit_and_serialize() {
    let module = powerio::parse(data("case9.m")).unwrap();
    let dir = std::env::temp_dir().join(format!(
        "powerio-io-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let by_path = dir.join("by_path.m");
    powerio::emit(&module, "matpower", by_path.as_path()).unwrap();
    let by_text = dir.join("by_text.m");
    powerio::emit(&module, "matpower", by_text.to_str().unwrap()).unwrap();
    let by_owned = dir.join("by_owned.m");
    powerio::emit(&module, "matpower", by_owned.clone()).unwrap();
    let by_destination = dir.join("by_destination.m");
    powerio::emit(
        &module,
        "matpower",
        powerio::Destination::path(&by_destination),
    )
    .unwrap();
    for written in [&by_path, &by_text, &by_owned, &by_destination] {
        assert!(written.is_file(), "{}", written.display());
    }

    let ir = dir.join("module.pio.json");
    powerio::serialize(&module, ir.as_path()).unwrap();
    let read_back = powerio::deserialize(ir.as_path()).unwrap();
    assert_eq!(read_back.value().type_name(), module.value().type_name());
    assert_eq!(
        powerio::deserialize(std::fs::read(&ir).unwrap())
            .unwrap()
            .value()
            .type_name(),
        module.value().type_name()
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

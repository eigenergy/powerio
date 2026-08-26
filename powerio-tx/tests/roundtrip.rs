//! Round-trip fidelity: `parse → write → parse` must reproduce every vendored
//! case losslessly. This is the property that makes powerio a *lossless* parser,
//! not just a fast one.
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use std::path::{Path, PathBuf};

use powerio_tx::network::BusId;
use powerio_tx::write_matpower;

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data")
}

/// Every `.m` file under `../tests/data` (recursively).
fn cases() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("m"))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&data_dir(), &mut out);
    out.sort();
    assert!(!out.is_empty(), "no .m fixtures found");
    out
}

fn echo_matpower_file(path: &std::path::Path) -> String {
    let module = parse_module(path, Some("matpower")).unwrap();
    powerio_tx::write_as(&module, powerio_tx::TargetFormat::Matpower)
        .unwrap()
        .text
}

fn echo_matpower_str(text: &str) -> String {
    let source = powerio_core::Source::from_bytes("case.m", text.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("matpower").unwrap());
    let module = powerio_tx::parse(source).unwrap();
    powerio_tx::write_as(&module, powerio_tx::TargetFormat::Matpower)
        .unwrap()
        .text
}

#[test]
fn same_format_write_reproduces_the_source_exactly() {
    // The echo lives on the module write path: an unchanged parsed module
    // writes back its retained bytes, comments, column headers, and exact
    // numeric tokens included.
    for path in cases() {
        let original = std::fs::read_to_string(&path).unwrap();
        let written = echo_matpower_file(&path);
        assert_eq!(
            written,
            original,
            "{}: the echo did not reproduce the source",
            path.display()
        );
    }
}

#[test]
fn round_trip_is_idempotent() {
    for path in cases() {
        let once = echo_matpower_file(&path);
        let twice = echo_matpower_str(&once);
        assert_eq!(once, twice, "{}: second write differs", path.display());
    }
}

#[test]
// The round-trip must preserve base_mva bit for bit, so the exact compare is the assertion.
#[allow(clippy::float_cmp)]
fn typed_data_survives_round_trip() {
    for path in cases() {
        let case = parse_matpower_file(&path).unwrap();
        let written = write_matpower(&case);
        let reparsed = parse_matpower(&written).unwrap();
        let where_ = path.display();
        assert_eq!(reparsed.base_mva, case.base_mva, "{where_}: baseMVA");
        assert_eq!(reparsed.buses.len(), case.buses.len(), "{where_}: buses");
        assert_eq!(
            reparsed.branches.len(),
            case.branches.len(),
            "{where_}: branches"
        );
        assert_eq!(
            reparsed.generators.len(),
            case.generators.len(),
            "{where_}: gens"
        );
        assert_eq!(
            reparsed.storage.len(),
            case.storage.len(),
            "{where_}: storage"
        );
    }
}

#[test]
fn parses_bus_names() {
    let case = parse_matpower_file(data_dir().join("case14.m")).unwrap();
    assert!(
        case.buses.iter().all(|b| b.name.is_some()),
        "bus_name not attached to every bus"
    );
    assert!(case.buses[0].name.as_deref().unwrap().starts_with("Bus 1"));
    // Names survive the round-trip.
    let reparsed = parse_matpower(&write_matpower(&case)).unwrap();
    assert_eq!(reparsed.buses[0].name, case.buses[0].name);
}

#[test]
// pf is `10` verbatim in the dcline fixture, so the exact compare is intended.
#[allow(clippy::float_cmp)]
fn parses_hvdc_dclines() {
    let case = parse_matpower_file(data_dir().join("t_case9_dcline.m")).unwrap();
    assert!(!case.hvdc.is_empty(), "mpc.dcline not parsed");
    let dc = &case.hvdc[0];
    assert_eq!((dc.from, dc.to), (BusId(30), BusId(4)));
    assert_eq!(dc.pf, 10.0);
    // HVDC survives the round-trip (document passthrough).
    let reparsed = parse_matpower(&write_matpower(&case)).unwrap();
    assert_eq!(reparsed.hvdc.len(), case.hvdc.len());
}

#[test]
fn captures_extra_generator_columns() {
    // MATPOWER gen rows carry ramp/Pc/Qc/apf columns past PMIN; they must not
    // be silently dropped.
    let case = parse_matpower_file(data_dir().join("case9.m")).unwrap();
    assert!(
        case.generators[0].caps.iter().any(Option::is_some),
        "generator columns past PMIN were dropped"
    );
}

#[test]
fn unescapes_doubled_quotes_in_bus_names() {
    let src = "function mpc = q\n\
        mpc.baseMVA = 100;\n\
        mpc.bus = [\n\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n\t2\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n];\n\
        mpc.branch = [\n\t1\t2\t0.01\t0.1\t0\t250\t250\t250\t0\t0\t1\t-360\t360;\n];\n\
        mpc.bus_name = {\n\t'O''Brien';\n\t'Plain';\n};\n";
    let case = parse_matpower(src).unwrap();
    assert_eq!(case.buses[0].name.as_deref(), Some("O'Brien"));
    assert_eq!(case.buses[1].name.as_deref(), Some("Plain"));
    // The raw `''` is preserved on round-trip regardless of the typed unescape.
    assert!(write_matpower(&case).contains("'O''Brien'"));
}

#[test]
fn preserves_scientific_notation_tokens() {
    // case2869pegase has 172 tokens like `7e-05`; an f64-based writer would
    // re-emit `0.00007`. The module echo keeps the original token.
    assert!(
        echo_matpower_file(&data_dir().join("case2869pegase.m")).contains("7e-05"),
        "scientific-notation token was reformatted"
    );
}

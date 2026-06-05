//! Round-trip fidelity: `parse → write → parse` must reproduce every vendored
//! case losslessly. This is the property that makes netmat a *lossless* parser,
//! not just a fast one.

use std::path::{Path, PathBuf};

use netmat::{parse_matpower, parse_matpower_file, write_matpower};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

/// Every `.m` file under `tests/data` (recursively).
fn cases() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "m") {
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

#[test]
fn writer_reproduces_source_modulo_trailing_newline() {
    for path in cases() {
        let original = std::fs::read_to_string(&path).unwrap();
        let written = write_matpower(&parse_matpower_file(&path).unwrap());
        // Compare modulo the final newline and `\r\n`; everything else —
        // comments, column headers, exact tokens — is byte-for-byte.
        assert_eq!(
            written.trim_end_matches('\n'),
            original.replace("\r\n", "\n").trim_end_matches('\n'),
            "{}: writer did not reproduce the source",
            path.display()
        );
    }
}

#[test]
fn round_trip_is_idempotent() {
    for path in cases() {
        let once = write_matpower(&parse_matpower_file(&path).unwrap());
        let twice = write_matpower(&parse_matpower(&once).unwrap());
        assert_eq!(once, twice, "{}: second write differs", path.display());
    }
}

#[test]
fn typed_data_survives_round_trip() {
    for path in cases() {
        let case = parse_matpower_file(&path).unwrap();
        let written = write_matpower(&case);
        let reparsed = parse_matpower(&written).unwrap();
        let where_ = path.display();
        assert_eq!(reparsed.base_mva, case.base_mva, "{where_}: baseMVA");
        assert_eq!(reparsed.buses.len(), case.buses.len(), "{where_}: buses");
        assert_eq!(reparsed.branches.len(), case.branches.len(), "{where_}: branches");
        assert_eq!(reparsed.gens.len(), case.gens.len(), "{where_}: gens");
        assert_eq!(reparsed.storage.len(), case.storage.len(), "{where_}: storage");
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
fn parses_hvdc_dclines() {
    let case = parse_matpower_file(data_dir().join("t_case9_dcline.m")).unwrap();
    assert!(!case.dclines.is_empty(), "mpc.dcline not parsed");
    let dc = &case.dclines[0];
    assert_eq!((dc.from_id, dc.to_id), (30, 4));
    assert_eq!(dc.pf, 10.0);
    // HVDC survives the round-trip (document passthrough).
    let reparsed = parse_matpower(&write_matpower(&case)).unwrap();
    assert_eq!(reparsed.dclines.len(), case.dclines.len());
}

#[test]
fn captures_extra_generator_columns() {
    // MATPOWER gen rows carry ramp/Pc/Qc/apf columns past PMIN; they must not
    // be silently dropped.
    let case = parse_matpower_file(data_dir().join("case9.m")).unwrap();
    assert!(
        !case.gens[0].extra.is_empty(),
        "generator columns past PMIN were dropped"
    );
}

#[test]
fn preserves_scientific_notation_tokens() {
    // case2869pegase has 172 tokens like `7e-05`; an f64-based writer would
    // re-emit `0.00007`. The document keeps the original token.
    let case = parse_matpower_file(data_dir().join("case2869pegase.m")).unwrap();
    assert!(
        write_matpower(&case).contains("7e-05"),
        "scientific-notation token was reformatted"
    );
}

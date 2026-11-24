//! PyPSA CSV element map: each mapped row of `buses.csv`, `lines.csv`,
//! `transformers.csv`, and `generators.csv` reads back as its element
//! (eigenergy/powerio#916).

mod common;

use common::element_map::{Kind, ReadBack, Written, case, check_format, emit};
use powerio::RowRef;

/// One CSV table of the written folder: its header and data rows. The case's
/// cells hold no quotes or commas, so a plain split reads them.
fn table(written: &Written, file: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let bytes = written
        .artifacts
        .iter()
        .find(|(name, _)| name == file)
        .map_or_else(|| panic!("no {file} written"), |(_, bytes)| bytes);
    let text = std::str::from_utf8(bytes).expect("the table is UTF-8");
    assert!(!text.contains('"'), "{file}: unexpected quoted cell");
    let mut lines = text.lines();
    let split = |line: &str| line.split(',').map(str::to_owned).collect::<Vec<_>>();
    let header = split(lines.next().expect("a header row"));
    (header, lines.map(split).collect())
}

/// The cell `column` of data row `index` in `file`.
fn cell(written: &Written, file: &str, index: usize, column: &str) -> String {
    let (header, rows) = table(written, file);
    let at = header
        .iter()
        .position(|name| name == column)
        .unwrap_or_else(|| panic!("{file} has no `{column}` column"));
    rows[index][at].clone()
}

/// The bus number a bus key in an element table names, looked up through
/// `buses.csv` so a named bus resolves the same way PyPSA's join does.
fn bus_number(written: &Written, key: &str) -> usize {
    let (header, rows) = table(written, "buses.csv");
    assert_eq!(header[0], "name");
    let row = rows
        .iter()
        .position(|row| row[0] == key)
        .unwrap_or_else(|| panic!("buses.csv has no bus `{key}`"));
    // case4par.m names no bus, so the writer keys each bus on its number.
    rows[row][0]
        .parse()
        .expect("an unnamed bus is keyed on its number")
}

fn read(written: &Written, kind: Kind, row: &RowRef) -> ReadBack {
    let file = format!("{}.csv", row.table);
    let name = cell(written, &file, row.index, "name");
    assert_eq!(
        row.key.as_deref(),
        Some(name.as_str()),
        "{file} row {}: the key is the name column",
        row.index
    );
    match kind {
        Kind::Bus => {
            assert_eq!(row.table, "buses");
            ReadBack::Bus(bus_number(written, &name))
        }
        Kind::Branch => {
            assert!(
                row.table == "lines" || row.table == "transformers",
                "a branch row in {}",
                row.table
            );
            ReadBack::Branch(
                bus_number(written, &cell(written, &file, row.index, "bus0")),
                bus_number(written, &cell(written, &file, row.index, "bus1")),
            )
        }
        Kind::Generator => {
            assert_eq!(row.table, "generators");
            ReadBack::Generator(bus_number(written, &cell(written, &file, row.index, "bus")))
        }
    }
}

#[test]
fn pypsa_rows_read_back_as_their_elements() {
    let written = check_format("pypsa-csv", read);
    let map = written.map();
    // The tap transformer 30-20 and the phase shifter 10-30 go to
    // transformers.csv; the five lines to lines.csv, in module order.
    let tables: Vec<(&str, usize)> = map
        .branch()
        .map(|(_, row)| {
            let row = row.expect("every branch is written");
            (row.table.as_str(), row.index)
        })
        .collect();
    assert_eq!(
        tables,
        [
            ("lines", 0),
            ("lines", 1),
            ("transformers", 0),
            ("lines", 2),
            ("lines", 3),
            ("transformers", 1),
            ("lines", 4),
        ]
    );
    // Every element is written, the out of service ones included.
    assert!(
        !written
            .result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == "EMIT.ELEMENT_MAP.UNMAPPED")
    );
}

/// Re-emitting a written PyPSA folder to PyPSA CSV echoes the retained
/// directory, runs no writer, and returns no map.
#[test]
fn a_retained_pypsa_folder_echo_has_no_map() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("case");
    powerio::emit(&case(), "pypsa-csv", powerio::Destination::path(&root))
        .expect("the folder writes");
    let reparsed = powerio::parse(&root).expect("the written folder parses");
    let echo = emit(&reparsed, "pypsa-csv");
    assert_eq!(echo.result.fidelity(), powerio::Fidelity::ExactSameFormat);
    assert!(echo.result.element_map().is_none());
}

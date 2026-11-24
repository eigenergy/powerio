//! PyPSA CSV element map: each mapped row of `buses.csv`, `lines.csv`,
//! `transformers.csv`, and `generators.csv` reads back as its element
//! (eigenergy/powerio#916).

mod common;

use common::element_map::{Kind, ReadBack, Written, case, check_format, check_map, emit, network};
use powerio::{BalancedNetwork, RowRef};

/// Split one CSV record, honoring double quoted cells (a quoted cell may
/// hold commas and doubled quotes). The written tables hold no line breaks
/// inside a cell.
fn split(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => cells.push(std::mem::take(&mut cell)),
            _ => cell.push(c),
        }
    }
    assert!(!quoted, "an unterminated quoted cell in `{line}`");
    cells.push(cell);
    cells
}

/// One CSV table of the written folder: its header and data rows.
fn table(written: &Written, file: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let bytes = written
        .artifacts
        .iter()
        .find(|(name, _)| name == file)
        .map_or_else(|| panic!("no {file} written"), |(_, bytes)| bytes);
    let text = std::str::from_utf8(bytes).expect("the table is UTF-8");
    let mut lines = text.lines();
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

/// The bus number a bus cell (a `buses.csv` name) refers to: the cell must
/// name a row of `buses.csv`, and the map's bus entry keyed on that name
/// gives the element, as PyPSA's own join by name does. A bus name need not
/// be a number.
fn bus_number(network: &BalancedNetwork, written: &Written, name: &str) -> usize {
    let (header, rows) = table(written, "buses.csv");
    assert_eq!(header[0], "name");
    assert!(
        rows.iter().any(|row| row[0] == name),
        "buses.csv has no bus `{name}`"
    );
    let position = written
        .map()
        .bus()
        .position(|(_, row)| row.and_then(|row| row.key.as_deref()) == Some(name))
        .unwrap_or_else(|| panic!("no bus entry keyed `{name}`"));
    network.buses()[position].id.0
}

fn read(network: &BalancedNetwork, written: &Written, kind: Kind, row: &RowRef) -> ReadBack {
    let file = format!("{}.csv", row.table);
    let name = cell(written, &file, row.index, "name");
    assert_eq!(
        row.key.as_deref(),
        Some(name.as_str()),
        "{file} row {}: the key is the name column",
        row.index
    );
    let bus = |column: &str| bus_number(network, written, &cell(written, &file, row.index, column));
    match kind {
        Kind::Bus => {
            assert_eq!(row.table, "buses");
            ReadBack::Bus(bus_number(network, written, &name))
        }
        Kind::Branch => {
            assert!(
                row.table == "lines" || row.table == "transformers",
                "a branch row in {}",
                row.table
            );
            ReadBack::Branch(bus("bus0"), bus("bus1"))
        }
        Kind::Generator => {
            assert_eq!(row.table, "generators");
            ReadBack::Generator(bus("bus"))
        }
    }
}

#[test]
fn pypsa_rows_read_back_as_their_elements() {
    let module = case();
    let network = network(&module);
    let written = check_format("pypsa-csv", |written, kind, row| {
        read(network, written, kind, row)
    });
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

/// The PyPSA example names its buses (`north`, `south`, `east`): bus cells
/// resolve through the map, not as numbers.
#[test]
fn named_pypsa_buses_read_back_through_the_map() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/data/pypsa/example");
    let module = powerio::parse(path)
        .expect("the example parses")
        .sever_source();
    let network = network(&module);
    let written = emit(&module, "pypsa-csv");
    check_map(network, &written, "pypsa-csv", |written, kind, row| {
        read(network, written, kind, row)
    });
    let keys: Vec<Option<&str>> = written
        .map()
        .bus()
        .map(|(_, row)| row.and_then(|row| row.key.as_deref()))
        .collect();
    assert_eq!(keys, [Some("north"), Some("south"), Some("east")]);
}

//! PSS/E `.raw` element map: each mapped row of the bus, branch,
//! transformer, and generator sections reads back as its element
//! (eigenergy/powerio#916).

mod common;

use common::element_map::{Kind, ReadBack, Written, check_format, check_retained_echo_has_no_map};
use powerio::RowRef;

/// The fields of one record, with quoted fields unquoted and trimmed as the
/// PSS/E reader trims them.
fn fields(record: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    for c in record.chars() {
        match c {
            '\'' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut field).trim().to_owned()),
            _ => field.push(c),
        }
    }
    out.push(field.trim().to_owned());
    out
}

/// The record lines of one data section, as written. The bus section of a
/// revision 33 file has no opening marker: it starts after the three header
/// lines.
fn section<'a>(text: &'a str, name: &str) -> Vec<&'a str> {
    let lines: Vec<&str> = text.lines().collect();
    let marker = format!("BEGIN {name} DATA");
    let start = lines
        .iter()
        .position(|line| line.starts_with("0 /") && line.ends_with(&marker))
        .map_or_else(
            || {
                assert_eq!(name, "BUS", "no {name} section");
                3
            },
            |marker| marker + 1,
        );
    lines[start..]
        .iter()
        .take_while(|line| !line.starts_with("0 /"))
        .copied()
        .collect()
}

fn number(field: &str) -> usize {
    field
        .parse()
        .unwrap_or_else(|_| panic!("bus number {field:?}"))
}

fn read(written: &Written, kind: Kind, row: &RowRef) -> ReadBack {
    let text = written.text();
    let key = row.key.as_deref().expect("every PSS/E row has a key");
    match kind {
        Kind::Bus => {
            assert_eq!(row.table, "bus");
            let record = fields(section(text, "BUS")[row.index]);
            assert_eq!(key, record[0]);
            ReadBack::Bus(number(&record[0]))
        }
        Kind::Branch => {
            // A two winding transformer is four lines; its first states
            // I, J, K = 0, and CKT.
            let record = match row.table.as_str() {
                "branch" => fields(section(text, "BRANCH")[row.index]),
                "transformer" => {
                    let record = fields(section(text, "TRANSFORMER")[4 * row.index]);
                    assert_eq!(record[2], "0", "a two winding transformer");
                    let mut record = record;
                    record.remove(2);
                    record
                }
                other => panic!("unexpected branch table {other}"),
            };
            assert_eq!(key, format!("{},{},{}", record[0], record[1], record[2]));
            ReadBack::Branch(number(&record[0]), number(&record[1]))
        }
        Kind::Generator => {
            assert_eq!(row.table, "generator");
            let record = fields(section(text, "GENERATOR")[row.index]);
            assert_eq!(key, format!("{},{}", record[0], record[1]));
            ReadBack::Generator(number(&record[0]))
        }
    }
}

#[test]
fn psse_rows_read_back_as_their_elements() {
    let written = check_format("psse", read);
    // The tap transformer 30-20 and the phase shifter 10-30 are the
    // transformer section; the other five branches are the branch section.
    let map = written.map();
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
            ("branch", 0),
            ("branch", 1),
            ("transformer", 0),
            ("branch", 2),
            ("branch", 3),
            ("transformer", 1),
            ("branch", 4),
        ]
    );
    let keys: Vec<&str> = map
        .branch()
        .filter_map(|(_, row)| row?.key.as_deref())
        .collect();
    assert_eq!(keys[..2], ["10,20,1", "10,20,2"]);
    let generators: Vec<&str> = map
        .generator()
        .filter_map(|(_, row)| row?.key.as_deref())
        .collect();
    assert_eq!(generators, ["10,1", "20,1", "10,2", "30,1"]);
    check_retained_echo_has_no_map(&written, "psse");
}

/// Revisions 34 and 35 add the system switching device section and widen the
/// records; the map still names the rows the writer wrote.
#[test]
fn psse_modern_revisions_read_back_as_their_elements() {
    for format in ["psse34", "psse35"] {
        check_format(format, read);
    }
}

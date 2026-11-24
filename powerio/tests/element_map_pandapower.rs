//! pandapower JSON element map: each mapped row of the `bus`, `line`,
//! `trafo`, and `gen` frames reads back as its element
//! (eigenergy/powerio#916).

mod common;

use common::element_map::{Kind, ReadBack, Written, check_format, check_retained_echo_has_no_map};
use powerio::RowRef;
use serde_json::Value;

/// The split oriented frame `table` as `(columns, index, data)`.
fn frame(document: &Value, table: &str) -> (Vec<String>, Vec<Value>, Vec<Vec<Value>>) {
    let inner = document["_object"][table]["_object"]
        .as_str()
        .unwrap_or_else(|| panic!("no {table} frame"));
    let inner: Value = serde_json::from_str(inner).unwrap();
    let columns = inner["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|column| column.as_str().unwrap().to_owned())
        .collect();
    let index = inner["index"].as_array().unwrap().clone();
    let data = inner["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row.as_array().unwrap().clone())
        .collect();
    (columns, index, data)
}

/// A pandapower bus reference (0-based index label) as a PowerIO bus number.
#[allow(clippy::cast_possible_truncation)]
fn bus_number(label: &Value) -> usize {
    label.as_u64().unwrap() as usize + 1
}

fn read(written: &Written, kind: Kind, row: &RowRef) -> ReadBack {
    let document: Value = serde_json::from_str(written.text()).unwrap();
    let (columns, index, data) = frame(&document, &row.table);
    let record = &data[row.index];
    let cell = |column: &str| {
        let at = columns.iter().position(|c| c == column).unwrap();
        &record[at]
    };
    let label = &index[row.index];
    match kind {
        Kind::Bus => {
            assert_eq!(row.table, "bus");
            assert_eq!(row.key.as_deref(), Some(label.to_string().as_str()));
            ReadBack::Bus(bus_number(label))
        }
        Kind::Branch => {
            assert_eq!(row.key, None);
            assert_eq!(label.as_u64(), Some(row.index as u64));
            match row.table.as_str() {
                "line" => {
                    ReadBack::Branch(bus_number(cell("from_bus")), bus_number(cell("to_bus")))
                }
                "trafo" => ReadBack::Branch(bus_number(cell("hv_bus")), bus_number(cell("lv_bus"))),
                other => panic!("a branch row in {other}"),
            }
        }
        Kind::Generator => {
            assert_eq!(row.table, "gen");
            assert_eq!(row.key, None);
            assert_eq!(label.as_u64(), Some(row.index as u64));
            ReadBack::Generator(bus_number(cell("bus")))
        }
    }
}

#[test]
fn pandapower_rows_read_back_as_their_elements() {
    let written = check_format("pandapower-json", read);
    // The case has lines and transformers (the tap and the phase shifter),
    // so both branch frames are exercised.
    let tables: Vec<&str> = written
        .map()
        .branch()
        .map(|(_, row)| row.expect("every branch is written").table.as_str())
        .collect();
    assert!(
        tables.contains(&"line") && tables.contains(&"trafo"),
        "{tables:?}"
    );
    assert!(written.map().generator().all(|(_, row)| row.is_some()));
    check_retained_echo_has_no_map(&written, "pandapower-json");
}

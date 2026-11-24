//! Surge JSON element map: each mapped row of `network.buses`,
//! `network.branches`, and `network.generators` reads back as its element
//! (eigenergy/powerio#916).

mod common;

use common::element_map::{Kind, ReadBack, Written, check_format, check_retained_echo_has_no_map};
use powerio::RowRef;
use serde_json::Value;

#[allow(clippy::cast_possible_truncation)]
fn read(written: &Written, kind: Kind, row: &RowRef) -> ReadBack {
    let document: Value = serde_json::from_str(written.text()).unwrap();
    let record = &document["network"][row.table.as_str()][row.index];
    let number = |field: &str| record[field].as_u64().unwrap() as usize;
    match kind {
        Kind::Bus => {
            assert_eq!(row.table, "buses");
            assert_eq!(
                row.key.as_deref(),
                Some(number("number").to_string().as_str())
            );
            ReadBack::Bus(number("number"))
        }
        Kind::Branch => {
            assert_eq!(row.table, "branches");
            assert_eq!(row.key, None);
            ReadBack::Branch(number("from_bus"), number("to_bus"))
        }
        Kind::Generator => {
            assert_eq!(row.table, "generators");
            assert_eq!(row.key.as_deref(), record["id"].as_str());
            ReadBack::Generator(number("bus"))
        }
    }
}

#[test]
fn surge_rows_read_back_as_their_elements() {
    let written = check_format("surge-json", read);
    check_retained_echo_has_no_map(&written, "surge-json");
}

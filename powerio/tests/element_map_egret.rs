//! egret JSON element map: each mapped row of `elements.bus`,
//! `elements.branch`, and `elements.generator` reads back as its element
//! (eigenergy/powerio#916).

mod common;

use common::element_map::{Kind, ReadBack, Written, check_format, check_retained_echo_has_no_map};
use powerio::RowRef;
use serde_json::Value;

fn read(written: &Written, kind: Kind, row: &RowRef) -> ReadBack {
    let document: Value = serde_json::from_str(written.text()).unwrap();
    let table = document["elements"][row.table.as_str()]
        .as_object()
        .unwrap_or_else(|| panic!("no elements.{} dictionary", row.table));
    // The index is the key's position in the dictionary as written.
    let (key, record) = table.iter().nth(row.index).expect("the row exists");
    assert_eq!(row.key.as_deref(), Some(key.as_str()));
    let number = |field: &str| record[field].as_str().unwrap().parse::<usize>().unwrap();
    match kind {
        Kind::Bus => {
            assert_eq!(row.table, "bus");
            ReadBack::Bus(key.parse().unwrap())
        }
        Kind::Branch => {
            assert_eq!(row.table, "branch");
            ReadBack::Branch(number("from_bus"), number("to_bus"))
        }
        Kind::Generator => {
            assert_eq!(row.table, "generator");
            ReadBack::Generator(number("bus"))
        }
    }
}

#[test]
fn egret_rows_read_back_as_their_elements() {
    let written = check_format("egret-json", read);
    check_retained_echo_has_no_map(&written, "egret-json");
}

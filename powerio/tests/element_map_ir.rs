//! PowerIO IR identity element map (eigenergy/powerio#916, rule 7).

mod common;

use common::element_map::{Kind, ReadBack, Written, case, check_map, network, serialize};
use powerio::RowRef;
use serde_json::Value;

#[allow(clippy::cast_possible_truncation)]
fn read(written: &Written, kind: Kind, row: &RowRef) -> ReadBack {
    let document: Value = serde_json::from_str(written.text()).unwrap();
    let network = &document["value"]["data"];
    let record = &network[row.table.as_str()][row.index];
    let number = |field: &str| record[field].as_u64().unwrap() as usize;
    match kind {
        Kind::Bus => ReadBack::Bus(number("id")),
        Kind::Branch => ReadBack::Branch(number("from"), number("to")),
        Kind::Generator => ReadBack::Generator(number("bus")),
    }
}

#[test]
fn serialize_returns_the_identity_map() {
    let module = case();
    let written = serialize(&module);
    check_map(network(&module), &written, "pio-ir", read);
    let map = written.map();
    for (position, (_, row)) in map.branch().enumerate() {
        let row = row.unwrap();
        assert_eq!((row.table.as_str(), row.index), ("branches", position));
    }
    assert_eq!(
        serialize(&module).map().to_json(),
        map.to_json(),
        "the identity map is deterministic"
    );
}

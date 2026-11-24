//! PowerSystems JSON element map: each mapped row is the `index`th component
//! of its `__metadata__.type` in `data.components`, named by its key, and
//! reads back as its element (eigenergy/powerio#916).

mod common;

use common::element_map::{Kind, ReadBack, Written, check_format, check_retained_echo_has_no_map};
use powerio::RowRef;
use serde_json::Value;

/// The `index`th component of type `table`.
fn component<'a>(document: &'a Value, table: &str, index: usize) -> &'a Value {
    document["data"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["__metadata__"]["type"] == table)
        .nth(index)
        .unwrap_or_else(|| panic!("no {table} component {index}"))
}

/// The component a PowerSystems reference (`{"value": uuid}`) names.
fn referenced<'a>(document: &'a Value, reference: &Value) -> &'a Value {
    let uuid = reference["value"].as_str().unwrap();
    document["data"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["internal"]["uuid"]["value"] == uuid)
        .unwrap_or_else(|| panic!("no component with uuid {uuid}"))
}

#[allow(clippy::cast_possible_truncation)]
fn bus_number(bus: &Value) -> usize {
    assert_eq!(bus["__metadata__"]["type"], "ACBus");
    bus["number"].as_u64().unwrap() as usize
}

fn read(written: &Written, kind: Kind, row: &RowRef) -> ReadBack {
    let document: Value = serde_json::from_str(written.text()).unwrap();
    let record = component(&document, &row.table, row.index);
    assert_eq!(row.key.as_deref(), record["name"].as_str());
    match kind {
        Kind::Bus => {
            assert_eq!(row.table, "ACBus");
            ReadBack::Bus(bus_number(record))
        }
        Kind::Branch => {
            assert!(
                matches!(
                    row.table.as_str(),
                    "Line" | "MonitoredLine" | "TapTransformer" | "PhaseShiftingTransformer"
                ),
                "{}",
                row.table
            );
            let arc = referenced(&document, &record["arc"]);
            ReadBack::Branch(
                bus_number(referenced(&document, &arc["from"])),
                bus_number(referenced(&document, &arc["to"])),
            )
        }
        Kind::Generator => ReadBack::Generator(bus_number(referenced(&document, &record["bus"]))),
    }
}

#[test]
fn powersystems_rows_read_back_as_their_elements() {
    let written = check_format("powersystems-json", read);
    // case4par writes its plain lines as Line, the 30-20 transformer and the
    // 115 kV / 230 kV line 40-10 as TapTransformer, and the phase shifter as
    // PhaseShiftingTransformer.
    let tables: Vec<&str> = written
        .map()
        .branch()
        .map(|(_, row)| row.unwrap().table.as_str())
        .collect();
    assert_eq!(
        tables,
        [
            "Line",
            "Line",
            "TapTransformer",
            "Line",
            "TapTransformer",
            "PhaseShiftingTransformer",
            "Line"
        ]
    );
    check_retained_echo_has_no_map(&written, "powersystems-json");
}

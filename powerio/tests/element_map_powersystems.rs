//! PowerSystems JSON element map: each mapped row is the `index`th component
//! of its `__metadata__.type` in `data.components`, named by its key, and
//! reads back as its element (eigenergy/powerio#916).

mod common;

use common::element_map::{
    Kind, ReadBack, Written, check_format, check_map, check_retained_echo_has_no_map, emit, network,
};
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
    let name = record["name"].as_str();
    if let Some(key) = &row.key {
        assert_eq!(Some(key.as_str()), name, "{} row {}", row.table, row.index);
    } else {
        // The writer leaves a row unkeyed only when another component of its
        // type carries the same name.
        let name = name.expect("an unkeyed component still has a name");
        let same_name = document["data"]["components"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|other| other["__metadata__"]["type"] == row.table.as_str())
            .filter(|other| other["name"] == name)
            .count();
        assert!(
            same_name > 1,
            "{} row {} is unkeyed but its name `{name}` is unique",
            row.table,
            row.index
        );
    }
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

/// IEEE_30_bus.raw names its buses: the map keys each component on its
/// written name, and the rows still read back as their elements.
#[test]
fn named_psse_buses_read_back_as_powersystems_rows() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/psse/IEEE_30_bus.raw");
    let module = powerio::parse(path)
        .expect("the case parses")
        .sever_source();
    let network = network(&module);
    let written = emit(&module, "powersystems-json");
    check_map(network, &written, "powersystems-json", read);
    let named = written
        .map()
        .bus()
        .filter(|(_, row)| row.is_some_and(|row| row.key.is_some()))
        .count();
    assert!(named > 0, "no bus row is keyed on its name");
}

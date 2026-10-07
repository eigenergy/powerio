use std::collections::BTreeMap;

use powerio_core::Source;

use super::{
    read, schema::NativeDatabase, tests::database, transformer_tests::transformer_database,
};

const NODE_DATA: &str = "
    ALTER TABLE Node ADD COLUMN Name TEXT DEFAULT 'same display name';
    ALTER TABLE Node ADD COLUMN VoltLevel_ID INTEGER DEFAULT 1;
    ALTER TABLE Node ADD COLUMN Stp_ID INTEGER DEFAULT 0;
    ALTER TABLE Node ADD COLUMN Un REAL DEFAULT 1234;
    ALTER TABLE Node ADD COLUMN Flag_Phase INTEGER DEFAULT 8;
    CREATE TABLE VoltageLevel (VoltLevel_ID INTEGER, Variant_ID INTEGER, Un REAL, f REAL, Flag_Volt INTEGER);
    INSERT INTO VoltageLevel VALUES (1, 1, 0.4, 50, 1);";

fn node_database(edit: &str) -> Vec<u8> {
    database(&format!(
        "{NODE_DATA}
         ALTER TABLE Element ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Terminal ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Terminal ADD COLUMN Flag_Terminal INTEGER DEFAULT 7;
         {edit}"
    ))
}

#[test]
fn node_nominal_voltage_and_conductors_ignore_initial_guesses_and_fault_selection() {
    let native = NativeDatabase::decode(&node_database(""), None).unwrap();
    let topology = native.topology_draft().unwrap();
    assert!((topology.nodes[&10].nominal_ll_volts - 400.0).abs() < 1e-12);
    assert!((topology.nodes[&10].level_frequency_hz - 50.0).abs() < 1e-12);
    assert_eq!(topology.nodes[&10].neutral_point, None);
    let buses = topology.buses();
    assert_eq!(buses[0].id, "10");
    assert_eq!(buses[1].id, "20");
    assert_eq!(buses[0].terminals, ["1", "2", "3"]);
    assert_eq!(buses[0].extras["sincal"]["name"], "same display name");
    assert_eq!(buses[1].extras["sincal"]["name"], "same display name");
    assert!(buses.iter().all(|bus| bus.grounded.is_empty()));
}

#[test]
fn transformer_coils_expand_secondary_phase_pairs_without_inventing_neutral_wires() {
    let bytes = transformer_database(&format!(
        "{NODE_DATA}
         UPDATE Terminal SET Flag_Terminal=1;
         UPDATE Node SET Stp_ID=15 WHERE Node_ID=10;"
    ));
    let topology = NativeDatabase::decode(&bytes, None)
        .unwrap()
        .topology_draft()
        .unwrap();
    let buses = topology.buses();
    assert_eq!(buses[0].terminals, ["1"]);
    assert_eq!(buses[1].terminals, ["1", "2"]);
    assert_eq!(topology.nodes[&10].neutral_point, Some(15));
    assert!(buses.iter().all(|b| b.grounded.is_empty()));
}

#[test]
fn declared_topology_keeps_open_ports_neutral_only_wires_and_isolated_nodes() {
    let bytes = node_database(
        "UPDATE Terminal SET Flag_Terminal=6, Flag_State=0;
         UPDATE Element SET Flag_State=0;
         INSERT INTO Element (Element_ID, Variant_ID, Type) VALUES (31, 1, 'Line');
         INSERT INTO Terminal VALUES (60, 1, 31, 10, 1, 1, 8), (70, 1, 31, 20, 2, 1, 8);
         INSERT INTO Node (Node_ID, Variant_ID) VALUES (99, 1);",
    );
    let topology = NativeDatabase::decode(&bytes, None)
        .unwrap()
        .topology_draft()
        .unwrap();
    let buses = topology.buses();
    assert_eq!(buses[0].terminals, ["1", "3", "n"]);
    assert_eq!(buses[1].terminals, ["1", "3", "n"]);
    assert!(buses[2].terminals.is_empty());
    assert_eq!(buses[2].id, "99");
    assert!(buses.iter().all(|b| b.grounded.is_empty()));
}

#[test]
fn voltage_levels_join_by_variant_and_reject_ambiguous_or_invalid_ratings() {
    let bytes = node_database(
        "INSERT INTO Variant VALUES (2, NULL, 1);
         INSERT INTO Node (Node_ID, Variant_ID) VALUES (10, 2);
         INSERT INTO VoltageLevel VALUES (1, 2, 11, 60, 1);",
    );
    let first = NativeDatabase::decode(&bytes, Some(1))
        .unwrap()
        .node_inputs()
        .unwrap();
    let second = NativeDatabase::decode(&bytes, Some(2))
        .unwrap()
        .node_inputs()
        .unwrap();
    assert!((first[&10].nominal_ll_volts - 400.0).abs() < 1e-12);
    assert!((second[&10].nominal_ll_volts - 11_000.0).abs() < 1e-12);
    assert!((second[&10].level_frequency_hz - 60.0).abs() < 1e-12);
    for edit in [
        "DELETE FROM VoltageLevel",
        "UPDATE VoltageLevel SET Variant_ID=2",
        "INSERT INTO VoltageLevel VALUES (1, 1, 11, 50, 1)",
        "UPDATE VoltageLevel SET Un=0",
        "UPDATE VoltageLevel SET Un=1e308",
        "UPDATE VoltageLevel SET f=NULL",
        "UPDATE Node SET Stp_ID=-1",
    ] {
        let native = NativeDatabase::decode(&node_database(edit), None).unwrap();
        assert!(native.node_inputs().is_err(), "accepted {edit}");
    }
}

#[test]
fn unsupported_equipment_does_not_silently_disappear_from_bus_connections() {
    let native = NativeDatabase::decode(
        &node_database("UPDATE Element SET Type='UnknownDevice'"),
        None,
    )
    .unwrap();
    assert!(native.topology_draft().is_err());
}

#[test]
fn node_voltage_basis_must_be_explicit_and_line_line() {
    for value in ["2", "0", "-1", "NULL", "1.5", "'unknown'"] {
        let native = NativeDatabase::decode(
            &node_database(&format!("UPDATE VoltageLevel SET Flag_Volt={value}")),
            None,
        )
        .unwrap();
        assert!(native.node_inputs().is_err(), "accepted {value}");
    }
    let native = NativeDatabase::decode(
        &node_database("ALTER TABLE VoltageLevel DROP COLUMN Flag_Volt"),
        None,
    )
    .unwrap();
    assert!(native.node_inputs().is_err());
}

#[test]
fn authentic_bus_voltage_bases_match_paired_csv() {
    let data = include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let source = Source::from_memory("case.sinx", &data[..]).unwrap();
    let topology = read(&source, None).unwrap().topology_draft().unwrap();
    let csv = include_str!("../../../tests/data/sincal/simbench-csv/Node.csv");
    let expected: BTreeMap<_, _> = csv
        .lines()
        .skip(1)
        .map(|line| {
            let fields: Vec<_> = line.split(';').collect();
            (fields[0], fields[4].parse::<f64>().unwrap() * 1000.0)
        })
        .collect();
    assert_eq!(topology.nodes.len(), 15);
    for node in topology.nodes.values() {
        assert!((node.nominal_ll_volts - expected[node.name.as_deref().unwrap()]).abs() < 1e-10);
    }
    assert!(
        topology
            .buses()
            .iter()
            .all(|bus| bus.terminals == ["1", "2", "3"])
    );
}

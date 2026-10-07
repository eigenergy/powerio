use rusqlite::Connection;

use crate::{DatabaseSnapshot, require_table};

fn snapshot(edit: &str) -> Vec<u8> {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE Version (Version_ID INTEGER, Version_No REAL, Calc_Type INTEGER);
         INSERT INTO Version VALUES (1,14.8,1);
         CREATE TABLE Variant (Variant_ID INTEGER, ParentVariant_ID INTEGER, Flag_Variant INTEGER);
         INSERT INTO Variant VALUES (1,NULL,1);
         CREATE TABLE Node (Node_ID INTEGER, Variant_ID INTEGER);
         INSERT INTO Node VALUES (100,1),(500,1);
         CREATE TABLE Element (Element_ID INTEGER, Variant_ID INTEGER, Type TEXT);
         INSERT INTO Element VALUES (90,1,'Line');
         CREATE TABLE Terminal (Terminal_ID INTEGER, Element_ID INTEGER, Node_ID INTEGER,
                                TerminalNo INTEGER, Variant_ID INTEGER);
         INSERT INTO Terminal VALUES (70,90,100,1,1),(80,90,500,2,1);",
        )
        .unwrap();
    connection.execute_batch(edit).unwrap();
    connection.serialize("main").unwrap().to_vec()
}

#[test]
fn identity_validation_is_independent_of_electrical_profile_and_result_tables() {
    // No sequence, phase, load or calculation settings are required by the
    // shared schema. Their interpretation belongs to the selected adapter.
    let bare = DatabaseSnapshot::decode(&snapshot(""), None).unwrap();
    let with_results = DatabaseSnapshot::decode(
        &snapshot(
            "CREATE TABLE LFNodeResult (Node_ID INTEGER);
         CREATE TABLE ULFNodeResult (Node_ID INTEGER);
         INSERT INTO LFNodeResult VALUES (100);
         INSERT INTO ULFNodeResult VALUES (500);",
        ),
        None,
    )
    .unwrap();
    assert_eq!(bare.version.to_bits(), 14.8_f64.to_bits());
    assert_eq!(bare.variant, 1);
    assert_eq!(bare.nodes.into_iter().collect::<Vec<_>>(), [100, 500]);
    assert_eq!(bare.elements, with_results.elements);
    assert_eq!(bare.terminals, with_results.terminals);
    assert_eq!(bare.terminals[&80].node, 500);
    assert_eq!(bare.terminals[&80].position, 2);
}

#[test]
fn identities_are_variant_local_and_selection_is_explicit() {
    let bytes = snapshot(
        "INSERT INTO Variant VALUES (2,0,1);
         INSERT INTO Node VALUES (100,2),(600,2);
         INSERT INTO Element VALUES (90,2,'Load');
         INSERT INTO Terminal VALUES (70,90,600,1,2);",
    );
    assert!(DatabaseSnapshot::decode(&bytes, None).is_err());
    let one = DatabaseSnapshot::decode(&bytes, Some(1)).unwrap();
    let two = DatabaseSnapshot::decode(&bytes, Some(2)).unwrap();
    assert_eq!(one.terminals[&70].node, 100);
    assert_eq!(two.terminals[&70].node, 600);
    assert_eq!(two.elements[&90], "Load");
    assert!(DatabaseSnapshot::decode(&bytes, Some(3)).is_err());
}

#[test]
fn query_only_and_attachment_limits_survive_shared_ownership() {
    let bytes = snapshot("");
    let before = bytes.clone();
    let database = DatabaseSnapshot::decode(&bytes, None).unwrap();
    assert!(database.connection.execute("DELETE FROM Node", []).is_err());
    assert!(
        database
            .connection
            .execute("ATTACH DATABASE ':memory:' AS other", [])
            .is_err()
    );
    require_table(&database.connection, "Node", &["Node_ID", "Variant_ID"]).unwrap();
    assert!(require_table(&database.connection, "Node", &["missing"]).is_err());
    assert_eq!(bytes, before);
}

#[test]
fn schema_and_identity_failures_do_not_admit_partial_snapshots() {
    for edit in [
        "UPDATE Version SET Version_No=11.5",
        "UPDATE Version SET Calc_Type=2",
        "INSERT INTO Version VALUES (2,14.8,1)",
        "UPDATE Variant SET ParentVariant_ID=2",
        "INSERT INTO Node VALUES (100,1)",
        "INSERT INTO Element VALUES (90,1,'Load')",
        "UPDATE Terminal SET Node_ID=999 WHERE Terminal_ID=70",
        "UPDATE Terminal SET TerminalNo=1 WHERE Terminal_ID=80",
        "INSERT INTO Terminal VALUES (70,90,100,3,1)",
        "ALTER TABLE Node RENAME TO NodeData; CREATE VIEW Node AS SELECT * FROM NodeData",
    ] {
        assert!(
            DatabaseSnapshot::decode(&snapshot(edit), None).is_err(),
            "{edit}"
        );
    }
    let mut bytes = snapshot("");
    bytes[18] = 2;
    assert!(DatabaseSnapshot::decode(&bytes, None).is_err());
}

#[test]
fn observed_modern_sqlite_schemas_share_identity_columns_but_unknown_versions_fail() {
    for version in [15.5, 16.0] {
        let db = DatabaseSnapshot::decode(
            &snapshot(&format!(
                "UPDATE Version SET Version_No={version}; UPDATE Element SET Type='Line    ';"
            )),
            None,
        )
        .unwrap();
        assert_eq!(db.elements[&90], "Line");
        assert_eq!(db.terminals[&70].node, 100);
    }
    for sql in [
        "UPDATE Version SET Version_No=15.6",
        "UPDATE Element SET Type='   '",
    ] {
        assert!(DatabaseSnapshot::decode(&snapshot(sql), None).is_err());
    }
}

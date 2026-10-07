use powerio_sincal::DatabaseSnapshot;
use rusqlite::types::ValueRef;
use serde_json::{Value, json};

use super::{mapping_tests::network_database, schema::NativeDatabase};

// Original synthetic SQL -> typed acquisition records, exercising the same
// boundary as MDB Tools without inventing a native SQLite schema-11.5 export.
fn legacy(edit: &str) -> NativeDatabase {
    let snapshot = DatabaseSnapshot::decode(&network_database(edit), None).unwrap();
    let names: Vec<String> = snapshot
        .connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut tables = Vec::new();
    for name in names {
        let mut statement = snapshot
            .connection
            .prepare(&format!("SELECT * FROM {name}"))
            .unwrap();
        let columns: Vec<_> = statement
            .column_names()
            .iter()
            .map(|n| json!({"name": n, "native_type": "synthetic"}))
            .collect();
        let count = columns.len();
        let mut cursor = statement.query([]).unwrap();
        let mut rows = Vec::new();
        while let Some(row) = cursor.next().unwrap() {
            let mut cells = Vec::new();
            for i in 0..count {
                cells.push(match row.get_ref(i).unwrap() {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => json!(n),
                    ValueRef::Real(n) => json!(n),
                    ValueRef::Text(t) => json!(std::str::from_utf8(t).unwrap()),
                    ValueRef::Blob(_) => panic!("no synthetic blobs"),
                });
            }
            if name == "Version" {
                cells[1] = json!(11.5);
            }
            rows.push(cells);
        }
        tables.push(json!({"name": name, "columns": columns, "rows": rows}));
    }
    let records = json!({
        "format": "powerio-sincal-tables", "version": 1, "transport": "access-mdbtools",
        "source": {"name": "synthetic.mdb", "sha256": "0".repeat(64), "bytes": 1},
        "tools": {"mdb-json": "synthetic", "mdb-schema": "synthetic", "mdb-tables": "synthetic"}, "tables": tables,
        "excluded_tables": [], "absent_requested_tables": [],
    });
    NativeDatabase::from_snapshot(
        DatabaseSnapshot::decode_records(&serde_json::to_vec(&records).unwrap(), None).unwrap(),
    )
    .unwrap()
}

#[test]
fn legacy_defaults_preserve_the_circuit_and_report_their_origin() {
    let explicit = legacy("").network().unwrap();
    let defaulted = legacy(
        "UPDATE VoltageLevel SET Flag_Volt=NULL;
         UPDATE TwoWindingTransformer SET Flag_Tap=NULL;",
    )
    .network()
    .unwrap();
    assert_eq!(explicit.lines(), defaulted.lines());
    assert_eq!(explicit.line_codes(), defaulted.line_codes());
    assert_eq!(explicit.shunts(), defaulted.shunts());
    assert_eq!(explicit.loads(), defaulted.loads());
    assert_eq!(explicit.sources(), defaulted.sources());
    assert_eq!(defaulted.defaulted()["VoltageLevel.1"], ["Flag_Volt"]);
    assert_eq!(defaulted.defaulted()["VoltageLevel.2"], ["Flag_Volt"]);
    assert_eq!(
        defaulted.defaulted()["TwoWindingTransformer.33"],
        ["Flag_Tap"]
    );
    assert!(explicit.defaulted().is_empty());
    let equipment_only = legacy(
        "INSERT INTO VoltageLevel VALUES (3,1,20,20,11,50,NULL);
         UPDATE Element SET VoltLevel_ID=3 WHERE Element_ID=32;",
    )
    .network()
    .unwrap();
    assert_eq!(equipment_only.defaulted()["VoltageLevel.3"], ["Flag_Volt"]);
    assert_eq!(equipment_only.sources(), explicit.sources());
    assert_eq!(
        defaulted.bus("10").unwrap().extras["sincal"]["legacy_voltage_basis"],
        true
    );
}

#[test]
fn legacy_null_defaults_never_replace_missing_rows_columns_or_invalid_values() {
    for edit in [
        "DELETE FROM VoltageLevel WHERE VoltLevel_ID=1",
        "UPDATE Element SET VoltLevel_ID=99 WHERE Element_ID=32",
        "ALTER TABLE VoltageLevel DROP COLUMN Flag_Volt",
        "UPDATE VoltageLevel SET Flag_Volt=2",
        "UPDATE VoltageLevel SET Flag_Volt=0",
        "UPDATE VoltageLevel SET Flag_Volt=1.5",
        "UPDATE TwoWindingTransformer SET Flag_Tap=2",
        "ALTER TABLE TwoWindingTransformer DROP COLUMN Flag_Tap",
        "UPDATE TwoWindingTransformer SET Flag_Tap='bad'",
    ] {
        assert!(legacy(edit).network().is_err(), "accepted {edit}");
    }
    for edit in [
        "UPDATE VoltageLevel SET Flag_Volt=NULL",
        "UPDATE TwoWindingTransformer SET Flag_Tap=NULL",
    ] {
        let modern = NativeDatabase::decode(&network_database(edit), None).unwrap();
        assert!(modern.network().is_err(), "accepted modern {edit}");
    }
}

#[test]
fn legacy_absent_new_controls_are_inactive_but_explicit_nulls_still_fail() {
    let edit = "ALTER TABLE Line DROP COLUMN Flag_Lf;
        ALTER TABLE Line DROP COLUMN ElemLoading_ID;
        ALTER TABLE Infeeder DROP COLUMN Flag_Pctrl;
        ALTER TABLE Infeeder DROP COLUMN Rlf;
        ALTER TABLE Infeeder DROP COLUMN Xlf;
        ALTER TABLE TwoWindingTransformer DROP COLUMN Flag_Lf;
        ALTER TABLE TwoWindingTransformer DROP COLUMN Flag_Boost;
        ALTER TABLE TwoWindingTransformer DROP COLUMN C01;
        ALTER TABLE TwoWindingTransformer DROP COLUMN C02;
        ALTER TABLE TwoWindingTransformer DROP COLUMN CtrlRange_ID;
        ALTER TABLE TwoWindingTransformer DROP COLUMN Ctrl_OpSer_ID;
        ALTER TABLE TwoWindingTransformer DROP COLUMN Ctrl_OpPnt_ID;
        ALTER TABLE TwoWindingTransformer DROP COLUMN ElemLoading_ID;";
    let net = legacy(edit).network().unwrap();
    let expected = legacy("").network().unwrap();
    assert_eq!(
        serde_json::to_value(net).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert!(
        NativeDatabase::decode(&network_database(edit), None)
            .unwrap()
            .network()
            .is_err()
    );
    for edit in [
        "UPDATE Line SET Flag_Lf=NULL",
        "UPDATE Infeeder SET Flag_Pctrl=NULL",
        "UPDATE Infeeder SET Rlf=NULL",
        "UPDATE TwoWindingTransformer SET Flag_Boost=NULL",
        "UPDATE TwoWindingTransformer SET C01=NULL",
        "ALTER TABLE Line DROP COLUMN Flag_Ground",
    ] {
        assert!(legacy(edit).network().is_err(), "accepted {edit}");
    }
}

#[test]
fn materialized_standard_types_use_stored_parameters_and_retain_the_selection() {
    let setup = "ALTER TABLE Line ADD COLUMN Flag_Typ_ID INTEGER DEFAULT 1;
        ALTER TABLE TwoWindingTransformer ADD COLUMN Flag_Typ_ID INTEGER DEFAULT 2;
        UPDATE Line SET Typ_ID=101;
        UPDATE TwoWindingTransformer SET Typ_ID=202;";
    let net = legacy(setup).network().unwrap();
    let expected = legacy("").network().unwrap();
    assert_eq!(net.line_codes(), expected.line_codes());
    assert_eq!(net.shunts(), expected.shunts());
    assert_eq!(
        net.extras()["sincal"]["materialized_types"]["30"]["type_id"],
        101
    );
    assert_eq!(
        net.extras()["sincal"]["materialized_types"]["33"]["scope"],
        2
    );
    for edit in [
        "UPDATE Line SET r=NULL",
        "UPDATE Line SET Flag_Typ_ID=0",
        "UPDATE Line SET Flag_Typ_ID=NULL",
        "UPDATE TwoWindingTransformer SET Sn=NULL",
        "UPDATE TwoWindingTransformer SET Flag_Typ_ID=3",
    ] {
        assert!(
            legacy(&format!("{setup}{edit}")).network().is_err(),
            "accepted {edit}"
        );
    }
    assert!(legacy("UPDATE Line SET Typ_ID=1").network().is_err());
}

#[test]
fn absent_coupling_references_preserve_lines_but_active_references_require_mapping() {
    let expected = legacy("").network().unwrap();
    let no_reference = legacy("UPDATE Line SET CoupData_ID=NULL")
        .network()
        .unwrap();
    assert_eq!(no_reference.line_codes(), expected.line_codes());
    for edit in [
        "UPDATE Line SET CoupData_ID=5",
        "UPDATE Line SET CoupData_ID=-1",
        "ALTER TABLE Line DROP COLUMN CoupData_ID",
    ] {
        assert!(legacy(edit).network().is_err(), "accepted {edit}");
    }
}

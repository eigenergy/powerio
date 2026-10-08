use powerio_sincal::DatabaseSnapshot;
use rusqlite::types::ValueRef;
use serde_json::{Value, json};

use super::{mapping_tests::network_database, schema::NativeDatabase};

// Original synthetic SQL -> typed acquisition records, exercising the same
// boundary as MDB Tools without inventing a native SQLite schema-11.5 export.
pub(super) fn legacy(edit: &str) -> NativeDatabase {
    acquired_version(edit, 11.5)
}

pub(super) fn acquired_records(edit: &str, version: f64) -> Value {
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
                cells[1] = json!(version);
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
    records
}

pub(super) fn acquired_version(edit: &str, version: f64) -> NativeDatabase {
    let records = acquired_records(edit, version);
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
        "UPDATE Line SET Flag_Typ_ID=3",
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

#[test]
fn finite_source_sequence_requires_the_current_variant_local_input_selection() {
    let base = "ALTER TABLE CalcParameter ADD COLUMN Flag_ScType INTEGER DEFAULT 1;
        UPDATE Infeeder SET R0=0.3, X0=0.6;";
    let net = legacy(base).network().unwrap();
    crate::require_electrical_readiness(&net).unwrap();
    let source = &net.sources()[0];
    assert_eq!(source.reference_terminal.as_deref(), Some("star"));
    let shunt = net
        .shunts()
        .iter()
        .find(|s| s.name == "sincal:infeeder:32:zero-sequence")
        .unwrap();
    assert_eq!(shunt.terminal_map, ["star"]);
    assert_eq!(shunt.bus, source.bus);
    assert!((shunt.g[0][0] - 2.0).abs() < 1e-12);
    assert!((shunt.b[0][0] + 4.0).abs() < 1e-12);
    for edit in [
        "UPDATE CalcParameter SET Flag_ScType=2",
        "UPDATE CalcParameter SET Flag_ScType=3",
        "UPDATE CalcParameter SET Flag_ScType=NULL",
        "DELETE FROM CalcParameter",
        "INSERT INTO CalcParameter SELECT * FROM CalcParameter",
    ] {
        assert!(
            legacy(&format!("{base}{edit}")).network().is_err(),
            "accepted {edit}"
        );
    }
    assert!(
        legacy("UPDATE Infeeder SET R0=0.3,X0=0.6")
            .network()
            .is_err()
    );
    assert!(
        NativeDatabase::decode(&network_database(base), None)
            .unwrap()
            .network()
            .is_err()
    );
    let restored: crate::MulticonductorNetwork =
        serde_json::from_value(serde_json::to_value(&net).unwrap()).unwrap();
    assert_eq!(restored.sources(), net.sources());
    assert_eq!(restored.shunts(), net.shunts());
}

#[test]
#[ignore = "exports original synthetic finite-source circuits for the independent OpenDSS check"]
fn export_source_zero_sequence_oracle() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("POWERIO_SINCAL_SOURCE_ORACLE_DIR")
            .expect("POWERIO_SINCAL_SOURCE_ORACLE_DIR"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    for (name, r, x) in [
        ("resistive", 0.3, 0.0),
        ("inductive", 0.3, 0.6),
        ("capacitive", 0.3, -0.6),
    ] {
        let net = legacy(&format!(
            "ALTER TABLE CalcParameter ADD COLUMN Flag_ScType INTEGER DEFAULT 1;
             DELETE FROM Terminal WHERE Element_ID IN (30,33);
             DELETE FROM Element WHERE Element_ID IN (30,33);
             UPDATE Terminal SET Node_ID=10 WHERE Element_ID=31;
             DELETE FROM Node WHERE Node_ID IN (20,30);
             UPDATE VoltageLevel SET Un=0.4;
             UPDATE Infeeder SET Ug=0.4,delta=0,R0={r},X0={x};
             UPDATE Load SET fP=1,fQ=1,P1=0.002,P2=0.004,P3=0.006,Q1=0.001,Q2=0.002,Q3=0.003;"
        ))
        .network()
        .unwrap();
        crate::require_electrical_readiness(&net).unwrap();
        let value = json!({"network": net, "native_input": {"r0": r, "x0": x,
            "voltage_ll_v": 400.0, "phase_p_w": [2000.0,4000.0,6000.0], "phase_q_var": [1000.0,2000.0,3000.0]}});
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn legacy_inactive_center_tap_defaults_keep_native_data_and_tap_provenance() {
    let setup = "ALTER TABLE TwoWindingTransformer ADD COLUMN uk_Ct REAL;
        ALTER TABLE TwoWindingTransformer ADD COLUMN ur_Ct REAL;
        ALTER TABLE TwoWindingTransformer ADD COLUMN StpCt_ID INTEGER;";
    let expected = legacy(setup).network().unwrap();
    for edit in [
        "UPDATE TwoWindingTransformer SET Flag_Ct=NULL",
        "UPDATE TwoWindingTransformer SET Flag_Ct=NULL,uk_Ct=0,ur_Ct=0",
        "UPDATE TwoWindingTransformer SET Flag_Ct=NULL,Flag_Tap=NULL",
    ] {
        let db = legacy(&format!("{setup}{edit}"));
        let mapped = db.network().unwrap();
        assert_eq!(expected.transformers(), mapped.transformers());
        assert_eq!(expected.buses(), mapped.buses());
        assert_eq!(expected.switches(), mapped.switches());
        assert_eq!(expected.shunts(), mapped.shunts());
        let defaults = &mapped.defaulted()["TwoWindingTransformer.33"];
        assert!(defaults.contains(&"Flag_Ct"));
        assert_eq!(defaults.contains(&"Flag_Tap"), edit.contains("Flag_Tap"));
        let native: Option<i64> = db
            .connection
            .query_row(
                "SELECT Flag_Ct FROM TwoWindingTransformer WHERE Element_ID=33",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(native, None);
    }
    for edit in [
        "UPDATE TwoWindingTransformer SET Flag_Ct=1",
        "UPDATE TwoWindingTransformer SET Flag_Ct=2",
        "UPDATE TwoWindingTransformer SET Flag_Ct=0.5",
        "UPDATE TwoWindingTransformer SET Flag_Ct='bad'",
        "UPDATE TwoWindingTransformer SET Flag_Ct=NULL,uk_Ct=16",
        "UPDATE TwoWindingTransformer SET Flag_Ct=NULL,StpCt_ID=1",
        "UPDATE TwoWindingTransformer SET Flag_Ct=NULL,ur_Ct=-0.1",
        "UPDATE TwoWindingTransformer SET Flag_Ct=NULL; ALTER TABLE TwoWindingTransformer DROP COLUMN uk_Ct",
        "ALTER TABLE TwoWindingTransformer DROP COLUMN Flag_Ct",
    ] {
        assert!(
            legacy(&format!("{setup}{edit}")).network().is_err(),
            "accepted {edit}"
        );
    }
    let modern = NativeDatabase::decode(
        &network_database(&format!(
            "{setup} UPDATE TwoWindingTransformer SET Flag_Ct=NULL"
        )),
        None,
    )
    .unwrap();
    assert!(modern.network().is_err());
}

#[test]
fn legacy_transformer_optional_defaults_preserve_the_explicit_nominal_circuit() {
    let fields = [
        "Flag_roh",
        "Flag_ConNode",
        "Flag_Macro",
        "AddRotate",
        "roh",
        "rohm",
        "ukr",
        "alpha",
        "phi",
        "Vfe",
        "i0",
    ];
    let explicit = fields
        .iter()
        .map(|field| {
            format!(
                "{field}={}",
                i32::from(matches!(*field, "Flag_ConNode" | "Flag_roh"))
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let absent = fields
        .iter()
        .map(|field| format!("{field}=NULL"))
        .collect::<Vec<_>>()
        .join(",");
    let expected = legacy(&format!("UPDATE TwoWindingTransformer SET {explicit}"))
        .network()
        .unwrap();
    let db = legacy(&format!("UPDATE TwoWindingTransformer SET {absent}"));
    let actual = db.network().unwrap();
    assert_eq!(actual.buses(), expected.buses());
    assert_eq!(actual.switches(), expected.switches());
    assert_eq!(actual.shunts(), expected.shunts());
    assert_eq!(actual.defaulted()["TwoWindingTransformer.33"], fields);
    for field in fields {
        let raw: rusqlite::types::Value = db
            .connection
            .query_row(
                &format!("SELECT {field} FROM TwoWindingTransformer"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(raw, rusqlite::types::Value::Null);
        let missing = format!("ALTER TABLE TwoWindingTransformer DROP COLUMN {field}");
        assert!(
            legacy(&missing).network().is_err(),
            "accepted missing {field}"
        );
        let invalid = format!("UPDATE TwoWindingTransformer SET {field}='bad'");
        assert!(legacy(&invalid).network().is_err(), "accepted text {field}");
        let modern = NativeDatabase::decode(
            &network_database(&format!("UPDATE TwoWindingTransformer SET {field}=NULL")),
            None,
        )
        .unwrap();
        assert!(modern.network().is_err(), "accepted modern NULL {field}");
    }
    for field in ["Un1", "Un2", "Sn", "uk", "ur", "VecGrp"] {
        assert!(
            legacy(&format!("UPDATE TwoWindingTransformer SET {field}=NULL"))
                .network()
                .is_err(),
            "defaulted required {field}"
        );
    }
    assert!(
        legacy("UPDATE TwoWindingTransformer SET Vfe=1,i0=NULL")
            .network()
            .is_err()
    );
    assert!(
        legacy("UPDATE TwoWindingTransformer SET Flag_Tap=1,roh1=NULL")
            .network()
            .is_err()
    );
    let individual =
        legacy("UPDATE TwoWindingTransformer SET Flag_Tap=1,roh=NULL,roh1=1,roh2=1,roh3=1")
            .network()
            .unwrap();
    assert!(
        !individual
            .defaulted()
            .contains_key("TwoWindingTransformer.33")
    );
}

#[test]
fn access_12_8_retains_its_own_control_presence_and_voltage_provenance() {
    let edit = "UPDATE VoltageLevel SET Flag_Volt=NULL;
        ALTER TABLE Line DROP COLUMN Flag_Lf;
        ALTER TABLE Line DROP COLUMN ElemLoading_ID;
        ALTER TABLE Infeeder DROP COLUMN Flag_Pctrl;
        ALTER TABLE Infeeder DROP COLUMN Rlf;
        ALTER TABLE Infeeder DROP COLUMN Xlf;
        ALTER TABLE TwoWindingTransformer DROP COLUMN Flag_Lf;
        ALTER TABLE TwoWindingTransformer DROP COLUMN C01;
        ALTER TABLE TwoWindingTransformer DROP COLUMN C02;
        ALTER TABLE TwoWindingTransformer DROP COLUMN ElemLoading_ID;";
    let db = acquired_version(edit, 12.8);
    let net = db.network().unwrap();
    let explicit = acquired_version("", 12.8).network().unwrap();
    assert_eq!(net.lines(), explicit.lines());
    assert_eq!(net.line_codes(), explicit.line_codes());
    assert_eq!(net.loads(), explicit.loads());
    assert_eq!(net.sources(), explicit.sources());
    assert_eq!(net.shunts(), explicit.shunts());
    assert_eq!(net.defaulted()["VoltageLevel.1"], ["Flag_Volt"]);
    assert_eq!(net.defaulted()["VoltageLevel.2"], ["Flag_Volt"]);
    let original: Option<i64> = db
        .connection
        .query_row(
            "SELECT Flag_Volt FROM VoltageLevel WHERE VoltLevel_ID=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(original, None);
    for invalid in [
        "ALTER TABLE TwoWindingTransformer DROP COLUMN Flag_Boost",
        "ALTER TABLE TwoWindingTransformer DROP COLUMN CtrlRange_ID",
        "ALTER TABLE TwoWindingTransformer DROP COLUMN Ctrl_OpSer_ID",
        "ALTER TABLE TwoWindingTransformer DROP COLUMN Ctrl_OpPnt_ID",
        "UPDATE TwoWindingTransformer SET Flag_Boost=NULL",
        "UPDATE TwoWindingTransformer SET Flag_Boost=1",
        "UPDATE TwoWindingTransformer SET CtrlRange_ID=5",
        "UPDATE TwoWindingTransformer SET Flag_Tap=NULL",
        "UPDATE TwoWindingTransformer SET Vfe=NULL",
        "UPDATE Infeeder SET Flag_Pctrl=NULL",
        "UPDATE Infeeder SET Rlf=NULL",
        "UPDATE Line SET Flag_Lf=NULL",
        "ALTER TABLE VoltageLevel DROP COLUMN Flag_Volt",
    ] {
        assert!(
            acquired_version(invalid, 12.8).network().is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn no_type_status_ignores_retained_library_ids_but_keeps_materialized_values() {
    for version in [11.5, 12.8] {
        let expected = acquired_version("", version).network().unwrap();
        let setup = "ALTER TABLE Line ADD COLUMN Flag_Typ_ID INTEGER DEFAULT 0;
            ALTER TABLE TwoWindingTransformer ADD COLUMN Flag_Typ_ID INTEGER DEFAULT 0;
            ALTER TABLE Infeeder ADD COLUMN Flag_Typ_ID INTEGER DEFAULT 0;
            UPDATE Line SET Typ_ID=101;
            UPDATE TwoWindingTransformer SET Typ_ID=202;
            UPDATE Infeeder SET Typ_ID=303;";
        let net = acquired_version(setup, version).network().unwrap();
        assert_eq!(net.line_codes(), expected.line_codes());
        assert_eq!(net.shunts(), expected.shunts());
        assert_eq!(net.sources(), expected.sources());
        for id in ["30", "32", "33"] {
            assert_eq!(net.extras()["sincal"]["materialized_types"][id]["scope"], 0);
        }
        for scope in [1, 2] {
            assert!(
                acquired_version(
                    &format!("{setup} UPDATE Infeeder SET Flag_Typ_ID={scope}"),
                    version
                )
                .network()
                .is_ok()
            );
        }
        for edit in [
            "Flag_Typ_ID=NULL",
            "Flag_Typ_ID=3",
            "Flag_Typ_ID=-1",
            "Ug=NULL",
        ] {
            assert!(
                acquired_version(&format!("{setup} UPDATE Infeeder SET {edit}"), version)
                    .network()
                    .is_err()
            );
        }
    }
}

#[test]
fn source_sequence_ratios_use_fault_impedance_even_for_an_ideal_load_flow_source() {
    let base = "ALTER TABLE CalcParameter ADD COLUMN Flag_ScType INTEGER DEFAULT 1;
        UPDATE Element SET Flag_Input=7 WHERE Element_ID=32;
        UPDATE Infeeder SET Flag_Typ=1,R=3,X=4,Flag_Z0_Input=1,Z0_Z1=2,R0_X0=0.75,R0=999,X0=999;";
    let ratio = legacy(base).network().unwrap();
    let direct = legacy(&format!(
        "{base} UPDATE Infeeder SET Flag_Z0_Input=2,R0=6,X0=8;"
    ))
    .network()
    .unwrap();
    assert_eq!(ratio.sources(), direct.sources());
    assert_eq!(ratio.shunts(), direct.shunts());
    let same = legacy(&format!("{base} UPDATE Infeeder SET Flag_Z0_Input=3;"))
        .network()
        .unwrap();
    let direct_same = legacy(&format!(
        "{base} UPDATE Infeeder SET Flag_Z0_Input=2,R0=3,X0=4;"
    ))
    .network()
    .unwrap();
    assert_eq!(same.shunts(), direct_same.shunts());
    let ideal_zero = legacy(&format!("{base} UPDATE Infeeder SET Z0_Z1=0;"))
        .network()
        .unwrap();
    assert!(ideal_zero.sources()[0].reference_terminal.is_none());
    let db = legacy(base);
    let before = db.infeeder_input(32).unwrap();
    assert!(matches!(
        before.grounding,
        super::infeeder::SourceGrounding::Solid(
            super::infeeder::SourceZeroSequence::MagnitudeRatio { .. }
        )
    ));
    for edit in [
        "UPDATE Infeeder SET Flag_Typ=2",
        "UPDATE Infeeder SET R=NULL",
        "UPDATE Infeeder SET R=-1",
        "UPDATE Infeeder SET R=0,X=0",
        "UPDATE Infeeder SET Z0_Z1=1e308",
        "UPDATE Element SET Flag_Input=6 WHERE Element_ID=32",
        "UPDATE CalcParameter SET Flag_ScType=2",
        "UPDATE CalcParameter SET Flag_ScType=3",
    ] {
        assert!(
            legacy(&format!("{base}{edit}")).network().is_err(),
            "accepted {edit}"
        );
    }
}

#[test]
fn legacy_fixed_tap_status_does_not_resolve_partial_mixed_winding_physics() {
    for group in [14, 59] {
        let db = legacy(&format!(
            "UPDATE TwoWindingTransformer SET VecGrp={group},Flag_roh=NULL;
             UPDATE Terminal SET Flag_Terminal=1 WHERE Element_ID=33;"
        ));
        let error = db
            .network()
            .expect_err("partial mixed windings remain unresolved")
            .to_string();
        assert!(error.contains("partial transformer windings"), "{error}");
    }
    for value in ["0", "2", "6", "-1", "0.5", "'bad'"] {
        assert!(
            legacy(&format!(
                "UPDATE TwoWindingTransformer SET Flag_roh={value}"
            ))
            .network()
            .is_err(),
            "accepted {value}"
        );
    }
    assert!(
        acquired_version("UPDATE TwoWindingTransformer SET Flag_roh=NULL", 12.8)
            .network()
            .is_err()
    );
}

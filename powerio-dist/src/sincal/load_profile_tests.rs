use super::{
    legacy_tests::legacy, load::PowerInput, mapping_tests::network_database, schema::NativeDatabase,
};

const PROFILE: &str = "
    UPDATE Load SET Flag_Lf=15, fP=1, fQ=1, fS=1, DayOpSer_ID=7;
    CREATE TABLE OpSer (OpSer_ID INTEGER, Variant_ID INTEGER, Flag_Variant INTEGER,
      Flag_Typ INTEGER, Flag_Ser INTEGER, BaseT REAL, Power_a1 REAL, Power_b1 REAL,
      Reduce_a2 REAL, Reduce_b2 REAL);
    INSERT INTO OpSer VALUES (7,1,1,3,1,0,0,0,0,0);
    CREATE TABLE OpSerVal (OpSerVal_ID INTEGER, OpSer_ID INTEGER, Variant_ID INTEGER,
      Flag_Variant INTEGER, OpTime REAL, Flag_Curve INTEGER, Factor REAL, P REAL, Q REAL, Op_ID INTEGER);
    INSERT INTO OpSerVal VALUES (1,7,1,1,0,1,NULL,6,-3,NULL),(2,7,1,1,12,2,NULL,18,9,NULL);
";

fn powers(db: &NativeDatabase, time: f64) -> (f64, f64) {
    let input = db.load_input_at(31, time).unwrap();
    assert_eq!(input.operating_series, [None; 3]);
    match input.power {
        PowerInput::DeltaTotal { p, q } => (p, q),
        _ => panic!("snapshot changed delta connection"),
    }
}

#[test]
fn exact_interpolated_discrete_and_cyclic_daily_powers_use_si() {
    let db = legacy(PROFILE);
    let apparent = legacy(&format!("{PROFILE} UPDATE Load SET Flag_Lf=4, fS=0.18;"));
    assert_eq!(
        apparent.load_input_at(31, 0.0).unwrap().power,
        PowerInput::Total {
            p: 6000.0,
            q: -3000.0
        }
    );
    assert!(db.network().is_err()); // No implicit choice of midnight.
    for (hours, expected) in [
        (0.0, (6000.0, -3000.0)),
        (6.0, (12000.0, 3000.0)),
        (12.0, (18000.0, 9000.0)),
        (23.5, (18000.0, 9000.0)),
        (24.0, (6000.0, -3000.0)),
        (30.0, (12000.0, 3000.0)),
        (60.0, (18000.0, 9000.0)),
    ] {
        assert_eq!(powers(&db, hours), expected);
    }
    let db = legacy(&format!(
        "{PROFILE} UPDATE OpSerVal SET Flag_Curve=1 WHERE OpTime=12;"
    ));
    assert_eq!(powers(&db, 18.0), (12000.0, 3000.0));
    let custom = legacy(&format!(
        "{PROFILE} UPDATE OpSer SET BaseT=8; UPDATE OpSerVal SET OpTime=4 WHERE OpTime=12;"
    ));
    assert_eq!(powers(&custom, 10.0), (12000.0, 3000.0));
}

#[test]
fn snapshot_uses_production_assembler_and_retains_selection_without_editing_native_input() {
    let db = legacy(PROFILE);
    let before = db.load_input(31).unwrap().power;
    let net = db.network_at(6.0).unwrap();
    let load = &net.loads()[0];
    assert_eq!(load.configuration, crate::Configuration::Delta);
    assert_eq!(load.p_nom, vec![4000.0; 3]);
    assert_eq!(load.q_nom, vec![1000.0; 3]);
    assert_eq!(load.extras["sincal_profile"]["profile"], 7);
    assert_eq!(load.extras["sincal_profile"]["requested_hours"], 6.0);
    assert_eq!(net.extras()["sincal"]["snapshot_hours"], 6.0);
    assert_eq!(db.load_input(31).unwrap().power, before);
    assert!(db.network().is_err());
    let report = db.mapping_report_at(Some(6.0)).unwrap();
    assert!(report.all_components_map());
    assert_eq!(report.snapshot_hours, Some(6.0));
    // A separate malformed element still fails the entire network snapshot.
    let bad = legacy(&format!("{PROFILE} UPDATE Line SET r=-1;"));
    assert!(bad.network_at(6.0).is_err());
}

#[test]
fn unprofiled_loads_and_non_loads_do_not_change_with_snapshot_selection() {
    let db = legacy("");
    let base = db.network().unwrap();
    let timed = db.network_at(24.0).unwrap();
    assert_eq!(base.loads(), timed.loads());
    assert_eq!(base.lines(), timed.lines());
    assert_eq!(base.shunts(), timed.shunts());
    assert_eq!(base.sources(), timed.sources());
    for t in [f64::NAN, f64::INFINITY, -1.0] {
        assert!(db.network_at(t).is_err());
        assert!(db.mapping_report_at(Some(t)).is_err());
    }
}

#[test]
fn malformed_or_unverified_profiles_reject_without_defaults_or_repairs() {
    for edit in [
        "DELETE FROM OpSer;",
        "DELETE FROM OpSerVal;",
        "INSERT INTO OpSer SELECT * FROM OpSer;",
        "INSERT INTO OpSerVal SELECT * FROM OpSerVal WHERE OpTime=0;",
        "UPDATE OpSerVal SET OpSerVal_ID=1;",
        "UPDATE OpSerVal SET OpTime=NULL;",
        "UPDATE OpSerVal SET OpTime=-1 WHERE OpTime=0;",
        "UPDATE OpSerVal SET OpTime=25 WHERE OpTime=12;",
        "UPDATE OpSerVal SET OpTime=1 WHERE OpTime=0;",
        "UPDATE OpSerVal SET Flag_Curve=3;",
        "UPDATE OpSerVal SET Op_ID=1;",
        "UPDATE OpSerVal SET P=1e308;",
        "UPDATE OpSerVal SET Q=NULL;",
        "UPDATE OpSerVal SET Flag_Variant=0;",
        "UPDATE OpSer SET Flag_Variant=0;",
        "UPDATE OpSer SET Flag_Typ=1;",
        "UPDATE OpSer SET Flag_Ser=2;",
        "UPDATE OpSer SET BaseT=-24;",
        "UPDATE OpSer SET BaseT=NULL;",
        "UPDATE OpSer SET Reduce_b2=1;",
        "UPDATE OpSer SET Power_a1=1;",
        "UPDATE Load SET fP=2;",
        "UPDATE Load SET fQ=NULL;",
        "UPDATE Load SET Flag_Lf=13;",
        "UPDATE Load SET WeekOpSer_ID=7;",
        "UPDATE Load SET DayOpSer_ID=99;",
        "ALTER TABLE OpSer DROP COLUMN BaseT;",
        "ALTER TABLE OpSer RENAME TO Original; CREATE VIEW OpSer AS SELECT * FROM Original;",
    ] {
        let db = legacy(&format!("{PROFILE}{edit}"));
        assert!(db.load_input_at(31, 0.0).is_err(), "accepted {edit}");
    }
    let modern = NativeDatabase::decode(&network_database(PROFILE), None).unwrap();
    assert!(modern.network_at(0.0).is_err());
}

#[test]
fn cyclic_endpoint_and_variant_identity_are_checked() {
    let good = "INSERT INTO OpSerVal VALUES (3,7,1,1,24,1,NULL,6,-3,NULL);";
    let db = legacy(&format!("{PROFILE}{good}"));
    assert_eq!(powers(&db, 24.0), (6000.0, -3000.0));
    assert!(
        legacy(&format!(
            "{PROFILE}{good} UPDATE OpSerVal SET P=7 WHERE OpTime=24;"
        ))
        .network_at(24.0)
        .is_err()
    );
    let other="INSERT INTO OpSer SELECT OpSer_ID,2,Flag_Variant,Flag_Typ,Flag_Ser,BaseT,Power_a1,Power_b1,Reduce_a2,Reduce_b2 FROM OpSer;
      INSERT INTO OpSerVal SELECT OpSerVal_ID,OpSer_ID,2,Flag_Variant,OpTime,Flag_Curve,Factor,999,999,Op_ID FROM OpSerVal;";
    assert_eq!(
        powers(&legacy(&format!("{PROFILE}{other}")), 0.0),
        (6000.0, -3000.0)
    );
}

#[test]
#[ignore = "exports external CSIRO daily load snapshots; not complete native networks"]
fn export_csiro_daily_loads() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("POWERIO_SINCAL_PROFILE_RECORDS").expect("record directory"),
    );
    let output = std::env::var_os("POWERIO_SINCAL_PROFILE_EXPORT").expect("export path");
    let mut cases = Vec::new();
    for case in [1, 4, 7] {
        let bytes = std::fs::read(directory.join(format!("representative{case:02}.json"))).unwrap();
        let db = NativeDatabase::from_snapshot(
            powerio_sincal::DatabaseSnapshot::decode_records(&bytes, Some(1)).unwrap(),
        )
        .unwrap();
        let nodes = db.node_inputs().unwrap();
        let mut components = Vec::new();
        let mut rejected = Vec::new();
        for (&id, kind) in &db.elements {
            if kind != "Load" {
                continue;
            }
            let raw = db.load_input(id).unwrap();
            if raw.terminal.connection.phases().unwrap().len() != 2
                || raw.operating_series == [None; 3]
            {
                continue;
            }
            if let Err(error) = db.load_input_at(id, 0.0) {
                rejected.push(serde_json::json!({"element":id,"profile":raw.operating_series[0],"error":error.to_string()}));
                continue;
            }
            let node = &nodes[&raw.terminal.node];
            let bus = crate::DistBus::new(
                node.id.to_string(),
                ["1", "2", "3"].map(str::to_owned).to_vec(),
            );
            for hours in [0.0, 0.25, 0.5, 23.75, 24.0] {
                let input = db.load_input_at(id, hours).unwrap();
                let circuit = input.circuit(&bus, node.nominal_ll_volts).unwrap();
                components.push(serde_json::json!({"element":id,"hours":hours,"load":circuit.load,"bus":circuit.bus,"switch":circuit.switch}));
            }
        }
        assert!(!components.is_empty());
        cases.push(serde_json::json!({"case":case,"components":components,"rejected":rejected}));
    }
    std::fs::write(output,serde_json::to_vec_pretty(&serde_json::json!({"scope":"profiled phase-pair load components only; no complete network","cases":cases})).unwrap()).unwrap();
}

#[test]
fn selected_profile_preserves_phase_connections_and_each_voltage_model() {
    for model in [1, 2, 3] {
        for (connection, branches, delta) in [(1, 1_u8, false), (4, 1, true), (7, 3, false)] {
            let db = legacy(&format!(
                "{PROFILE} UPDATE Load SET Flag_Lf=1, Flag_LoadType={model}; UPDATE Terminal SET Flag_Terminal={connection} WHERE Element_ID=31;"
            ));
            let net = db.network_at(6.0).unwrap();
            let load = &net.loads()[0];
            assert_eq!(
                load.p_nom,
                vec![12000.0 / f64::from(branches); usize::from(branches)]
            );
            assert_eq!(
                load.q_nom,
                vec![3000.0 / f64::from(branches); usize::from(branches)]
            );
            assert_eq!(load.terminal_map.last().unwrap() == "0", !delta);
            assert!(matches!(
                (&load.voltage_model, model),
                (crate::DistLoadVoltageModel::ConstantImpedance { .. }, 1)
                    | (crate::DistLoadVoltageModel::ConstantPower { .. }, 2)
                    | (crate::DistLoadVoltageModel::ConstantCurrent { .. }, 3)
            ));
        }
    }
}

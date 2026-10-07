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

const RELATIVE: &str = "UPDATE OpSer SET Flag_Typ=1,Reduce_b2=1;
    UPDATE OpSerVal SET Factor=OpTime/6, P=NULL, Q=NULL;";

fn relative_unequal(mode: i64, model: i64, extra: &str) -> NativeDatabase {
    legacy(&format!(
        "{PROFILE}{RELATIVE}
        UPDATE Load SET Flag_Lf={mode},Flag_LoadType={model},fP=2,fQ=3,
        P1=.003,P2=.006,P3=.009,Q1=-.001,Q2=.002,Q3=.003,
        P12=.003,P23=.006,P31=.009,Q12=-.001,Q23=.002,Q31=.003; {extra}"
    ))
}

#[test]
fn relative_daily_profiles_preserve_unequal_wye_and_delta_branch_powers() {
    for (mode, configuration) in [
        (13, crate::Configuration::Wye),
        (14, crate::Configuration::Delta),
    ] {
        for model in [1, 2, 3] {
            let db = relative_unequal(mode, model, "");
            let before = db.connection.serialize("main").unwrap().to_vec();
            assert!(db.network().is_err());
            for (hours, factor) in [
                (0.0, 0.0),
                (3.0, 0.5),
                (6.0, 1.0),
                (12.0, 2.0),
                (23.5, 2.0),
                (24.0, 0.0),
                (30.0, 1.0),
            ] {
                let net = db.network_at(hours).unwrap();
                let load = &net.loads()[0];
                assert_eq!(load.configuration, configuration);
                for (actual, expected) in load.p_nom.iter().zip([6000.0, 12000.0, 18000.0]) {
                    assert!((actual - expected * factor).abs() < 1e-8);
                }
                for (actual, expected) in load.q_nom.iter().zip([-3000.0, 6000.0, 9000.0]) {
                    assert!((actual - expected * factor).abs() < 1e-8);
                }
                let selection = &load.extras["sincal_profile"];
                assert_eq!(selection["relative_factor"], factor);
                assert!(selection.get("power_factors").is_none());
                assert_eq!(selection["requested_hours"], hours);
                assert!(matches!(
                    (&load.voltage_model, model),
                    (crate::DistLoadVoltageModel::ConstantImpedance { .. }, 1)
                        | (crate::DistLoadVoltageModel::ConstantPower { .. }, 2)
                        | (crate::DistLoadVoltageModel::ConstantCurrent { .. }, 3)
                ));
            }
            assert_eq!(db.connection.serialize("main").unwrap().to_vec(), before);
        }
    }
}

#[test]
fn relative_profiles_scale_the_selected_base_input_once() {
    let db = legacy(&format!(
        "{PROFILE}{RELATIVE}
        UPDATE Load SET Flag_Lf=3,S=.01,fS=4,cosphi=.6,fP=19,fQ=23;"
    ));
    let PowerInput::Total { p, q } = db.load_input_at(31, 3.0).unwrap().power else {
        panic!("connection changed")
    };
    assert!((p - 12000.0).abs() < 1e-8);
    assert!((q - 16000.0).abs() < 1e-8);
    for selection in 1..=7 {
        let db = legacy(&format!(
            "{PROFILE}{RELATIVE}
            UPDATE Load SET Flag_Lf=1,P=.012,Q=-.006,fP=2,fQ=3;
            UPDATE Terminal SET Flag_Terminal={selection} WHERE Element_ID=31;"
        ));
        let net = db.network_at(3.0).unwrap();
        let load = &net.loads()[0];
        assert!((load.p_nom.iter().sum::<f64>() - 12000.0).abs() < 1e-8);
        assert!((load.q_nom.iter().sum::<f64>() + 9000.0).abs() < 1e-8);
    }
}

#[test]
fn malformed_relative_profiles_and_unverified_functions_reject_atomically() {
    for edit in [
        "UPDATE OpSerVal SET Factor=NULL;",
        "UPDATE OpSerVal SET Factor=-1;",
        "UPDATE OpSerVal SET Factor='bad';",
        "UPDATE OpSerVal SET Factor=1e999;",
        "UPDATE OpSerVal SET Factor=1e308; UPDATE Load SET P=1;",
        "UPDATE OpSerVal SET Factor=1e-300; UPDATE Load SET P=1e-300;",
        "ALTER TABLE OpSerVal DROP COLUMN Factor;",
        "UPDATE OpSer SET Flag_Typ=2;",
        "UPDATE OpSer SET Flag_Typ=4;",
        "UPDATE OpSer SET Flag_Typ=5;",
        "UPDATE OpSer SET Power_b1=1;",
        "UPDATE OpSer SET Reduce_a2=1;",
        "UPDATE OpSer SET Reduce_b2=0;",
        "INSERT INTO OpSerVal SELECT * FROM OpSerVal WHERE OpTime=0;",
        "INSERT INTO OpSerVal VALUES(3,7,1,1,24,1,1,NULL,NULL,NULL);",
    ] {
        let db = legacy(&format!("{PROFILE}{RELATIVE}{edit}"));
        assert!(db.network_at(3.0).is_err(), "accepted {edit}");
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
        "UPDATE Load SET fP=-1;",
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
            if raw.terminal.connection.phases().unwrap().len() > 2
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
    std::fs::write(output,serde_json::to_vec_pretty(&serde_json::json!({"scope":"profiled single-phase and phase-pair load components only; no complete network","cases":cases})).unwrap()).unwrap();
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

#[test]
fn absolute_profile_factors_apply_once_after_sampling_and_preserve_connection() {
    let edit = format!("{PROFILE} UPDATE Load SET fP=2,fQ=3,fS=999;");
    let db = legacy(&edit);
    assert_eq!(powers(&db, 6.0), (24000.0, 9000.0));
    assert_eq!(powers(&db, 24.0), (12000.0, -9000.0));
    let net = db.network_at(6.0).unwrap();
    assert_eq!(net.loads()[0].p_nom, vec![8000.0; 3]);
    assert_eq!(net.loads()[0].q_nom, vec![3000.0; 3]);
    assert_eq!(
        net.loads()[0].extras["sincal_profile"]["power_factors"],
        serde_json::json!([2.0, 3.0])
    );
    assert_eq!(
        powers(
            &legacy(&format!("{PROFILE} UPDATE Load SET fP=0,fQ=0;")),
            6.0
        ),
        (0.0, 0.0)
    );
    for edit in [
        "UPDATE Load SET fP=NULL",
        "UPDATE Load SET fQ=-1",
        "UPDATE Load SET P=0, fP=1e308",
        "UPDATE OpSerVal SET P=1e-300; UPDATE Load SET fP=1e-300",
    ] {
        assert!(
            legacy(&format!("{PROFILE}{edit}"))
                .load_input_at(31, 0.0)
                .is_err(),
            "accepted {edit}"
        );
    }
}

#[test]
fn materialized_load_manipulators_never_multiply_static_inputs_twice() {
    for version in [11.5_f64, 12.8, 14.8] {
        let make = |edit: &str| {
            if version.to_bits() == 14.8_f64.to_bits() {
                super::mapping_tests::native(edit)
            } else {
                super::legacy_tests::acquired_version(edit, version)
            }
        };
        let edit = "UPDATE Load SET Mpl_ID=7,fP=0.28,fQ=0.5;";
        let db = make(edit);
        let source_before = db.connection.serialize("main").unwrap().to_vec();
        let actual = db.network().unwrap();
        let expected = make("UPDATE Load SET fP=0.28,fQ=0.5;").network().unwrap();
        assert_eq!(actual.loads()[0].p_nom, expected.loads()[0].p_nom);
        assert_eq!(actual.loads()[0].q_nom, expected.loads()[0].q_nom);
        assert_eq!(
            actual.loads()[0].extras["sincal_manipulation"],
            serde_json::json!({"id":7,"semantics":"materialized_input"})
        );
        assert_eq!(
            db.connection.serialize("main").unwrap().to_vec(),
            source_before
        );
        // A saved UI definition is not applied again, even if a user later
        // changes it without committing edits into the electrical inputs.
        let with_ui = make(&format!(
            "{edit}
            CREATE TABLE Manipulation (Mpl_ID INTEGER, Variant_ID INTEGER, fP REAL, fQ REAL);
            INSERT INTO Manipulation VALUES (7,1,17,23);"
        ))
        .network()
        .unwrap();
        assert_eq!(with_ui.loads(), actual.loads());
    }
    for edit in [
        "Mpl_ID=-1",
        "Mpl_ID='bad'",
        "Mpl_ID=7,fP=NULL",
        "Mpl_ID=7,P1=NULL",
    ] {
        assert!(
            legacy(&format!("UPDATE Load SET {edit}"))
                .network()
                .is_err(),
            "invalid {edit}"
        );
    }
}

#[test]
fn materialized_load_manipulators_preserve_daily_selection_and_typed_serialization() {
    let db = legacy(&format!(
        "{PROFILE} UPDATE Load SET Mpl_ID=7,fP=0.28,fQ=0.5;"
    ));
    for (hours, p, q) in [
        (0.0, 1680.0, -1500.0),
        (6.0, 3360.0, 1500.0),
        (24.0, 1680.0, -1500.0),
    ] {
        let (actual_p, actual_q) = powers(&db, hours);
        assert!((actual_p - p).abs() < 1e-10);
        assert!((actual_q - q).abs() < 1e-10);
        let net = db.network_at(hours).unwrap();
        let load = &net.loads()[0];
        assert!((load.p_nom.iter().sum::<f64>() - p).abs() < 1e-10);
        assert!((load.q_nom.iter().sum::<f64>() - q).abs() < 1e-10);
        assert_eq!(
            load.extras["sincal_profile"]["power_factors"],
            serde_json::json!([0.28, 0.5])
        );
        assert_eq!(load.extras["sincal_manipulation"]["id"], 7);
        let value = serde_json::to_value(&net).unwrap();
        let restored: crate::MulticonductorNetwork = serde_json::from_value(value).unwrap();
        assert_eq!(restored.loads(), net.loads());
    }
    assert!(db.network().is_err()); // A UI manipulator never selects a daily snapshot.
}

#[test]
#[ignore = "exports external CSIRO05 materialized load snapshots; not a complete native network"]
fn export_csiro05_materialized_loads() {
    let path = std::env::var_os("POWERIO_SINCAL_MANIPULATOR_RECORDS").expect("record path");
    let output = std::env::var_os("POWERIO_SINCAL_MANIPULATOR_EXPORT").expect("export path");
    let bytes = std::fs::read(path).unwrap();
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
            let circuit = db
                .load_input_at(id, hours)
                .unwrap()
                .circuit(&bus, node.nominal_ll_volts)
                .unwrap();
            components.push(serde_json::json!({"element":id,"hours":hours,"load":circuit.load,"bus":circuit.bus,"switch":circuit.switch}));
        }
    }
    assert_eq!(components.len() / 5 + rejected.len(), 458);
    std::fs::write(
        output,
        serde_json::to_vec_pretty(
            &serde_json::json!({"case":5,"components":components,"rejected":rejected}),
        )
        .unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "exports original relative-profile cases for independent OpenDSS checks"]
fn export_relative_daily_profiles() {
    let mut cases = Vec::new();
    for mode in [13, 14] {
        for model in [1, 2, 3] {
            let db = relative_unequal(mode, model, "UPDATE OpSerVal SET Flag_Curve=1;");
            for hours in [0.0, 3.0, 6.0, 12.0, 18.0, 24.0, 30.0] {
                let network = db.network_at(hours).unwrap();
                let load = &network.loads()[0];
                let bus = network
                    .buses()
                    .iter()
                    .find(|bus| bus.id == load.bus)
                    .unwrap();
                cases.push(serde_json::json!({"mode":mode,"model":model,"hours":hours,"load":load,"bus":bus}));
            }
        }
    }
    std::fs::write(
        std::env::var_os("POWERIO_SINCAL_RELATIVE_EXPORT").unwrap(),
        serde_json::to_vec_pretty(&cases).unwrap(),
    )
    .unwrap();
}

#[test]
fn public_relative_snapshot_retains_original_source_and_conductor_family() {
    use powerio_core::Source;
    use sha2::{Digest, Sha256};
    let original =
        b"\0\x01\0\0Standard Jet DB\0original synthetic source for relative-profile test";
    let mut records = super::legacy_tests::acquired_records(
        &format!(
            "{PROFILE}{RELATIVE} UPDATE Load SET Flag_Lf=14,fP=2,
         P12=.003,P23=.006,P31=.009;"
        ),
        11.5,
    );
    records["source"]["bytes"] = serde_json::json!(original.len());
    records["source"]["sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(original)));
    let source = Source::from_memory("relative.mdb", original.to_vec())
        .unwrap()
        .with_named_buffer("records.json", serde_json::to_vec(&records).unwrap())
        .unwrap();
    let mut options = crate::SincalReadOptions {
        acquired_tables: Some("records.json".into()),
        ..Default::default()
    };
    assert!(crate::parse_sincal(source.clone(), &options).is_err());
    options.snapshot_hours = Some(3.0);
    let module = crate::parse_sincal(source, &options).unwrap();
    assert_eq!(
        module.value().loads()[0].configuration,
        crate::Configuration::Delta
    );
    assert_eq!(module.value().loads()[0].p_nom, [3000.0, 6000.0, 9000.0]);
    assert_eq!(
        *module.value().source_format(),
        Some(crate::DistSourceFormat::Sincal)
    );
    assert_eq!(
        module.source().unwrap().primary_buffer().unwrap().bytes(),
        original
    );
}

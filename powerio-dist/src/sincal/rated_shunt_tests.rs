use num_complex::Complex64;
use std::fmt::Write;

use super::{legacy_tests, mapping_tests, schema::NativeDatabase};
use crate::DistBus;

fn sql(kind: &str) -> String {
    let mut sql = format!(
        "CREATE TABLE {kind} (
        Element_ID INTEGER,Variant_ID INTEGER,Typ_ID INTEGER DEFAULT 0,Flag_Typ_ID INTEGER DEFAULT 0,
        Sn REAL DEFAULT 0.06,Un REAL DEFAULT 0.4,Vcu REAL DEFAULT 0.2,Vfe REAL DEFAULT 0.4,Vdi REAL DEFAULT 0.6,
        Flag_Lf INTEGER DEFAULT 1,Flag_Macro INTEGER DEFAULT 0,Macro_ID INTEGER DEFAULT 0,
        Flag_roh INTEGER DEFAULT 1,roh REAL DEFAULT 1,rohl REAL DEFAULT 0,rohm REAL DEFAULT 1,rohu REAL DEFAULT 3,
        deltaS REAL DEFAULT 0.01,Node_ID INTEGER DEFAULT 0,Terminal_ID INTEGER DEFAULT 0,
        Ctrl_OpSer_ID INTEGER DEFAULT 0,Ctrl_OpPnt_ID INTEGER DEFAULT 0,Flag_Step INTEGER DEFAULT 0,
        Flag_Z0 INTEGER DEFAULT 1,Flag_Z0_Input INTEGER DEFAULT 3,Z0_Z1 REAL DEFAULT 1,
        R0_X0 REAL DEFAULT 0,R0 REAL DEFAULT 0,X0 REAL DEFAULT 1,Stp_ID INTEGER DEFAULT 0);
        INSERT INTO {kind} (Element_ID,Variant_ID) VALUES (34,1);
        INSERT INTO Element (Element_ID,Variant_ID,Type,Flag_Input) VALUES (34,1,'{kind}',6);
        INSERT INTO Terminal (Terminal_ID,Variant_ID,Element_ID,Node_ID,TerminalNo) VALUES (62,1,34,30,1);"
    );
    if kind == "ShuntCondensator" {
        let rx = -0.6 / (60.0_f64.powi(2) - 0.6_f64.powi(2)).sqrt();
        write!(
            sql,
            "UPDATE ShuntCondensator SET Flag_Z0_Input=1,R0_X0={rx};"
        )
        .unwrap();
    }
    sql
}

fn native(kind: &str, edit: &str) -> NativeDatabase {
    mapping_tests::native(&format!("{}{edit}", sql(kind)))
}
fn bus() -> DistBus {
    DistBus::new("30", ["n", "3", "1", "2"].map(str::to_owned).to_vec())
}

#[test]
fn rated_banks_preserve_total_rating_and_reactive_sign_for_every_port() {
    for (kind, sign) in [("ShuntReactor", 1.0), ("ShuntCondensator", -1.0)] {
        for selection in 1..=7 {
            let db = native(
                kind,
                &format!("UPDATE Terminal SET Flag_Terminal={selection} WHERE Element_ID=34"),
            );
            let input = db.rated_shunt_input(34).unwrap();
            let circuit = input.circuit(&bus()).unwrap();
            let v = circuit
                .shunt
                .terminal_map
                .iter()
                .map(|p| match p.as_str() {
                    "1" => Complex64::from_polar(400.0 / 3.0_f64.sqrt(), 0.0),
                    "2" => {
                        Complex64::from_polar(400.0 / 3.0_f64.sqrt(), -std::f64::consts::TAU / 3.0)
                    }
                    "3" => {
                        Complex64::from_polar(400.0 / 3.0_f64.sqrt(), std::f64::consts::TAU / 3.0)
                    }
                    "earth" => Complex64::new(0.0, 0.0),
                    _ => panic!("unexpected conductor"),
                })
                .collect::<Vec<_>>();
            let s: Complex64 = circuit
                .shunt
                .g
                .iter()
                .zip(&circuit.shunt.b)
                .enumerate()
                .map(|(i, (g, b))| {
                    let current: Complex64 = g
                        .iter()
                        .zip(b)
                        .zip(&v)
                        .map(|((&g, &b), v)| Complex64::new(g, b) * v)
                        .sum();
                    v[i] * current.conj()
                })
                .sum();
            assert!((s.re - 600.0).abs() < 1e-9, "{kind} {selection}: {s}");
            assert!((s.im - sign * (60_000.0_f64.powi(2) - 600.0_f64.powi(2)).sqrt()).abs() < 1e-9);
            assert!(bus().grounded.is_empty());
            assert!(
                !circuit
                    .switch
                    .terminal_map_from
                    .iter()
                    .any(|p| p == "earth" || p == "n")
            );
            db.network().unwrap();
        }
    }
}

#[test]
fn bank_fixed_steps_zero_steps_and_service_states_remain_explicit() {
    for kind in ["ShuntReactor", "ShuntCondensator"] {
        let original = native(kind, "")
            .rated_shunt_input(34)
            .unwrap()
            .circuit(&bus())
            .unwrap();
        let stepped = native(kind, &format!("UPDATE {kind} SET roh=2"))
            .rated_shunt_input(34)
            .unwrap()
            .circuit(&bus())
            .unwrap();
        for (a, b) in stepped
            .shunt
            .g
            .iter()
            .flatten()
            .zip(original.shunt.g.iter().flatten())
        {
            assert!((a - b * 7.0 / 6.0).abs() < 1e-12);
        }
        for edit in [
            "UPDATE Element SET Flag_State=0 WHERE Element_ID=34",
            "UPDATE Terminal SET Flag_State=0 WHERE Element_ID=34",
        ] {
            let circuit = native(kind, edit)
                .rated_shunt_input(34)
                .unwrap()
                .circuit(&bus())
                .unwrap();
            assert_eq!(circuit.shunt, original.shunt);
            assert!(circuit.switch.open);
        }
        let zero = native(kind, &format!("UPDATE {kind} SET roh=0,deltaS=0.06"))
            .rated_shunt_input(34)
            .unwrap()
            .circuit(&bus())
            .unwrap();
        assert!(
            zero.shunt
                .g
                .iter()
                .chain(&zero.shunt.b)
                .flatten()
                .all(|v| v.abs() < 1e-15)
        );
    }
}

#[test]
fn floating_banks_do_not_consume_common_mode_current() {
    let input=native("ShuntReactor","UPDATE ShuntReactor SET Flag_Z0=0,Flag_Z0_Input=999; UPDATE Element SET Flag_Input=2 WHERE Element_ID=34").rated_shunt_input(34).unwrap();
    let circuit = input.circuit(&bus()).unwrap();
    assert!(circuit.bus.grounded.is_empty());
    assert_eq!(circuit.bus.terminals.len(), 3);
    for row in circuit.shunt.g.iter().chain(&circuit.shunt.b) {
        assert!(row.iter().sum::<f64>().abs() < 1e-12);
    }
}

#[test]
fn grounded_banks_keep_independent_rotating_and_zero_sequence_response() {
    for (kind, x) in [("ShuntReactor", 4.0), ("ShuntCondensator", -4.0)] {
        let mut outputs = Vec::new();
        // Same Z0 expressed independently as direct ohms and as a magnitude
        // ratio with signed R/X. Both must retain the positive-sequence bank.
        for edit in [
            format!("Flag_Z0_Input=2,R0=3,X0={x}"),
            format!("Flag_Z0_Input=1,Z0_Z1=1.875,R0_X0={}", 3.0 / x),
        ] {
            let c = native(kind, &format!("UPDATE {kind} SET {edit}"))
                .rated_shunt_input(34)
                .unwrap()
                .circuit(&bus())
                .unwrap();
            let y = (0..3)
                .map(|i| {
                    (0..3)
                        .map(|j| Complex64::new(c.shunt.g[i][j], c.shunt.b[i][j]))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let y0 = Complex64::new(3.0, x).inv();
            for row in &y {
                assert!((row.iter().sum::<Complex64>() - y0).norm() < 1e-12);
            }
            let v = [
                Complex64::new(1.0, 0.0),
                Complex64::from_polar(1.0, -std::f64::consts::TAU / 3.0),
                Complex64::from_polar(1.0, std::f64::consts::TAU / 3.0),
            ];
            let yp = Complex64::new(
                600.0 / 160_000.0,
                -x.signum() * (60_000.0_f64.powi(2) - 600.0_f64.powi(2)).sqrt() / 160_000.0,
            );
            for (i, row) in y.iter().enumerate() {
                assert!(
                    (row.iter().zip(v).map(|(y, v)| y * v).sum::<Complex64>() - yp * v[i]).norm()
                        < 1e-12
                );
            }
            outputs.push(y);
        }
        for (a, b) in outputs[0].iter().flatten().zip(outputs[1].iter().flatten()) {
            assert!((a - b).norm() < 1e-12);
        }
    }
}

#[test]
fn legacy_bank_defaults_are_recorded_but_required_ratings_and_modern_nulls_reject() {
    let edit = format!(
        "{} UPDATE ShuntCondensator SET Vdi=NULL,Flag_roh=NULL,Flag_Macro=NULL,Flag_Z0=NULL;",
        sql("ShuntCondensator")
    );
    let old = legacy_tests::legacy(&edit).network().unwrap();
    assert_eq!(
        old.defaulted()["ShuntCondensator.34"],
        ["Flag_Macro", "Flag_roh", "Vdi", "Flag_Z0"]
    );
    assert!(mapping_tests::native(&edit).network().is_err());
    for field in ["Sn", "Un"] {
        assert!(
            legacy_tests::legacy(&format!(
                "{} UPDATE ShuntCondensator SET {field}=NULL",
                sql("ShuntCondensator")
            ))
            .network()
            .is_err()
        );
    }
}

#[test]
fn rated_bank_rejects_unresolved_controls_grounding_and_invalid_nameplates() {
    assert!(native("ShuntReactor", "UPDATE Terminal SET Flag_Terminal=1 WHERE Element_ID=34; UPDATE ShuntReactor SET Flag_Z0_Input=2,R0=3,X0=4").rated_shunt_input(34).is_err());
    for edit in [
        "Sn=0",
        "Un=0",
        "Vdi=-1",
        "Vdi=1000",
        "Sn=1e999",
        "Un=1e-300",
        "roh=4",
        "Flag_roh=2",
        "Flag_Macro=1",
        "Macro_ID=3",
        "Node_ID=4",
        "Terminal_ID=5",
        "Flag_Lf=2",
        "Ctrl_OpSer_ID=3",
        "Flag_Step=1",
        "Flag_Z0=2",
        "Stp_ID=1",
        "Z0_Z1=0,Flag_Z0_Input=1",
        "Flag_Z0_Input=2,R0=0,X0=0",
        "Flag_Z0_Input=2,roh=2",
    ] {
        assert!(
            native(
                "ShuntCondensator",
                &format!("UPDATE ShuntCondensator SET {edit}")
            )
            .rated_shunt_input(34)
            .is_err(),
            "accepted {edit}"
        );
    }
    assert!(
        native(
            "ShuntCondensator",
            "UPDATE Element SET Flag_Input=2 WHERE Element_ID=34"
        )
        .rated_shunt_input(34)
        .is_err()
    );
    let input = native("ShuntCondensator", "")
        .rated_shunt_input(34)
        .unwrap();
    let mut wrong = bus();
    wrong.id = "99".into();
    assert!(input.circuit(&wrong).is_err());
}

#[test]
#[ignore = "exports original and external rated banks for independent circuit checks"]
fn export_rated_shunt_circuits() {
    let mut synthetic = Vec::new();
    for kind in ["ShuntReactor", "ShuntCondensator"] {
        for selection in 1..=7 {
            let db = native(
                kind,
                &format!("UPDATE Terminal SET Flag_Terminal={selection} WHERE Element_ID=34"),
            );
            let c = db.rated_shunt_input(34).unwrap().circuit(&bus()).unwrap();
            synthetic.push(serde_json::json!({"kind":kind,"selection":selection,"bus":c.bus,"shunt":c.shunt,"switch":c.switch}));
        }
    }
    let mut native_cases = Vec::new();
    let dir =
        std::path::PathBuf::from(std::env::var_os("POWERIO_SINCAL_SHUNT_RECORDS_DIR").unwrap());
    for case in [3, 8, 11, 12, 16, 17] {
        let bytes = std::fs::read(dir.join(format!("representative{case:02}.json"))).unwrap();
        let db = NativeDatabase::from_snapshot(
            powerio_sincal::DatabaseSnapshot::decode_records(&bytes, Some(1)).unwrap(),
        )
        .unwrap();
        for (&id, kind) in &db.elements {
            if !matches!(kind.as_str(), "ShuntReactor" | "ShuntCondensator") {
                continue;
            }
            let input = db.rated_shunt_input(id).unwrap();
            let b = DistBus::new(
                input.terminal.node.to_string(),
                ["1", "2", "3"].map(str::to_owned).to_vec(),
            );
            let c = input.circuit(&b).unwrap();
            native_cases.push(serde_json::json!({"case":case,"element":id,"kind":kind,"defaulted":input.defaulted,"bus":c.bus,"shunt":c.shunt,"switch":c.switch}));
        }
    }
    std::fs::write(
        std::env::var_os("POWERIO_SINCAL_SHUNT_EXPORT").unwrap(),
        serde_json::to_vec_pretty(
            &serde_json::json!({"synthetic":synthetic,"native":native_cases}),
        )
        .unwrap(),
    )
    .unwrap();
}

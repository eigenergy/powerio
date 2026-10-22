//! Synthetic schema/profile changes; no native Access files are vendored.
use super::*;

fn legacy(edit: &str) -> DatabaseSnapshot {
    let sql = format!(
        "UPDATE CalcParameter SET Temp_Cond=20;
         UPDATE VoltageLevel SET Temp_Cable=20;
         UPDATE Load SET DayOpSer_ID=NULL,WeekOpSer_ID=NULL,YearOpSer_ID=NULL,IncrSer_ID=NULL;
         UPDATE Load SET DayOpSer_ID=901,Flag_Lf=1,Flag_LoadType=2,fP=2,fQ=3 WHERE Element_ID=1;
         DROP TABLE IF EXISTS OpSerVal;
         DROP TABLE IF EXISTS OpSer;
         CREATE TABLE OpSer (OpSer_ID INTEGER,Variant_ID INTEGER,Flag_Ser INTEGER,Flag_Typ INTEGER,BaseT REAL,
            Power_a1 REAL,Power_b1 REAL,Reduce_a2 REAL,Reduce_b2 REAL);
         INSERT INTO OpSer VALUES(901,1,1,3,24,0,0,0,0);
         CREATE TABLE OpSerVal (OpSerVal_ID INTEGER,Variant_ID INTEGER,OpSer_ID INTEGER,OpTime REAL,
            Flag_Curve INTEGER,Op_ID INTEGER,P REAL,Q REAL);
         INSERT INTO OpSerVal VALUES(801,1,901,0,1,NULL,12,-4),(802,1,901,12,1,NULL,24,8);
         {edit}"
    );
    let mut db = super::tests::native(&sql);
    // Exercise the family adapter directly, independently of the transport's
    // schema admission. Native Access decoding is checked by the external oracle.
    db.version = 11.5;
    db
}

fn selected_load(net: &BalancedNetwork) -> &crate::network::Load {
    net.loads()
        .iter()
        .find(|l| l.uid.as_deref() == Some("sincal:element:1"))
        .unwrap()
}

#[test]
fn legacy_absolute_snapshots_keep_units_factors_and_cyclic_interpolation() {
    let db = legacy("");
    let before = db.connection.serialize("main").unwrap().to_vec();
    assert!(
        read_balanced_snapshot(&db, "no-time")
            .unwrap_err()
            .to_string()
            .contains("snapshot_hours")
    );
    for (hours, p, q) in [
        (0., 0.024, -0.012),
        (6., 0.036, 0.006),
        (12., 0.048, 0.024),
        (18., 0.036, 0.006),
        (24., 0.024, -0.012),
        (30., 0.036, 0.006),
    ] {
        let net = read_balanced_snapshot_at(&db, "daily", Some(hours)).unwrap();
        let load = selected_load(&net);
        assert!((load.p - p).abs() < 1e-14 && (load.q - q).abs() < 1e-14);
        assert_eq!(net.loads().len(), 13);
        assert_eq!(net.source_format(), SourceFormat::Sincal);
        assert_eq!(load.extras["sincal_profile"]["profile"], 901);
    }
    assert_eq!(&*db.connection.serialize("main").unwrap(), before);
    let db = legacy("UPDATE OpSerVal SET Flag_Curve=2 WHERE OpTime=0");
    let net = read_balanced_snapshot_at(&db, "step", Some(6.)).unwrap();
    assert!((selected_load(&net).p - 0.024).abs() < 1e-14);
}

#[test]
fn invalid_legacy_profiles_and_wrong_families_reject_the_whole_network() {
    for edit in [
        "UPDATE Load SET Flag_Lf=13 WHERE Element_ID=1",
        "UPDATE Terminal SET Flag_Terminal=1 WHERE Element_ID=1",
        "UPDATE Load SET WeekOpSer_ID=901 WHERE Element_ID=1",
        "UPDATE OpSer SET Flag_Typ=1",
        "UPDATE OpSer SET Power_a1=1",
        "UPDATE OpSer SET BaseT=-1",
        "UPDATE OpSerVal SET OpTime=0 WHERE OpSerVal_ID=802",
        "UPDATE OpSerVal SET OpTime=24 WHERE OpSerVal_ID=802",
        "UPDATE OpSerVal SET Op_ID=1 WHERE OpSerVal_ID=802",
        "UPDATE OpSerVal SET P=NULL WHERE OpSerVal_ID=802",
        "UPDATE Load SET fP=-1 WHERE Element_ID=1",
        "UPDATE Load SET fP=1e308 WHERE Element_ID=1; UPDATE OpSerVal SET P=1e308",
        "UPDATE Infeeder SET Flag_LfCtrl=NULL",
        "UPDATE Infeeder SET Flag_Qctrl=NULL",
        "UPDATE Line SET r=NULL",
        "UPDATE CalcParameter SET Temp_Cond=30",
        "UPDATE NetworkGroup SET Flag_IC=1; UPDATE CalcParameter SET Flag_Unit=1",
    ] {
        assert!(
            read_balanced_snapshot_at(&legacy(edit), "invalid", Some(6.)).is_err(),
            "{edit}"
        );
    }
    for hours in [-1., f64::NAN, f64::INFINITY] {
        assert!(read_balanced_snapshot_at(&legacy(""), "invalid", Some(hours)).is_err());
    }
    assert!(read_balanced_snapshot_at(&super::tests::native(""), "modern", Some(0.)).is_err());
}

#[test]
fn legacy_voltage_and_temperature_defaults_stay_scoped_and_recorded() {
    let db = legacy("UPDATE VoltageLevel SET Flag_Volt=NULL,Temp_Cable=NULL");
    let net = read_balanced_snapshot_at(&db, "default", Some(0.)).unwrap();
    assert!(
        net.buses()
            .iter()
            .all(|b| b.extras.contains_key("sincal_voltage_basis"))
    );
    assert!(
        net.branches()
            .iter()
            .filter(|b| b.uid.as_deref() != Some("sincal:element:19"))
            .all(|b| b.extras.contains_key("sincal_defaulted_temperature"))
    );
    assert!(
        read_balanced_snapshot(
            &super::tests::native("UPDATE VoltageLevel SET Flag_Volt=NULL"),
            "modern"
        )
        .is_err()
    );
    let low = read_balanced_snapshot_at(&legacy(""), "20C", Some(0.)).unwrap();
    let high = read_balanced_snapshot_at(
        &legacy("UPDATE VoltageLevel SET Temp_Cable=70; UPDATE Line SET alpha=0.004"),
        "70C",
        Some(0.),
    )
    .unwrap();
    for (a, b) in low.branches().iter().zip(high.branches()) {
        if a.uid.as_deref() != Some("sincal:element:19") {
            assert!((b.r / a.r - 1.2).abs() < 1e-12);
        }
    }
}

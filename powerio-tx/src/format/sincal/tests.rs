use super::*;

const ARCHIVE: &[u8] = include_bytes!("../../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");

fn native(edit: &str) -> DatabaseSnapshot {
    let bytes = powerio_sincal::database_bytes(ARCHIVE).unwrap();
    if edit.is_empty() {
        return DatabaseSnapshot::decode(&bytes, None).unwrap();
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &bytes).unwrap();
    let connection = rusqlite::Connection::open(file.path()).unwrap();
    connection.execute_batch(edit).unwrap();
    let changed = connection.serialize("main").unwrap();
    DatabaseSnapshot::decode(&changed, None).unwrap()
}

#[test]
fn complete_authentic_simbench_case_preserves_every_element() {
    let net = read_balanced_snapshot(&native(""), "SimBench").unwrap();
    assert_eq!(net.source_format(), SourceFormat::Sincal);
    assert_eq!(net.buses().len(), 15);
    assert_eq!(net.loads().len(), 13);
    assert_eq!(net.generators().len(), 5);
    assert_eq!(net.branches().len(), 14);
    assert!(net.switches().is_empty());
    let ids = net
        .loads()
        .iter()
        .map(|l| l.uid.as_ref().unwrap())
        .chain(net.generators().iter().map(|g| g.uid.as_ref().unwrap()))
        .chain(net.branches().iter().map(|b| b.uid.as_ref().unwrap()))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 32);
    assert!(net.generators().iter().all(|g| g.pmax.is_infinite()));
    let slack = net.buses().iter().find(|b| b.kind == BusType::Ref).unwrap();
    assert!((slack.vm - 1.025).abs() < 1e-12);
    let tr = net
        .branches()
        .iter()
        .find(|b| b.uid.as_deref() == Some("sincal:element:19"))
        .unwrap();
    assert_eq!(tr.shift.to_bits(), 150.0_f64.to_bits());
    assert_eq!(tr.tap.to_bits(), 1.0_f64.to_bits());
    assert!((tr.r - 9.179_687_5).abs() < 1e-10);
    assert!((tr.charging.unwrap().g_fr - 2.3e-6).abs() < 1e-15);
}

#[test]
fn errors_identify_required_input_instead_of_returning_partial_networks() {
    for (sql, field) in [
        (
            "UPDATE Load SET Flag_Lf=13 WHERE Element_ID=1",
            "Load[1].Flag_Lf",
        ),
        ("UPDATE Load SET P=NULL WHERE Element_ID=1", "Load[1].P"),
        (
            "UPDATE Terminal SET Flag_Terminal=1 WHERE Terminal_ID=1",
            "Terminal[1].Flag_Terminal",
        ),
        (
            "UPDATE Line SET CoupData_ID=2 WHERE Element_ID=20",
            "Line[20].CoupData_ID",
        ),
        (
            "UPDATE DCInfeeder SET Flag_Qctrl=1 WHERE Element_ID=14",
            "DCInfeeder[14].Flag_Qctrl",
        ),
        (
            "UPDATE Element SET Type='Unknown' WHERE Element_ID=32",
            "orphan/mistyped Line[32]",
        ),
        (
            "UPDATE TwoWindingTransformer SET Flag_Tap=1",
            "TwoWindingTransformer[19].Flag_Tap",
        ),
        (
            "UPDATE CalcParameter SET Flag_UseTimeSer=1; UPDATE Load SET DayOpSer_ID=1 WHERE Element_ID=1",
            "Load[1].DayOpSer_ID",
        ),
    ] {
        // Re-decode structural identity changes; the mapper never mutates a
        // validated snapshot in production.
        let db = native(sql);
        let bytes = db.connection.serialize("main").unwrap();
        let db = DatabaseSnapshot::decode(&bytes, None).unwrap();
        let message = read_balanced_snapshot(&db, "invalid")
            .unwrap_err()
            .to_string();
        assert!(message.contains(field), "{field}: {message}");
    }
}

#[test]
fn open_terminal_retains_energized_line_and_charging() {
    let db = native("UPDATE Terminal SET Flag_State=0 WHERE Element_ID=20 AND TerminalNo=2;");
    let net = read_balanced_snapshot(&db, "open").unwrap();
    let line = net
        .branches()
        .iter()
        .find(|b| b.uid.as_deref() == Some("sincal:element:20"))
        .unwrap();
    assert!(line.in_service);
    assert!(line.b > 0.0);
    assert_eq!(net.switches().len(), 1);
    assert!(!net.switches()[0].closed);
    assert_eq!(line.to, net.switches()[0].to);
    assert_eq!(net.buses().len(), 16);
}

#[test]
fn inactive_equipment_is_present_without_prescribing_a_reference() {
    let net = read_balanced_snapshot(
        &native("UPDATE Element SET Flag_State=0 WHERE Element_ID IN (1,18,20)"),
        "inactive",
    )
    .unwrap();
    assert_eq!(net.loads().len(), 13);
    assert!(!net.loads()[0].in_service);
    assert!(
        !net.branches()
            .iter()
            .find(|b| b.uid.as_deref() == Some("sincal:element:20"))
            .unwrap()
            .in_service
    );
    assert!(!net.generators().last().unwrap().in_service);
    assert!(!net.buses().iter().any(|b| b.kind == BusType::Ref));
}

#[test]
fn electrical_mapping_does_not_consume_result_tables_or_fault_impedance() {
    let baseline = read_balanced_snapshot(&native(""), "baseline").unwrap();
    let changed=read_balanced_snapshot(&native("DELETE FROM LFNodeResult; UPDATE Infeeder SET Sk2=1, R=1234, X=5678; UPDATE Line SET r0=999, x0=888;"),"changed").unwrap();
    assert_eq!(baseline.buses(), changed.buses());
    assert_eq!(baseline.branches(), changed.branches());
    assert_eq!(baseline.generators(), changed.generators());
}

#[test]
fn opening_a_transformer_at_the_slack_does_not_create_a_second_slack() {
    let net = read_balanced_snapshot(
        &native("UPDATE Terminal SET Flag_State=0 WHERE Element_ID=19 AND TerminalNo=1"),
        "open transformer",
    )
    .unwrap();
    assert_eq!(
        net.buses()
            .iter()
            .filter(|b| b.kind == BusType::Ref)
            .count(),
        1
    );
    let switch = &net.switches()[0];
    let auxiliary = net.buses().iter().find(|b| b.id == switch.to).unwrap();
    assert_eq!(auxiliary.kind, BusType::Pq);
    assert!(!switch.closed);
}

#[test]
fn documented_modern_source_modes_and_voltage_only_limits_preserve_capability() {
    for (version, mode, voltage) in [(15.5, 8, 1.04), (16.0, 9, 1.05), (14.8, 6, 1.05)] {
        let db = native(&format!(
            "UPDATE Version SET Version_No={version};
             UPDATE Infeeder SET Flag_Lf={mode}, u=104, Ug=21, Flag_LfLimit=1, ull=95, uul=107;
             UPDATE Element SET Type='Infeeder   ' WHERE Type='Infeeder';"
        ));
        let net = read_balanced_snapshot(&db, "source modes").unwrap();
        let bus = net.buses().iter().find(|b| b.kind == BusType::Ref).unwrap();
        assert!((bus.vm - voltage).abs() < 1e-12);
        assert_eq!((bus.vmin, bus.vmax), (0.95, 1.07));
        let source = net.generators().iter().find(|g| g.bus == bus.id).unwrap();
        assert_eq!(source.pmax.to_bits(), f64::INFINITY.to_bits());
        assert_eq!(source.qmin.to_bits(), f64::NEG_INFINITY.to_bits());
    }
    let invalid = native("UPDATE Infeeder SET Flag_Lf=8, Xlf=0.1");
    assert!(
        read_balanced_snapshot(&invalid, "nonideal")
            .unwrap_err()
            .to_string()
            .contains("Xlf")
    );
}

#[test]
fn initial_voltage_is_absolute_and_only_used_without_flat_start() {
    let db = native(
        "UPDATE Node SET Un=0.38, Phi=12 WHERE Node_ID=1; UPDATE CalcParameter SET Flag_ABW=0",
    );
    let net = read_balanced_snapshot(&db, "start").unwrap();
    let bus = net.buses().iter().find(|b| b.id == BusId(1)).unwrap();
    assert!((bus.vm - 0.95).abs() < 1e-12);
    assert_eq!(bus.va.to_bits(), 12.0_f64.to_bits());
    assert_eq!(bus.base_kv.to_bits(), 0.4_f64.to_bits());
    let flat = native(
        "UPDATE Node SET Un=0.38, Phi=12 WHERE Node_ID=1; UPDATE CalcParameter SET Flag_ABW=1",
    );
    let net = read_balanced_snapshot(&flat, "flat").unwrap();
    let bus = net.buses().iter().find(|b| b.id == BusId(1)).unwrap();
    assert_eq!((bus.vm, bus.va), (1.0, 0.0));
}

#[test]
fn inactive_profiles_and_interchange_are_distinct_from_active_controls() {
    let baseline = read_balanced_snapshot(&native(""), "controls").unwrap();
    let harmless = native(
        "UPDATE Load SET DayOpSer_ID=123 WHERE Element_ID=1;
        UPDATE CalcParameter SET Flag_UseTimeSer=0, Flag_Unit=1;
        UPDATE Line SET Flag_Cond=3, Flag_Vart=2;",
    );
    let unchanged = read_balanced_snapshot(&harmless, "controls").unwrap();
    assert_eq!(baseline.loads(), unchanged.loads());
    assert_eq!(baseline.branches(), unchanged.branches());
    for (sql, field) in [
        (
            "UPDATE CalcParameter SET Flag_Unit=1; UPDATE NetworkGroup SET Flag_IC=1",
            "Flag_IC",
        ),
        ("UPDATE NetworkGroup SET Flag_Temp=1", "Flag_Temp"),
        (
            "UPDATE CalcParameter SET Flag_UseTimeSer=1; UPDATE Load SET DayOpSer_ID=123 WHERE Element_ID=1",
            "DayOpSer_ID",
        ),
        (
            "UPDATE CalcParameter SET Flag_UseTimeSer=2",
            "Flag_UseTimeSer",
        ),
    ] {
        assert!(
            read_balanced_snapshot(&native(sql), "active")
                .unwrap_err()
                .to_string()
                .contains(field)
        );
    }
}

#[test]
fn fixed_capacitor_step_scales_loss_and_reactive_admittance() {
    // Original toy capacitor values, inserted into the existing licensed
    // schema. No data from the unlicensed external models are vendored.
    let db = native("INSERT INTO Element (Element_ID,Variant_ID,Flag_Variant,Type,Flag_Input,Flag_State,Name)
          VALUES (999,1,1,'ShuntCondensator',2,1,'toy capacitor');
        INSERT INTO Terminal (Terminal_ID,Variant_ID,Element_ID,Node_ID,TerminalNo,Flag_Terminal,Flag_State,Flag_Switch)
          VALUES (999,1,999,1,1,7,1,0);
        INSERT INTO ShuntCondensator (Element_ID,Variant_ID,Flag_Lf,Flag_roh,Typ_ID,Flag_Typ_ID,Flag_Macro,Macro_ID,
          Flag_Step,Ctrl_OpSer_ID,Ctrl_OpPnt_ID,Node_ID,Terminal_ID,Sn,Vdi,Un,deltaS,roh,rohm)
          VALUES (999,1,1,1,0,0,0,0,0,0,0,0,0,0.2,2,0.5,0.01,3,1);");
    let net = read_balanced_snapshot(&db, "capacitor").unwrap();
    let cap = &net.shunts()[0];
    // S=0.22 MVA, P=0.0022 MW at 0.5 kV; bus base is 0.4 kV.
    assert!((cap.g - 0.0022 * 0.64).abs() < 1e-12);
    assert!((cap.b - (0.22_f64.powi(2) - 0.0022_f64.powi(2)).sqrt() * 0.64).abs() < 1e-12);
    assert_eq!(cap.uid.as_deref(), Some("sincal:element:999"));
    assert!(cap.in_service);
}

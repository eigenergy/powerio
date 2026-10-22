use super::{legacy_tests::legacy, transformer_mapping::phase_admittance};
use num_complex::Complex64;

const FINITE: &str = "UPDATE TwoWindingTransformer SET VecGrp=71,Vfe=0,i0=0,Flag_Z0_Input=3,R0_R1=1,X0_X1=1,uk=5,ur=1;";
const IDEAL: &str = "ALTER TABLE TwoWindingTransformer ADD COLUMN rohl REAL DEFAULT -16;
 ALTER TABLE TwoWindingTransformer ADD COLUMN rohu REAL DEFAULT 16;
 UPDATE TwoWindingTransformer SET VecGrp=71,Un1=0.4,Un2=0.4,Vfe=0,i0=0,roh=0,rohm=0,ukr=0.625,Flag_Z0_Input=3,R0_R1=1,X0_X1=1;
 UPDATE Node SET VoltLevel_ID=2 WHERE Node_ID=20;";

#[test]
fn y0_keeps_galvanic_common_mode_transfer_without_grounding() {
    let db = legacy(FINITE);
    let input = db.transformer_nominal(33).unwrap();
    let y = phase_admittance(&input).unwrap();
    let z = input.series_secondary_ohm;
    let expected = Complex64::new(1.0, 0.0) / z;
    for (row, values) in y.iter().enumerate() {
        let i: Complex64 = values[..3].iter().sum();
        assert!((i - if row < 3 { expected } else { -expected }).norm() < 1e-11 * expected.norm());
        assert!(values.iter().sum::<Complex64>().norm() < 1e-11 * expected.norm());
    }
    let isolated = legacy(&FINITE.replace("VecGrp=71", "VecGrp=6"));
    let isolated_y = phase_admittance(&isolated.transformer_nominal(33).unwrap()).unwrap();
    assert!(isolated_y[0][..3].iter().sum::<Complex64>().norm() < 1e-11);
    let net = db.network().unwrap();
    crate::require_electrical_readiness(&net).unwrap();
    let output = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
    let restored = crate::testkit::parse_str(&output.text, "pmd-json").unwrap();
    assert_eq!(net.shunts()[0].g, restored.shunts()[0].g);
    assert_eq!(net.shunts()[0].b, restored.shunts()[0].b);
}

#[test]
fn neutral_tap_y0_is_an_exact_selected_phase_connection() {
    for code in 1..=7 {
        for terminal_open in [false, true] {
            let db = legacy(&format!(
                "{IDEAL} UPDATE Terminal SET Flag_Terminal={code} WHERE Element_ID=33; UPDATE Terminal SET Flag_State={} WHERE Terminal_ID=61;",
                i32::from(!terminal_open)
            ));
            let net = db.network().unwrap();
            let switch = net.switches().iter().find(|s| s.name == "33").unwrap();
            assert_eq!(switch.open, terminal_open);
            let phases = super::semantics::Connection::decode(code)
                .unwrap()
                .phases()
                .unwrap();
            assert_eq!(
                switch.terminal_map_from,
                phases
                    .iter()
                    .map(|p| (p + 1).to_string())
                    .collect::<Vec<_>>()
            );
            assert_eq!(switch.terminal_map_from, switch.terminal_map_to);
            assert!(net.shunts().iter().all(|s| s.name != "33"));
            assert!(
                net.buses()
                    .iter()
                    .all(|b| b.grounded.is_empty() || b.id.starts_with("sincal:load:"))
            );
            crate::require_electrical_readiness(&net).unwrap();
        }
    }
    for edit in [
        "roh=1",
        "Vfe=1",
        "i0=1",
        "rohu=rohl",
        "ukr=0",
        "rohl=NULL",
        "uk=-1",
        "ur=uk+1",
        "R0_R1=-1",
        "Flag_Z0_Input=1",
        "Stp_ID1=1",
        "AddRotate=1",
        "phi=1",
        "Flag_roh=2",
    ] {
        assert!(
            legacy(&format!("{IDEAL} UPDATE TwoWindingTransformer SET {edit}"))
                .network()
                .is_err(),
            "accepted {edit}"
        );
    }
}

#[test]
fn topology_decodes_autotransformer_connections_without_resolving_control_state() {
    for code in [71, 72, 73] {
        let db = legacy(&format!(
            "UPDATE TwoWindingTransformer SET VecGrp={code},Flag_roh=2;"
        ));
        assert!(db.topology_draft().is_ok());
        assert!(db.network().is_err());
        let report = db.mapping_report().unwrap();
        assert!(
            report
                .components
                .iter()
                .any(|c| c.element == 33 && !c.component_maps)
        );
    }
    for edit in ["VecGrp=74", "VecGrp=NULL"] {
        assert!(
            legacy(&format!("UPDATE TwoWindingTransformer SET {edit}"))
                .topology_draft()
                .is_err()
        );
    }
}

#[test]
#[ignore = "exports synthetic Y0 primitives for independent electrical validation"]
fn export_y0_primitives() {
    let mut cases = Vec::new();
    for (v1, v2, side) in [(11.0, 0.4, 1), (0.4, 11.0, 2)] {
        let db = legacy(&format!(
            "{FINITE} UPDATE TwoWindingTransformer SET Un1={v1},Un2={v2},Flag_ConNode={side};"
        ));
        let input = db.transformer_nominal(33).unwrap();
        let y = phase_admittance(&input).unwrap();
        cases.push(serde_json::json!({"kv":[v1,v2],"kva":100.0,"uk_percent":5.0,"ur_percent":1.0,"r0_r1":1.0,"x0_x1":1.0,"y_re":y.map(|row|row.map(|v|v.re)),"y_im":y.map(|row|row.map(|v|v.im))}));
    }
    let path = std::env::var_os("POWERIO_SINCAL_AUTO_EXPORT").expect("explicit export path");
    std::fs::write(path, serde_json::to_vec_pretty(&cases).unwrap()).unwrap();
}

#[test]
#[ignore = "exports the two authentic CSIRO01 neutral-tap Y0 components, not a complete feeder"]
fn export_native_neutral_y0() {
    let path = std::env::var_os("POWERIO_SINCAL_AUTO_RECORDS").expect("explicit records path");
    let output =
        std::env::var_os("POWERIO_SINCAL_AUTO_NATIVE_EXPORT").expect("explicit output path");
    let bytes = std::fs::read(path).unwrap();
    let db = super::schema::NativeDatabase::from_snapshot(
        powerio_sincal::DatabaseSnapshot::decode_records(&bytes, Some(1)).unwrap(),
    )
    .unwrap();
    let buses = db
        .node_inputs()
        .unwrap()
        .keys()
        .map(|id| {
            (
                *id,
                crate::DistBus::new(
                    id.to_string(),
                    ["1", "2", "3", "n"].map(str::to_owned).to_vec(),
                ),
            )
        })
        .collect();
    let mut stmt=db.connection.prepare("SELECT Element_ID FROM TwoWindingTransformer WHERE Variant_ID=1 AND VecGrp=71 AND Un1=Un2 AND roh=rohm ORDER BY Element_ID").unwrap();
    let ids: Vec<i64> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(ids, [2451, 2453]);
    let switches: Vec<_> = ids
        .iter()
        .map(|id| {
            db.ideal_autotransformer_switch(*id, &buses)
                .unwrap()
                .unwrap()
        })
        .collect();
    std::fs::write(output, serde_json::to_vec_pretty(&switches).unwrap()).unwrap();
}

#[test]
fn unverified_autotransformer_physics_rejects_after_topology() {
    for edit in [
        "VecGrp=72",
        "VecGrp=73",
        "Un1=Un2",
        "Vfe=1",
        "i0=1",
        "roh=2",
        "AddRotate=1",
        "Stp_ID1=1",
        "Flag_Z0_Input=1",
        "R0_R1=0,X0_X1=0",
    ] {
        let db = legacy(&format!("{FINITE} UPDATE TwoWindingTransformer SET {edit}"));
        assert!(db.topology_draft().is_ok());
        assert!(db.network().is_err(), "accepted {edit}");
    }
    let db = legacy(&format!(
        "{FINITE} UPDATE Element SET Flag_Input=2 WHERE Element_ID=33"
    ));
    assert!(db.network().is_err());
    for mask in 0..8 {
        let db = legacy(&format!(
            "{IDEAL} UPDATE Element SET Flag_State={} WHERE Element_ID=33; UPDATE Terminal SET Flag_State={} WHERE Terminal_ID=60; UPDATE Terminal SET Flag_State={} WHERE Terminal_ID=61;",
            mask & 1,
            (mask >> 1) & 1,
            (mask >> 2) & 1
        ));
        let net = db.network().unwrap();
        let switch = net.switches().iter().find(|s| s.name == "33").unwrap();
        assert_eq!(switch.open, mask != 7);
    }
}

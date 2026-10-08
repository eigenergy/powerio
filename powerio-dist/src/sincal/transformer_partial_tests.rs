use std::collections::BTreeMap;

use num_complex::Complex64;

use super::{
    schema::NativeDatabase, transformer_mapping::phase_admittance,
    transformer_mapping_tests::native,
};
use crate::DistBus;

fn partial(group: i64, selection: i64) -> NativeDatabase {
    native(&format!(
        "UPDATE TwoWindingTransformer SET VecGrp={group}; UPDATE Terminal SET Flag_Terminal={selection};"
    ))
}

#[test]
fn installed_delta_coils_sum_to_full_primitive_without_changing_rating() {
    for group in [1, 35] {
        let full = partial(group, 7).transformer_nominal(30).unwrap();
        let expected = phase_admittance(&full).unwrap();
        let mut sum = [[Complex64::new(0.0, 0.0); 6]; 6];
        for selection in 1..=3 {
            let input = partial(group, selection).transformer_nominal(30).unwrap();
            let actual = phase_admittance(&input).unwrap();
            for i in 0..6 {
                for j in 0..6 {
                    sum[i][j] += actual[i][j];
                    assert!((actual[i][j] - actual[j][i]).norm() < 1e-12);
                }
                // Each side remains floating independently, for arbitrary
                // common-mode voltages, even with only one installed coil.
                for side in 0..2 {
                    assert!(
                        actual[i][3 * side..3 * side + 3]
                            .iter()
                            .sum::<Complex64>()
                            .norm()
                            < 1e-12
                    );
                }
            }
        }
        for i in 0..6 {
            for j in 0..6 {
                assert!((sum[i][j] - expected[i][j]).norm() < 1e-11);
            }
        }
    }
}

#[test]
fn partial_delta_ports_connect_coil_endpoints_and_preserve_open_state() {
    for (selection, phases) in [
        (1, vec!["1", "2"]),
        (2, vec!["2", "3"]),
        (3, vec!["1", "3"]),
        (4, vec!["1", "2", "3"]),
        (5, vec!["1", "2", "3"]),
        (6, vec!["1", "2", "3"]),
    ] {
        let buses: BTreeMap<_, _> = [10, 20]
            .into_iter()
            .map(|id| {
                (
                    id,
                    DistBus::new(id.to_string(), phases.iter().map(|p| (*p).into()).collect()),
                )
            })
            .collect();
        let db = native(&format!(
            "UPDATE TwoWindingTransformer SET VecGrp=35; UPDATE Terminal SET Flag_Terminal={selection}; UPDATE Terminal SET Flag_State=0 WHERE TerminalNo=2"
        ));
        let circuit = db.transformer_circuit(30, &buses).unwrap();
        assert_eq!(circuit.shunt.terminal_map.len(), phases.len() * 2);
        assert_eq!(circuit.auxiliary_bus.terminals, circuit.shunt.terminal_map);
        assert_eq!(circuit.auxiliary_bus.grounded.as_slice(), []);
        for (side, switch) in circuit.terminal_switches.iter().enumerate() {
            assert_eq!(switch.terminal_map_from, phases);
            assert_eq!(switch.open, side == 1);
        }
        let mut wrong = buses.clone();
        wrong.get_mut(&10).unwrap().terminals.remove(0);
        assert!(db.transformer_circuit(30, &wrong).is_err());
    }
}

#[test]
fn partial_delta_does_not_admit_unverified_winding_modes() {
    for edit in [
        "VecGrp=14",
        "VecGrp=73",
        "AddRotate=30",
        "roh=rohm+1",
        "Stp_ID1=3",
        "uk=0,ur=0",
    ] {
        let db = native(&format!(
            "UPDATE TwoWindingTransformer SET VecGrp=1; UPDATE Terminal SET Flag_Terminal=1; UPDATE TwoWindingTransformer SET {edit}"
        ));
        assert!(
            db.transformer_nominal(30)
                .and_then(|input| phase_admittance(&input))
                .is_err(),
            "{edit}"
        );
    }
}

fn export(db: &NativeDatabase, element: i64) -> serde_json::Value {
    let input = db.transformer_nominal(element).unwrap();
    let y = phase_admittance(&input).unwrap();
    let buses: BTreeMap<_, _> = input
        .connection
        .ports
        .iter()
        .map(|p| {
            (
                p.node,
                DistBus::new(
                    p.node.to_string(),
                    ["1", "2", "3"].map(str::to_owned).to_vec(),
                ),
            )
        })
        .collect();
    let circuit = db.transformer_circuit(element, &buses).unwrap();
    serde_json::json!({"element":element, "kv":input.connection.rated_ll_volts.map(|v|v/1000.0),
        "kva":input.connection.rated_va/1000.0,"clock":input.connection.vector_group.clock,
        "windings":input.connection.coils.iter().map(|c|c.winding).collect::<Vec<_>>(),
        "z_re":input.series_secondary_ohm.re,"z_im":input.series_secondary_ohm.im,
        "ym_re":input.no_load_secondary_siemens.re,"ym_im":input.no_load_secondary_siemens.im,
        "y_re":y.map(|row|row.map(|v|v.re)),"y_im":y.map(|row|row.map(|v|v.im)),
        "shunt":circuit.shunt,"switches":circuit.terminal_switches})
}

#[test]
#[ignore = "exports original synthetic and external native partial delta circuits for independent checks"]
fn export_partial_delta_circuits() {
    let mut synthetic = Vec::new();
    for group in [1, 35] {
        for selection in 1..=6 {
            synthetic.push(export(&partial(group, selection), 30));
        }
    }
    let records =
        std::fs::read(std::env::var_os("POWERIO_SINCAL_PARTIAL_RECORDS").unwrap()).unwrap();
    let db = NativeDatabase::from_snapshot(
        powerio_sincal::DatabaseSnapshot::decode_records(&records, Some(1)).unwrap(),
    )
    .unwrap();
    let mut native = Vec::new();
    for (&id, kind) in &db.elements {
        if kind != "TwoWindingTransformer" {
            continue;
        }
        let connection = db.transformer_connection(id).unwrap();
        if connection.vector_group.primary != super::transformer::WindingKind::Delta
            || connection.vector_group.secondary != super::transformer::WindingKind::Delta
            || connection.coils.len() == 3
        {
            continue;
        }
        match db
            .transformer_nominal(id)
            .and_then(|input| phase_admittance(&input))
        {
            Ok(_) => native.push(export(&db, id)),
            Err(error) => native.push(serde_json::json!({"element":id,"error":error.to_string()})),
        }
    }
    let value = serde_json::json!({"synthetic":synthetic,"native":native});
    std::fs::write(
        std::env::var_os("POWERIO_SINCAL_PARTIAL_EXPORT").unwrap(),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

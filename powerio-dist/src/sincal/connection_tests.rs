use std::collections::BTreeMap;

use super::{line_mapping_tests::line_database, mapping_tests, schema::NativeDatabase};
use crate::{DistBus, MulticonductorNetwork};

fn native(edit: &str) -> NativeDatabase {
    NativeDatabase::decode(
        &line_database(&format!("UPDATE Line SET Flag_LineTyp=3; {edit}")),
        None,
    )
    .unwrap()
}

fn buses() -> BTreeMap<i64, DistBus> {
    [10, 20]
        .into_iter()
        .map(|id| {
            (
                id,
                DistBus::new(
                    id.to_string(),
                    ["1", "2", "3", "n"].map(str::to_owned).to_vec(),
                ),
            )
        })
        .collect()
}

#[test]
fn steady_state_connection_is_an_exact_switch_not_a_small_impedance_line() {
    let db= mapping_tests::native("UPDATE Line SET Flag_LineTyp=3,r=999,x=999,c=999,r0=NULL,x0=NULL,c0=NULL,Flag_Z0_Input=NULL;
        UPDATE Element SET Flag_Input=2 WHERE Element_ID=30;");
    let before = db.connection.serialize("main").unwrap().to_vec();
    let net = db.network().unwrap();
    assert!(net.lines().is_empty());
    assert!(net.line_codes().is_empty());
    let switch = net.switches().iter().find(|s| s.name == "30").unwrap();
    assert_eq!(switch.bus_from, "10");
    assert_eq!(switch.bus_to, "20");
    assert_eq!(switch.terminal_map_from, ["1", "2", "3"]);
    assert!(!switch.open);
    assert_eq!(db.connection.serialize("main").unwrap().to_vec(), before);
    let report = db.mapping_report().unwrap();
    assert!(report.all_components_map());
    let evidence = report.components[0].zero_sequence.as_ref().unwrap();
    assert!(evidence.findings.is_empty());
    assert_eq!(
        evidence.fields["Line.x0"],
        super::mapping_report::ObservedInput::Null
    );
}

#[test]
fn both_terminal_states_and_service_state_control_the_ideal_path() {
    for service in [0, 1] {
        for from in [0, 1] {
            for to in [0, 1] {
                let db = native(&format!(
                    "UPDATE Element SET Flag_State={service};
                    UPDATE Terminal SET Flag_State={from} WHERE TerminalNo=1;
                    UPDATE Terminal SET Flag_State={to} WHERE TerminalNo=2;
                    UPDATE Line SET Ith=0.2,fr=0.8,ParSys=2;"
                ));
                let (switch, _) = db.connection_switch(30, &buses()).unwrap().unwrap();
                assert_eq!(switch.open, service == 0 || from == 0 || to == 0);
                assert_eq!(switch.i_max, Some(vec![320.0; 3]));
                assert!(!switch.terminal_map_from.contains(&"n".into()));
                assert_eq!(
                    switch.extras["sincal_connection"]["terminal_closed"],
                    serde_json::json!([from == 1, to == 1])
                );
                assert_eq!(
                    switch.extras["sincal_connection"]["in_service"],
                    service == 1
                );
                let mut net = MulticonductorNetwork::new();
                net.buses_mut().extend(buses().into_values());
                net.switches_mut().push(switch.clone());
                crate::require_electrical_readiness(&net).unwrap();
                let graph = net.to_graph();
                assert_eq!(graph.edges.len(), 1);
                assert_eq!(graph.edges[0].closed, !switch.open);
                assert_eq!(graph.edges[0].conductors.len(), 3);
                let emitted =
                    crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
                let reread = crate::testkit::parse_str(&emitted.text, "pmd-json").unwrap();
                let result = &reread.switches()[0];
                assert_eq!(result.open, switch.open);
                assert_eq!(result.i_max, switch.i_max);
                assert_eq!(result.terminal_map_from, switch.terminal_map_from);
                assert_eq!(result.terminal_map_to, switch.terminal_map_to);
                assert!(
                    emitted
                        .diagnostics
                        .iter()
                        .any(|d| d.code() == "EMIT.PMD.FIELD_DROPPED")
                );
            }
        }
    }
    assert!(
        native("UPDATE Line SET Ith=0;")
            .connection_switch(30, &buses())
            .unwrap()
            .unwrap()
            .0
            .i_max
            .is_none()
    );
}

#[test]
fn unsupported_connection_modes_and_malformed_active_data_reject() {
    for edit in [
        "UPDATE Terminal SET Flag_Terminal=1;",
        "UPDATE Terminal SET Flag_Terminal=8;",
        "UPDATE Element SET Flag_Input=4;",
        "UPDATE Element SET Flag_State=2;",
        "UPDATE Line SET Flag_Ground=1;",
        "UPDATE Line SET Flag_Ll=1;",
        "UPDATE Line SET Flag_Macro=1;",
        "UPDATE Line SET CoupData_ID=1;",
        "UPDATE Line SET Macro_ID=1;",
        "UPDATE Line SET LineTemp_ID=1;",
        "UPDATE Line SET ElemLoading_ID=1;",
        "UPDATE Line SET Flag_Lf=2;",
        "UPDATE Line SET Ith=-1;",
        "UPDATE Line SET Ith=NULL;",
        "UPDATE Line SET Ith=1e308;",
        "UPDATE Line SET ParSys=0;",
        "UPDATE Line SET fr=0;",
        "UPDATE Line SET ParSys=1e308,fr=1e308;",
        "INSERT INTO Line SELECT * FROM Line;",
        "INSERT INTO LineSeg VALUES (30,1);",
        "ALTER TABLE Line RENAME TO Original; CREATE VIEW Line AS SELECT * FROM Original;",
    ] {
        assert!(
            native(edit).connection_switch(30, &buses()).is_err(),
            "accepted {edit}"
        );
    }
    let mut missing = buses();
    missing.get_mut(&20).unwrap().terminals.retain(|p| p != "2");
    assert!(native("").connection_switch(30, &missing).is_err());
    let ordinary = native("UPDATE Line SET Flag_LineTyp=1;");
    assert!(ordinary.connection_switch(30, &buses()).unwrap().is_none());
    assert!(ordinary.line_circuit(30, &buses()).is_ok());
}

#[test]
#[ignore = "exports all native CSIRO09 ideal connections for independent topology validation"]
fn export_csiro_connections() {
    let path = std::env::var_os("POWERIO_SINCAL_CONNECTION_RECORDS").expect("records path");
    let output = std::env::var_os("POWERIO_SINCAL_CONNECTION_EXPORT").expect("export path");
    let bytes = std::fs::read(path).unwrap();
    let db = NativeDatabase::from_snapshot(
        powerio_sincal::DatabaseSnapshot::decode_records(&bytes, Some(1)).unwrap(),
    )
    .unwrap();
    let topology = db.topology_draft().unwrap();
    let buses = topology
        .nodes
        .keys()
        .copied()
        .zip(topology.buses())
        .collect();
    let mut network = MulticonductorNetwork::new();
    for (&id, kind) in &db.elements {
        if kind == "Line"
            && let Some((switch, _)) = db.connection_switch(id, &buses).unwrap()
        {
            network.switches_mut().push(switch);
        }
    }
    assert_eq!(network.switches().len(), 144);
    network.buses_mut().extend(buses.into_values());
    crate::require_electrical_readiness(&network).unwrap();
    let data = serde_json::json!({"scope":"connection components only; no complete feeder","switches":network.switches(),"graph":network.to_graph()});
    std::fs::write(output, serde_json::to_vec_pretty(&data).unwrap()).unwrap();
}

#[test]
fn ordinary_zero_impedance_lines_keep_exact_connectivity_ratings_and_open_ports() {
    for kind in [1, 2] {
        for phase in [1, 4, 7] {
            for open in [0, 1] {
                let db = mapping_tests::native(&format!(
                    "UPDATE Line SET Flag_LineTyp={kind},r=0,x=0,r0=0,x0=0,c=0,c0=0,va=0,Ith=0.2,ParSys=2,fr=0.8;
                     UPDATE Terminal SET Flag_Terminal={phase} WHERE Element_ID=30;
                     UPDATE Terminal SET Flag_State={open} WHERE Element_ID=30 AND TerminalNo=2;"
                ));
                let net = db.network().unwrap();
                assert!(net.lines().is_empty());
                assert!(net.line_codes().is_empty());
                let switch = net.switches().iter().find(|s| s.name == "30").unwrap();
                assert!(!switch.open);
                assert_eq!(
                    switch.i_max,
                    Some(vec![320.0; switch.terminal_map_from.len()])
                );
                assert!(switch.extras.contains_key("sincal_zero_impedance_line"));
                if open == 0 {
                    let terminal = net
                        .switches()
                        .iter()
                        .find(|s| s.name == "sincal:terminal:50")
                        .unwrap();
                    assert!(terminal.open);
                    assert_eq!(switch.bus_to, terminal.bus_to);
                } else {
                    assert_eq!(switch.bus_to, "20");
                }
                crate::require_electrical_readiness(&net).unwrap();
            }
        }
    }
    // Neither nonzero charging nor an arbitrarily small series impedance is
    // an ideal connection. Keep every matrix entry for the consuming model.
    for edit in ["c=1", "c0=1", "va=1", "r=1e-15", "x0=1e-15"] {
        let db = mapping_tests::native(&format!(
            "UPDATE Line SET r=0,x=0,r0=0,x0=0,c=0,c0=0,va=0; UPDATE Line SET {edit};"
        ));
        let net = db.network().unwrap();
        assert_eq!(net.lines().len(), 1, "incorrectly fused {edit}");
        assert!(!net.switches().iter().any(|s| s.name == "30"));
    }
    for edit in ["r=1e-323", "x0=1e-323", "c=1e-320", "c0=1e-320"] {
        let db = mapping_tests::native(&format!(
            "UPDATE Line SET r=0,x=0,r0=0,x0=0,c=0,c0=0,va=0; UPDATE Line SET {edit};"
        ));
        assert!(
            db.network().is_err(),
            "underflow became an ideal connection: {edit}"
        );
    }
}

#[test]
fn sparse_legacy_connections_keep_ratings_and_record_only_active_defaults() {
    let sparse = "UPDATE Line SET Flag_LineTyp=3,Flag_Ll=NULL,Flag_Ground=NULL,Flag_Macro=NULL,ParSys=NULL,fr=NULL,va=NULL,alpha=NULL;";
    let actual = super::legacy_tests::legacy(sparse).network().unwrap();
    let expected = super::legacy_tests::legacy("UPDATE Line SET Flag_LineTyp=3")
        .network()
        .unwrap();
    assert_eq!(actual.switches(), expected.switches());
    assert_eq!(
        actual.defaulted()["Line.30"],
        ["Flag_Ll", "Flag_Ground", "Flag_Macro", "ParSys", "fr"]
    );
    assert!(mapping_tests::native(sparse).network().is_err());
    for field in ["Flag_Ll", "Flag_Ground", "Flag_Macro", "ParSys", "fr"] {
        let value = if field.starts_with("Flag") { "1" } else { "0" };
        assert!(
            super::legacy_tests::legacy(&format!("{sparse} UPDATE Line SET {field}={value}"))
                .network()
                .is_err()
        );
    }
}

use std::f64::consts::{PI, TAU};

use powerio_core::Source;
use rusqlite::Connection;

use super::{
    infeeder_tests::infeeder_database, line_mapping_tests::line_database, read,
    schema::NativeDatabase, tests::load_database, transformer_mapping_tests,
};
use crate::{Configuration, DistBus, DistLoadVoltageModel};

fn table_definition(db: &NativeDatabase, name: &str) -> String {
    db.connection
        .query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| {
            r.get(0)
        })
        .unwrap()
}

// Combine the original synthetic component tables in one native-schema
// snapshot. No third-party native data or SQL definitions are copied.
pub(super) fn network_database(edit: &str) -> Vec<u8> {
    let source = NativeDatabase::decode(&infeeder_database(""), None).unwrap();
    let load = NativeDatabase::decode(&load_database(""), None).unwrap();
    let transformer = transformer_mapping_tests::native("");
    let path = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(path.path(), line_database("")).unwrap();
    let conn = Connection::open(path.path()).unwrap();
    for (db, name) in [
        (&source, "Infeeder"),
        (&load, "Load"),
        (&transformer, "TwoWindingTransformer"),
    ] {
        conn.execute_batch(&table_definition(db, name)).unwrap();
    }
    conn.execute_batch(&format!(
        "ALTER TABLE Node ADD COLUMN Name TEXT DEFAULT 'node';
         ALTER TABLE Node ADD COLUMN VoltLevel_ID INTEGER DEFAULT 1;
         ALTER TABLE Node ADD COLUMN Stp_ID INTEGER DEFAULT 0;
         INSERT INTO Node (Node_ID,Variant_ID,VoltLevel_ID) VALUES (30,1,2);
         UPDATE VoltageLevel SET Un=11;
         INSERT INTO VoltageLevel VALUES (2,1,20,20,0.4,50,1);
         UPDATE Line SET Un=11;
         INSERT INTO Element (Element_ID,Variant_ID,Type,Flag_Input) VALUES
             (31,1,'Load',6),(32,1,'Infeeder',6),(33,1,'TwoWindingTransformer',6);
         INSERT INTO Terminal (Terminal_ID,Variant_ID,Element_ID,Node_ID,TerminalNo) VALUES
             (41,1,31,30,1),(42,1,32,10,1),(60,1,33,20,1),(61,1,33,30,2);
         ALTER TABLE Load ADD COLUMN Flag_Z0_Input INTEGER DEFAULT 3;
         ALTER TABLE Load ADD COLUMN Pneg REAL DEFAULT 0;
         ALTER TABLE Load ADD COLUMN Qneg REAL DEFAULT 0;
         INSERT INTO Load (Element_ID,Variant_ID,Flag_Lf,Flag_LoadType) VALUES (31,1,13,1);
         INSERT INTO Infeeder (Element_ID,Variant_ID,Flag_Lf,Ug,delta,Flag_Z0,Flag_Z0_Input,R0,X0)
             VALUES (32,1,6,11,30,1,2,0,0);
         INSERT INTO TwoWindingTransformer (Element_ID,Variant_ID,roh) VALUES (33,1,1);
         {edit}"
    ))
    .unwrap();
    conn.serialize("main").unwrap().to_vec()
}

pub(super) fn native(edit: &str) -> NativeDatabase {
    NativeDatabase::decode(&network_database(edit), None).unwrap()
}

/// Report every component's current mapping disposition without editing the
/// authentic archive or returning a partial network as a successful parse.
#[test]
#[ignore = "writes the optional licensed native-case mapping audit"]
#[allow(clippy::too_many_lines)] // One provenance report keeps all dispositions together.
fn audit_authentic_mapping_coverage() {
    let archive = include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let source = Source::from_memory("1-LV-rural1--0-sw.sinx", archive.to_vec()).unwrap();
    let db = read(&source, None).unwrap();
    let topology = db.topology_draft().unwrap();
    let buses: std::collections::BTreeMap<_, _> = topology
        .nodes
        .keys()
        .copied()
        .zip(topology.buses())
        .collect();
    let mapping_report = db.mapping_report().unwrap();
    let network = db.network();
    let source_boundaries: Vec<_> = db
        .elements
        .iter()
        .filter(|(_, kind)| kind.as_str() == "Infeeder")
        .map(|(&element, _)| {
            let input = db.infeeder_input(element).unwrap();
            let node = input.terminal.node;
            let circuit =
                input.ideal_boundary_circuit(&buses[&node], topology.nodes[&node].nominal_ll_volts);
            match circuit {
                Ok(circuit) => serde_json::json!({
                    "element": element,
                    "internal_boundary_constructed": true,
                    "bus": circuit.boundary.bus,
                    "equations": circuit.boundary.equations().iter().map(|equation| {
                        serde_json::json!({
                            "positive": equation.positive,
                            "negative": equation.negative,
                            "voltage_re_v": equation.voltage.re,
                            "voltage_im_v": equation.voltage.im,
                        })
                    }).collect::<Vec<_>>(),
                        "rust_model_transport": true,
                        "legacy_c_view_supported": false,
                }),
                Err(error) => serde_json::json!({
                    "element": element, "internal_boundary_constructed": false,
                    "first_error": error.to_string(),
                }),
            }
        })
        .collect();
    // Probe the current reader's forward-field behavior before proposing a
    // new source reference field. This is evidence about compatibility, not
    // an endorsement of ignoring a future field that changes the physics.
    let current = native("").network().unwrap();
    let mut hypothetical = serde_json::to_value(&current).unwrap();
    hypothetical["sources"][0]["reference_terminal"] = "floating-star".into();
    let reread = serde_json::from_value::<crate::MulticonductorNetwork>(hypothetical);
    let ignored_reference = reread
        .as_ref()
        .is_ok_and(|value| value.sources() == current.sources());
    let report = serde_json::json!({
        "source": "tests/data/sincal/1-LV-rural1--0-sw.sinx",
        "expected_source_sha256": "019f52397b4bc5673abed79a5fe1ed0d484d9674ffca04d4102806cd29ccc659",
        "schema_version": db.version, "variant": db.variant,
        "whole_network_maps": network.is_ok(),
        "whole_network_first_error": network.err().map(|error| error.to_string()),
        "all_components_map": mapping_report.all_components_map(),
        "components": mapping_report.components,
        "internal_source_boundaries": source_boundaries,
        "current_model_reader_ignores_hypothetical_source_reference": ignored_reference,
        "interpretation": "Component success is local mapping only, not a partial network or native acceptance. Each component uses the production assembler and reports its first guard. Selected load/line zero-sequence fields retain native absence, NULL and numeric states; findings describe the current mapping profile, not all unsupported fields.",
    });
    let path =
        std::env::var_os("POWERIO_SINCAL_AUDIT_REPORT").expect("POWERIO_SINCAL_AUDIT_REPORT");
    std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}

/// Optional external oracle handoff. The SQL is original synthetic input,
/// not an assertion that SINCAL accepts our reduced schema.
#[test]
#[ignore = "exports synthetic mapped circuits for the optional OpenDSS oracle"]
fn export_unbalanced_oracle_cases() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("POWERIO_SINCAL_ORACLE_DIR").expect("POWERIO_SINCAL_ORACLE_DIR"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    for (name, group, load_mode, port) in [
        ("dyn11_wye", 59, 13, 7),
        ("dyn1_wye", 10, 13, 7),
        ("dyn11_delta", 59, 14, 7),
        ("dyn11_l31", 59, 1, 6),
    ] {
        let net = native(&format!(
            "UPDATE TwoWindingTransformer SET VecGrp={group}, Vfe=0, i0=0, R0=0.048, X0=0.064;
             UPDATE Load SET Flag_Lf={load_mode};
             UPDATE Terminal SET Flag_Terminal={port} WHERE Element_ID=31;"
        ))
        .network()
        .unwrap();
        crate::require_electrical_readiness(&net).unwrap();
        let output = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
        std::fs::write(directory.join(format!("{name}.pmd.json")), output.text).unwrap();
        std::fs::write(
            directory.join(format!("{name}.boundary.json")),
            serde_json::to_vec_pretty(&serde_json::json!({
                "loads": net.loads(), "sources": net.sources(),
                "native_acceptance": false,
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn whole_network_maps_a_single_phase_feeder_without_inventing_remote_phases() {
    let net = native(
        "DELETE FROM Terminal WHERE Element_ID=33;
         DELETE FROM TwoWindingTransformer; DELETE FROM Element WHERE Element_ID=33;
         DELETE FROM Node WHERE Node_ID=30;
         UPDATE Terminal SET Node_ID=20, Flag_Terminal=3 WHERE Element_ID=31;
         UPDATE Load SET Flag_Lf=1;
         UPDATE Terminal SET Flag_Terminal=3 WHERE Element_ID=30;
         UPDATE Line SET r0=r, x0=x, c0=c;",
    )
    .network()
    .unwrap();
    crate::require_electrical_readiness(&net).unwrap();
    assert_eq!(net.bus("20").unwrap().terminals, ["3"]);
    assert_eq!(net.lines()[0].terminal_map_from, ["3"]);
    assert_eq!(net.lines()[0].terminal_map_to, ["3"]);
    assert_eq!(net.line_codes()[0].n_conductors, 1);
    assert_eq!(net.loads()[0].terminal_map, ["3", "0"]);
    assert!(net.bus("20").unwrap().grounded.is_empty());
    let output = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
    let parsed = crate::testkit::parse_str(&output.text, "pmd-json").unwrap();
    assert_eq!(parsed.bus("20").unwrap().terminals, ["3"]);
    assert_eq!(parsed.loads()[0].p_nom, net.loads()[0].p_nom);
    assert_eq!(parsed.lines()[0].terminal_map_to, ["3"]);
}

#[test]
fn whole_network_keeps_converter_generation_and_its_open_phase_pair_port() {
    let setup = format!(
        "{} ALTER TABLE VoltageLevel ADD COLUMN Flag_DCInfeeder INTEGER DEFAULT 0;
         INSERT INTO Element (Element_ID,Variant_ID,Type,Flag_Input) VALUES (34,1,'DCInfeeder',2);
         INSERT INTO Terminal (Terminal_ID,Variant_ID,Element_ID,Node_ID,TerminalNo,Flag_Terminal,Flag_State)
             VALUES (62,1,34,30,1,6,0);
         INSERT INTO DCInfeeder (Element_ID,Variant_ID) VALUES (34,1);",
        super::dc_infeeder_tests::schema_sql(),
    );
    let net = native(&setup).network().unwrap();
    crate::require_electrical_readiness(&net).unwrap();
    assert_eq!(net.generators().len(), 1);
    assert_eq!(net.generators()[0].terminal_map, ["3", "1"]);
    assert_eq!(net.generators()[0].p_nom, [30_000.0]);
    assert_eq!(net.generators()[0].q_nom, [-60_000.0]);
    let port = net
        .switches()
        .iter()
        .find(|s| s.name == "sincal:terminal:62")
        .unwrap();
    assert!(port.open);
    assert_eq!(port.bus_from, "30");
    assert_eq!(port.terminal_map_from, ["3", "1"]);
    assert_eq!(port.bus_to, net.generators()[0].bus);
    let output = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
    let parsed = crate::testkit::parse_str(&output.text, "pmd-json").unwrap();
    assert_eq!(parsed.generators()[0].p_nom, net.generators()[0].p_nom);
    assert_eq!(
        parsed.generators()[0].terminal_map,
        net.generators()[0].terminal_map
    );
    assert!(
        parsed
            .switches()
            .iter()
            .find(|s| s.name == port.name)
            .unwrap()
            .open
    );
    let error = native(&format!("{setup} UPDATE DCInfeeder SET DayOpSer_ID=1"))
        .network()
        .unwrap_err()
        .to_string();
    assert!(error.contains("Element 34 (DCInfeeder)"), "{error}");
}

#[test]
fn whole_network_keeps_a_shunt_and_its_open_phase_pair_port() {
    let setup = format!(
        "{} INSERT INTO Element (Element_ID,Variant_ID,Type,Flag_Input) VALUES (34,1,'ShuntImpedance',6);
         INSERT INTO Terminal (Terminal_ID,Variant_ID,Element_ID,Node_ID,TerminalNo,Flag_Terminal,Flag_State)
             VALUES (62,1,34,30,1,6,0);
         INSERT INTO ShuntImpedance (Element_ID,Variant_ID,Flag_Z0) VALUES (34,1,0);",
        super::shunt_impedance_tests::schema_sql(),
    );
    let net = native(&setup).network().unwrap();
    crate::require_electrical_readiness(&net).unwrap();
    let shunt = net.shunts().iter().find(|s| s.name == "34").unwrap();
    assert_eq!(shunt.terminal_map, ["3", "1"]);
    assert!((shunt.g[0][0] - 0.12).abs() < 1e-14);
    assert!((shunt.b[0][0] + 0.16).abs() < 1e-14);
    let port = net
        .switches()
        .iter()
        .find(|s| s.name == "sincal:terminal:62")
        .unwrap();
    assert!(port.open);
    assert_eq!(port.terminal_map_from, ["3", "1"]);
    assert_eq!(port.bus_to, shunt.bus);
    let output = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
    let parsed = crate::testkit::parse_str(&output.text, "pmd-json").unwrap();
    let projected = parsed.shunts().iter().find(|s| s.name == "34").unwrap();
    assert_eq!(projected.g, shunt.g);
    assert_eq!(projected.b, shunt.b);
    let error = native(&format!("{setup} UPDATE ShuntImpedance SET Ireg=1"))
        .network()
        .unwrap_err()
        .to_string();
    assert!(error.contains("Element 34 (ShuntImpedance)"), "{error}");
}

#[test]
fn equipment_voltage_basis_is_checked_independently_of_node_basis() {
    // The unsupported level is initially unused. Refer each equipment type
    // to it without changing its node, so a node-only guard cannot pass.
    let level = "INSERT INTO VoltageLevel VALUES (99,1,20,20,0.23,50,2);";
    assert!(native(level).network().is_ok());
    for element in [30, 31, 32, 33] {
        let db = native(&format!(
            "{level} UPDATE Element SET VoltLevel_ID=99 WHERE Element_ID={element};"
        ));
        let error = db.network().unwrap_err().to_string();
        assert!(error.contains(&format!("Element {element}:")), "{error}");
        assert!(error.contains("Flag_Volt=1"), "{error}");
    }
    for edit in [
        "UPDATE Element SET VoltLevel_ID=99 WHERE Element_ID=32",
        "UPDATE VoltageLevel SET Flag_Volt=NULL WHERE VoltLevel_ID=1",
        "INSERT INTO VoltageLevel SELECT * FROM VoltageLevel WHERE VoltLevel_ID=1",
    ] {
        assert!(native(edit).network().is_err(), "accepted {edit}");
    }
}

#[test]
fn complete_native_network_keeps_unequal_loads_ports_and_voltage_bases() {
    let bytes = network_database("");
    let source = Source::from_memory("synthetic.db", bytes.clone()).unwrap();
    let net = read(&source, None).unwrap().network().unwrap();
    assert_eq!(source.primary_buffer().unwrap().bytes(), bytes);
    assert_eq!(net.buses().len(), 6); // three native buses and three auxiliary buses
    assert_eq!(net.lines().len(), 1);
    assert_eq!(net.line_codes().len(), 1);
    assert_eq!(net.loads().len(), 1);
    assert_eq!(net.sources().len(), 1);
    assert_eq!(net.shunts().len(), 1);
    assert_eq!(net.switches().len(), 4);
    assert!(net.source_format().is_none()); // still a private mapper
    let load = &net.loads()[0];
    assert_eq!(load.bus, "sincal:load:31");
    assert_eq!(load.configuration, Configuration::Wye);
    assert_eq!(load.terminal_map, ["1", "2", "3", "0"]);
    assert_eq!(load.p_nom, [2000.0, 4000.0, 6000.0]);
    assert!(matches!(
        load.voltage_model,
        DistLoadVoltageModel::ConstantImpedance { .. }
    ));
    for v in load.voltage_model.v_nom() {
        assert!((v - 400.0 / 3.0_f64.sqrt()).abs() < 1e-10);
    }
    let bus = net.buses().iter().find(|b| b.id == load.bus).unwrap();
    assert_eq!(bus.grounded, ["0"]);
    assert!(!bus.terminals.contains(&"n".into()));
    assert!(
        net.buses()
            .iter()
            .filter(|b| b.id != load.bus)
            .all(|b| b.grounded.is_empty())
    );
    let source = &net.sources()[0];
    assert_eq!(source.bus, "sincal:infeeder:32");
    for v in &source.v_magnitude {
        assert!((v - 11000.0 / 3.0_f64.sqrt()).abs() < 1e-9);
    }
    for (a, b) in source
        .v_angle
        .iter()
        .zip([PI / 6.0, -PI / 2.0, 5.0 * PI / 6.0])
    {
        assert!((a - b).abs() < 1e-14);
    }
    assert_eq!(net.lines()[0].bus_from, "10");
    assert_eq!(net.lines()[0].bus_to, "20");
    assert_eq!(net.shunts()[0].bus, "sincal:transformer:33");
    assert_eq!(net.shunts()[0].g.len(), 6);
    crate::require_electrical_readiness(&net).unwrap();
}

#[test]
fn source_voltage_modes_switching_and_phase_maps_are_explicit() {
    let db = NativeDatabase::decode(
        &infeeder_database(
            "UPDATE Infeeder SET Flag_Z0=1,R0=0,X0=0;
         UPDATE Terminal SET Flag_State=0",
        ),
        None,
    )
    .unwrap();
    let input = db.infeeder_input(30).unwrap();
    let bus = DistBus::new("10", ["3", "n", "1", "2"].map(str::to_owned).to_vec());
    let circuit = input.ideal_circuit(&bus, 400.0).unwrap();
    assert_eq!(circuit.switch.terminal_map_from, ["1", "2", "3"]);
    assert!(circuit.switch.open);
    assert!(bus.grounded.is_empty());
    assert!(circuit.bus.grounded.is_empty());
    for v in &circuit.source.v_magnitude {
        assert!((v - 420.0 / 3.0_f64.sqrt()).abs() < 1e-10);
    }
    assert!((circuit.source.v_angle[1] - circuit.source.v_angle[0] + TAU / 3.0).abs() < 1e-14);
    assert!(
        input
            .ideal_circuit(&DistBus::new("11", bus.terminals.clone()), 400.0)
            .is_err()
    );
    assert!(
        input
            .ideal_circuit(&DistBus::new("10", vec!["1".into()]), 400.0)
            .is_err()
    );
    assert!(input.ideal_circuit(&bus, f64::INFINITY).is_err());
    for mode in [3, 6, 8, 9] {
        let db = NativeDatabase::decode(
            &infeeder_database(&format!(
                "UPDATE Infeeder SET Flag_Lf={mode}, u=110, Ug=0.44, Flag_Z0=1,R0=0,X0=0"
            )),
            None,
        )
        .unwrap();
        let actual = db
            .infeeder_input(30)
            .unwrap()
            .ideal_circuit(&bus, 400.0)
            .unwrap();
        for v in &actual.source.v_magnitude {
            assert!((v - 440.0 / 3.0_f64.sqrt()).abs() < 1e-10);
        }
    }
}

#[test]
fn complete_circuit_draft_retains_floating_source_without_returning_a_partial_network() {
    let db = native("UPDATE Infeeder SET Flag_Z0=0");
    let draft = db.circuit_draft().unwrap();
    assert_eq!(draft.source_boundaries().len(), 1);
    let boundary = &draft.source_boundaries()[0];
    assert_eq!(boundary.name, "32");
    assert!(
        boundary
            .equations()
            .iter()
            .all(|row| row.negative.as_deref() == Some("star"))
    );
    let network = db.network().unwrap();
    assert_eq!(
        network.sources()[0].reference_terminal.as_deref(),
        Some("star")
    );
    crate::require_electrical_readiness(&network).unwrap();
    let stored = serde_json::to_value(&network).unwrap();
    assert_eq!(
        stored["sources"][0]["type"],
        "powerio.ReferencedVoltageSource"
    );
    let recovered: crate::MulticonductorNetwork = serde_json::from_value(stored).unwrap();
    assert_eq!(network.sources(), recovered.sources());
    // Complete draft construction still rejects unrelated unsupported
    // components. It is not a tolerant partial interpretation of a file.
    assert!(
        native("UPDATE Infeeder SET Flag_Z0=0; UPDATE Load SET Flag_Z0_Input=2")
            .circuit_draft()
            .is_err()
    );
}

#[test]
fn whole_network_retains_open_source_line_load_and_transformer_ports() {
    let net = native("UPDATE Terminal SET Flag_State=0 WHERE Terminal_ID IN (41,42,50,61)")
        .network()
        .unwrap();
    assert_eq!(net.sources().len(), 1);
    assert_eq!(net.lines().len(), 1);
    assert_eq!(net.shunts().len(), 1);
    assert_eq!(net.switches().iter().filter(|s| s.open).count(), 4);
    assert_eq!(net.buses().len(), 7);
    let baseline = native("").network().unwrap();
    assert_eq!(net.line_codes(), baseline.line_codes());
    assert_eq!(net.shunts(), baseline.shunts());
    assert_eq!(net.sources(), baseline.sources());
    assert_eq!(net.loads(), baseline.loads());
}

#[test]
fn unsupported_elements_and_physics_reject_the_whole_network_with_identity() {
    for (edit, identity) in [
        ("UPDATE Load SET DayOpSer_ID=2", "Element 31"),
        ("UPDATE Infeeder SET xi=1", "Element 32"),
        ("UPDATE Infeeder SET R0=0.1", "Element 32"),
        ("UPDATE Infeeder SET Flag_Z0_Input=3", "Element 32"),
        ("UPDATE Infeeder SET Flag_Lf=7", "Element 32"),
        ("UPDATE Infeeder SET DayOpSer_ID=2", "Element 32"),
        ("UPDATE Infeeder SET Flag_Pctrl=1", "Element 32"),
        (
            "UPDATE Element SET Flag_State=0 WHERE Element_ID=32",
            "Element 32",
        ),
        (
            "UPDATE Terminal SET Flag_Terminal=1 WHERE Terminal_ID=42",
            "Element 32",
        ),
        (
            "UPDATE TwoWindingTransformer SET AddRotate=30",
            "Element 33",
        ),
        ("UPDATE Load SET Stp_ID=8", "Element 31"),
        ("UPDATE Line SET CoupData_ID=8", "Element 30"),
        ("UPDATE Node SET Stp_ID=8 WHERE Node_ID=20", "Node 20"),
        (
            "UPDATE VoltageLevel SET f=60 WHERE VoltLevel_ID=2",
            "Node 30",
        ),
        (
            "UPDATE Element SET Type='DCInfeeder' WHERE Element_ID=32",
            "Element 32",
        ),
    ] {
        let error = native(edit).network().unwrap_err().to_string();
        assert!(error.contains(identity), "{edit}: {error}");
    }
    for edit in [
        "UPDATE CalcParameter SET f=0",
        "INSERT INTO CalcParameter VALUES (1,1,50,20)",
    ] {
        assert!(native(edit).network().is_err());
    }
}

#[test]
fn delta_loads_do_not_create_an_earth_coordinate_and_isolated_nodes_remain() {
    let net = native(
        "UPDATE Load SET Flag_Lf=14;
        INSERT INTO Node (Node_ID,Variant_ID) VALUES (99,1)",
    )
    .network()
    .unwrap();
    assert_eq!(net.loads()[0].configuration, Configuration::Delta);
    assert_eq!(net.loads()[0].terminal_map, ["1", "2", "3"]);
    assert!(net.buses().iter().all(|b| b.grounded.is_empty()));
    let isolated = net.buses().iter().find(|b| b.id == "99").unwrap();
    assert!(isolated.terminals.is_empty());
    // Parsing preserves the node without inventing conductors or silently
    // deleting it. The existing numerical-readiness gate remains separate.
    assert!(
        crate::require_electrical_readiness(&net)
            .unwrap_err()
            .to_string()
            .contains("READINESS.BUS.TERMINALS_EMPTY 99")
    );
}

#[test]
fn assembly_does_not_mix_equal_identities_from_different_variants() {
    let path = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(path.path(), network_database("")).unwrap();
    let conn = Connection::open(path.path()).unwrap();
    conn.execute_batch("INSERT INTO Variant VALUES (2,NULL,1)")
        .unwrap();
    // Both variants have the same identity sets, with different source and
    // load values. Copy original synthetic rows, not native external data.
    for table in [
        "Node",
        "Element",
        "Terminal",
        "VoltageLevel",
        "CalcParameter",
        "Line",
        "LineSeg",
        "Load",
        "Infeeder",
        "TwoWindingTransformer",
    ] {
        let statement = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let columns = statement
            .column_names()
            .iter()
            .map(|name| {
                if *name == "Variant_ID" {
                    "2".to_owned()
                } else {
                    format!("\"{name}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        conn.execute_batch(&format!(
            "INSERT INTO {table} SELECT {columns} FROM {table} WHERE Variant_ID=1"
        ))
        .unwrap();
    }
    conn.execute_batch(
        "UPDATE Infeeder SET Ug=12 WHERE Variant_ID=2;
        UPDATE Load SET P1=0.01 WHERE Variant_ID=2",
    )
    .unwrap();
    let bytes = conn.serialize("main").unwrap();
    assert!(NativeDatabase::decode(&bytes, None).is_err());
    let first = NativeDatabase::decode(&bytes, Some(1))
        .unwrap()
        .network()
        .unwrap();
    let second = NativeDatabase::decode(&bytes, Some(2))
        .unwrap()
        .network()
        .unwrap();
    assert!((first.sources()[0].v_magnitude[0] - 11000.0 / 3.0_f64.sqrt()).abs() < 1e-9);
    assert!((second.sources()[0].v_magnitude[0] - 12000.0 / 3.0_f64.sqrt()).abs() < 1e-9);
    assert_eq!(first.loads()[0].p_nom, [2000.0, 4000.0, 6000.0]);
    assert_eq!(second.loads()[0].p_nom, [20000.0, 4000.0, 6000.0]);
    assert_eq!(first.lines(), second.lines());
    assert_eq!(first.shunts(), second.shunts());
}

#[test]
fn fresh_pmd_keeps_the_assembled_network_and_reports_projection_losses() {
    for edit in [
        "UPDATE Terminal SET Flag_State=0 WHERE Terminal_ID=61",
        "UPDATE Terminal SET Flag_State=0 WHERE Terminal_ID=61;
         UPDATE TwoWindingTransformer SET VecGrp=6, AddRotate=150",
    ] {
        check_assembled_pmd(edit);
    }
}

fn check_assembled_pmd(edit: &str) {
    let net = native(edit).network().unwrap();
    let output = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
    assert!(!output.diagnostics.is_empty());
    assert!(
        output.diagnostics.iter().all(|d| matches!(
            d.code(),
            "EMIT.PMD.VALUE_SUBSTITUTED" | "EMIT.PMD.VALUE_DEFAULTED" | "EMIT.PMD.FIELD_DROPPED"
        )),
        "{:?}",
        output.diagnostics
    );
    let parsed = crate::testkit::parse_str(&output.text, "pmd-json").unwrap();
    crate::require_electrical_readiness(&parsed).unwrap();
    assert_eq!(parsed.lines().len(), 1);
    assert_eq!(parsed.lines()[0].name, net.lines()[0].name);
    assert_eq!(parsed.lines()[0].bus_from, net.lines()[0].bus_from);
    assert_eq!(parsed.lines()[0].bus_to, net.lines()[0].bus_to);
    assert_eq!(
        parsed.lines()[0].terminal_map_from,
        net.lines()[0].terminal_map_from
    );
    assert_eq!(
        parsed.lines()[0].terminal_map_to,
        net.lines()[0].terminal_map_to
    );
    assert!((parsed.lines()[0].length - net.lines()[0].length).abs() < 1e-12);
    assert_eq!(parsed.loads().len(), 1);
    assert_eq!(parsed.loads()[0].p_nom, net.loads()[0].p_nom);
    assert_eq!(
        parsed.loads()[0].voltage_model,
        net.loads()[0].voltage_model
    );
    assert_eq!(parsed.sources().len(), 1);
    assert_eq!(parsed.switches().iter().filter(|s| s.open).count(), 1);
    for (a, b) in parsed.sources()[0]
        .v_magnitude
        .iter()
        .zip(&net.sources()[0].v_magnitude)
    {
        assert!((a - b).abs() < 1e-9);
    }
    for (a, b) in parsed.shunts()[0]
        .g
        .iter()
        .flatten()
        .chain(parsed.shunts()[0].b.iter().flatten())
        .zip(
            net.shunts()[0]
                .g
                .iter()
                .flatten()
                .chain(net.shunts()[0].b.iter().flatten()),
        )
    {
        assert!((a - b).abs() < 1e-12 * b.abs().max(1.0));
    }
}

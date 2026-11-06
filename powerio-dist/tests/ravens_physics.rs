//! Electrical regressions independently identified against the native schema
//! and OpenDSS source/connection conventions (MG-RAVENS 094ad116).
mod helpers;

use helpers::{emit_value, parse_dss_str, parse_str};
use powerio_dist::{DistBus, DistSwitch, DistTargetFormat, MulticonductorNetwork};
use serde_json::{Value, json};

fn terminal(bus: &str, phases: &str) -> Value {
    json!({"Ravens.cimObjectType":"Terminal", "Terminal.ConnectivityNode":format!("ConnectivityNode::'{bus}'"), "Terminal.phases":format!("PhaseCode.{phases}"), "ACDCTerminal.sequenceNumber":1})
}

fn visit(value: &mut Value, kind: &str, action: &mut impl FnMut(&mut Value)) {
    if value.get("Ravens.cimObjectType").and_then(Value::as_str) == Some(kind) {
        action(value);
    }
    match value {
        Value::Object(map) => {
            for child in map.values_mut() {
                visit(child, kind, action);
            }
        }
        Value::Array(items) => {
            for child in items {
                visit(child, kind, action);
            }
        }
        _ => {}
    }
}

#[test]
fn source_sequence_impedance_is_an_electrical_branch() {
    let text = json!({"EnergySource":{"s":{
        "Ravens.cimObjectType":"EnergySource",
        "EnergySource.voltageMagnitude":400.0,
        "EnergySource.r":2.0,"EnergySource.r0":5.0,
        "EnergySource.x":3.0,"EnergySource.x0":9.0,
        "ConductingEquipment.Terminals":[terminal("port","ABC")]
    }}})
    .to_string();
    let net = parse_str(&text, "ravens-json").unwrap();
    assert_ne!(
        net.sources()[0].bus,
        "port",
        "EMF must be behind the impedance"
    );
    assert_eq!(net.lines().len(), 1);
    let line = &net.lines()[0];
    assert_eq!(line.bus_to, "port");
    let z = net.linecode(&line.linecode).unwrap();
    // Zabc = Z1 I + (Z0-Z1)/3 11', not a zero-impedance source.
    assert_eq!(
        z.r_series,
        vec![
            vec![3.0, 1.0, 1.0],
            vec![1.0, 3.0, 1.0],
            vec![1.0, 1.0, 3.0]
        ]
    );
    assert_eq!(
        z.x_series,
        vec![
            vec![5.0, 2.0, 2.0],
            vec![2.0, 5.0, 2.0],
            vec![2.0, 2.0, 5.0]
        ]
    );
    let first = net.emit_value(DistTargetFormat::RavensJson);
    let mut text = first.text.clone();
    for _ in 0..3 {
        let back = parse_str(&text, "ravens-json").unwrap();
        assert_eq!(back.lines().len(), 1, "do not apply source impedance twice");
        assert_eq!(back.buses().len(), 2, "do not grow auxiliary EMF buses");
        for source in back.sources() {
            assert!(
                ["r1", "x1", "r0", "x0"]
                    .iter()
                    .all(|key| !source.extras.contains_key(*key))
            );
        }
        text = back.emit_value(DistTargetFormat::RavensJson).text;
        assert_eq!(text, first.text);
    }
}

#[test]
fn inactive_equipment_does_not_enter_the_active_network() {
    let mut doc = json!({"EnergyConsumer":{"l":{
        "Ravens.cimObjectType":"EnergyConsumer", "Equipment.inService":false,
        "EnergyConsumer.p":30000.0,"EnergyConsumer.phaseConnection":"PhaseShuntConnectionKind.Y",
        "ConductingEquipment.Terminals":[terminal("b","ABC")]
    }},"EnergySource":{"s":{
        "Ravens.cimObjectType":"EnergySource", "Equipment.inService":false,
        "EnergySource.voltageMagnitude":400.0,"ConductingEquipment.Terminals":[terminal("b","ABC")]
    }}});
    let net = parse_str(&doc.to_string(), "ravens-json").unwrap();
    assert_eq!(net.loads().len(), 0);
    assert_eq!(net.sources().len(), 0);
    assert_eq!(net.untyped_objects().len(), 2);
    doc = serde_json::from_str(&net.emit_value(DistTargetFormat::RavensJson).text).unwrap();
    let mut found = 0;
    visit(&mut doc, "EnergyConsumer", &mut |v| {
        assert_eq!(v["Equipment.inService"], false);
        found += 1;
    });
    assert_eq!(found, 1);
}

#[test]
fn four_pole_switch_keeps_neutral_and_pair_order() {
    let map = vec!["3".into(), "2".into(), "1".into(), "4".into()];
    let mut net = MulticonductorNetwork::new();
    net.buses_mut().extend([
        DistBus::new("a", map.clone()),
        DistBus::new("b", map.clone()),
    ]);
    net.switches_mut().push(DistSwitch::new(
        "s",
        "a",
        "b",
        map.clone(),
        map.clone(),
        false,
    ));
    let emitted = emit_value(&net, DistTargetFormat::RavensJson);
    let back = parse_str(&emitted.text, "ravens-json").unwrap();
    assert_eq!(back.switches()[0].terminal_map_from, map);
    assert_eq!(back.switches()[0].terminal_map_to, map);
    assert!(emitted.text.contains("PhaseCode.ABCN"));
}

#[test]
fn load_voltage_reference_is_not_replaced_by_the_bus_base() {
    let net = parse_dss_str(
        "clear\nnew circuit.c basekv=0.4 bus1=b\nnew load.l bus1=b phases=3 kv=0.38 kw=30 kvar=3 model=2\n",
    );
    let back = parse_str(
        &net.emit_value(DistTargetFormat::RavensJson).text,
        "ravens-json",
    )
    .unwrap();
    for (a, b) in net.loads()[0]
        .voltage_model
        .v_nom()
        .iter()
        .zip(back.loads()[0].voltage_model.v_nom())
    {
        assert!((a - b).abs() < 1e-10);
    }
}

#[test]
fn finite_transformer_ground_impedance_does_not_ground_the_bus() {
    let net = parse_dss_str(
        "clear\nnew circuit.c phases=1 basekv=7.2 bus1=p.1\nnew transformer.t phases=1 windings=2 buses=(p.1,s.1.4) conns=(wye,wye) kvs=(7.2,0.24) kvas=(25,25) xhl=2 wdg=2 rneut=10 xneut=0\n",
    );
    let mut doc: Value =
        serde_json::from_str(&net.emit_value(DistTargetFormat::RavensJson).text).unwrap();
    // Native producers omit Ground records. Disable these generated records
    // so this test exercises TransformerEnd.grounded's finite-impedance case.
    visit(&mut doc, "Ground", &mut |v| {
        v["Equipment.inService"] = false.into();
    });
    let back = parse_str(&doc.to_string(), "ravens-json").unwrap();
    assert_eq!(back.bus("s").unwrap().grounded.len(), 0);
    assert_eq!(back.transformers()[0].windings[1].r_neutral, Some(10.0));
    let dss = back.emit_value(DistTargetFormat::Dss);
    assert!(dss.text.contains("s.1.4"), "{}", dss.text);
    assert!(dss.text.contains("rneut=10"), "{}", dss.text);
}

#[test]
fn delta_branch_codes_match_the_native_opendss_exporter() {
    // The native DssExport/RavensImport producer emits A for a branch 1-2,
    // B for 2-3, C for 3-1 (not a pair-valued Terminal PhaseCode).
    for (label, pair) in [("A", ["1", "2"]), ("B", ["2", "3"]), ("C", ["3", "1"])] {
        let doc = json!({"EnergyConsumer":{"l":{
            "Ravens.cimObjectType":"EnergyConsumer","EnergyConsumer.phaseConnection":"PhaseShuntConnectionKind.D",
            "EnergyConsumer.EnergyConsumerPhase":[{"Ravens.cimObjectType":"EnergyConsumerPhase","EnergyConsumerPhase.phase":format!("SinglePhaseKind.{label}"),"EnergyConsumerPhase.p":10000.0,"EnergyConsumerPhase.q":2000.0}],
            "ConductingEquipment.Terminals":[terminal("b",label)]
        }}});
        let net = parse_str(&doc.to_string(), "ravens-json").unwrap();
        assert_eq!(net.loads().len(), 1);
        assert_eq!(net.loads()[0].terminal_map, pair);
        let back = parse_str(
            &net.emit_value(DistTargetFormat::RavensJson).text,
            "ravens-json",
        )
        .unwrap();
        assert_eq!(back.loads()[0].terminal_map, pair);
        assert_eq!(back.loads()[0].p_nom, [10000.0]);
    }
}

#[test]
fn line_phase_sequence_orders_both_ends_and_the_impedance() {
    let net = parse_dss_str(
        "clear\nnew circuit.c basekv=0.4 bus1=a\nnew line.l bus1=a.3.2 bus2=b.3.2 phases=2 length=1 r1=1 x1=0 r0=1 x0=0\n",
    );
    let emitted = net.emit_value(DistTargetFormat::RavensJson);
    assert!(!emitted.text.contains("PhaseCode.CB"));
    let back = parse_str(&emitted.text, "ravens-json").unwrap();
    let line = back.lines().iter().find(|l| l.name == "l").unwrap();
    assert_eq!(line.terminal_map_from, ["3", "2"]);
    assert_eq!(line.terminal_map_to, ["3", "2"]);
}

#[test]
fn unsupported_conductor_projection_does_not_leave_invalid_empty_buses() {
    let net = helpers::parse_file(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/dist/micro/linecode_10x10.dss"),
        Some("dss"),
    )
    .unwrap();
    let emitted = net.emit_value(DistTargetFormat::RavensJson);
    assert!(
        emitted
            .warnings
            .iter()
            .any(|w| w.contains("RECORD_DROPPED") && w.contains("line l10"))
    );
    assert!(
        emitted
            .warnings
            .iter()
            .any(|w| w.contains("RECORD_DROPPED") && w.contains("load la"))
    );
    assert!(!emitted.text.contains("PhaseCode.ABCABCABC"));
    assert!(!emitted.text.contains("\"PhaseCode.\""));
    let back = parse_str(&emitted.text, "ravens-json").unwrap();
    assert!(back.buses().iter().all(|bus| !bus.terminals.is_empty()));
    assert!(
        back.untyped_objects()
            .iter()
            .any(|obj| obj.class == "ConnectivityNode" && obj.name == "loadbus")
    );
    let _ = back.emit_value(DistTargetFormat::Dss);
}

#[test]
fn one_sided_voltage_limit_survives() {
    let mut net = MulticonductorNetwork::new();
    let mut bus = DistBus::new("b", vec!["1".into(), "4".into()]);
    bus.v_max = Some(250.0);
    bus.grounded.push("4".into());
    net.buses_mut().push(bus);
    let back = parse_str(
        &emit_value(&net, DistTargetFormat::RavensJson).text,
        "ravens-json",
    )
    .unwrap();
    assert_eq!(back.bus("b").unwrap().v_min, None);
    assert_eq!(back.bus("b").unwrap().v_max, Some(250.0));
}

#[test]
fn unsupported_delta_inverter_is_diagnosed_instead_of_rewired() {
    let net = parse_dss_str(
        "clear\nnew circuit.c basekv=0.4 bus1=b\nnew pvsystem.pv bus1=b.1.2 phases=1 conn=delta kv=0.4 kva=10 pmpp=10\n",
    );
    let output = net.emit_value(DistTargetFormat::RavensJson);
    assert!(
        output
            .warnings
            .iter()
            .any(|w| w.contains("RECORD_DROPPED") && w.contains("ibr pv"))
    );
    let back = parse_str(&output.text, "ravens-json").unwrap();
    assert_eq!(back.ibrs().len(), 0);
}

#[test]
fn source_auxiliary_names_respect_case_insensitive_network_identity() {
    let doc = json!({
        "ConnectivityNode":{"_PIO_RAVENS_s_emf":{"Ravens.cimObjectType":"ConnectivityNode"}},
        "Ground":{"g":{"Ravens.cimObjectType":"Ground","ConductingEquipment.Terminals":[terminal("_PIO_RAVENS_s_emf","A")]}},
        "PerLengthPhaseImpedance":{"_PIO_RAVENS_s_impedance":{"Ravens.cimObjectType":"PerLengthPhaseImpedance","PerLengthPhaseImpedance.conductorCount":1,
            "PerLengthPhaseImpedance.PhaseImpedanceData":[{"PhaseImpedanceData.row":1,"PhaseImpedanceData.column":1,"PhaseImpedanceData.r":99.0}]}},
        "EnergySource":{"s":{"Ravens.cimObjectType":"EnergySource","EnergySource.voltageMagnitude":400.0,"EnergySource.r":1.0,"EnergySource.r0":1.0,"ConductingEquipment.Terminals":[terminal("b","ABC")]}}
    });
    let net = parse_str(&doc.to_string(), "ravens-json").unwrap();
    assert!(
        !net.sources()[0]
            .bus
            .eq_ignore_ascii_case("_PIO_RAVENS_s_emf")
    );
    assert!(
        !net.lines()[0]
            .linecode
            .eq_ignore_ascii_case("_PIO_RAVENS_s_impedance")
    );
    assert_eq!(
        net.linecode(&net.lines()[0].linecode)
            .unwrap()
            .r_series
            .len(),
        3
    );
}

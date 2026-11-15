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

/// A tank's clocks are its vector group shift, the delta orientation the
/// physics stamps: a delta–wye tank is clock 1 when the high voltage winding
/// leads (OpenDSS's default) and clock 11 when the low voltage winding leads
/// (`leadlag=euro`, or a step-up unit whose first winding is the low voltage
/// delta). The writer states that clock and the vector group, the reader
/// reads clock 11 back as `leadlag` euro, and a clock neither convention
/// represents is refused with the represented clock named.
#[test]
fn phase_angle_clock_follows_the_winding_layout() {
    let deck = |kvs: &str, leadlag: &str| {
        format!(
            "Clear
New Circuit.c basekv=115 pu=1 phases=3 bus1=src
New Transformer.sub phases=3 windings=2 buses=[src lv] conns=[delta wye] kvs=[{kvs}] kvas=[5000 5000] xhl=8 {leadlag}
New Load.l bus1=lv.1.2.3 phases=3 kv=4.16 kw=100 kvar=10
Set VoltageBases=[115 4.16]
Calcvoltagebases
Solve
"
        )
    };
    let clocks_of = |doc: &mut Value| {
        let mut clocks = Vec::new();
        visit(doc, "TransformerEndInfo", &mut |info| {
            clocks.push((
                info["TransformerEndInfo.endNumber"].as_u64().unwrap(),
                info["TransformerEndInfo.phaseAngleClock"].as_f64().unwrap(),
            ));
        });
        clocks.sort_by_key(|(end, _)| *end);
        clocks
    };
    let shift = |net: &MulticonductorNetwork| {
        powerio_dist::transformer::calc_transformer_phase_shift(&net.transformers()[0]).unwrap()
    };
    for (kvs, leadlag, clock, group, degrees) in [
        ("115 4.16", "", 1.0, "Dyn1", 30.0),
        ("115 4.16", "leadlag=euro", 11.0, "Dyn11", -30.0),
        ("115 4.16", "leadlag=lead", 11.0, "Dyn11", -30.0),
        ("4.16 115", "", 11.0, "Dyn11", -30.0),
    ] {
        let source = parse_dss_str(&deck(kvs, leadlag));
        assert_eq!(shift(&source), degrees, "{kvs} {leadlag}");
        let out = source.emit_value(DistTargetFormat::RavensJson);
        assert!(
            !out.warnings.iter().any(|w| w.contains("leadlag")),
            "{:#?}",
            out.warnings
        );
        let mut doc: Value = serde_json::from_str(&out.text).unwrap();
        assert_eq!(
            clocks_of(&mut doc),
            [(1, 0.0), (2, clock)],
            "{kvs} {leadlag}"
        );
        let mut groups = Vec::new();
        visit(&mut doc, "PowerTransformer", &mut |pt| {
            groups.push(pt["PowerTransformer.vectorGroup"].clone());
        });
        assert_eq!(groups, [json!(group)]);
        let read = parse_str(&out.text, "ravens-json").unwrap();
        assert_eq!(read.transformers().len(), 1, "{:#?}", read.warnings);
        assert!(
            !read.warnings.iter().any(|w| w.contains("phaseAngleClock")),
            "{:#?}",
            read.warnings
        );
        assert_eq!(shift(&read), degrees, "{kvs} {leadlag}");
    }

    let mut doc: Value = serde_json::from_str(
        &parse_dss_str(&deck("115 4.16", ""))
            .emit_value(DistTargetFormat::RavensJson)
            .text,
    )
    .unwrap();
    visit(&mut doc, "TransformerEndInfo", &mut |info| {
        if info["TransformerEndInfo.endNumber"].as_u64() == Some(2) {
            info["TransformerEndInfo.phaseAngleClock"] = json!(5);
        }
    });
    let refused = parse_str(&doc.to_string(), "ravens-json").unwrap();
    assert_eq!(refused.transformers().len(), 0);
    assert!(
        refused.warnings.iter().any(|w| w.contains(
            "end 2 phaseAngleClock 5 is not represented by its winding layout (clock 1, or its \
             European counterpart)"
        )),
        "{:#?}",
        refused.warnings
    );
}

/// A single phase wye load stating `grounded` false whose neutral no
/// equipment but such loads reaches hangs from a floating node, so it does
/// not draw its stated power. The reader keeps the statement, as a round
/// trip needs, and says so for every such load; a neutral a four wire line
/// carries is not reported.
#[test]
fn isolated_single_phase_load_neutrals_are_reported() {
    let deck = |line_conductors: &str| {
        let n = line_conductors.split('.').count();
        let diagonal = |v: &str| {
            (0..n)
                .map(|i| {
                    (0..=i)
                        .map(|j| if i == j { v } else { "0" })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        format!(
            "Clear
New Circuit.c basekv=4.16 pu=1 phases=3 bus1=src
New Linecode.lc nphases={n} rmatrix=[{r}] xmatrix=[{x}] units=km
New Line.l bus1=src.{line_conductors} bus2=lv.{line_conductors} linecode=lc length=1 units=km phases={n}
New Load.a bus1=lv.1.4 phases=1 kv=2.4 kw=100 kvar=10
New Load.b bus1=lv.2.4 phases=1 kv=2.4 kw=50 kvar=5
Set VoltageBases=[4.16]
Calcvoltagebases
Solve
",
            r = diagonal("0.1"),
            x = diagonal("0.2"),
        )
    };
    let read = |conductors: &str| {
        let written = parse_dss_str(&deck(conductors))
            .emit_value(DistTargetFormat::RavensJson)
            .text;
        parse_str(&written, "ravens-json").unwrap()
    };
    let three_wire = read("1.2.3");
    let reported: Vec<&String> = three_wire
        .warnings
        .iter()
        .filter(|w| w.contains("the neutral floats"))
        .collect();
    assert_eq!(reported.len(), 2, "{:#?}", three_wire.warnings);
    assert!(reported.iter().any(|w| w.contains("load a:")));
    assert!(reported.iter().any(|w| w.contains("load b:")));
    // Kept as stated: the neutral is not grounded.
    assert!(
        !three_wire
            .bus("lv")
            .unwrap()
            .grounded
            .iter()
            .any(|t| t == "4")
    );

    let four_wire = read("1.2.3.4");
    assert!(
        !four_wire
            .warnings
            .iter()
            .any(|w| w.contains("the neutral floats")),
        "{:#?}",
        four_wire.warnings
    );
}

//! Independent hand-calculated magnitude and electrical-state regressions.

mod helpers;
use helpers::{emit_value, parse_str};
use powerio_dist::DistTargetFormat;

fn tank() -> String {
    r##"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cim="http://iec.ch/TC57/CIM100#">
 <cim:ConnectivityNode rdf:ID="_a"><cim:IdentifiedObject.name>a</cim:IdentifiedObject.name></cim:ConnectivityNode>
 <cim:ConnectivityNode rdf:ID="_b"><cim:IdentifiedObject.name>b</cim:IdentifiedObject.name></cim:ConnectivityNode>
 <cim:PowerTransformer rdf:ID="_pt"><cim:IdentifiedObject.name>pt</cim:IdentifiedObject.name></cim:PowerTransformer>
 <cim:TransformerTank rdf:ID="_tank"><cim:IdentifiedObject.name>tank</cim:IdentifiedObject.name><cim:TransformerTank.PowerTransformer rdf:resource="#_pt"/></cim:TransformerTank>
 <cim:Terminal rdf:ID="_t1"><cim:Terminal.ConductingEquipment rdf:resource="#_pt"/><cim:Terminal.ConnectivityNode rdf:resource="#_a"/></cim:Terminal>
 <cim:Terminal rdf:ID="_t2"><cim:Terminal.ConductingEquipment rdf:resource="#_pt"/><cim:Terminal.ConnectivityNode rdf:resource="#_b"/></cim:Terminal>
 <cim:TransformerTankEnd rdf:ID="_e1"><cim:TransformerTankEnd.TransformerTank rdf:resource="#_tank"/><cim:TransformerEnd.endNumber>1</cim:TransformerEnd.endNumber><cim:TransformerEnd.Terminal rdf:resource="#_t1"/><cim:TransformerTankEnd.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.A"/></cim:TransformerTankEnd>
 <cim:TransformerTankEnd rdf:ID="_e2"><cim:TransformerTankEnd.TransformerTank rdf:resource="#_tank"/><cim:TransformerEnd.endNumber>2</cim:TransformerEnd.endNumber><cim:TransformerEnd.Terminal rdf:resource="#_t2"/><cim:TransformerTankEnd.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.A"/></cim:TransformerTankEnd>
 <cim:TransformerTankInfo rdf:ID="_info"/>
 <cim:TransformerEndInfo rdf:ID="_i1"><cim:TransformerEndInfo.TransformerTankInfo rdf:resource="#_info"/><cim:TransformerEndInfo.endNumber>1</cim:TransformerEndInfo.endNumber><cim:TransformerEndInfo.connectionKind rdf:resource="http://iec.ch/TC57/CIM100#WindingConnection.I"/><cim:TransformerEndInfo.phaseAngleClock>0</cim:TransformerEndInfo.phaseAngleClock><cim:TransformerEndInfo.ratedU>1000</cim:TransformerEndInfo.ratedU><cim:TransformerEndInfo.ratedS>100000</cim:TransformerEndInfo.ratedS><cim:TransformerEndInfo.r>0.15</cim:TransformerEndInfo.r></cim:TransformerEndInfo>
 <cim:TransformerEndInfo rdf:ID="_i2"><cim:TransformerEndInfo.TransformerTankInfo rdf:resource="#_info"/><cim:TransformerEndInfo.endNumber>2</cim:TransformerEndInfo.endNumber><cim:TransformerEndInfo.connectionKind rdf:resource="http://iec.ch/TC57/CIM100#WindingConnection.I"/><cim:TransformerEndInfo.phaseAngleClock>0</cim:TransformerEndInfo.phaseAngleClock><cim:TransformerEndInfo.ratedU>1000</cim:TransformerEndInfo.ratedU><cim:TransformerEndInfo.ratedS>100000</cim:TransformerEndInfo.ratedS><cim:TransformerEndInfo.r>0.15</cim:TransformerEndInfo.r></cim:TransformerEndInfo>
 <cim:ShortCircuitTest rdf:ID="_sc"><cim:ShortCircuitTest.EnergisedEnd rdf:resource="#_i1"/><cim:ShortCircuitTest.GroundedEnds rdf:resource="#_i2"/><cim:ShortCircuitTest.leakageImpedance>0.5</cim:ShortCircuitTest.leakageImpedance><cim:ShortCircuitTest.loss>3</cim:ShortCircuitTest.loss><cim:TransformerTest.basePower>100000</cim:TransformerTest.basePower></cim:ShortCircuitTest>
 <cim:Asset rdf:ID="_asset"><cim:Asset.AssetInfo rdf:resource="#_info"/><cim:Asset.PowerSystemResources rdf:resource="#_tank"/></cim:Asset>
 </rdf:RDF>"##.to_string()
}

#[test]
fn magnitude_and_nonzero_resistance_give_hand_calculated_reactance() {
    // 1 kV / 100 kVA gives 10 ohm; R=.3 and |Z|=.5 imply X=.4 ohm = 4%.
    let net = parse_str(&tank(), "cim-dist").unwrap();
    assert!((net.transformers()[0].xsc_pct[0] - 4.0).abs() < 1e-12);
    // A producer-specific X=.4 input is not silently interpreted as X.
    let xml = tank().replace("leakageImpedance>0.5", "leakageImpedance>0.4");
    let net = parse_str(&xml, "cim-dist").unwrap();
    assert!((net.transformers()[0].xsc_pct[0] - 7.0_f64.sqrt()).abs() < 1e-12);
}

#[test]
fn zero_test_loss_does_not_erase_nameplate_resistance_from_magnitude() {
    let xml = tank().replace("ShortCircuitTest.loss>3", "ShortCircuitTest.loss>0");
    let net = parse_str(&xml, "cim-dist").unwrap();
    assert!((net.transformers()[0].xsc_pct[0] - 4.0).abs() < 1e-12);
}

#[test]
fn fresh_writer_states_magnitude_for_independent_consumers() {
    let net = parse_str(&tank(), "cim-dist").unwrap();
    let out = emit_value(&net, DistTargetFormat::CimDist);
    let value: f64 = out
        .text
        .split("<cim:ShortCircuitTest.leakageImpedance>")
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!((value - 0.5).abs() < 1e-12, "{value}");
}

#[test]
fn tank_and_parent_service_states_cannot_become_live_transformers() {
    for (class, id) in [("TransformerTank", "_tank"), ("PowerTransformer", "_pt")] {
        for property in ["inService", "normallyInService"] {
            let tag = format!("<cim:{class} rdf:ID=\"{id}\">");
            let xml = tank().replace(
                &tag,
                &format!("{tag}<cim:Equipment.{property}>false</cim:Equipment.{property}>"),
            );
            let error = parse_str(&xml, "cim-dist").unwrap_err().to_string();
            assert!(error.contains("out-of-service"), "{error}");
            let xml = xml.replace(&format!("<cim:Equipment.{property}>false"), &format!("<cim:Equipment.inService>true</cim:Equipment.inService><cim:Equipment.{property}>false"));
            if property == "normallyInService" {
                assert_eq!(parse_str(&xml, "cim-dist").unwrap().transformers().len(), 1);
            }
        }
    }
}

#[test]
fn either_tank_end_terminal_is_refused_when_disconnected() {
    for id in ["_t1", "_t2"] {
        let tag = format!("<cim:Terminal rdf:ID=\"{id}\">");
        let xml = tank().replace(
            &tag,
            &format!("{tag}<cim:ACDCTerminal.connected>false</cim:ACDCTerminal.connected>"),
        );
        for xml in [
            xml.clone(),
            xml.replace(
                "<cim:Terminal.ConductingEquipment rdf:resource=\"#_pt\"/>",
                "",
            ),
        ] {
            let error = parse_str(&xml, "cim-dist").unwrap_err().to_string();
            assert!(error.contains("connected is false"), "{error}");
        }
    }
}

fn with_dy_clock(clock: &str) -> String {
    let source = include_str!("../../tests/data/dist/cim/tank_feeder.xml");
    let start = source
        .find("<cim:TransformerEndInfo rdf:ID=\"_dy_info_e2\">")
        .unwrap();
    let end = start + source[start..].find("</cim:TransformerEndInfo>").unwrap();
    let block =
        source[start..end].replace("phaseAngleClock>1<", &format!("phaseAngleClock>{clock}<"));
    let mut xml = source.to_owned();
    xml.replace_range(start..end, &block);
    xml
}

#[test]
fn clock_one_is_represented_and_clock_eleven_is_explicitly_refused() {
    parse_str(&with_dy_clock("1"), "cim-dist").unwrap();
    for clock in ["11", "0", "-1", "12", "1.5", "NaN", "bad"] {
        let error = parse_str(&with_dy_clock(clock), "cim-dist")
            .unwrap_err()
            .to_string();
        assert!(error.contains("phase-angle clock"), "{clock}: {error}");
    }
}

#[test]
fn writer_derives_clocks_from_current_windings_and_reports_stale_metadata() {
    let mut net = parse_str(&with_dy_clock("1"), "cim-dist").unwrap();
    let dy = net
        .transformers_mut()
        .iter_mut()
        .find(|t| t.name == "dy_tank")
        .unwrap();
    dy.extras
        .insert("cim_phase_angle_clock".into(), serde_json::json!([0, 11]));
    let out = emit_value(&net, DistTargetFormat::CimDist);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("retained phase-angle clocks"))
    );
    let back = parse_str(&out.text, "cim-dist").unwrap();
    let dy = back
        .transformers()
        .iter()
        .find(|t| t.name == "dy_tank")
        .unwrap();
    assert_eq!(
        dy.extras["cim_phase_angle_clock"],
        serde_json::json!([0.0, 1.0])
    );
}

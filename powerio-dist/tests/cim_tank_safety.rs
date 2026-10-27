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

/// A three-winding tank on one phase: every end is rated 1 kV, 100 kVA,
/// states no resistance, and every pair is tested at |Z| = 0.5 ohm. Only
/// the 1-2 test states a load loss.
fn three_winding_tank(losses: [Option<&str>; 3]) -> String {
    let end = |n: u8| {
        format!(
            r##" <cim:ConnectivityNode rdf:ID="_n{n}"><cim:IdentifiedObject.name>n{n}</cim:IdentifiedObject.name></cim:ConnectivityNode>
 <cim:Terminal rdf:ID="_t{n}"><cim:Terminal.ConductingEquipment rdf:resource="#_pt"/><cim:Terminal.ConnectivityNode rdf:resource="#_n{n}"/></cim:Terminal>
 <cim:TransformerTankEnd rdf:ID="_e{n}"><cim:TransformerTankEnd.TransformerTank rdf:resource="#_tank"/><cim:TransformerEnd.endNumber>{n}</cim:TransformerEnd.endNumber><cim:TransformerEnd.Terminal rdf:resource="#_t{n}"/><cim:TransformerTankEnd.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.A"/></cim:TransformerTankEnd>
 <cim:TransformerEndInfo rdf:ID="_i{n}"><cim:TransformerEndInfo.TransformerTankInfo rdf:resource="#_info"/><cim:TransformerEndInfo.endNumber>{n}</cim:TransformerEndInfo.endNumber><cim:TransformerEndInfo.connectionKind rdf:resource="http://iec.ch/TC57/CIM100#WindingConnection.I"/><cim:TransformerEndInfo.phaseAngleClock>0</cim:TransformerEndInfo.phaseAngleClock><cim:TransformerEndInfo.ratedU>1000</cim:TransformerEndInfo.ratedU><cim:TransformerEndInfo.ratedS>100000</cim:TransformerEndInfo.ratedS></cim:TransformerEndInfo>
"##
        )
    };
    let test = |id: &str, from: u8, to: u8, loss: Option<&str>| {
        let loss = loss.map_or_else(String::new, |l| {
            format!("<cim:ShortCircuitTest.loss>{l}</cim:ShortCircuitTest.loss>")
        });
        format!(
            r##" <cim:ShortCircuitTest rdf:ID="_{id}"><cim:ShortCircuitTest.EnergisedEnd rdf:resource="#_i{from}"/><cim:ShortCircuitTest.GroundedEnds rdf:resource="#_i{to}"/><cim:ShortCircuitTest.leakageImpedance>0.5</cim:ShortCircuitTest.leakageImpedance>{loss}<cim:TransformerTest.basePower>100000</cim:TransformerTest.basePower></cim:ShortCircuitTest>
"##
        )
    };
    format!(
        r##"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cim="http://iec.ch/TC57/CIM100#">
 <cim:PowerTransformer rdf:ID="_pt"><cim:IdentifiedObject.name>pt</cim:IdentifiedObject.name></cim:PowerTransformer>
 <cim:TransformerTank rdf:ID="_tank"><cim:IdentifiedObject.name>tank</cim:IdentifiedObject.name><cim:TransformerTank.PowerTransformer rdf:resource="#_pt"/></cim:TransformerTank>
 <cim:TransformerTankInfo rdf:ID="_info"/>
{}{}{}{}{}{} <cim:Asset rdf:ID="_asset"><cim:Asset.AssetInfo rdf:resource="#_info"/><cim:Asset.PowerSystemResources rdf:resource="#_tank"/></cim:Asset>
</rdf:RDF>"##,
        end(1),
        end(2),
        end(3),
        test("sc12", 1, 2, losses[0]),
        test("sc13", 1, 3, losses[1]),
        test("sc23", 2, 3, losses[2]),
    )
}

#[test]
fn three_winding_split_with_unstated_pair_loss_carries_no_negative_resistance() {
    // Only pair 1-2 states a loss (the 2-3 test states 0).
    let net = parse_str(
        &three_winding_tank([Some("3"), None, Some("0")]),
        "cim-dist",
    )
    .unwrap();
    // Counting the unstated pairs as 0 would give r1 = r2 = rab/2 and
    // r3 = -rab/2. The split is not carried and the reader says so.
    for w in &net.transformers()[0].windings {
        assert!(w.r_pct.abs() < 1e-12, "{}", w.r_pct);
    }
    let notes: Vec<_> = net
        .warnings
        .iter()
        .filter(|w| w.starts_with("READ.CIM_DIST.VALUE_UNSUPPORTED"))
        .collect();
    assert_eq!(notes.len(), 1, "{:#?}", net.warnings);
    assert!(notes[0].contains("1-3, 2-3"), "{}", notes[0]);
    let dss = helpers::emit_dss(&net);
    assert!(!dss.text.contains("%r=-") && !dss.text.contains("%rs=-"));
}

#[test]
fn three_winding_split_with_every_pair_loss_is_still_carried() {
    let net = parse_str(
        &three_winding_tank([Some("3"), Some("3"), Some("3")]),
        "cim-dist",
    )
    .unwrap();
    // Each pair loses 3 kW on 100 kVA: 3% pair resistance, 1.5% per winding.
    for w in &net.transformers()[0].windings {
        assert!((w.r_pct - 1.5).abs() < 1e-9, "{}", w.r_pct);
    }
}

#[test]
fn short_circuit_test_without_leakage_gets_one_note() {
    let xml = tank().replace(
        "<cim:ShortCircuitTest.leakageImpedance>0.5</cim:ShortCircuitTest.leakageImpedance>",
        "",
    );
    let net = parse_str(&xml, "cim-dist").unwrap();
    assert!(net.transformers()[0].xsc_pct[0].abs() < 1e-12);
    let leakage: Vec<_> = net
        .warnings
        .iter()
        .filter(|w| w.contains("ShortCircuitTest") || w.contains("leakage"))
        .collect();
    assert_eq!(leakage.len(), 1, "{:#?}", net.warnings);
    assert!(
        leakage[0].starts_with("READ.CIM_DIST.VALUE_DEFAULTED"),
        "{}",
        leakage[0]
    );
}

/// A two-winding `PowerTransformerEnd` transformer, delta primary and
/// grounded wye secondary, the Dyn layout of `tank_feeder.xml`.
fn dyn_end_form(clock: Option<&str>) -> String {
    let end = |n: u8, kind: &str| {
        let clock = clock.filter(|_| n == 2).map_or_else(String::new, |c| {
            format!(
                "<cim:PowerTransformerEnd.phaseAngleClock>{c}</cim:PowerTransformerEnd.phaseAngleClock>"
            )
        });
        format!(
            r##" <cim:ConnectivityNode rdf:ID="_n{n}"><cim:IdentifiedObject.name>n{n}</cim:IdentifiedObject.name></cim:ConnectivityNode>
 <cim:Terminal rdf:ID="_t{n}"><cim:Terminal.ConductingEquipment rdf:resource="#_pt"/><cim:Terminal.ConnectivityNode rdf:resource="#_n{n}"/><cim:Terminal.phases rdf:resource="http://iec.ch/TC57/CIM100#PhaseCode.ABC"/><cim:ACDCTerminal.sequenceNumber>{n}</cim:ACDCTerminal.sequenceNumber></cim:Terminal>
 <cim:PowerTransformerEnd rdf:ID="_e{n}"><cim:PowerTransformerEnd.PowerTransformer rdf:resource="#_pt"/><cim:TransformerEnd.endNumber>{n}</cim:TransformerEnd.endNumber><cim:TransformerEnd.Terminal rdf:resource="#_t{n}"/><cim:PowerTransformerEnd.ratedU>12470</cim:PowerTransformerEnd.ratedU><cim:PowerTransformerEnd.ratedS>1000000</cim:PowerTransformerEnd.ratedS><cim:PowerTransformerEnd.connectionKind rdf:resource="http://iec.ch/TC57/CIM100#WindingConnection.{kind}"/>{clock}</cim:PowerTransformerEnd>
"##
        )
    };
    format!(
        r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:cim="http://iec.ch/TC57/CIM100#">
 <cim:PowerTransformer rdf:ID="_pt"><cim:IdentifiedObject.name>pt</cim:IdentifiedObject.name></cim:PowerTransformer>
{}{}</rdf:RDF>"#,
        end(1, "D"),
        end(2, "Yn"),
    )
}

#[test]
fn end_form_refuses_the_clocks_the_tank_form_refuses() {
    // A Dyn layout represents clock 1 on the secondary.
    parse_str(&dyn_end_form(None), "cim-dist").unwrap();
    parse_str(&dyn_end_form(Some("1")), "cim-dist").unwrap();
    for clock in ["11", "0", "-1", "12", "1.5", "NaN", "bad"] {
        let error = parse_str(&dyn_end_form(Some(clock)), "cim-dist")
            .unwrap_err()
            .to_string();
        assert!(error.contains("phase-angle clock"), "{clock}: {error}");
    }
    let error = parse_str(&dyn_end_form(Some("11")), "cim-dist")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("not represented by its winding layout (clock"),
        "{error}"
    );
}

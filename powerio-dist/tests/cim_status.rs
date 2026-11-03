//! Source electrical state must not become an energized connection.

mod helpers;
use helpers::parse_str;

const FEEDER: &str = include_str!("../../tests/data/dist/cim/micro_feeder.xml");

fn insert(class: &str, id: &str, property: &str) -> String {
    let tag = format!("<cim:{class} rdf:ID=\"{id}\">");
    assert_eq!(FEEDER.matches(&tag).count(), 1);
    FEEDER.replacen(&tag, &format!("{tag}{property}"), 1)
}

#[test]
fn inactive_non_switch_equipment_is_refused_before_becoming_live() {
    for (class, id) in [
        ("ACLineSegment", "_line1"),
        ("EnergyConsumer", "_load1"),
        ("LinearShuntCompensator", "_cap1"),
        ("PowerTransformer", "_xfm1"),
        ("EnergySource", "_src"),
    ] {
        for property in ["inService", "normallyInService"] {
            let xml = insert(
                class,
                id,
                &format!("<cim:Equipment.{property}>false</cim:Equipment.{property}>"),
            );
            let error = parse_str(&xml, "cim-dist").unwrap_err().to_string();
            assert!(
                error.contains(id.trim_start_matches('_')) && error.contains("out-of-service"),
                "{error}"
            );
        }
        // The actual state takes precedence over the normal-state fallback.
        let xml = insert(
            class,
            id,
            "<cim:Equipment.inService>true</cim:Equipment.inService><cim:Equipment.normallyInService>false</cim:Equipment.normallyInService>",
        );
        let net = parse_str(&xml, "cim-dist").unwrap();
        assert_eq!(net.transformers().len(), 1);
        assert_eq!(net.lines().len(), 1);
    }
}

#[test]
fn disconnected_non_switch_terminals_are_refused() {
    for id in [
        "_t_l1_1", "_t_l1_2", "_t_load1", "_t_cap1", "_t_src", "_t_xf1", "_t_xf2",
    ] {
        let xml = insert(
            "Terminal",
            id,
            "<cim:ACDCTerminal.connected>false</cim:ACDCTerminal.connected>",
        );
        let error = parse_str(&xml, "cim-dist").unwrap_err().to_string();
        assert!(
            error.contains(id.trim_start_matches('_')) && error.contains("connected is false"),
            "{error}"
        );
        let connected = xml.replace("connected>false", "connected>true");
        parse_str(&connected, "cim-dist").unwrap();
    }
}

#[test]
fn transformer_end_terminal_cannot_bypass_state_check_without_equipment_link() {
    let xml = insert(
        "Terminal",
        "_t_xf2",
        "<cim:ACDCTerminal.connected>false</cim:ACDCTerminal.connected>",
    );
    let xml = xml.replace(
        "<cim:Terminal.ConductingEquipment rdf:resource=\"#_xfm1\"/>",
        "",
    );
    let error = parse_str(&xml, "cim-dist").unwrap_err().to_string();
    assert!(
        error.contains("t_xf2") && error.contains("connected is false"),
        "{error}"
    );
}

#[test]
fn either_disconnected_switch_terminal_opens_the_graph_edge() {
    for id in ["_t_sw1_1", "_t_sw1_2"] {
        let xml = insert(
            "Terminal",
            id,
            "<cim:ACDCTerminal.connected>false</cim:ACDCTerminal.connected>",
        );
        let net = parse_str(&xml, "cim-dist").unwrap();
        assert!(net.switches()[0].open);
        let graph = net.to_graph();
        let edge = graph.edges.iter().find(|edge| edge.id == "sw1").unwrap();
        assert!(!edge.closed);
        assert!(
            net.warnings
                .iter()
                .any(|w| w.contains("disconnected terminal") && w.contains("open switch"))
        );
    }
}

#[test]
fn disconnected_switch_wins_over_explicit_closed_state() {
    let xml = insert(
        "Terminal",
        "_t_sw1_1",
        "<cim:ACDCTerminal.connected>false</cim:ACDCTerminal.connected>",
    );
    let xml = xml.replace("<cim:Switch.open>false</cim:Switch.open>", "<cim:Switch.open>false</cim:Switch.open><cim:Equipment.inService>true</cim:Equipment.inService>");
    assert!(parse_str(&xml, "cim-dist").unwrap().switches()[0].open);
}

#[test]
fn per_phase_delta_compensator_is_refused_instead_of_grounded() {
    let xml = insert(
        "LinearShuntCompensator",
        "_cap1",
        "<cim:ShuntCompensator.phaseConnection rdf:resource=\"http://iec.ch/TC57/CIM100#PhaseShuntConnectionKind.D\"/>",
    );
    let error = parse_str(&xml, "cim-dist").unwrap_err().to_string();
    assert!(
        error.contains("cap1") && error.contains("per-phase delta"),
        "{error}"
    );
    // The original phase-to-ground fixture remains supported.
    assert_eq!(parse_str(FEEDER, "cim-dist").unwrap().shunts().len(), 1);
}

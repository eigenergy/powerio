use super::{mapping_report::ObservedInput, mapping_tests::native, read};
use powerio_core::Source;

#[test]
fn report_uses_production_mapping_and_keeps_input_bytes_unchanged() {
    let db = native("");
    let before = db.connection.serialize("main").unwrap().to_vec();
    let report = db.mapping_report().unwrap();
    assert!(report.all_components_map());
    assert!(db.network().is_ok());
    assert_eq!(report.components.len(), 4);
    assert_eq!(
        report
            .components
            .iter()
            .map(|c| c.element)
            .collect::<Vec<_>>(),
        [30, 31, 32, 33]
    );
    assert!(report.components.iter().all(|c| c.first_error.is_none()));
    assert_eq!(db.connection.serialize("main").unwrap().to_vec(), before);
}

#[test]
fn report_collects_independent_failures_without_returning_a_partial_network() {
    let db = native(
        "UPDATE Element SET Flag_Input=2 WHERE Element_ID=31;
        UPDATE Line SET x0=NULL;
        UPDATE Infeeder SET Flag_Lf=999;",
    );
    let report = db.mapping_report().unwrap();
    assert!(!report.all_components_map());
    assert!(db.network().is_err());
    assert_eq!(
        report
            .components
            .iter()
            .filter(|c| c.component_maps)
            .count(),
        1
    );
    let line = report.components[0].zero_sequence.as_ref().unwrap();
    assert_eq!(line.fields["Line.x0"], ObservedInput::Null);
    assert_eq!(line.findings.len(), 1);
    assert_eq!(line.findings[0].field, "Line.x0");
    let load = report.components[1].zero_sequence.as_ref().unwrap();
    assert_eq!(load.findings.len(), 1);
    assert_eq!(load.findings[0].field, "Element.Flag_Input");
    // A stored selector/zero does not activate the category that is absent.
    assert_eq!(load.fields["Load.Flag_Z0_Input"], ObservedInput::Integer(3));
    assert!(load.findings[0].reason.contains("inactive"));
    assert!(
        report.components[2]
            .first_error
            .as_ref()
            .unwrap()
            .contains("999")
    );
    assert!(report.components[2].zero_sequence.is_none());
}

#[test]
fn explicit_zero_is_present_even_when_its_circuit_mode_is_not_implemented() {
    let db = native(
        "ALTER TABLE Load ADD COLUMN R0 REAL; ALTER TABLE Load ADD COLUMN X0 REAL;
        UPDATE Load SET Flag_Z0_Input=2, R0=0, X0=0;
        INSERT INTO Load (Element_ID,Variant_ID,Flag_Z0_Input,R0,X0) VALUES (31,2,2,99,99);",
    );
    let report = db.mapping_report().unwrap();
    let load = &report.components[1];
    assert!(!load.component_maps);
    assert!(
        load.first_error
            .as_ref()
            .unwrap()
            .contains("zero-sequence circuit resolution")
    );
    let evidence = load.zero_sequence.as_ref().unwrap();
    assert_eq!(evidence.fields["Load.R0"], ObservedInput::Real(0.0));
    assert_eq!(evidence.fields["Load.X0"], ObservedInput::Real(0.0));
    assert!(evidence.findings.is_empty());
    assert_eq!(report.variant, 1);
}

#[test]
fn evidence_distinguishes_absence_null_type_errors_and_ambiguity() {
    for (edit, expected) in [
        (
            "ALTER TABLE Line DROP COLUMN x0;",
            ObservedInput::MissingColumn,
        ),
        ("UPDATE Line SET x0=NULL;", ObservedInput::Null),
        (
            "UPDATE Line SET x0='unknown';",
            ObservedInput::InvalidType("text"),
        ),
        ("UPDATE Line SET x0=1e999;", ObservedInput::Nonfinite),
        ("DELETE FROM Line;", ObservedInput::MissingRow),
        ("DROP TABLE Line;", ObservedInput::MissingTable),
        (
            "INSERT INTO Line SELECT * FROM Line;",
            ObservedInput::AmbiguousRows,
        ),
    ] {
        let report = native(edit).mapping_report().unwrap();
        let line = &report.components[0];
        assert!(!line.component_maps, "{edit}");
        assert_eq!(
            line.zero_sequence.as_ref().unwrap().fields["Line.x0"],
            expected,
            "{edit}"
        );
        assert!(report.components[1..].iter().all(|c| c.component_maps));
    }
}

#[test]
fn global_context_failure_prevents_claiming_component_readiness() {
    let db = native("UPDATE CalcParameter SET Flag_LFZ0=0;");
    assert!(
        db.mapping_report()
            .unwrap_err()
            .to_string()
            .contains("calculation setting")
    );
    assert!(db.network().is_err());
}

#[test]
fn known_machine_ports_allow_audit_but_never_a_partial_network() {
    for (selector, expected) in [
        (1, vec!["1"]),
        (2, vec!["2"]),
        (3, vec!["3"]),
        (4, vec!["1", "2"]),
        (5, vec!["2", "3"]),
        (6, vec!["1", "3"]),
        (7, vec!["1", "2", "3"]),
    ] {
        // Move the source port to its own native node so no other equipment
        // can accidentally supply the machine's phase declaration.
        let db = native(&format!(
            "INSERT INTO Node SELECT * FROM Node WHERE Node_ID=10;
             UPDATE Node SET Node_ID=90 WHERE rowid=(SELECT MAX(rowid) FROM Node);
             UPDATE Element SET Type='SynchronousMachine' WHERE Element_ID=32;
             UPDATE Terminal SET Node_ID=90, Flag_Terminal={selector} WHERE Element_ID=32;"
        ));
        let before = db.connection.serialize("main").unwrap().to_vec();
        let topology = db.topology_draft().unwrap();
        let buses = topology.buses();
        let machine_bus = buses.iter().find(|bus| bus.id == "90").unwrap();
        assert_eq!(machine_bus.terminals, expected);
        assert!(machine_bus.grounded.is_empty());
        let report = db.mapping_report().unwrap();
        assert_eq!(report.components.len(), 4);
        assert!(!report.all_components_map());
        for component in &report.components {
            assert_eq!(component.component_maps, component.element != 32);
        }
        let machine = &report.components[2];
        assert_eq!(machine.element_type, "SynchronousMachine");
        assert!(
            machine
                .first_error
                .as_ref()
                .unwrap()
                .contains("SynchronousMachine")
        );
        assert!(db.network().is_err());
        assert_eq!(db.connection.serialize("main").unwrap().to_vec(), before);
    }
}

#[test]
fn machine_port_admission_does_not_guess_unknown_or_missing_connections() {
    for edit in [
        "UPDATE Terminal SET Flag_Terminal=NULL WHERE Element_ID=32;",
        "UPDATE Terminal SET Flag_Terminal=99 WHERE Element_ID=32;",
        "UPDATE Element SET Type='UnknownMachine' WHERE Element_ID=32;",
    ] {
        let db = native(&format!(
            "UPDATE Element SET Type='SynchronousMachine' WHERE Element_ID=32; {edit}"
        ));
        assert!(db.mapping_report().is_err());
        assert!(db.network().is_err());
    }
}

#[test]
fn licensed_native_report_accounts_for_all_components_and_missing_categories() {
    let bytes = include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let source = Source::from_memory("native.sinx", bytes.to_vec()).unwrap();
    let db = read(&source, None).unwrap();
    let report = db.mapping_report().unwrap();
    assert_eq!(report.components.len(), 32);
    assert_eq!(
        report
            .components
            .iter()
            .filter(|c| c.component_maps)
            .count(),
        6
    );
    let missing: Vec<_> = report
        .components
        .iter()
        .filter_map(|c| c.zero_sequence.as_ref())
        .collect();
    assert_eq!(missing.len(), 26);
    for input in missing {
        assert_eq!(input.findings.len(), 1);
        assert_eq!(input.findings[0].field, "Element.Flag_Input");
        assert!(input.findings[0].reason.contains("bit 0x4 is absent"));
    }
    assert!(!report.all_components_map());
    assert!(db.network().is_err());
}

#[test]
fn evidence_uses_sqlite_case_insensitive_identifiers() {
    let db = native(
        "ALTER TABLE Line RENAME TO temporary_line;
        ALTER TABLE temporary_line RENAME TO line;",
    );
    let report = db.mapping_report().unwrap();
    // The pinned native schema requires its original table spelling, but
    // the evidence query must not claim that the stored values are absent.
    assert!(db.network().is_err());
    assert!(!report.components[0].component_maps);
    assert!(matches!(
        report.components[0].zero_sequence.as_ref().unwrap().fields["Line.x0"],
        ObservedInput::Real(_) | ObservedInput::Integer(_)
    ));
    assert!(
        report.components[0]
            .zero_sequence
            .as_ref()
            .unwrap()
            .findings
            .is_empty()
    );
}

#[test]
fn report_does_not_evaluate_a_view_in_place_of_a_native_table() {
    let db = native(
        "ALTER TABLE Line RENAME TO stored_line;
        CREATE VIEW Line AS SELECT * FROM stored_line WHERE unknown_function()=1;",
    );
    let report = db.mapping_report().unwrap();
    let line = &report.components[0];
    assert!(!line.component_maps);
    assert!(
        line.first_error
            .as_ref()
            .unwrap()
            .contains("expected native table Line")
    );
    assert_eq!(
        line.zero_sequence.as_ref().unwrap().fields["Line.x0"],
        ObservedInput::UnsupportedObject
    );
    assert!(report.components[1..].iter().all(|c| c.component_maps));
}

#[test]
fn phase_pair_load_needs_no_inactive_zero_sequence_fields() {
    let db = native(
        "UPDATE Element SET Flag_Input=2 WHERE Element_ID=31; UPDATE Load SET Flag_Lf=15; UPDATE Terminal SET Flag_Terminal=4 WHERE Element_ID=31;",
    );
    let report = db.mapping_report().unwrap();
    let load = report.components.iter().find(|c| c.element == 31).unwrap();
    assert!(load.component_maps);
    let evidence = load.zero_sequence.as_ref().unwrap();
    assert!(evidence.findings.is_empty());
    assert_eq!(
        evidence.fields["Element.Flag_Input"],
        ObservedInput::Integer(2)
    );
    assert!(db.network().is_ok());
}

//! TARA operand, dispatch-boundary, and retained-rule regressions.
mod helpers;

use powerio_tx::{
    BalancedNetwork, ContingencySet, DispatchEntry, SubsystemDispatchRule, SubsystemSet,
    UnresolvedReason,
};

fn network() -> BalancedNetwork {
    helpers::parse_file(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/data/psse/contingency/resolve_v33.raw"),
        Some("psse"),
    )
    .unwrap()
    .network
}

fn case(action: &str) -> ContingencySet {
    ContingencySet::parse(&format!(
        "BUSNUMBERS\nCONTINGENCY 'operand regression'\n{action}\nEND\nEND\n"
    ))
    .unwrap()
    .set
}

fn components(set: &ContingencySet, net: &BalancedNetwork) -> Vec<(&'static str, usize)> {
    let resolution = set.resolve(net);
    assert_eq!(
        resolution.unresolved, 0,
        "{:?}",
        resolution.cases[0].unresolved
    );
    resolution.cases[0]
        .components
        .iter()
        .map(|component| (component.component_type, component.row))
        .collect()
}

#[test]
fn named_buses_work_with_every_optional_bus_keyword_combination() {
    let net = network();
    let expected_branch = components(&case("OPEN LINE FROM BUS 1 TO BUS 2 CKT '2'"), &net);
    for from_keyword in ["", "BUS "] {
        for to_keyword in ["", "BUS "] {
            let action =
                format!("OPEN LINE FROM {from_keyword}'B1 230' TO {to_keyword}'B2 230' CKT '2'");
            assert_eq!(
                components(&case(&action), &net),
                expected_branch,
                "{action}"
            );
        }
    }
    let expected_transformer = components(&case("OPEN THREEWINDING AT 2 TO 4 TO 5"), &net);
    for first_keyword in ["", "BUS "] {
        for second_keyword in ["", "BUS "] {
            for third_keyword in ["", "BUS "] {
                let action = format!(
                    "OPEN THREEWINDING AT {first_keyword}'B2 230' TO {second_keyword}'B4 138' TO {third_keyword}'B5 13.8'"
                );
                assert_eq!(
                    components(&case(&action), &net),
                    expected_transformer,
                    "{action}"
                );
            }
        }
    }
}

#[test]
fn numeric_bus_operands_keep_their_identity_inside_dispatch() {
    let mut net = network();
    // A name lookup of '2' would bind the wrong generator on bus 1.
    net.buses_mut()[0].name = Some("2".into());
    let expected = components(&case("REMOVE MACHINE 1 FROM BUS 2"), &net);
    assert_eq!(expected, [("generator", 3)]);
    for operand in ["2", "'2'", "\"2\""] {
        for keyword in ["", "BUS "] {
            let plain = format!("REMOVE MACHINE 1 FROM {keyword}{operand}");
            let dispatched = format!("{plain} DISPATCH\nSUBSYSTEM 'GEN'\nEND");
            let set = case(&dispatched);
            let view = set.cases[0].actions[0].calc_dispatched_action().unwrap();
            assert_eq!(view.action, case(&plain).cases[0].actions[0]);
            assert_eq!(components(&case(&plain), &net), expected, "{plain}");
            assert_eq!(components(&set, &net), expected, "{dispatched}");
        }
    }
}

#[test]
fn named_machine_and_change_actions_use_the_same_optional_bus_grammar() {
    let net = network();
    for (named, numbered) in [
        (
            "REMOVE MACHINE '1' FROM 'B2 230'",
            "REMOVE MACHINE '1' FROM 2",
        ),
        ("ADD UNIT '1' TO 'B2 230'", "ADD UNIT '1' TO 2"),
        (
            "SET 'B2 230' GENERATION TO 100 MW",
            "SET 2 GENERATION TO 100 MW",
        ),
    ] {
        let expected = components(&case(numbered), &net);
        assert_eq!(components(&case(named), &net), expected);
        assert_eq!(
            components(
                &case(&format!("{named} DISPATCH\nPARTICIPATING MACHINES\nEND")),
                &net
            ),
            expected
        );
    }
}

#[test]
fn a_quoted_bus_name_can_itself_be_the_bus_keyword() {
    let mut net = network();
    net.buses_mut()[0].name = Some("BUS".into());
    let expected = components(&case("OPEN LINE FROM 1 TO 2 CKT 2"), &net);
    for action in [
        "OPEN LINE FROM 'BUS' TO 'B2 230' CKT 2",
        "OPEN LINE FROM BUS 'BUS' TO BUS 'B2 230' CKT 2",
    ] {
        assert_eq!(components(&case(action), &net), expected);
    }
}

#[test]
fn quoted_equipment_ids_and_circuit_ids_do_not_become_bus_names() {
    let net = network();
    for (named, numbered) in [
        (
            "REMOVE MACHINE 'B1' FROM 'B2 230'",
            "REMOVE MACHINE 'B1' FROM 2",
        ),
        (
            "OPEN LINE FROM 'B1 230' TO 'B2 230' CKT 'B2 230'",
            "OPEN LINE FROM 1 TO 2 CKT 'B2 230'",
        ),
    ] {
        let expected = case(numbered).resolve(&net);
        let actual = case(named).resolve(&net);
        assert_eq!(expected.unresolved, 1);
        assert_eq!(actual.unresolved, 1);
        assert_eq!(
            actual.cases[0].unresolved[0].reason,
            expected.cases[0].unresolved[0].reason
        );
    }
}

#[test]
fn malformed_actions_do_not_become_name_lookup_errors() {
    let net = network();
    for action in [
        "OPEN LINE FROM 'missing bus' TOWARDS 'B2 230'",
        "OPEN LINE FROM 'missing bus' TO 'B2 230' GARBAGE",
    ] {
        let resolution = case(action).resolve(&net);
        assert_eq!(resolution.unresolved, 1);
        assert!(matches!(
            resolution.cases[0].unresolved[0].reason,
            UnresolvedReason::Unrecognized
        ));
    }
}

#[test]
fn default_dispatch_after_file_end_stays_inert_text() {
    let set = ContingencySet::parse(
        "DEFAULT DISPATCH\nSUBSYSTEM 'LIVE'\nEND\nEND\n\
         DEFAULT DISPATCH DOWN\nSUBSYSTEM 'DEAD'\nEND\n",
    )
    .unwrap()
    .set;
    let defaults = set.calc_default_dispatch();
    assert_eq!(defaults.len(), 1);
    assert_eq!(
        defaults[0].block.entries,
        [DispatchEntry::Subsystem {
            name: "LIVE".into(),
            share: None
        }]
    );
    assert!(set.retained.iter().any(|statement| statement.after_end));
}

#[test]
fn nonfinite_scale_thresholds_stay_retained_instead_of_becoming_typed_rules() {
    for threshold in ["NaN", "Inf", "-Inf", "infinity", "-infinity", "1e309"] {
        let text = format!("SCALE ALL GENERATION WITH PMAX GREATER {threshold} MW");
        let set = SubsystemSet::parse(&format!("SUBSYSTEM 'x'\n{text}\nEND\nEND\n"))
            .unwrap()
            .set;
        assert!(set.subsystems[0].calc_dispatch_rules().is_empty(), "{text}");
        assert!(
            set.subsystems[0]
                .retained
                .iter()
                .any(|statement| statement.text == text)
        );
    }
    for threshold in ["0", "-1", "25.5"] {
        let set = SubsystemSet::parse(&format!(
            "SUBSYSTEM 'x'\nSCALE ALL GENERATION WITH PMAX GREATER {threshold} MW\nEND\nEND\n"
        ))
        .unwrap()
        .set;
        let expected = threshold.parse::<f64>().unwrap();
        let rules = set.subsystems[0].calc_dispatch_rules();
        assert!(
            matches!(rules.as_slice(), [SubsystemDispatchRule::Scale {pmax_greater_mw:Some(value),..}]
            if (value-expected).abs()<1e-12)
        );
    }
}

#[test]
fn dispatch_bad_shares_stay_unread_and_ir_roundtrip_preserves_views() {
    let set = case(
        "SET BUS 2 GENERATION TO 100 MW DISPATCH\nBUS 3 NaN\n\
         SUBSYSTEM 'GEN' infinity\nPARTICIPATING MACHINES\nEND",
    );
    let view = set.cases[0].actions[0].calc_dispatched_action().unwrap();
    assert_eq!(view.block.unread.len(), 2);
    assert_eq!(view.block.entries, [DispatchEntry::ParticipatingMachines]);
    let encoded = serde_json::to_string(&set).unwrap();
    let decoded: ContingencySet = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        decoded.cases[0].actions[0].calc_dispatched_action(),
        Some(view)
    );
    assert_eq!(
        components(&decoded, &network()),
        components(&set, &network())
    );
}

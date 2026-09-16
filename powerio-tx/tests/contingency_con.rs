//! PSS/E contingency description files: the grammar the reader accepts, the
//! statements it keeps as text, the structural refusals, and the writer's
//! fixed point.

mod common;
#[allow(unused_imports)]
use common::*;

use std::path::{Path, PathBuf};

use powerio_tx::{
    AutomaticOrder, AutomaticTarget, BusId, Change, ChangeOp, ChangeUnit, ContingencyAction,
    ContingencyParsed, ContingencySet, SkipRule,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/psse/contingency")
        .join(name)
}

fn parse_fixture(name: &str) -> ContingencyParsed {
    let text = std::fs::read_to_string(fixture(name)).expect("read fixture");
    ContingencySet::parse(&text).expect("parse fixture")
}

fn codes(parsed: &ContingencyParsed) -> Vec<&str> {
    parsed
        .diagnostics
        .iter()
        .map(powerio_core::Diagnostic::code)
        .collect()
}

/// The set with every kept statement's line number cleared. The writer states
/// the cases and the kept statements in its own order, so a statement read
/// from the middle of a file is written after the cases and reads back from a
/// different line; everything else must survive unchanged.
fn without_line_numbers(set: &ContingencySet) -> ContingencySet {
    let mut set = set.clone();
    for statement in &mut set.retained {
        statement.line = 0;
    }
    set
}

/// Writing a set and reading it back gives the same set, and writing that
/// second set gives the same text.
fn check_fixed_point(parsed: &ContingencyParsed) -> String {
    let written = parsed.set.to_con();
    let again = ContingencySet::parse(&written).expect("read the written set");
    assert_eq!(
        without_line_numbers(&parsed.set),
        without_line_numbers(&again.set)
    );
    assert_eq!(written, again.set.to_con());
    written
}

const FIXTURES: [&str; 6] = [
    "psse35_generated.con",
    "explicit_mixed.con",
    "no_file_end.con",
    "automatic_skip.con",
    "tara_extensions.con",
    "resolve_cases.con",
];

#[test]
fn every_fixture_writes_back_to_itself() {
    for name in FIXTURES {
        let parsed = parse_fixture(name);
        let written = check_fixed_point(&parsed);
        // The writer states the file END after the cases, and then the
        // statements the file stated after its own END.
        let after_end = parsed
            .set
            .retained
            .iter()
            .filter(|kept| kept.after_end)
            .count();
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(
            lines[lines.len() - after_end - 1],
            "END",
            "{name} has no file END"
        );
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

#[test]
fn a_generated_psse_35_file_keeps_its_header_and_specification() {
    let parsed = parse_fixture("psse35_generated.con");
    assert!(parsed.diagnostics.is_empty(), "{:?}", codes(&parsed));
    assert_eq!(
        parsed.set.header,
        vec![
            "/PSS(R)E 35".to_owned(),
            "COM PSS(R)E CONTINGENCY DESCRIPTION FILE".to_owned(),
            "COM WRITTEN BY THE CONTINGENCY DESCRIPTION BUILDER".to_owned(),
        ]
    );
    assert!(parsed.set.cases.is_empty());
    assert_eq!(parsed.set.automatic.len(), 1);
    let spec = &parsed.set.automatic[0];
    assert_eq!(spec.order, AutomaticOrder::Single);
    assert_eq!(spec.target, AutomaticTarget::Branch);
    assert_eq!(spec.subsystem, "WOA");
    assert!(!spec.low_voltage_3w);
    assert!(parsed.set.retained.is_empty());
    assert!(
        parsed
            .set
            .to_con()
            .contains("SINGLE BRANCH IN SUBSYSTEM 'WOA'")
    );
}

#[test]
fn explicit_cases_read_every_action_the_grammar_states() {
    let parsed = parse_fixture("explicit_mixed.con");
    assert!(parsed.diagnostics.is_empty(), "{:?}", codes(&parsed));
    let cases = &parsed.set.cases;
    let names: Vec<&str> = cases.iter().map(|case| case.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "L_000001ODES",
            "L_000022O'~1",
            "F01",
            "2000",
            "G_000023",
            "S_001965NACO",
            "EMPTY",
            "LOADS",
            "MISC",
        ]
    );

    assert_eq!(
        cases[0].actions,
        vec![ContingencyAction::OpenBranch {
            from: BusId(1001),
            to: BusId(1064),
            circuit: "1".into(),
        }]
    );
    assert_eq!(
        cases[1].actions[0],
        ContingencyAction::OpenBranch {
            from: BusId(1022),
            to: BusId(1003),
            circuit: "2".into(),
        }
    );
    // TRIP BRANCH ... CKT BL
    assert_eq!(
        cases[2].actions[0],
        ContingencyAction::OpenBranch {
            from: BusId(7),
            to: BusId(8),
            circuit: "BL".into(),
        }
    );
    // DISCONNECT BRANCH with no circuit tail defaults to circuit 1.
    assert_eq!(
        cases[3].actions[0],
        ContingencyAction::OpenBranch {
            from: BusId(9),
            to: BusId(10),
            circuit: "1".into(),
        }
    );
    assert_eq!(
        cases[4].actions,
        vec![
            ContingencyAction::RemoveMachine {
                bus: BusId(23),
                id: "5".into(),
            },
            ContingencyAction::RemoveMachine {
                bus: BusId(24),
                id: "1".into(),
            },
        ]
    );
    assert_eq!(
        cases[5].actions,
        vec![
            ContingencyAction::RemoveShunt {
                bus: BusId(8125),
                id: Some("2".into()),
            },
            ContingencyAction::RemoveShunt {
                bus: BusId(8125),
                id: None,
            },
            ContingencyAction::RemoveSwitchedShunt { bus: BusId(8125) },
        ]
    );
    assert!(cases[6].actions.is_empty());
    // The mid-file COM line is a comment, not a header line and not a
    // statement.
    assert!(parsed.set.header.is_empty());
    assert!(parsed.set.retained.is_empty());
}

#[test]
fn explicit_cases_read_the_change_and_machine_statements() {
    let parsed = parse_fixture("explicit_mixed.con");
    let cases = &parsed.set.cases;
    assert_eq!(
        cases[7].actions,
        vec![
            ContingencyAction::ChangeLoad {
                bus: BusId(154),
                change: Change {
                    op: ChangeOp::Increase,
                    amount: 50.0,
                    unit: ChangeUnit::Percent,
                },
            },
            ContingencyAction::ChangeLoad {
                bus: BusId(205),
                change: Change {
                    op: ChangeOp::Set,
                    amount: 0.0,
                    unit: ChangeUnit::Mw,
                },
            },
            ContingencyAction::ChangeLoad {
                bus: BusId(3000),
                change: Change {
                    op: ChangeOp::Increase,
                    amount: 1.0,
                    unit: ChangeUnit::Mw,
                },
            },
            ContingencyAction::ChangeGeneration {
                bus: BusId(34),
                change: Change {
                    op: ChangeOp::Decrease,
                    amount: 23.0,
                    unit: ChangeUnit::Mw,
                },
            },
        ]
    );
    assert_eq!(
        cases[8].actions,
        vec![
            ContingencyAction::AddMachine {
                bus: BusId(56),
                id: "3".into(),
            },
            ContingencyAction::DisconnectBus { bus: BusId(54321) },
            ContingencyAction::OpenThreeWinding {
                buses: [BusId(1), BusId(2), BusId(3)],
                circuit: "A".into(),
            },
            ContingencyAction::OpenThreeWinding {
                buses: [BusId(1), BusId(2), BusId(3)],
                circuit: "A".into(),
            },
            // Lowercase keywords and the CIRCUT spelling.
            ContingencyAction::OpenBranch {
                from: BusId(6),
                to: BusId(7),
                circuit: "4".into(),
            },
            // TO without the BUS keyword.
            ContingencyAction::OpenBranch {
                from: BusId(7),
                to: BusId(8),
                circuit: "1".into(),
            },
        ]
    );
}

#[test]
fn a_file_with_no_final_end_reads_and_gains_one() {
    let parsed = parse_fixture("no_file_end.con");
    assert!(parsed.diagnostics.is_empty(), "{:?}", codes(&parsed));
    assert_eq!(parsed.set.cases.len(), 3);
    assert_eq!(
        parsed.set.cases[1].actions,
        vec![ContingencyAction::RemoveMachine {
            bus: BusId(1090),
            id: "2".into(),
        }]
    );
    let written = parsed.set.to_con();
    assert!(written.ends_with("END\nEND\n"));
    assert!(written.contains("OPEN LINE FROM BUS   1001 TO BUS   1064 CIRCUIT 1"));
}

#[test]
fn automatic_specifications_skip_rules_and_text_after_the_file_end() {
    let parsed = parse_fixture("automatic_skip.con");
    assert_eq!(codes(&parsed), vec!["READ.CON.TEXT_AFTER_END"]);
    let specs = &parsed.set.automatic;
    assert_eq!(specs.len(), 3);
    assert!(specs[0].low_voltage_3w);
    assert_eq!(specs[0].target, AutomaticTarget::Branch);
    assert_eq!(specs[1].order, AutomaticOrder::Double);
    assert_eq!(specs[1].target, AutomaticTarget::Unit);
    assert_eq!(specs[1].subsystem, "X");
    assert_eq!(specs[2].target, AutomaticTarget::Tie);
    assert_eq!(specs[2].subsystem, "X");
    assert_eq!(
        parsed.set.skips,
        vec![
            SkipRule {
                from: BusId(100),
                to: BusId(200),
                circuit: "1".into(),
            },
            SkipRule {
                from: BusId(300),
                to: BusId(400),
                circuit: "2".into(),
            },
        ]
    );
    assert_eq!(parsed.set.retained.len(), 1);
    assert_eq!(parsed.set.retained[0].text, "BUSNAMES");
    assert_eq!(parsed.set.retained[0].line, 9);
    assert!(parsed.set.retained[0].after_end);
    let written = parsed.set.to_con();
    assert!(written.ends_with("END\nBUSNAMES\n"));
    assert!(written.contains("SINGLE TIE FROM SUBSYSTEM 'X'"));
    assert!(written.contains("SINGLE BRANCH IN SUBSYSTEM 'X' 3WLOWVOLTAGE"));
    assert!(written.contains("SKIP\n"));
}

#[test]
fn tara_statements_keep_their_lines_and_are_reported() {
    let parsed = parse_fixture("tara_extensions.con");
    // BUSNUMBERS, the bus-name mode line, the three dispatch blocks, and
    // BRANCHNAMES, in line order.
    assert_eq!(codes(&parsed), vec!["READ.CON.STATEMENT_UNRECOGNIZED"; 6]);
    let messages: Vec<&str> = parsed
        .diagnostics
        .iter()
        .map(powerio_core::Diagnostic::message)
        .collect();
    assert!(
        messages[2].starts_with("line 6: dispatch block"),
        "{messages:?}"
    );
    assert!(
        messages[3].starts_with("line 10: dispatch block"),
        "{messages:?}"
    );
    assert!(
        messages[4].starts_with("line 14: dispatch block"),
        "{messages:?}"
    );
    assert_eq!(
        parsed.set.header,
        vec!["// TARA contingency file".to_owned()]
    );
    let retained: Vec<&str> = parsed
        .set
        .retained
        .iter()
        .map(|statement| statement.text.as_str())
        .collect();
    assert_eq!(
        retained,
        vec![
            "BUSNUMBERS",
            "DEFAULT DISPATCH\nSUBSYSTEM 'SYSTEM'\nPARTICIPATING MACHINES\nEND",
            // A direction after DEFAULT DISPATCH still opens a block, so its
            // END does not read as the file END.
            "DEFAULT DISPATCH DOWN\nSUBSYSTEM 'X' 34\nEND",
            "BRANCHNAMES",
        ]
    );
    assert_eq!(parsed.set.cases.len(), 1);
    let actions = &parsed.set.cases[0].actions;
    assert_eq!(actions.len(), 3);
    // Bus-name mode: a non-numeric bus token keeps the line as text.
    assert_eq!(
        actions[0],
        ContingencyAction::Unrecognized {
            text: "TRIP LINE FROM BUS '02CHAMBR 345' TO BUS '03LAKESD 345' CKT 1".into(),
        }
    );
    // A padded quoted circuit trims, and a trailing `/` comment drops.
    assert_eq!(
        actions[1],
        ContingencyAction::OpenBranch {
            from: BusId(101),
            to: BusId(102),
            circuit: "1".into(),
        }
    );
    // A dispatch block keeps every one of its lines, its END included.
    assert_eq!(
        actions[2],
        ContingencyAction::Unrecognized {
            text: "SET BUS 4 GENERATION TO 100 PERCENT DISPATCH\nSUBSYSTEM 'AREA1'\nEND".into(),
        }
    );
}

// ---------------------------------------------------------------------------
// Grammar details
// ---------------------------------------------------------------------------

const SAMPLE: &str = "\
/ comment line
CONTINGENCY 'L_000001ODES'
OPEN LINE FROM BUS   1001 TO BUS   1064 CIRCUIT 1
END
CONTINGENCY 'T_000004O'DO'
OPEN LINE FROM BUS   1004 TO BUS   1003 CIRCUIT  2
END
CONTINGENCY 'G_1090_2'
REMOVE MACHINE 2 FROM BUS   1090
END
CONTINGENCY 'S_001965NACO'
REMOVE SWSHUNT FROM BUS   8125
REMOVE SHUNT FROM BUS   8125
REMOVE SHUNT 2 FROM BUS   8125
END
CONTINGENCY 'X'
DISCONNECT BUS 5
END
";

#[test]
fn the_sample_list_reads_every_case() {
    let parsed = ContingencySet::parse(SAMPLE).expect("parse");
    assert!(parsed.diagnostics.is_empty(), "{:?}", codes(&parsed));
    let cases = &parsed.set.cases;
    assert_eq!(cases.len(), 5);
    assert_eq!(parsed.set.header, vec!["/ comment line".to_owned()]);
    assert_eq!(cases[0].name, "L_000001ODES");
    assert_eq!(
        cases[0].actions,
        vec![ContingencyAction::OpenBranch {
            from: BusId(1001),
            to: BusId(1064),
            circuit: "1".into(),
        }]
    );
    assert_eq!(cases[1].name, "T_000004O'DO");
    assert_eq!(
        cases[1].actions[0],
        ContingencyAction::OpenBranch {
            from: BusId(1004),
            to: BusId(1003),
            circuit: "2".into(),
        }
    );
    assert_eq!(
        cases[2].actions,
        vec![ContingencyAction::RemoveMachine {
            bus: BusId(1090),
            id: "2".into(),
        }]
    );
    assert_eq!(cases[3].actions.len(), 3);
    assert_eq!(
        cases[3].actions[2],
        ContingencyAction::RemoveShunt {
            bus: BusId(8125),
            id: Some("2".into()),
        }
    );
    assert_eq!(
        cases[4].actions,
        vec![ContingencyAction::DisconnectBus { bus: BusId(5) }]
    );
    check_fixed_point(&parsed);
}

#[test]
fn keywords_are_case_insensitive_and_circuit_defaults_to_one() {
    let parsed = ContingencySet::parse("contingency abc\nopen branch from bus 1 to bus 2\nend\n")
        .expect("parse");
    assert_eq!(parsed.set.cases[0].name, "abc");
    assert_eq!(
        parsed.set.cases[0].actions[0],
        ContingencyAction::OpenBranch {
            from: BusId(1),
            to: BusId(2),
            circuit: "1".into(),
        }
    );
}

#[test]
fn structural_errors_name_their_line() {
    let unterminated = ContingencySet::parse("CONTINGENCY 'A'\nOPEN LINE FROM BUS 1 TO BUS 2\n")
        .expect_err("a case with no END is refused");
    assert!(
        unterminated.to_string().contains("line 1"),
        "{unterminated}"
    );
    assert!(unterminated.to_string().contains("has no END"));

    let nested = ContingencySet::parse("CONTINGENCY 'A'\nCONTINGENCY 'B'\nEND\n")
        .expect_err("a case inside a case is refused");
    assert!(nested.to_string().contains("line 2"), "{nested}");

    let skip = ContingencySet::parse("SKIP\n100 TO 200\n").expect_err("an open SKIP is refused");
    assert!(skip.to_string().contains("line 1"), "{skip}");

    let block = ContingencySet::parse("DEFAULT DISPATCH\nSUBSYSTEM 'S'\n")
        .expect_err("an open dispatch block is refused");
    assert!(block.to_string().contains("line 1"), "{block}");
}

#[test]
fn a_statement_outside_a_case_is_kept_at_file_level() {
    let parsed = ContingencySet::parse("OPEN LINE FROM BUS 1 TO BUS 2\n").expect("parse");
    assert_eq!(codes(&parsed), vec!["READ.CON.STATEMENT_UNRECOGNIZED"]);
    assert!(parsed.set.cases.is_empty());
    assert_eq!(parsed.set.retained[0].text, "OPEN LINE FROM BUS 1 TO BUS 2");
    assert_eq!(parsed.set.retained[0].line, 1);
    assert!(!parsed.set.retained[0].after_end);
}

#[test]
fn comment_and_blank_lines_after_the_file_end_state_nothing() {
    let parsed = ContingencySet::parse("CONTINGENCY 'A'\nEND\nEND\nCOM done\n\n").expect("parse");
    assert!(parsed.diagnostics.is_empty(), "{:?}", codes(&parsed));
    assert!(parsed.set.retained.is_empty());
    assert_eq!(parsed.set.cases.len(), 1);
    check_fixed_point(&parsed);
}

#[test]
fn a_trailing_comment_stays_out_of_the_case_name() {
    let parsed = ContingencySet::parse("CONTINGENCY 'A' / it's\nEND\n").expect("parse");
    assert_eq!(parsed.set.cases[0].name, "A");
    // The same rule keeps an apostrophe that belongs to the name.
    let held = ContingencySet::parse("CONTINGENCY 'L_000022O'~1'\nEND\n").expect("parse");
    assert_eq!(held.set.cases[0].name, "L_000022O'~1");
    check_fixed_point(&parsed);
    check_fixed_point(&held);
}

#[test]
fn a_file_of_one_end_is_an_empty_set() {
    let parsed = ContingencySet::parse("END\n").expect("parse");
    assert!(parsed.diagnostics.is_empty(), "{:?}", codes(&parsed));
    assert_eq!(parsed.set, ContingencySet::default());
    assert_eq!(parsed.set.to_con(), "END\n");
}

#[test]
fn a_skip_line_that_states_no_branch_is_reported_and_kept() {
    let parsed = ContingencySet::parse("SKIP\nALL TIES\nEND\nEND\n").expect("parse");
    assert_eq!(codes(&parsed), vec!["READ.CON.SOURCE_MALFORMED"]);
    assert!(parsed.set.skips.is_empty());
    assert_eq!(parsed.set.retained[0].text, "ALL TIES");
}

/// A case holding `findings` statements the grammar does not cover.
fn unreadable_case(findings: usize) -> ContingencyParsed {
    use std::fmt::Write as _;

    let mut text = String::from("CONTINGENCY 'A'\n");
    for index in 0..findings {
        let _ = writeln!(text, "NOT A STATEMENT {index}");
    }
    text.push_str("END\n");
    ContingencySet::parse(&text).expect("parse")
}

#[test]
fn the_note_budget_bounds_the_reader() {
    let parsed = unreadable_case(50);
    assert_eq!(parsed.diagnostics.len(), 17);
    assert_eq!(
        parsed
            .diagnostics
            .last()
            .map(powerio_core::Diagnostic::code),
        Some("READ.CON.NOTES_TRUNCATED")
    );
    // Every line is still kept, only the notes stop.
    assert_eq!(parsed.set.cases[0].actions.len(), 50);
}

#[test]
fn a_file_of_exactly_the_budget_gets_no_truncation_marker() {
    let parsed = unreadable_case(16);
    assert_eq!(codes(&parsed), vec!["READ.CON.STATEMENT_UNRECOGNIZED"; 16]);
}

#[test]
fn one_finding_past_the_budget_records_the_marker_in_its_place() {
    let parsed = unreadable_case(17);
    let mut expected = vec!["READ.CON.STATEMENT_UNRECOGNIZED"; 16];
    expected.push("READ.CON.NOTES_TRUNCATED");
    assert_eq!(codes(&parsed), expected);
    assert_eq!(parsed.set.cases[0].actions.len(), 17);
}

#[test]
fn statements_after_the_file_end_are_written_after_the_end() {
    let parsed = ContingencySet::parse("END\nCONTINGENCY 'B'\nEND\n").expect("parse");
    assert_eq!(codes(&parsed), vec!["READ.CON.TEXT_AFTER_END"]);
    assert!(parsed.set.cases.is_empty());
    let retained: Vec<&str> = parsed
        .set
        .retained
        .iter()
        .map(|statement| statement.text.as_str())
        .collect();
    assert_eq!(retained, vec!["CONTINGENCY 'B'", "END"]);
    assert!(
        parsed
            .set
            .retained
            .iter()
            .all(|statement| statement.after_end)
    );
    assert_eq!(parsed.set.to_con(), "END\nCONTINGENCY 'B'\nEND\n");
    check_fixed_point(&parsed);
}

#[test]
fn a_skip_block_after_the_file_end_stays_text() {
    let parsed = ContingencySet::parse("END\nSKIP\n100 TO 200 CIRCUIT 1\nEND\n").expect("parse");
    assert!(parsed.set.skips.is_empty());
    let again = ContingencySet::parse(&parsed.set.to_con()).expect("read the written set");
    assert!(again.set.skips.is_empty());
    assert_eq!(again.set.retained.len(), 3);
    check_fixed_point(&parsed);
}

#[test]
fn a_value_holding_an_apostrophe_is_written_inside_double_quotes() {
    let parsed = ContingencySet::parse(concat!(
        "CONTINGENCY \"O' HARE\"\n",
        "REMOVE MACHINE \"O' HARE\" FROM BUS 1\n",
        "REMOVE LOAD \"O' HARE\" FROM BUS 2\n",
        "REMOVE SHUNT \"O' HARE\" FROM BUS 3\n",
        "END\n",
        "SINGLE BRANCH IN SUBSYSTEM \"O' HARE\"\n",
        "END\n",
    ))
    .expect("parse");
    assert!(parsed.diagnostics.is_empty(), "{:?}", codes(&parsed));
    assert_eq!(parsed.set.cases[0].name, "O' HARE");
    assert_eq!(parsed.set.automatic[0].subsystem, "O' HARE");
    let written = check_fixed_point(&parsed);
    assert!(written.contains("CONTINGENCY \"O' HARE\"\n"), "{written}");
    assert!(
        written.contains("REMOVE MACHINE \"O' HARE\" FROM BUS      1\n"),
        "{written}"
    );
    assert!(
        written.contains("REMOVE LOAD \"O' HARE\" FROM BUS      2\n"),
        "{written}"
    );
    assert!(
        written.contains("REMOVE SHUNT \"O' HARE\" FROM BUS      3\n"),
        "{written}"
    );
    assert!(
        written.contains("SINGLE BRANCH IN SUBSYSTEM \"O' HARE\"\n"),
        "{written}"
    );
}

#[test]
fn an_id_opening_with_a_slash_is_written_quoted() {
    let parsed = ContingencySet::parse(concat!(
        "CONTINGENCY 'A'\n",
        "OPEN LINE FROM BUS 1 TO BUS 2 CIRCUIT '/1'\n",
        "END\n",
        "SKIP\n",
        "100 TO 200 CIRCUIT '/2'\n",
        "END\n",
        "END\n",
    ))
    .expect("parse");
    assert!(parsed.diagnostics.is_empty(), "{:?}", codes(&parsed));
    assert_eq!(
        parsed.set.cases[0].actions[0],
        ContingencyAction::OpenBranch {
            from: BusId(1),
            to: BusId(2),
            circuit: "/1".into(),
        }
    );
    let written = check_fixed_point(&parsed);
    // An unquoted token opening with `/` would end the statement.
    assert!(written.contains("CIRCUIT '/1'\n"), "{written}");
    assert!(written.contains("CIRCUIT '/2'\n"), "{written}");
}

#[test]
fn a_value_holding_both_quote_characters_is_kept_as_text() {
    let name = ContingencySet::parse("CONTINGENCY A'B\"C\nEND\n").expect("parse");
    assert_eq!(codes(&name), vec!["READ.CON.SOURCE_MALFORMED"]);
    assert!(name.set.cases.is_empty());
    assert_eq!(name.set.retained[0].text, "CONTINGENCY A'B\"C");
    assert!(!name.set.retained[0].after_end);
    check_fixed_point(&name);

    let id = ContingencySet::parse("CONTINGENCY 'A'\nREMOVE MACHINE A'B\"C FROM BUS 1\nEND\n")
        .expect("parse");
    assert_eq!(codes(&id), vec!["READ.CON.SOURCE_MALFORMED"]);
    assert_eq!(
        id.set.cases[0].actions[0],
        ContingencyAction::Unrecognized {
            text: "REMOVE MACHINE A'B\"C FROM BUS 1".into(),
        }
    );
    check_fixed_point(&id);

    let subsystem =
        ContingencySet::parse("SINGLE BRANCH IN SUBSYSTEM A'B\"C\nEND\n").expect("parse");
    assert_eq!(codes(&subsystem), vec!["READ.CON.SOURCE_MALFORMED"]);
    assert!(subsystem.set.automatic.is_empty());
    assert_eq!(
        subsystem.set.retained[0].text,
        "SINGLE BRANCH IN SUBSYSTEM A'B\"C"
    );
    check_fixed_point(&subsystem);

    let skip = ContingencySet::parse("SKIP\n100 TO 200 CIRCUIT A'B\"C\nEND\nEND\n").expect("parse");
    assert_eq!(codes(&skip), vec!["READ.CON.SOURCE_MALFORMED"]);
    assert!(skip.set.skips.is_empty());
    assert_eq!(skip.set.retained[0].text, "100 TO 200 CIRCUIT A'B\"C");
    check_fixed_point(&skip);
}

#[test]
fn a_contingency_line_states_one_name_and_reports_the_rest() {
    let parsed = ContingencySet::parse("CONTINGENCY A B\nOPEN LINE FROM BUS 1 TO BUS 2\nEND\n")
        .expect("parse");
    assert_eq!(codes(&parsed), vec!["READ.CON.SOURCE_MALFORMED"]);
    assert!(
        parsed.diagnostics[0].message().ends_with("not kept: B"),
        "{:?}",
        parsed.diagnostics[0].message()
    );
    // The case still opens, so its END closes the case rather than the file.
    assert_eq!(parsed.set.cases.len(), 1);
    assert_eq!(parsed.set.cases[0].name, "A");
    assert_eq!(parsed.set.cases[0].actions.len(), 1);
    assert!(parsed.set.retained.is_empty());
    check_fixed_point(&parsed);
}

#[test]
fn a_percent_sign_states_the_unit() {
    let parsed = ContingencySet::parse("CONTINGENCY 'A'\nINCREASE BUS 7 LOAD BY 100%\nEND\n")
        .expect("parse");
    assert_eq!(
        parsed.set.cases[0].actions[0],
        ContingencyAction::ChangeLoad {
            bus: BusId(7),
            change: Change {
                op: ChangeOp::Increase,
                amount: 100.0,
                unit: ChangeUnit::Percent,
            },
        }
    );
    assert!(
        parsed
            .set
            .to_con()
            .contains("INCREASE BUS 7 LOAD BY 100 PERCENT")
    );
}

// ---------------------------------------------------------------------------
// The machine-local corpus
// ---------------------------------------------------------------------------

#[test]
fn the_local_activsg2000_corpus_reads_completely() {
    let Some(path) = local_manifest_path("local_psse_contingency_corpus.tsv", "activsg2000") else {
        eprintln!("skipped: local_psse_contingency_corpus.tsv has no readable activsg2000 entry");
        return;
    };
    let text = std::fs::read_to_string(&path).expect("read the corpus file");
    let parsed = ContingencySet::parse(&text).expect("parse the corpus file");
    assert!(parsed.diagnostics.is_empty(), "{:?}", codes(&parsed));
    assert_eq!(parsed.set.cases.len(), 3875);
    let empty = parsed
        .set
        .cases
        .iter()
        .filter(|case| case.actions.is_empty())
        .count();
    assert_eq!(empty, 1);
    let apostrophes = parsed
        .set
        .cases
        .iter()
        .filter(|case| case.name.contains('\''))
        .count();
    assert_eq!(apostrophes, 8);
    check_fixed_point(&parsed);
}

#[test]
fn the_local_stressed_corpus_reads_completely() {
    let Some(path) =
        local_manifest_path("local_psse_contingency_corpus.tsv", "activsg2000_stressed")
    else {
        eprintln!(
            "skipped: local_psse_contingency_corpus.tsv has no readable activsg2000_stressed entry"
        );
        return;
    };
    let text = std::fs::read_to_string(&path).expect("read the corpus file");
    let parsed = ContingencySet::parse(&text).expect("parse the corpus file");
    assert!(parsed.diagnostics.is_empty(), "{:?}", codes(&parsed));
    assert!(!parsed.set.cases.is_empty());
    check_fixed_point(&parsed);
}

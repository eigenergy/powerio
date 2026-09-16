//! PSS/E subsystem description files and monitored element files: the grammar
//! the readers accept, the statements they keep as text, the writers' fixed
//! point, bus selection over a network, and monitored element resolution.

mod common;
mod helpers;
#[allow(unused_imports)]
use common::*;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use powerio_tx::network::BalancedNetwork;
use powerio_tx::{
    BranchRef, BusId, InterfaceMember, JoinName, MonitorScope, MonitorStatement, MonitoredParsed,
    MonitoredSet, PsseEquipmentIndex, RetainedStatement, SubsystemParsed, SubsystemSelector,
    SubsystemSet, UnresolvedMonitorReason,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/psse/contingency")
        .join(name)
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("read fixture")
}

fn parse_sub(name: &str) -> SubsystemParsed {
    SubsystemSet::parse(&read(name)).expect("parse subsystem fixture")
}

fn parse_mon(name: &str) -> MonitoredParsed {
    MonitoredSet::parse(&read(name)).expect("parse monitored element fixture")
}

fn sub_codes(parsed: &SubsystemParsed) -> Vec<&str> {
    parsed
        .diagnostics
        .iter()
        .map(powerio_core::Diagnostic::code)
        .collect()
}

fn mon_codes(parsed: &MonitoredParsed) -> Vec<&str> {
    parsed
        .diagnostics
        .iter()
        .map(powerio_core::Diagnostic::code)
        .collect()
}

fn select_network() -> BalancedNetwork {
    helpers::parse_file(fixture("select_v33.raw"), Some("psse"))
        .expect("parse select_v33.raw")
        .network
}

/// The text of each kept statement, in order.
fn kept_text(statements: &[RetainedStatement]) -> Vec<&str> {
    statements
        .iter()
        .map(|statement| statement.text.as_str())
        .collect()
}

/// The set with every kept statement's line number cleared. The writer states
/// the subsystems and the kept statements in its own order, so a statement
/// read from the middle of a file can read back from a different line;
/// everything else must survive unchanged.
fn sub_without_line_numbers(set: &SubsystemSet) -> SubsystemSet {
    let mut set = set.clone();
    for statement in &mut set.retained {
        statement.line = 0;
    }
    for subsystem in &mut set.subsystems {
        for statement in &mut subsystem.retained {
            statement.line = 0;
        }
        for group in &mut subsystem.groups {
            for statement in &mut group.retained {
                statement.line = 0;
            }
        }
    }
    set
}

fn mon_without_line_numbers(set: &MonitoredSet) -> MonitoredSet {
    let mut set = set.clone();
    for statement in &mut set.retained {
        statement.line = 0;
    }
    for statement in &mut set.statements {
        let (MonitorStatement::Branches { retained, .. }
        | MonitorStatement::Interface { retained, .. }) = statement
        else {
            continue;
        };
        for kept in retained {
            kept.line = 0;
        }
    }
    set
}

fn check_sub_fixed_point(parsed: &SubsystemParsed) -> String {
    let written = parsed.set.to_sub();
    let again = SubsystemSet::parse(&written).expect("read the written set");
    assert_eq!(
        sub_without_line_numbers(&parsed.set),
        sub_without_line_numbers(&again.set)
    );
    assert_eq!(written, again.set.to_sub());
    written
}

fn check_mon_fixed_point(parsed: &MonitoredParsed) -> String {
    let written = parsed.set.to_mon();
    let again = MonitoredSet::parse(&written).expect("read the written set");
    assert_eq!(
        mon_without_line_numbers(&parsed.set),
        mon_without_line_numbers(&again.set)
    );
    assert_eq!(written, again.set.to_mon());
    written
}

const SUB_FIXTURES: [&str; 2] = ["psse35_area.sub", "selectors.sub"];
const MON_FIXTURES: [&str; 2] = ["generated.mon", "blocks.mon"];

#[test]
fn every_fixture_writes_back_to_itself() {
    for name in SUB_FIXTURES {
        let written = check_sub_fixed_point(&parse_sub(name));
        assert!(written.ends_with("END\n"), "{name} has no file END");
    }
    for name in MON_FIXTURES {
        let written = check_mon_fixed_point(&parse_mon(name));
        assert!(written.ends_with("END\n"), "{name} has no file END");
    }
}

// ---------------------------------------------------------------------------
// Subsystem fixtures
// ---------------------------------------------------------------------------

#[test]
fn a_generated_psse_35_subsystem_file_keeps_its_header_and_selector() {
    let parsed = parse_sub("psse35_area.sub");
    assert!(parsed.diagnostics.is_empty(), "{:?}", sub_codes(&parsed));
    assert_eq!(
        parsed.set.header,
        vec![
            "/PSS(R)E 35".to_owned(),
            "COM PSS(R)E SUBSYSTEM DESCRIPTION FILE".to_owned(),
            "COM WRITTEN BY THE CONFIG FILE BUILDER".to_owned(),
        ]
    );
    assert_eq!(parsed.set.subsystems.len(), 1);
    let subsystem = &parsed.set.subsystems[0];
    assert_eq!(subsystem.name, "WOA");
    assert_eq!(subsystem.groups.len(), 1);
    assert_eq!(subsystem.groups[0].join, None);
    assert_eq!(
        subsystem.groups[0].selectors,
        vec![SubsystemSelector::Area { from: 1, to: 1 }]
    );
    assert!(parsed.set.retained.is_empty());
    let written = parsed.set.to_sub();
    assert!(written.contains("SUBSYSTEM 'WOA'\n   AREA 1\nEND\n"));
}

#[test]
fn every_selector_spelling_reads() {
    let parsed = parse_sub("selectors.sub");
    assert_eq!(
        sub_codes(&parsed),
        vec!["READ.SUB.STATEMENT_UNRECOGNIZED"],
        "only the TARA line is outside the grammar"
    );
    assert_eq!(
        parsed.set.header,
        vec!["COM selectors: every selector spelling the reader states.".to_owned(),]
    );
    let names: Vec<&str> = parsed
        .set
        .subsystems
        .iter()
        .map(|subsystem| subsystem.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "A1",
            "AREARANGE",
            "BARE",
            "BUSLIST",
            "BUSRANGE",
            "KV",
            "JOINED",
            "BAREJOIN",
            "ONELINE",
            "A2",
        ]
    );

    let group = |name: &str| parsed.set.get(name).expect("subsystem").groups.clone();
    assert_eq!(
        group("AREARANGE")[0].selectors,
        vec![SubsystemSelector::Area { from: 1, to: 2 }]
    );
    // Tab indentation, a bare name, and two selector families in one group.
    assert_eq!(
        group("BARE")[0].selectors,
        vec![
            SubsystemSelector::Zone { from: 3, to: 3 },
            SubsystemSelector::Owner { from: 3, to: 3 },
        ]
    );
    assert_eq!(
        group("BUSLIST")[0].selectors,
        vec![
            SubsystemSelector::Bus {
                from: BusId(101),
                to: BusId(101),
            },
            SubsystemSelector::Bus {
                from: BusId(203),
                to: BusId(203),
            },
        ]
    );
    assert_eq!(
        group("BUSRANGE")[0].selectors,
        vec![SubsystemSelector::Bus {
            from: BusId(101),
            to: BusId(103),
        }]
    );
    assert_eq!(
        group("KV")[0].selectors,
        vec![SubsystemSelector::KvRange {
            lo: 100.0,
            hi: 240.0,
        }]
    );

    // A one line subsystem: the selectors follow the name and END closes it.
    assert_eq!(
        group("ONELINE")[0].selectors,
        vec![
            SubsystemSelector::Area { from: 1, to: 1 },
            SubsystemSelector::Zone { from: 2, to: 2 },
        ]
    );

    // A TARA line inside a subsystem is kept on that subsystem.
    let tara = parsed.set.get("A2").expect("subsystem A2");
    assert_eq!(tara.retained.len(), 1);
    assert_eq!(
        tara.retained[0].text,
        "SCALE ALL FOR EXPORT INCLUDE OFFLINE"
    );
    assert!(parsed.set.retained.is_empty());

    // The file states no final END and the writer adds one.
    assert!(parsed.set.to_sub().ends_with("END\nEND\n"));
}

#[test]
fn each_join_group_reads_as_its_own_group() {
    let parsed = parse_sub("selectors.sub");
    let group = |name: &str| parsed.set.get(name).expect("subsystem").groups.clone();

    // SYSTEM is the SUBSYSTEM synonym, and each JOIN is its own group.
    let joined = group("JOINED");
    assert_eq!(joined.len(), 2);
    assert_eq!(
        joined[0].join,
        Some(JoinName::Named {
            name: "HIGH".into()
        })
    );
    assert_eq!(joined[0].selectors.len(), 2);
    assert_eq!(joined[1].join, Some(JoinName::Named { name: "LOW".into() }));
    assert_eq!(
        joined[1].selectors,
        vec![SubsystemSelector::Bus {
            from: BusId(203),
            to: BusId(203),
        }]
    );

    // A JOIN with no name is its own group, distinct from the implicit one,
    // and two of them stay two groups.
    let bare = group("BAREJOIN");
    assert_eq!(bare.len(), 2);
    assert_eq!(bare[0].join, Some(JoinName::Anonymous));
    assert_eq!(bare[0].selectors.len(), 2);
    assert_eq!(bare[1].join, Some(JoinName::Anonymous));
    assert_eq!(
        bare[1].selectors,
        vec![SubsystemSelector::Bus {
            from: BusId(203),
            to: BusId(203),
        }]
    );
    // The writer states a group with no name as a bare JOIN.
    assert!(parsed.set.to_sub().contains("   JOIN\n   AREA 1\n"));
}

#[test]
fn a_subsystem_name_matches_without_case_or_padding() {
    let parsed = parse_sub("selectors.sub");
    assert_eq!(
        parsed.set.get("a1").map(|held| held.name.as_str()),
        Some("A1")
    );
    assert_eq!(
        parsed.set.get("  A1  ").map(|held| held.name.as_str()),
        Some("A1")
    );
    assert!(parsed.set.get("NOSUCH").is_none());
}

#[test]
fn subsystem_structural_errors_name_their_line() {
    let unterminated = SubsystemSet::parse("SUBSYSTEM 'A'\n   AREA 1\n")
        .expect_err("a subsystem with no END is refused");
    assert!(
        unterminated.to_string().contains("line 1"),
        "{unterminated}"
    );
    assert!(unterminated.to_string().contains("has no END"));

    let nested = SubsystemSet::parse("SUBSYSTEM 'A'\nSUBSYSTEM 'B'\nEND\nEND\n")
        .expect_err("a subsystem inside a subsystem is refused");
    assert!(nested.to_string().contains("line 2"), "{nested}");

    // An END closes the open JOIN, so a file that ends there leaves the JOIN
    // open rather than the subsystem.
    let join = SubsystemSet::parse("SUBSYSTEM 'A'\n   JOIN 'G'\n      AREA 1\n")
        .expect_err("a JOIN with no END is refused");
    assert!(join.to_string().contains("line 2"), "{join}");
    assert!(join.to_string().contains("JOIN has no END"));

    let closed = SubsystemSet::parse("SUBSYSTEM 'A'\n   JOIN 'G'\n      AREA 1\nEND\n")
        .expect_err("the JOIN's END leaves the subsystem open");
    assert!(
        closed.to_string().contains("SUBSYSTEM 'A' has no END"),
        "{closed}"
    );
}

#[test]
fn a_selector_that_states_no_number_is_reported_and_kept() {
    let parsed = SubsystemSet::parse("SUBSYSTEM 'A'\n   AREA WEST\nEND\nEND\n").expect("parse");
    assert_eq!(sub_codes(&parsed), vec!["READ.SUB.SOURCE_MALFORMED"]);
    assert!(parsed.set.subsystems[0].groups.is_empty());
    assert_eq!(parsed.set.subsystems[0].retained[0].text, "AREA WEST");
    check_sub_fixed_point(&parsed);
}

#[test]
fn a_join_reads_the_selectors_stated_on_its_own_line() {
    let parsed =
        SubsystemSet::parse("SUBSYSTEM 'A'\n   JOIN 'G' AREA 1 ZONE 2\n   END\nEND\nEND\n")
            .expect("parse");
    assert!(parsed.diagnostics.is_empty(), "{:?}", sub_codes(&parsed));
    let groups = &parsed.set.subsystems[0].groups;
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].join, Some(JoinName::Named { name: "G".into() }));
    assert_eq!(
        groups[0].selectors,
        vec![
            SubsystemSelector::Area { from: 1, to: 1 },
            SubsystemSelector::Zone { from: 2, to: 2 },
        ]
    );
    check_sub_fixed_point(&parsed);

    // The token after the keyword is a name only when it opens no selector.
    let bare =
        SubsystemSet::parse("SUBSYSTEM 'A'\n   JOIN AREA 1\n   END\nEND\nEND\n").expect("parse");
    assert!(bare.diagnostics.is_empty(), "{:?}", sub_codes(&bare));
    assert_eq!(
        bare.set.subsystems[0].groups[0].join,
        Some(JoinName::Anonymous)
    );
    assert_eq!(
        bare.set.subsystems[0].groups[0].selectors,
        vec![SubsystemSelector::Area { from: 1, to: 1 }]
    );
    check_sub_fixed_point(&bare);
}

#[test]
fn a_join_line_tail_outside_the_grammar_is_reported_and_kept() {
    let parsed = SubsystemSet::parse("SUBSYSTEM 'A'\n   JOIN 'G' PARTICIPATE\n   END\nEND\nEND\n")
        .expect("parse");
    assert_eq!(sub_codes(&parsed), vec!["READ.SUB.STATEMENT_UNRECOGNIZED"]);
    let subsystem = &parsed.set.subsystems[0];
    assert_eq!(
        subsystem.groups[0].join,
        Some(JoinName::Named { name: "G".into() })
    );
    assert!(subsystem.groups[0].selectors.is_empty());
    // The tail belongs to the group the line opened.
    assert_eq!(
        kept_text(&subsystem.groups[0].retained),
        vec!["PARTICIPATE"]
    );
    assert!(subsystem.retained.is_empty());
    check_sub_fixed_point(&parsed);

    let malformed = SubsystemSet::parse("SUBSYSTEM 'A'\n   JOIN 'G' AREA WEST\n   END\nEND\nEND\n")
        .expect("parse");
    assert_eq!(sub_codes(&malformed), vec!["READ.SUB.SOURCE_MALFORMED"]);
    assert_eq!(
        kept_text(&malformed.set.subsystems[0].groups[0].retained),
        vec!["AREA WEST"]
    );
    check_sub_fixed_point(&malformed);
}

#[test]
fn an_end_on_a_selector_line_closes_the_open_join_alone() {
    let parsed =
        SubsystemSet::parse("SUBSYSTEM 'A'\n   JOIN 'G'\n      AREA 1 END\n   ZONE 2\nEND\nEND\n")
            .expect("parse");
    assert!(parsed.diagnostics.is_empty(), "{:?}", sub_codes(&parsed));
    let groups = &parsed.set.subsystems[0].groups;
    // The implicit group holds the selectors stated after the JOIN closed.
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].join, None);
    assert_eq!(
        groups[0].selectors,
        vec![SubsystemSelector::Zone { from: 2, to: 2 }]
    );
    assert_eq!(groups[1].join, Some(JoinName::Named { name: "G".into() }));
    assert_eq!(
        groups[1].selectors,
        vec![SubsystemSelector::Area { from: 1, to: 1 }]
    );
    check_sub_fixed_point(&parsed);
}

#[test]
fn a_base_kv_band_stated_high_to_low_is_kept_as_text() {
    let parsed =
        SubsystemSet::parse("SUBSYSTEM 'A'\n   KVRANGE 240.0 100.0\n   AREA 1\nEND\nEND\n")
            .expect("parse");
    assert_eq!(sub_codes(&parsed), vec!["READ.SUB.SOURCE_MALFORMED"]);
    let subsystem = &parsed.set.subsystems[0];
    assert_eq!(
        subsystem.groups[0].selectors,
        vec![SubsystemSelector::Area { from: 1, to: 1 }]
    );
    assert_eq!(subsystem.retained[0].text, "KVRANGE 240.0 100.0");
    check_sub_fixed_point(&parsed);
}

#[test]
fn subsystem_text_after_the_file_end_is_reported_once() {
    let parsed = SubsystemSet::parse("SUBSYSTEM 'A'\n   AREA 1\nEND\nEND\nBUSNAMES\nBUSNUMBERS\n")
        .expect("parse");
    assert_eq!(sub_codes(&parsed), vec!["READ.SUB.TEXT_AFTER_END"]);
    assert_eq!(
        kept_text(&parsed.set.retained),
        vec!["BUSNAMES", "BUSNUMBERS"]
    );
    assert!(parsed.set.retained.iter().all(|kept| kept.after_end));
    check_sub_fixed_point(&parsed);
}

/// Text after the file `END` that reads as grammar is written back after the
/// `END`, so the file it writes states the same subsystems: none.
#[test]
fn a_subsystem_stated_after_the_file_end_stays_text() {
    let parsed = SubsystemSet::parse("END\nSUBSYSTEM 'A'\nAREA 1\nEND\n").expect("parse");
    assert_eq!(sub_codes(&parsed), vec!["READ.SUB.TEXT_AFTER_END"]);
    assert!(parsed.set.subsystems.is_empty());
    assert_eq!(
        kept_text(&parsed.set.retained),
        vec!["SUBSYSTEM 'A'", "AREA 1"]
    );

    let written = check_sub_fixed_point(&parsed);
    let again = SubsystemSet::parse(&written).expect("read the written set");
    assert!(again.set.subsystems.is_empty(), "{written}");
    assert_eq!(
        kept_text(&again.set.retained),
        vec!["SUBSYSTEM 'A'", "AREA 1"]
    );
}

#[test]
fn a_line_read_inside_a_join_is_kept_in_that_group() {
    let parsed = SubsystemSet::parse(
        "SUBSYSTEM 'A'\n   JOIN 'G'\n      JOIN 'H'\n      AREA 1\n   END\n   ZONE 2\nEND\nEND\n",
    )
    .expect("parse");
    assert_eq!(sub_codes(&parsed), vec!["READ.SUB.STATEMENT_UNRECOGNIZED"]);
    let subsystem = &parsed.set.subsystems[0];
    // A `JOIN` inside an open one opens no group: its line is kept as text.
    assert_eq!(subsystem.groups.len(), 2);
    let join = subsystem
        .groups
        .iter()
        .find(|group| group.join == Some(JoinName::Named { name: "G".into() }))
        .expect("the named group");
    assert_eq!(
        join.selectors,
        vec![SubsystemSelector::Area { from: 1, to: 1 }]
    );
    assert_eq!(kept_text(&join.retained), vec!["JOIN 'H'"]);
    // The line belongs to the group, not to the subsystem around it.
    assert!(subsystem.retained.is_empty());

    let written = check_sub_fixed_point(&parsed);
    let again = SubsystemSet::parse(&written).expect("read the written set");
    let group = again.set.subsystems[0]
        .groups
        .iter()
        .find(|group| group.join == Some(JoinName::Named { name: "G".into() }))
        .expect("the named group");
    assert_eq!(kept_text(&group.retained), vec!["JOIN 'H'"]);
}

#[test]
fn a_subsystem_with_no_selector_names_no_bus() {
    let net = select_network();
    let parsed = SubsystemSet::parse("SUBSYSTEM 'EMPTY'\nEND\nEND\n").expect("parse");
    assert!(parsed.set.subsystems[0].select_buses(&net).is_empty());
}

// ---------------------------------------------------------------------------
// Bus selection
// ---------------------------------------------------------------------------

fn selected(name: &str, net: &BalancedNetwork, set: &SubsystemSet) -> Vec<usize> {
    set.get(name)
        .unwrap_or_else(|| panic!("subsystem {name} is in the set"))
        .select_buses(net)
        .iter()
        .map(|bus| bus.0)
        .collect()
}

#[test]
fn each_selector_family_names_the_buses_psse_would_name() {
    let net = select_network();
    let set = parse_sub("selectors.sub").set;

    assert_eq!(selected("A1", &net, &set), [101, 102, 103]);
    assert_eq!(selected("AREARANGE", &net, &set), [101, 102, 103, 201, 202]);
    // Two families in one group intersect: zone 3 holds 201 and 202, owner 3
    // holds 202 alone.
    assert_eq!(selected("BARE", &net, &set), [202]);
    // Two selectors of one family union.
    assert_eq!(selected("BUSLIST", &net, &set), [101, 203]);
    assert_eq!(selected("BUSRANGE", &net, &set), [101, 102, 103]);
    assert_eq!(selected("KV", &net, &set), [101, 102, 103, 201]);
    // The groups of a subsystem union: HIGH is area 1 within 200 to 240 kV.
    assert_eq!(selected("JOINED", &net, &set), [101, 102, 203]);
    // Two JOIN groups with no name union like two named ones. One group
    // holding every selector would intersect to nothing.
    assert_eq!(selected("BAREJOIN", &net, &set), [101, 102, 203]);
    assert_eq!(selected("ONELINE", &net, &set), [103]);
    assert_eq!(selected("A2", &net, &set), [201, 202]);
}

#[test]
fn an_owner_selector_reads_the_retained_psse_owner() {
    let net = select_network();
    // Bus 202 is the only bus the case states owner 3 for; the reader keeps
    // an owner other than 1 in `extras["psse_owner"]`.
    assert_eq!(
        net.buses()
            .iter()
            .filter(|bus| bus.extras.contains_key("psse_owner"))
            .map(|bus| bus.id.0)
            .collect::<Vec<usize>>(),
        [103, 202]
    );
    let set = SubsystemSet::parse("SUBSYSTEM 'O'\n   OWNER 1\nEND\nEND\n").expect("parse");
    // An absent property means owner 1.
    assert_eq!(selected("O", &net, &set.set), [101, 102, 201, 203]);

    let range = SubsystemSet::parse("SUBSYSTEM 'O'\n   OWNERS 2 3\nEND\nEND\n").expect("parse");
    assert_eq!(selected("O", &net, &range.set), [103, 202]);
}

// ---------------------------------------------------------------------------
// Monitored element fixtures
// ---------------------------------------------------------------------------

#[test]
fn a_generated_monitored_element_file_reads_every_statement() {
    let parsed = parse_mon("generated.mon");
    assert!(parsed.diagnostics.is_empty(), "{:?}", mon_codes(&parsed));
    assert_eq!(
        parsed.set.header,
        vec![
            "/PSS(R)E 34".to_owned(),
            "COM PSS(R)E MONITORED ELEMENT FILE".to_owned(),
            "COM WRITTEN BY THE CONFIG FILE BUILDER".to_owned(),
        ]
    );
    let statements = &parsed.set.statements;
    assert_eq!(statements.len(), 12);
    assert_eq!(
        statements[0],
        MonitorStatement::VoltageRange {
            scope: MonitorScope::Subsystem { name: "A1".into() },
            vmin: 0.95,
            vmax: 1.05,
        }
    );
    assert_eq!(
        statements[1],
        MonitorStatement::VoltageDeviation {
            scope: MonitorScope::Subsystem { name: "A1".into() },
            down: 0.03,
            up: Some(0.06),
        }
    );
    assert_eq!(
        statements[2],
        MonitorStatement::BranchesInSubsystem {
            subsystem: "A1".into(),
            low_voltage_3w: true,
        }
    );
    // LINES is the BRANCHES synonym.
    assert_eq!(
        statements[3],
        MonitorStatement::BranchesInSubsystem {
            subsystem: "A2".into(),
            low_voltage_3w: false,
        }
    );
    assert_eq!(
        statements[4],
        MonitorStatement::TiesFromSubsystem {
            subsystem: "A1".into(),
        }
    );
    assert_eq!(
        statements[5],
        MonitorStatement::VoltageRange {
            scope: MonitorScope::AllBuses,
            vmin: 0.94,
            vmax: 1.06,
        }
    );
    // A deviation statement may name one value.
    assert_eq!(
        statements[6],
        MonitorStatement::VoltageDeviation {
            scope: MonitorScope::AllBuses,
            down: 0.05,
            up: None,
        }
    );
    let scopes: Vec<&MonitorScope> = statements[7..]
        .iter()
        .map(|statement| match statement {
            MonitorStatement::VoltageRange { scope, .. }
            | MonitorStatement::VoltageDeviation { scope, .. } => scope,
            other => panic!("expected a voltage statement, found {other:?}"),
        })
        .collect();
    assert_eq!(
        scopes,
        vec![
            &MonitorScope::Bus { bus: BusId(101) },
            &MonitorScope::Area { area: 2 },
            &MonitorScope::Zone { zone: 3 },
            &MonitorScope::Owner { owner: 3 },
            &MonitorScope::Kv { kv: 230.0 },
        ]
    );
    assert!(parsed.set.retained.is_empty());
    // The file states two ENDs; the writer states one.
    assert!(parsed.set.to_mon().ends_with("KV 230.0 0.93 1.07\nEND\n"));
}

#[test]
fn the_block_forms_read_their_branches() {
    let parsed = parse_mon("blocks.mon");
    assert_eq!(mon_codes(&parsed), vec!["READ.MON.STATEMENT_UNRECOGNIZED"]);
    let statements = &parsed.set.statements;
    assert_eq!(statements.len(), 4);
    assert_eq!(
        statements[0],
        MonitorStatement::Branches {
            // An absent circuit reads as 1; mixed indentation and tabs read
            // the same as spaces.
            branches: vec![
                BranchRef {
                    from: BusId(101),
                    to: BusId(102),
                    circuit: "1".into(),
                },
                BranchRef {
                    from: BusId(101),
                    to: BusId(102),
                    circuit: "2".into(),
                },
                BranchRef {
                    from: BusId(102),
                    to: BusId(103),
                    circuit: "1".into(),
                },
                BranchRef {
                    from: BusId(101),
                    to: BusId(203),
                    circuit: "1".into(),
                },
            ],
            retained: Vec::new(),
        }
    );
    assert_eq!(
        statements[1],
        MonitorStatement::Interface {
            name: "WEST".into(),
            rating_mw: Some(200.0),
            branches: vec![
                BranchRef {
                    from: BusId(103),
                    to: BusId(201),
                    circuit: "1".into(),
                },
                BranchRef {
                    from: BusId(102),
                    to: BusId(201),
                    circuit: "1".into(),
                },
            ],
            retained: Vec::new(),
        }
    );
    // An interface with a bare name and no rating.
    assert_eq!(
        statements[2],
        MonitorStatement::Interface {
            name: "NORTH".into(),
            rating_mw: None,
            branches: vec![BranchRef {
                from: BusId(201),
                to: BusId(202),
                circuit: "1".into(),
            }],
            retained: Vec::new(),
        }
    );
    assert_eq!(
        statements[3],
        MonitorStatement::BranchesInSubsystem {
            subsystem: "NOSUCH".into(),
            low_voltage_3w: false,
        }
    );
    assert_eq!(parsed.set.retained.len(), 1);
    assert_eq!(parsed.set.retained[0].text, "MONITOR FLOWS ON EVERYTHING");
}

#[test]
fn monitored_structural_errors_name_their_line() {
    let branches = MonitoredSet::parse("MONITOR BRANCHES\n101 102 1\n")
        .expect_err("an open branch block is refused");
    assert!(branches.to_string().contains("line 1"), "{branches}");
    assert!(branches.to_string().contains("has no END"));

    let interface = MonitoredSet::parse("MONITOR INTERFACE 'W'\n101 102 1\n")
        .expect_err("an open interface block is refused");
    assert!(interface.to_string().contains("line 1"), "{interface}");
    assert!(interface.to_string().contains("INTERFACE 'W'"));
}

#[test]
fn a_block_line_that_states_no_branch_is_reported_and_kept_in_the_block() {
    let parsed =
        MonitoredSet::parse("MONITOR BRANCHES\nALL TIES\n101 102 1\nEND\nEND\n").expect("parse");
    assert_eq!(mon_codes(&parsed), vec!["READ.MON.SOURCE_MALFORMED"]);
    let MonitorStatement::Branches { branches, retained } = &parsed.set.statements[0] else {
        panic!(
            "the block reads as a branch list: {:?}",
            parsed.set.statements
        );
    };
    assert_eq!(
        branches,
        &vec![BranchRef {
            from: BusId(101),
            to: BusId(102),
            circuit: "1".into(),
        }]
    );
    assert_eq!(kept_text(retained), vec!["ALL TIES"]);
    // The line stays inside the block, so the set states none at file level.
    assert!(parsed.set.retained.is_empty());
    check_mon_fixed_point(&parsed);
}

#[test]
fn a_voltage_range_stated_high_to_low_is_kept_as_text() {
    let parsed = MonitoredSet::parse(
        "MONITOR VOLTAGE RANGE ALL BUSES 1.05 0.95\nMONITOR VOLTAGE RANGE ALL BUSES 0.95 1.05\nEND\n",
    )
    .expect("parse");
    assert_eq!(mon_codes(&parsed), vec!["READ.MON.STATEMENT_UNRECOGNIZED"]);
    assert_eq!(
        parsed.set.statements,
        vec![MonitorStatement::VoltageRange {
            scope: MonitorScope::AllBuses,
            vmin: 0.95,
            vmax: 1.05,
        }]
    );
    assert_eq!(
        parsed.set.retained[0].text,
        "MONITOR VOLTAGE RANGE ALL BUSES 1.05 0.95"
    );
    check_mon_fixed_point(&parsed);
}

#[test]
fn monitored_text_after_the_file_end_is_reported_once() {
    let parsed = MonitoredSet::parse("MONITOR TIES FROM SUBSYSTEM 'A'\nEND\nEND\nBUSNAMES\n")
        .expect("parse");
    assert_eq!(mon_codes(&parsed), vec!["READ.MON.TEXT_AFTER_END"]);
    assert_eq!(kept_text(&parsed.set.retained), vec!["BUSNAMES"]);
    assert!(parsed.set.retained[0].after_end);
    check_mon_fixed_point(&parsed);
}

/// Text after the file `END` that reads as grammar is written back after the
/// `END`, so the file it writes states the same statements: none.
#[test]
fn a_monitor_statement_after_the_file_end_stays_text() {
    let parsed =
        MonitoredSet::parse("END\nMONITOR VOLTAGE RANGE ALL BUSES 0.9 1.1\n").expect("parse");
    assert_eq!(mon_codes(&parsed), vec!["READ.MON.TEXT_AFTER_END"]);
    assert!(parsed.set.statements.is_empty());
    assert_eq!(
        kept_text(&parsed.set.retained),
        vec!["MONITOR VOLTAGE RANGE ALL BUSES 0.9 1.1"]
    );

    let written = check_mon_fixed_point(&parsed);
    let again = MonitoredSet::parse(&written).expect("read the written set");
    assert!(again.set.statements.is_empty(), "{written}");
    assert_eq!(
        kept_text(&again.set.retained),
        vec!["MONITOR VOLTAGE RANGE ALL BUSES 0.9 1.1"]
    );
}

/// A statement line inside a block opens no statement: the block runs to its
/// own `END`, so the line is kept where it was read.
#[test]
fn a_statement_read_inside_a_block_is_kept_in_that_block() {
    let parsed = MonitoredSet::parse(
        "MONITOR BRANCHES\n101 102 1\nMONITOR VOLTAGE RANGE ALL BUSES 0.9 1.1\nEND\nEND\n",
    )
    .expect("parse");
    assert_eq!(mon_codes(&parsed), vec!["READ.MON.SOURCE_MALFORMED"]);
    assert_eq!(parsed.set.statements.len(), 1);
    let MonitorStatement::Branches { retained, .. } = &parsed.set.statements[0] else {
        panic!(
            "the block reads as a branch list: {:?}",
            parsed.set.statements
        );
    };
    assert_eq!(
        kept_text(retained),
        vec!["MONITOR VOLTAGE RANGE ALL BUSES 0.9 1.1"]
    );
    assert!(parsed.set.retained.is_empty());

    let written = check_mon_fixed_point(&parsed);
    let again = MonitoredSet::parse(&written).expect("read the written set");
    let MonitorStatement::Branches { retained, .. } = &again.set.statements[0] else {
        panic!("the written block reads as a branch list: {written}");
    };
    assert_eq!(
        kept_text(retained),
        vec!["MONITOR VOLTAGE RANGE ALL BUSES 0.9 1.1"]
    );
    assert_eq!(again.set.statements.len(), 1);
}

#[test]
fn the_monitored_note_budget_bounds_the_reader() {
    use std::fmt::Write as _;

    let mut text = String::new();
    for index in 0..50 {
        let _ = writeln!(text, "NOT A STATEMENT {index}");
    }
    text.push_str("END\n");
    let parsed = MonitoredSet::parse(&text).expect("parse");
    assert_eq!(parsed.diagnostics.len(), 17);
    assert_eq!(
        parsed
            .diagnostics
            .last()
            .map(powerio_core::Diagnostic::code),
        Some("READ.MON.NOTES_TRUNCATED")
    );
    assert_eq!(parsed.set.retained.len(), 50);
}

// ---------------------------------------------------------------------------
// Monitored element resolution
// ---------------------------------------------------------------------------

fn rows(set: &BTreeSet<usize>) -> Vec<usize> {
    set.iter().copied().collect()
}

#[test]
fn a_generated_monitored_set_binds_to_the_rows_of_the_network() {
    let net = select_network();
    let subsystems = parse_sub("selectors.sub").set;
    let resolution = parse_mon("generated.mon").set.resolve(&net, &subsystems);

    // Branches inside A1 are rows 0, 1, and 2; the A2 statement adds row 4.
    // An out of service branch is monitored like any other.
    assert_eq!(rows(&resolution.branch_rows), [0, 1, 2, 4]);
    // 3WLOWVOLTAGE adds the transformer whose 138 kV winding sits in A1.
    assert_eq!(rows(&resolution.transformer_3w_rows), [0]);
    // A tie has exactly one terminal inside.
    assert_eq!(rows(&resolution.tie_rows), [3, 5]);
    assert!(resolution.interfaces.is_empty());
    assert!(resolution.unresolved.is_empty());

    let ranges: Vec<(Vec<usize>, f64, Option<f64>)> = resolution
        .voltage_ranges
        .iter()
        .map(|scope| (rows(&scope.bus_rows), scope.low, scope.high))
        .collect();
    assert_eq!(
        ranges,
        vec![
            (vec![0, 1, 2], 0.95, Some(1.05)),
            (vec![0, 1, 2, 3, 4, 5], 0.94, Some(1.06)),
            (vec![0], 0.95, Some(1.05)),
            (vec![3, 4], 0.94, Some(1.06)),
            (vec![4], 0.92, Some(1.08)),
            (vec![0, 1, 3], 0.93, Some(1.07)),
        ]
    );

    let deviations: Vec<(Vec<usize>, f64, Option<f64>)> = resolution
        .voltage_deviations
        .iter()
        .map(|scope| (rows(&scope.bus_rows), scope.low, scope.high))
        .collect();
    assert_eq!(
        deviations,
        vec![
            (vec![0, 1, 2], 0.03, Some(0.06)),
            (vec![0, 1, 2, 3, 4, 5], 0.05, None),
            (vec![3, 4], 0.06, Some(0.055)),
        ]
    );
    assert!(resolution.diagnostics().is_empty());
}

#[test]
fn block_statements_bind_their_branches_and_report_what_named_nothing() {
    let net = select_network();
    let subsystems = parse_sub("selectors.sub").set;
    let resolution = parse_mon("blocks.mon").set.resolve(&net, &subsystems);

    assert_eq!(rows(&resolution.branch_rows), [0, 1, 2]);
    assert!(resolution.tie_rows.is_empty());
    assert_eq!(resolution.interfaces.len(), 2);
    assert_eq!(resolution.interfaces[0].name, "WEST");
    assert_eq!(resolution.interfaces[0].rating_mw, Some(200.0));
    assert_eq!(
        resolution.interfaces[0].members,
        vec![
            InterfaceMember {
                row: 3,
                reversed: false,
            },
            InterfaceMember {
                row: 5,
                reversed: false,
            },
        ]
    );
    assert_eq!(resolution.interfaces[1].name, "NORTH");
    assert_eq!(resolution.interfaces[1].rating_mw, None);
    assert_eq!(
        resolution.interfaces[1].members,
        vec![InterfaceMember {
            row: 4,
            reversed: false,
        }]
    );

    let reasons: Vec<&UnresolvedMonitorReason> = resolution
        .unresolved
        .iter()
        .map(|entry| &entry.reason)
        .collect();
    assert_eq!(
        reasons,
        vec![
            &UnresolvedMonitorReason::NoSuchBranch {
                from: BusId(101),
                to: BusId(203),
                circuit: "1".into(),
            },
            &UnresolvedMonitorReason::NoSuchSubsystem,
        ]
    );

    let diagnostics = resolution.diagnostics();
    assert_eq!(diagnostics.len(), 2);
    assert!(
        diagnostics
            .iter()
            .all(|note| note.code() == "BUILD.MON.STATEMENT_UNRESOLVED")
    );
    assert!(diagnostics[0].message().contains("no branch 101 to 203"));
    assert!(diagnostics[1].message().contains("'NOSUCH'"));
}

#[test]
fn an_interface_member_states_its_orientation_against_the_stored_row() {
    let net = select_network();
    let subsystems = SubsystemSet::default();
    // Row 3 is stored 103 to 201. The first member names it the other way
    // round, so its flow enters the interface sum with the opposite sign.
    let parsed = MonitoredSet::parse("MONITOR INTERFACE 'W'\n201 103 1\n103 201 1\nEND\nEND\n")
        .expect("parse");
    let resolution = parsed.set.resolve(&net, &subsystems);
    assert_eq!(
        resolution.interfaces[0].members,
        vec![
            InterfaceMember {
                row: 3,
                reversed: true,
            },
            InterfaceMember {
                row: 3,
                reversed: false,
            },
        ]
    );
    assert!(resolution.unresolved.is_empty());
}

#[test]
fn an_index_built_once_binds_the_same_set() {
    let net = select_network();
    let subsystems = parse_sub("selectors.sub").set;
    let set = parse_mon("generated.mon").set;
    let index = PsseEquipmentIndex::new(&net);
    assert_eq!(
        set.resolve_with(&index, &subsystems),
        set.resolve(&net, &subsystems)
    );
}

#[test]
fn a_terminal_pair_naming_two_branches_binds_to_neither() {
    let mut net = select_network();
    // A second branch stored the other way round takes circuit '1' too,
    // because the writer allocates ids per stored terminal pair.
    let mut reversed = net.branches()[0].clone();
    std::mem::swap(&mut reversed.from, &mut reversed.to);
    reversed.uid = Some("reversed-101-102".to_owned());
    net.branches_mut().push(reversed);

    let subsystems = SubsystemSet::default();
    let parsed = MonitoredSet::parse("MONITOR BRANCHES\n101 102 1\nEND\nEND\n").expect("parse");
    let resolution = parsed.set.resolve(&net, &subsystems);
    assert!(resolution.branch_rows.is_empty());
    assert_eq!(
        resolution.unresolved[0].reason,
        UnresolvedMonitorReason::AmbiguousBranch {
            from: BusId(101),
            to: BusId(102),
            circuit: "1".into(),
            matches: 2,
        }
    );
}

#[test]
fn a_scope_naming_nothing_in_the_network_names_no_row() {
    let net = select_network();
    let subsystems = SubsystemSet::default();
    let parsed = MonitoredSet::parse(
        "MONITOR VOLTAGE RANGE AREA 9 0.95 1.05\nMONITOR VOLTAGE RANGE BUS 999 0.95 1.05\nEND\n",
    )
    .expect("parse");
    let resolution = parsed.set.resolve(&net, &subsystems);
    assert_eq!(resolution.voltage_ranges.len(), 2);
    assert!(
        resolution
            .voltage_ranges
            .iter()
            .all(|scope| scope.bus_rows.is_empty())
    );
    assert!(resolution.unresolved.is_empty());
}

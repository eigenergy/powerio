//! Expanding a `.con` file's automatic specifications against a network and a
//! subsystem set: which elements each specification names, how the generated
//! cases are named, and what a `SKIP` rule removes.

mod common;
mod helpers;
#[allow(unused_imports)]
use common::*;

use std::path::{Path, PathBuf};

use powerio_tx::network::BalancedNetwork;
use powerio_tx::{
    AutomaticTarget, BusId, ContingencyAction, ContingencySet, Expanded, PsseEquipmentIndex,
    SkipRule, SubsystemSet,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/psse/contingency")
        .join(name)
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("read fixture")
}

fn select_network() -> BalancedNetwork {
    helpers::parse_file(fixture("select_v33.raw"), Some("psse"))
        .expect("parse select_v33.raw")
        .network
}

fn subsystems() -> SubsystemSet {
    SubsystemSet::parse(&read("selectors.sub"))
        .expect("parse selectors.sub")
        .set
}

fn expand(text: &str, net: &BalancedNetwork, subsystems: &SubsystemSet) -> Expanded {
    ContingencySet::parse(text)
        .expect("parse the contingency text")
        .set
        .expand(net, subsystems)
}

fn names(expanded: &Expanded) -> Vec<&str> {
    expanded
        .set
        .cases
        .iter()
        .map(|case| case.name.as_str())
        .collect()
}

#[test]
fn the_fixture_expands_every_target_and_keeps_the_unknown_subsystem() {
    let net = select_network();
    let subsystems = subsystems();
    let expanded = expand(&read("expand.con"), &net, &subsystems);

    assert_eq!(
        names(&expanded),
        vec![
            // The explicit case comes first.
            "EXPLICIT",
            // SINGLE BRANCH IN SUBSYSTEM 'A1' 3WLOWVOLTAGE. Circuit 2 between
            // 101 and 102 is the one the SKIP block names.
            "L_101_102_1",
            "L_102_103_1",
            "T_101_102_103_1",
            // SINGLE UNIT IN SUBSYSTEM 'A1'; the machine at bus 103 is out of
            // service.
            "G_101_1",
            "G_101_2",
            // SINGLE TIE FROM SUBSYSTEM 'A1'; the 102 to 201 tie is out of
            // service.
            "L_103_201_1",
            // DOUBLE BRANCH IN SUBSYSTEM 'A1'.
            "L_101_102_1+L_102_103_1",
        ]
    );

    // The specification naming a subsystem the set does not state stays, and
    // the SKIP rules stay with it.
    assert_eq!(expanded.set.automatic.len(), 1);
    assert_eq!(expanded.set.automatic[0].subsystem, "NOSUCH");
    assert_eq!(expanded.set.automatic[0].target, AutomaticTarget::Branch);
    assert_eq!(expanded.set.skips.len(), 1);
    assert_eq!(
        expanded
            .diagnostics
            .iter()
            .map(powerio_core::Diagnostic::code)
            .collect::<Vec<&str>>(),
        vec!["BUILD.CON.SUBSYSTEM_UNKNOWN"]
    );
    assert!(expanded.diagnostics[0].message().contains("'NOSUCH'"));

    // The header and the statements kept as text survive.
    assert_eq!(expanded.set.header.len(), 1);
    assert!(expanded.set.retained.is_empty());
}

#[test]
fn every_generated_case_states_the_action_that_outages_its_element() {
    let net = select_network();
    let expanded = expand(&read("expand.con"), &net, &subsystems());
    let case = |name: &str| {
        expanded
            .set
            .cases
            .iter()
            .find(|case| case.name == name)
            .unwrap_or_else(|| panic!("case {name} was generated"))
    };

    assert_eq!(
        case("L_101_102_1").actions,
        vec![ContingencyAction::OpenBranch {
            from: BusId(101),
            to: BusId(102),
            circuit: "1".into(),
        }]
    );
    assert_eq!(
        case("T_101_102_103_1").actions,
        vec![ContingencyAction::OpenThreeWinding {
            buses: [BusId(101), BusId(102), BusId(103)],
            circuit: "1".into(),
        }]
    );
    assert_eq!(
        case("G_101_2").actions,
        vec![ContingencyAction::RemoveMachine {
            bus: BusId(101),
            id: "2".into(),
        }]
    );
    assert_eq!(
        case("L_103_201_1").actions,
        vec![ContingencyAction::OpenBranch {
            from: BusId(103),
            to: BusId(201),
            circuit: "1".into(),
        }]
    );
    // A double case states both single cases' actions, in order.
    assert_eq!(
        case("L_101_102_1+L_102_103_1").actions,
        vec![
            ContingencyAction::OpenBranch {
                from: BusId(101),
                to: BusId(102),
                circuit: "1".into(),
            },
            ContingencyAction::OpenBranch {
                from: BusId(102),
                to: BusId(103),
                circuit: "1".into(),
            },
        ]
    );
}

#[test]
fn a_skip_rule_removes_a_branch_in_either_orientation() {
    let net = select_network();
    let subsystems = subsystems();
    let without = expand("SINGLE BRANCH IN SUBSYSTEM 'A1'\nEND\n", &net, &subsystems);
    assert_eq!(
        names(&without),
        vec!["L_101_102_1", "L_101_102_2", "L_102_103_1"]
    );

    // The rule names the branch the other way round and on the same circuit.
    let with = expand(
        "SINGLE BRANCH IN SUBSYSTEM 'A1'\nSKIP\n102 TO 101 CIRCUIT 2\nEND\nEND\n",
        &net,
        &subsystems,
    );
    assert_eq!(names(&with), vec!["L_101_102_1", "L_102_103_1"]);
    // Every specification expanded, so no SKIP rule is left to apply.
    assert!(with.set.automatic.is_empty());
    assert!(with.set.skips.is_empty());

    // A rule on another circuit removes nothing.
    let other = expand(
        "SINGLE BRANCH IN SUBSYSTEM 'A1'\nSKIP\n101 TO 102 CIRCUIT 9\nEND\nEND\n",
        &net,
        &subsystems,
    );
    assert_eq!(names(&other).len(), 3);
}

#[test]
fn a_double_specification_states_one_case_per_unordered_pair() {
    let net = select_network();
    let subsystems = subsystems();
    // Area 1 and area 2 together hold three in service machines.
    let singles = expand(
        "SINGLE UNIT IN SUBSYSTEM 'AREARANGE'\nEND\n",
        &net,
        &subsystems,
    );
    assert_eq!(names(&singles), vec!["G_101_1", "G_101_2", "G_201_1"]);

    let doubles = expand(
        "DOUBLE UNIT IN SUBSYSTEM 'AREARANGE'\nEND\n",
        &net,
        &subsystems,
    );
    let count = singles.set.cases.len();
    assert_eq!(doubles.set.cases.len(), count * (count - 1) / 2);
    assert_eq!(
        names(&doubles),
        vec!["G_101_1+G_101_2", "G_101_1+G_201_1", "G_101_2+G_201_1"]
    );
    assert_eq!(doubles.set.cases[0].actions.len(), 2);
}

#[test]
fn a_tie_specification_names_the_branches_that_cross_the_border() {
    let net = select_network();
    let subsystems = subsystems();
    let ties = expand("SINGLE TIE FROM SUBSYSTEM 'A2'\nEND\n", &net, &subsystems);
    // Area 2 holds buses 201 and 202: the 103 to 201 branch crosses, and the
    // 102 to 201 branch is out of service.
    assert_eq!(names(&ties), vec!["L_103_201_1"]);
}

#[test]
fn a_three_winding_transformer_expands_only_under_3wlowvoltage() {
    let net = select_network();
    let subsystems = subsystems();
    let plain = expand("SINGLE BRANCH IN SUBSYSTEM 'KV'\nEND\n", &net, &subsystems);
    assert!(
        !names(&plain).iter().any(|name| name.starts_with("T_")),
        "{:?}",
        names(&plain)
    );

    // The KV subsystem holds buses 101, 102, 103, and 201. The first
    // transformer's lowest winding is the 138 kV one at bus 103; the second
    // transformer's is the 13.8 kV one at bus 203, which is outside.
    let low = expand(
        "SINGLE BRANCH IN SUBSYSTEM 'KV' 3WLOWVOLTAGE\nEND\n",
        &net,
        &subsystems,
    );
    let generated: Vec<&str> = names(&low)
        .into_iter()
        .filter(|name| name.starts_with("T_"))
        .collect();
    assert_eq!(generated, vec!["T_101_102_103_1"]);
}

#[test]
fn an_expanded_set_writes_and_reads_back() {
    let net = select_network();
    let expanded = expand(&read("expand.con"), &net, &subsystems());
    let written = expanded.set.to_con();
    let again = ContingencySet::parse(&written).expect("read the written set");
    assert_eq!(expanded.set, again.set);
    assert_eq!(written, again.set.to_con());
    assert!(written.contains("CONTINGENCY 'T_101_102_103_1'"));
    assert!(written.contains("SINGLE BRANCH IN SUBSYSTEM 'NOSUCH'"));
}

#[test]
fn a_specification_that_names_nothing_in_its_subsystem_is_noted() {
    let net = select_network();
    let subsystems = subsystems();
    // The BUSLIST subsystem holds buses 101 and 203, which no branch joins.
    let empty = expand(
        "SINGLE BRANCH IN SUBSYSTEM 'BUSLIST'\nEND\n",
        &net,
        &subsystems,
    );
    assert!(empty.set.cases.is_empty());
    assert!(empty.set.automatic.is_empty());
    assert_eq!(
        empty
            .diagnostics
            .iter()
            .map(powerio_core::Diagnostic::code)
            .collect::<Vec<&str>>(),
        vec!["BUILD.CON.SPECIFICATION_EMPTY"]
    );
    assert!(empty.diagnostics[0].message().contains("'BUSLIST'"));

    // A specification that names one element is not empty.
    let filled = expand("SINGLE UNIT IN SUBSYSTEM 'A2'\nEND\n", &net, &subsystems);
    assert_eq!(names(&filled), vec!["G_201_1"]);
    assert!(filled.diagnostics.is_empty());
}

#[test]
fn a_double_specification_that_names_one_element_is_noted() {
    let net = select_network();
    let subsystems = subsystems();
    // Subsystem 'A2' holds one in service generator, and a pair needs two.
    let expanded = expand("DOUBLE UNIT IN SUBSYSTEM 'A2'\nEND\n", &net, &subsystems);
    assert!(expanded.set.cases.is_empty());
    assert!(expanded.set.automatic.is_empty());
    assert_eq!(
        expanded
            .diagnostics
            .iter()
            .map(powerio_core::Diagnostic::code)
            .collect::<Vec<&str>>(),
        vec!["BUILD.CON.SPECIFICATION_EMPTY"]
    );
    let message = expanded.diagnostics[0].message();
    assert!(message.contains("DOUBLE UNIT"), "{message}");
    assert!(message.contains("holds 1 in service element"), "{message}");
    assert!(message.contains("needs 2"), "{message}");
}

#[test]
fn skip_rules_stay_on_a_set_that_states_no_specification() {
    let net = select_network();
    let set = ContingencySet {
        skips: vec![SkipRule {
            from: BusId(101),
            to: BusId(102),
            circuit: "2".into(),
        }],
        ..ContingencySet::default()
    };
    let expanded = set.expand(&net, &SubsystemSet::default());
    assert_eq!(expanded.set.skips, set.skips);
    assert!(expanded.diagnostics.is_empty());
}

#[test]
fn an_index_built_once_expands_the_same_set() {
    let net = select_network();
    let subsystems = subsystems();
    let set = ContingencySet::parse(&read("expand.con"))
        .expect("parse")
        .set;
    let index = PsseEquipmentIndex::new(&net);
    assert_eq!(
        set.expand_with(&index, &subsystems).set,
        set.expand(&net, &subsystems).set
    );
}

#[test]
fn a_set_with_no_automatic_specification_expands_to_itself() {
    let net = select_network();
    let set = ContingencySet::parse(&read("expand.con"))
        .expect("parse")
        .set;
    let explicit = ContingencySet {
        automatic: Vec::new(),
        skips: Vec::new(),
        ..set
    };
    let expanded = explicit.expand(&net, &SubsystemSet::default());
    assert_eq!(expanded.set, explicit);
    assert!(expanded.diagnostics.is_empty());
}

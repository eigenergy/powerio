//! The TARA statements beyond the PSS/E grammar: elements named by name,
//! dispatch blocks, and subsystem dispatch rules.

mod common;
mod helpers;
#[allow(unused_imports)]
use common::*;

use std::path::{Path, PathBuf};

use powerio_tx::network::BalancedNetwork;
use powerio_tx::{
    BusId, ChangeOp, ChangeUnit, ContingencyAction, ContingencyResolution, ContingencySet,
    DispatchEntry, DispatchLevel, ScaledQuantity, SubsystemDispatchRule, SubsystemSet,
    UnresolvedReason,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/psse/contingency")
        .join(name)
}

/// Buses 1 to 3 at 230 kV, bus 4 at 138 kV, and bus 5 at 13.8 kV, named
/// `B1` to `B5`.
fn resolve_network() -> BalancedNetwork {
    helpers::parse_file(fixture("resolve_v33.raw"), Some("psse"))
        .expect("parse resolve_v33.raw")
        .network
}

fn resolve(text: &str, net: &BalancedNetwork) -> ContingencyResolution {
    ContingencySet::parse(text).expect("parse").set.resolve(net)
}

/// What each case bound to, or the reason its first unbound action did not.
fn outcome(
    resolution: &ContingencyResolution,
) -> Vec<Result<Vec<(&str, usize)>, UnresolvedReason>> {
    resolution
        .cases
        .iter()
        .map(|case| match case.unresolved.first() {
            Some(first) => Err(first.reason),
            None => Ok(case
                .components
                .iter()
                .map(|component| (component.component_type, component.row))
                .collect()),
        })
        .collect()
}

#[test]
fn statements_naming_buses_by_name_bind_like_numbered_ones() {
    let net = resolve_network();
    let named = resolve(
        "BUSNAMES\n\
         CONTINGENCY 'LINE'\n\
         TRIP LINE FROM BUS 'B1 230' TO BUS 'B2 230' CKT 2\n\
         END\n\
         CONTINGENCY 'LOOSE SPELLING'\n\
         DISCONNECT BUS 'b3    230.0'\n\
         END\n\
         CONTINGENCY 'MACHINE'\n\
         REMOVE MACHINE 3 FROM BUS 'B2 230'\n\
         END\n\
         CONTINGENCY 'THREE WINDING'\n\
         OPEN THREEWINDING AT BUS 'B2 230' TO BUS 'B4 138' TO BUS 'B5 13.8'\n\
         END\n\
         CONTINGENCY 'NO KV'\n\
         DISCONNECT BUS 'B4'\n\
         END\n\
         END\n",
        &net,
    );
    let numbered = resolve(
        "CONTINGENCY 'LINE'\n\
         TRIP LINE FROM BUS 1 TO BUS 2 CKT 2\n\
         END\n\
         CONTINGENCY 'LOOSE SPELLING'\n\
         DISCONNECT BUS 3\n\
         END\n\
         CONTINGENCY 'MACHINE'\n\
         REMOVE MACHINE 3 FROM BUS 2\n\
         END\n\
         CONTINGENCY 'THREE WINDING'\n\
         OPEN THREEWINDING AT BUS 2 TO BUS 4 TO BUS 5\n\
         END\n\
         CONTINGENCY 'NO KV'\n\
         DISCONNECT BUS 4\n\
         END\n\
         END\n",
        &net,
    );
    assert_eq!(named.unresolved, 0, "{:?}", outcome(&named));
    assert_eq!(outcome(&named), outcome(&numbered));
}

#[test]
fn a_name_that_names_no_bus_or_several_binds_none() {
    let mut net = resolve_network();
    // Bus 3 takes bus 2's name exactly, and bus 5 a name equal to bus 4's
    // without regard to case at bus 4's base kV.
    net.buses_mut()[2].name = Some("B2".into());
    net.buses_mut()[4].name = Some("b4".into());
    net.buses_mut()[4].base_kv = 138.0;
    let resolution = resolve(
        "CONTINGENCY 'UNKNOWN'\n\
         DISCONNECT BUS 'B9 230'\n\
         END\n\
         CONTINGENCY 'WRONG KV'\n\
         DISCONNECT BUS 'B1 138'\n\
         END\n\
         CONTINGENCY 'EXACT TWICE'\n\
         DISCONNECT BUS 'B2 230'\n\
         END\n\
         CONTINGENCY 'EXACT ONCE'\n\
         DISCONNECT BUS 'b4 138'\n\
         END\n\
         CONTINGENCY 'LOOSE TWICE'\n\
         DISCONNECT BUS ' B4  138'\n\
         END\n\
         END\n",
        &net,
    );
    assert_eq!(
        outcome(&resolution),
        [
            Err(UnresolvedReason::NoSuchBusName),
            Err(UnresolvedReason::NoSuchBusName),
            Err(UnresolvedReason::AmbiguousBusName { matches: 2 }),
            Ok(vec![("bus", 4)]),
            // ' B4  138' trims to the exact name `B4`, so it binds bus 4.
            Ok(vec![("bus", 3)]),
        ]
    );
    // A name differing in more than case and whitespace names nothing.
    let other = resolve(
        "CONTINGENCY 'SUFFIX'\nDISCONNECT BUS 'B4X 138'\nEND\n\
         CONTINGENCY 'SPLIT'\nDISCONNECT BUS 'B 4 138'\nEND\nEND\n",
        &net,
    );
    assert_eq!(
        outcome(&other),
        [
            Err(UnresolvedReason::NoSuchBusName),
            Err(UnresolvedReason::NoSuchBusName)
        ]
    );
    let notes = resolution.diagnostics();
    assert!(
        notes[0].message().contains("a bus name names no bus"),
        "{}",
        notes[0].message()
    );
}

#[test]
fn a_name_matching_only_without_case_binds_when_it_is_the_only_one() {
    let mut net = resolve_network();
    net.buses_mut()[0].name = Some("North  Plant".into());
    net.buses_mut()[1].name = Some("NORTH PLANT".into());
    net.buses_mut()[1].base_kv = 230.0;
    let resolution = resolve(
        "CONTINGENCY 'ONE LOOSE'\nDISCONNECT BUS 'north plant 230'\nEND\n\
         CONTINGENCY 'EXACT'\nDISCONNECT BUS 'NORTH PLANT 230'\nEND\nEND\n",
        &net,
    );
    // Both buses match without regard to case and spacing, so the loose
    // spelling is ambiguous; the exact spelling names bus 2 alone.
    assert_eq!(
        outcome(&resolution),
        [
            Err(UnresolvedReason::AmbiguousBusName { matches: 2 }),
            Ok(vec![("bus", 1)]),
        ]
    );
}

#[test]
fn a_branch_named_by_name_binds_exactly_then_loosely() {
    let mut net = resolve_network();
    net.branches_mut()[0].name = Some("NORTH TIE".into());
    net.branches_mut()[1].name = Some("South  Tie".into());
    net.branches_mut()[2].name = Some("SOUTH TIE".into());
    let resolution = resolve(
        "BRANCHNAMES\n\
         CONTINGENCY 'EXACT'\nOPEN \"NORTH TIE\"\nEND\n\
         CONTINGENCY 'LOOSE'\nTRIP BRANCH 'north   tie'\nEND\n\
         CONTINGENCY 'TWO LOOSE'\nOPEN 'south tie'\nEND\n\
         CONTINGENCY 'ONE EXACT'\nOPEN 'SOUTH TIE'\nEND\n\
         CONTINGENCY 'NONE'\nOPEN 'EAST TIE'\nEND\n\
         END\n",
        &net,
    );
    assert_eq!(
        outcome(&resolution),
        [
            Ok(vec![("branch", 0)]),
            Ok(vec![("branch", 0)]),
            Err(UnresolvedReason::AmbiguousBranchName { matches: 2 }),
            Ok(vec![("branch", 2)]),
            Err(UnresolvedReason::NoSuchBranchName),
        ]
    );
}

#[test]
fn dispatch_blocks_read_their_level_entries_and_action() {
    let text = std::fs::read_to_string(fixture("tara_extensions.con")).expect("read");
    let set = ContingencySet::parse(&text).expect("parse").set;
    let defaults = set.calc_default_dispatch();
    assert_eq!(defaults.len(), 2);
    assert_eq!(defaults[0].level, DispatchLevel::Default);
    assert_eq!(
        defaults[0].block.entries,
        [
            DispatchEntry::Subsystem {
                name: "SYSTEM".into(),
                share: None,
            },
            DispatchEntry::ParticipatingMachines,
        ]
    );
    assert_eq!(defaults[1].level, DispatchLevel::Down);
    assert_eq!(
        defaults[1].block.entries,
        [DispatchEntry::Subsystem {
            name: "X".into(),
            share: Some(34.0),
        }]
    );

    // The case action ahead of DISPATCH, and the block under it.
    let dispatched: Vec<_> = set.cases[0]
        .actions
        .iter()
        .filter_map(ContingencyAction::calc_dispatched_action)
        .collect();
    assert_eq!(dispatched.len(), 1);
    let ContingencyAction::ChangeGeneration { bus, change } = &dispatched[0].action else {
        panic!("a generation change: {:?}", dispatched[0].action);
    };
    assert_eq!(*bus, BusId(4));
    assert_eq!(
        (change.op, change.unit),
        (ChangeOp::Set, ChangeUnit::Percent)
    );
    assert_eq!(
        dispatched[0].block.entries,
        [DispatchEntry::Subsystem {
            name: "AREA1".into(),
            share: None,
        }]
    );

    // Against a network, the dispatched action binds its bus; the bus names
    // of the fixture name no bus of this network.
    let resolution = set.resolve(&resolve_network());
    let case = &resolution.cases[0];
    assert!(
        case.components
            .iter()
            .any(|c| c.component_type == "bus" && c.row == 3)
    );
    assert_eq!(case.unresolved[0].reason, UnresolvedReason::NoSuchBusName);
}

#[test]
fn shares_bus_entries_and_unread_lines_in_a_dispatch_block() {
    let set = ContingencySet::parse(
        "CONTINGENCY 'SPLIT'\n\
         SET BUS 2 GENERATION TO 100 MW DISPATCH\n\
         SUBSYSTEM ABC 32\n\
         SUBSYSTEM 'DEF' 68\n\
         BUS 3 10 /comment\n\
         SOMETHING ELSE\n\
         END\n\
         END\n\
         DEFAULT DISPATCH FIRSTLEVEL\n\
         SUBSYSTEM 'FIRST'\n\
         END\n\
         DEFAULT DISPATCH UP\n\
         SUBSYSTEM 'UPSYS' 77\n\
         END\n\
         END\n",
    )
    .expect("parse")
    .set;
    let dispatched = set.cases[0].actions[0]
        .calc_dispatched_action()
        .expect("a dispatched action");
    assert_eq!(
        dispatched.block.entries,
        [
            DispatchEntry::Subsystem {
                name: "ABC".into(),
                share: Some(32.0),
            },
            DispatchEntry::Subsystem {
                name: "DEF".into(),
                share: Some(68.0),
            },
            DispatchEntry::Bus {
                bus: BusId(3),
                share: Some(10.0),
            },
        ]
    );
    assert_eq!(dispatched.block.unread, ["SOMETHING ELSE"]);
    let levels: Vec<_> = set
        .calc_default_dispatch()
        .iter()
        .map(|block| block.level)
        .collect();
    assert_eq!(levels, [DispatchLevel::FirstLevel, DispatchLevel::Up]);
    // Every case of the set binds against a network holding its buses.
    let resolution = set.resolve(&resolve_network());
    assert_eq!(resolution.unresolved, 0);
}

#[test]
fn subsystem_dispatch_lines_read_as_typed_rules() {
    let fixture_set =
        SubsystemSet::parse(&std::fs::read_to_string(fixture("selectors.sub")).unwrap())
            .expect("parse")
            .set;
    let a2 = fixture_set
        .subsystems
        .iter()
        .find(|s| s.name == "A2")
        .expect("A2");
    assert_eq!(
        a2.calc_dispatch_rules(),
        [SubsystemDispatchRule::Scale {
            quantity: ScaledQuantity::Export,
            pmax_greater_mw: None,
            include_offline: true,
            include_nonconforming: false,
        }]
    );

    let set = SubsystemSet::parse(
        "SUBSYSTEM 'GEN'\n\
         AREA 1\n\
         PARTICIPATE INCLUDE OFFLINE\n\
         SCALE ALL GENERATION WITH PMAX GREATER 50 MW\n\
         SCALE ALL LOAD INCLUDE NONCONFORMING\n\
         SCALE ALL FOR IMPORT\n\
         BASELOAD 2\n\
         TURBINETYPE 13\n\
         EXCEPT BUS 5\n\
         SCALE ALL SOMETHING\n\
         JOIN\n\
         ZONE 2\n\
         PARTICIPATE\n\
         END\n\
         END\n\
         END\n",
    )
    .expect("parse")
    .set;
    assert_eq!(
        set.subsystems[0].calc_dispatch_rules(),
        [
            SubsystemDispatchRule::Participate {
                include_offline: true,
            },
            SubsystemDispatchRule::Scale {
                quantity: ScaledQuantity::Generation,
                pmax_greater_mw: Some(50.0),
                include_offline: false,
                include_nonconforming: false,
            },
            SubsystemDispatchRule::Scale {
                quantity: ScaledQuantity::Load,
                pmax_greater_mw: None,
                include_offline: false,
                include_nonconforming: true,
            },
            SubsystemDispatchRule::Scale {
                quantity: ScaledQuantity::Import,
                pmax_greater_mw: None,
                include_offline: false,
                include_nonconforming: false,
            },
            SubsystemDispatchRule::Baseload { code: 2 },
            SubsystemDispatchRule::TurbineType { code: 13 },
            SubsystemDispatchRule::Except {
                words: vec!["BUS".into(), "5".into()],
            },
            // The JOIN group's own line follows the subsystem's.
            SubsystemDispatchRule::Participate {
                include_offline: false,
            },
        ]
    );
}

//! Binding PSS/E contingency cases to a network: the recomputed equipment
//! ids, their agreement with what the RAW writer states, and what each
//! statement resolves to.

mod common;
mod helpers;
#[allow(unused_imports)]
use common::*;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use powerio_core::ComponentId;
use powerio_tx::network::BalancedNetwork;
use powerio_tx::{
    BusId, ContingencyResolution, ContingencySet, PsseEquipmentIndex, ResolvedCase,
    ResolvedComponent, UnresolvedReason,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/psse/contingency")
        .join(name)
}

fn resolve_network() -> BalancedNetwork {
    helpers::parse_file(fixture("resolve_v33.raw"), Some("psse"))
        .expect("parse resolve_v33.raw")
        .network
}

fn resolve_cases() -> ContingencySet {
    let text = std::fs::read_to_string(fixture("resolve_cases.con")).expect("read fixture");
    ContingencySet::parse(&text).expect("parse fixture").set
}

fn case<'a>(resolution: &'a ContingencyResolution, name: &str) -> &'a ResolvedCase {
    resolution
        .cases
        .iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("case {name} is in the set"))
}

/// The component type, row, and in service flag of everything a case bound to.
fn bound<'a>(resolution: &'a ContingencyResolution, name: &str) -> Vec<(&'a str, usize, bool)> {
    case(resolution, name)
        .components
        .iter()
        .map(|component| {
            (
                component.component_type,
                component.row,
                component.in_service,
            )
        })
        .collect()
}

/// The identity of one bound component. Every row of the fixture network
/// carries a `uid`, so every component states one.
fn identity(component: &ResolvedComponent) -> &ComponentId {
    component
        .id
        .as_ref()
        .unwrap_or_else(|| panic!("row {} states its uid", component.row))
}

#[test]
fn index_states_the_machine_and_circuit_ids_the_raw_writer_allocates() {
    let net = resolve_network();
    let index = PsseEquipmentIndex::new(&net);

    // Bus 1 states '1' then '2', both of which the writer would allocate
    // positionally. Bus 2 states '3' then '1': '3' is kept as `psse_eqid`
    // because the writer would have allocated '1' there, and the second
    // machine's '1' is both what the file states and what the writer
    // allocates next.
    assert_eq!(index.machine_ids(), ["1", "2", "3", "1"]);
    assert_eq!(index.circuit_ids(), ["1", "2", "1", "BL", "1"]);

    assert_eq!(index.bus_row(BusId(1)), Some(0));
    assert_eq!(index.bus_row(BusId(5)), Some(4));
    assert_eq!(index.bus_row(BusId(9)), None);

    // The branch stored 3-1 answers a statement in either orientation.
    assert_eq!(index.branch_rows(BusId(3), BusId(1), "1"), vec![2]);
    assert_eq!(index.branch_rows(BusId(1), BusId(3), "1"), vec![2]);
    assert!(index.branch_rows(BusId(3), BusId(4), "1").is_empty());

    assert_eq!(index.machine_row(BusId(2), "3"), Some(2));
    assert_eq!(index.machine_row(BusId(2), "1"), Some(3));
    assert_eq!(index.machine_row(BusId(2), "2"), None);

    assert_eq!(index.fixed_shunt_rows(BusId(3), None), vec![0, 1]);
    assert_eq!(index.fixed_shunt_rows(BusId(3), Some("2")), vec![1]);
    assert_eq!(index.switched_shunt_rows(BusId(3)), vec![2]);
    assert_eq!(index.load_rows(BusId(4), None), vec![0, 1]);
    assert_eq!(index.load_rows(BusId(4), Some("2")), vec![1]);

    // Any bus order names the same three winding transformer.
    let windings = [BusId(2), BusId(4), BusId(5)];
    assert_eq!(index.transformer_3w_row(windings, "1"), Some(0));
    assert_eq!(
        index.transformer_3w_row([BusId(5), BusId(2), BusId(4)], "1"),
        Some(0)
    );
    assert_eq!(index.transformer_3w_row(windings, "2"), None);
}

#[test]
fn index_ids_agree_with_the_ids_a_fresh_raw_emission_states() {
    let net = resolve_network();
    let index = PsseEquipmentIndex::new(&net);

    let emitted = helpers::emit_psse_rev(&net, 33);
    let again = helpers::parse_psse(&emitted.text).expect("parse the emitted case");
    let rewritten = PsseEquipmentIndex::new(&again);

    assert_eq!(index.machine_ids(), rewritten.machine_ids());
    assert_eq!(index.circuit_ids(), rewritten.circuit_ids());

    // The index says bus 2 carries a machine '3'; the generator section of the
    // emitted case states exactly that record.
    let generators = emitted
        .text
        .split_once("BEGIN GENERATOR DATA")
        .expect("generator section")
        .1
        .split_once("END OF GENERATOR DATA")
        .expect("generator section end")
        .0;
    assert!(
        generators.contains("2, '3',"),
        "the emitted generator section states machine '3' at bus 2:\n{generators}"
    );
}

#[test]
fn every_statement_binds_to_the_element_psse_would_address() {
    let net = resolve_network();
    let set = resolve_cases();
    let resolution = set.resolve(&net);

    assert_eq!(resolution.cases.len(), 20);
    assert_eq!(resolution.resolved, 17);
    assert_eq!(resolution.unresolved, 3);
    assert_eq!(resolution.unrecognized_statements, 1);

    assert_eq!(bound(&resolution, "BR_1_2_C1"), [("branch", 0, true)]);
    assert_eq!(bound(&resolution, "BR_1_2_C2"), [("branch", 1, true)]);
    assert_eq!(bound(&resolution, "BR_REVERSED"), [("branch", 2, true)]);
    assert_eq!(
        bound(&resolution, "BR_NAMED_CIRCUIT"),
        [("branch", 3, true)]
    );
    // An element the case already finds out of service still binds.
    assert_eq!(
        bound(&resolution, "BR_OUT_OF_SERVICE"),
        [("branch", 4, false)]
    );
    assert_eq!(bound(&resolution, "MACHINE_B1_1"), [("generator", 0, true)]);
    assert_eq!(bound(&resolution, "MACHINE_B1_2"), [("generator", 1, true)]);
    assert_eq!(bound(&resolution, "MACHINE_B2_3"), [("generator", 2, true)]);
    assert_eq!(bound(&resolution, "MACHINE_B2_1"), [("generator", 3, true)]);
    // No id names every fixed shunt at the bus; an id names the one.
    assert_eq!(
        bound(&resolution, "SHUNTS_AT_BUS"),
        [("shunt", 0, true), ("shunt", 1, true)]
    );
    assert_eq!(bound(&resolution, "SHUNT_BY_ID"), [("shunt", 1, true)]);
    assert_eq!(bound(&resolution, "SWITCHED_SHUNT"), [("shunt", 2, true)]);
    assert_eq!(
        bound(&resolution, "LOADS_AT_BUS"),
        [("load", 0, true), ("load", 1, true)]
    );
    assert_eq!(
        bound(&resolution, "THREE_WINDING"),
        [("transformer_3w", 0, true)]
    );
    // A bus statement binds to the bus alone; expanding it is the consumer's
    // work.
    assert_eq!(bound(&resolution, "BUS_DISCONNECT"), [("bus", 4, true)]);
    assert_eq!(bound(&resolution, "LOAD_CHANGE"), [("bus", 3, true)]);
    assert!(bound(&resolution, "NO_ACTIONS").is_empty());
    assert!(case(&resolution, "NO_ACTIONS").is_resolved());

    let unresolved: Vec<(&str, UnresolvedReason)> = resolution
        .cases
        .iter()
        .filter(|case| !case.is_resolved())
        .map(|case| (case.name.as_str(), case.unresolved[0].reason))
        .collect();
    assert_eq!(
        unresolved,
        [
            ("BR_MISSING", UnresolvedReason::NoSuchBranch),
            ("MACHINE_MISSING", UnresolvedReason::NoSuchMachine),
            ("UNRECOGNIZED", UnresolvedReason::Unrecognized),
        ]
    );

    let diagnostics = resolution.diagnostics();
    assert_eq!(diagnostics.len(), 3);
    assert!(
        diagnostics
            .iter()
            .all(|note| note.code() == "BUILD.CON.CASE_UNRESOLVED")
    );
    assert!(diagnostics[0].message().contains("BR_MISSING"));
    assert!(diagnostics[0].message().contains("no branch 3 to 4"));
}

#[test]
fn a_terminal_pair_and_circuit_naming_two_branches_binds_to_neither() {
    let mut net = resolve_network();
    // Row 0 is the branch stored 1-2 circuit '1'. A second branch stored the
    // other way round takes circuit '1' too, because the writer allocates ids
    // per stored terminal pair, so a statement naming 1 to 2 circuit 1 now
    // names both.
    let mut reversed = net.branches()[0].clone();
    std::mem::swap(&mut reversed.from, &mut reversed.to);
    reversed.uid = Some("reversed-1-2".to_owned());
    net.branches_mut().push(reversed);

    let index = PsseEquipmentIndex::new(&net);
    assert_eq!(index.branch_rows(BusId(1), BusId(2), "1"), vec![0, 5]);

    let set = resolve_cases();
    let resolution = set.resolve(&net);
    assert_eq!(
        case(&resolution, "BR_1_2_C1").unresolved[0].reason,
        UnresolvedReason::AmbiguousBranch { matches: 2 }
    );
    assert!(case(&resolution, "BR_1_2_C1").components.is_empty());
}

/// The component type strings `powerio_prob`'s update resolver requires are
/// `load`, `generator`, and `branch`; the strings the balanced payload gives
/// the remaining tables are `bus`, `shunt`, and `transformer_3w`.
#[test]
fn component_ids_carry_the_type_strings_the_update_resolver_requires() {
    let net = resolve_network();
    let resolution = resolve_cases().resolve(&net);
    let mut types: Vec<&str> = resolution
        .cases
        .iter()
        .flat_map(|case| case.components.iter())
        .map(|component| component.component_type)
        .collect();
    types.sort_unstable();
    types.dedup();
    assert_eq!(
        types,
        [
            "branch",
            "bus",
            "generator",
            "load",
            "shunt",
            "transformer_3w"
        ]
    );

    // An identity carries the same component type as the field beside it.
    for component in resolution.cases.iter().flat_map(|case| &case.components) {
        assert_eq!(
            identity(component).component_type(),
            component.component_type
        );
    }

    // Every identity is the row's own uid, which an update batch resolves.
    let uids: Vec<&str> = net
        .loads()
        .iter()
        .filter_map(|load| load.uid.as_deref())
        .collect();
    for component in &case(&resolution, "LOADS_AT_BUS").components {
        assert!(
            uids.contains(&identity(component).local_id()),
            "{} is not a load uid",
            identity(component)
        );
    }
}

/// A row the network states no `uid` for has no identity to state. The row
/// still binds, and `row` still indexes the table.
#[test]
fn a_row_with_no_uid_states_no_identity() {
    let mut net = resolve_network();
    for load in net.loads_mut() {
        load.uid = None;
    }
    let resolution = resolve_cases().resolve(&net);
    let loads = &case(&resolution, "LOADS_AT_BUS").components;
    assert_eq!(loads.len(), 2);
    assert!(loads.iter().all(|component| component.id.is_none()));
    // The table the row indexes is named whether or not the row has an
    // identity.
    assert!(
        loads
            .iter()
            .all(|component| component.component_type == "load")
    );
    assert_eq!(
        loads
            .iter()
            .map(|component| component.row)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    // A row that does carry a uid still states it, under the same type.
    let bus = &case(&resolution, "BUS_DISCONNECT").components[0];
    assert!(bus.id.is_some());
    assert_eq!(bus.component_type, "bus");
}

/// The local ACTIVSg2000 corpus, when the manifest names both files.
#[test]
fn activsg2000_contingencies_resolve_against_activsg2000_raw() {
    let (Some(raw), Some(con)) = (
        local_manifest_path("local_psse_contingency_corpus.tsv", "activsg2000_raw"),
        local_manifest_path("local_psse_contingency_corpus.tsv", "activsg2000"),
    ) else {
        eprintln!("skipping: the local contingency corpus manifest names no ACTIVSg2000 pair");
        return;
    };

    let net = helpers::parse_file(&raw, Some("psse"))
        .expect("parse ACTIVSg2000.RAW")
        .network;
    let text = std::fs::read_to_string(&con).expect("read ACTIVSg2000.con");
    let set = ContingencySet::parse(&text)
        .expect("parse ACTIVSg2000.con")
        .set;
    let resolution = set.resolve(&net);

    assert_eq!(resolution.cases.len(), 3875);
    if resolution.unresolved > 0 {
        let failures: Vec<String> = resolution
            .cases
            .iter()
            .filter(|case| !case.is_resolved())
            .take(10)
            .map(|case| {
                format!(
                    "{}: {:?} {:?}",
                    case.name, case.unresolved[0].action, case.unresolved[0].reason
                )
            })
            .collect();
        panic!(
            "{} of {} cases did not resolve; the first ten:\n{}",
            resolution.unresolved,
            resolution.cases.len(),
            failures.join("\n")
        );
    }
    assert_eq!(resolution.unresolved, 0);
    assert_eq!(resolution.resolved, 3875);
    assert_eq!(
        resolution
            .cases
            .iter()
            .filter(|case| case.components.is_empty())
            .count(),
        1,
        "the corpus holds exactly one empty case"
    );
}

#[test]
fn the_index_is_reusable_across_sets() {
    let net = resolve_network();
    let index = PsseEquipmentIndex::new(&net);
    let set = resolve_cases();
    assert_eq!(set.resolve_with(&index), set.resolve(&net));
    assert_eq!(index.network().buses().len(), net.buses().len());
}

#[test]
fn two_transformers_on_one_bus_triple_bind_to_neither() {
    let mut net = resolve_network();
    // The writer allocates a three winding transformer's id per ordered bus
    // triple, so a second transformer on the same three buses in a different
    // winding order takes the same id.
    let mut reordered = net.transformers_3w()[0].clone();
    reordered.uid = Some("reordered-3w".to_owned());
    reordered.windings.swap(0, 2);
    net.transformers_3w_mut().push(reordered);

    let index = PsseEquipmentIndex::new(&net);
    let windings = [BusId(2), BusId(4), BusId(5)];
    assert_eq!(index.transformer_3w_rows(windings, "1"), vec![0, 1]);

    let resolution = resolve_cases().resolve(&net);
    let case = case(&resolution, "THREE_WINDING");
    assert_eq!(
        case.unresolved[0].reason,
        UnresolvedReason::AmbiguousTransformer3w { matches: 2 }
    );
    assert!(case.components.is_empty());
    let note = resolution
        .diagnostics()
        .into_iter()
        .find(|note| note.message().contains("THREE_WINDING"))
        .expect("the ambiguous case is reported");
    assert!(note.message().contains("names 2 transformers"), "{note:?}");
}

#[test]
fn a_line_and_a_two_winding_transformer_on_one_pair_are_keyed_apart() {
    let mut net = resolve_network();
    // The writer allocates the line ids and the transformer ids in separate
    // namespaces, so a transformer between buses 1 and 2 takes circuit '1'
    // just as the line stored 1-2 already does.
    let mut transformer = net.branches()[0].clone();
    transformer.uid = Some("xf-1-2".to_owned());
    transformer.extras.remove("id");
    transformer.tap = 1.05;
    net.branches_mut().push(transformer);

    let index = PsseEquipmentIndex::new(&net);
    assert_eq!(index.circuit_ids(), ["1", "2", "1", "BL", "1", "1"]);
    // The lines answer the statement; the transformer answers only a circuit
    // id no line carries.
    assert_eq!(index.branch_rows(BusId(1), BusId(2), "1"), vec![0]);
    assert_eq!(index.branch_rows(BusId(1), BusId(2), "2"), vec![1]);

    let resolution = resolve_cases().resolve(&net);
    assert_eq!(bound(&resolution, "BR_1_2_C1"), [("branch", 0, true)]);
}

/// Set the retained PSS/E id of one generator, the property the index reads
/// when it recomputes the machine ids.
fn set_machine_eqid(net: &mut BalancedNetwork, uid: &str, eqid: &str) {
    let component = ComponentId::new("generator", uid).expect("a generator identity");
    let detailed = net
        .detailed_connectivity_mut()
        .as_mut()
        .expect("the PSS/E reader states detailed connectivity");
    let detailed = Arc::make_mut(detailed);
    if let Some(metadata) = detailed
        .component_metadata
        .iter_mut()
        .find(|metadata| metadata.component == component)
    {
        metadata
            .properties
            .insert("psse_eqid".to_owned(), eqid.to_owned());
        return;
    }
    let mut added = detailed
        .component_metadata
        .iter()
        .find(|metadata| metadata.component.component_type() == "generator")
        .expect("the fixture states generator metadata")
        .clone();
    added.component = component;
    added.properties.clear();
    added
        .properties
        .insert("psse_eqid".to_owned(), eqid.to_owned());
    detailed.component_metadata.push(added);
}

/// PSS/E forbids an apostrophe inside a quoted field, so the writer replaces
/// one with a space, and reads a quoted id by its trimmed text. An id whose
/// sanitized form trims onto an id already stated at the bus therefore takes a
/// free positional id, and each row answers to the id the RAW file states.
#[test]
fn a_sanitized_id_does_not_take_another_row_s_place() {
    let mut net = resolve_network();

    net.loads_mut()[0]
        .extras
        .insert("id".to_owned(), serde_json::Value::String("a'".to_owned()));
    net.loads_mut()[1]
        .extras
        .insert("id".to_owned(), serde_json::Value::String("a".to_owned()));
    net.shunts_mut()[0]
        .extras
        .insert("id".to_owned(), serde_json::Value::String("a'".to_owned()));
    net.shunts_mut()[1]
        .extras
        .insert("id".to_owned(), serde_json::Value::String("a".to_owned()));

    let third = net.generators()[2].uid.clone().expect("a generator uid");
    let fourth = net.generators()[3].uid.clone().expect("a generator uid");
    set_machine_eqid(&mut net, &third, "a'");
    set_machine_eqid(&mut net, &fourth, "a");

    let index = PsseEquipmentIndex::new(&net);
    // The second row's `a` trims onto the first row's `a '`, so it takes the
    // lowest free positional id.
    assert_eq!(index.machine_ids(), ["1", "2", "a ", "1"]);

    // No two rows share a trimmed id, and either spelling of the first row's
    // id names it.
    assert_eq!(index.machine_row(BusId(2), "a "), Some(2));
    assert_eq!(index.machine_row(BusId(2), "a"), Some(2));
    assert_eq!(index.machine_row(BusId(2), "1"), Some(3));
    assert_eq!(index.load_rows(BusId(4), Some("a")), vec![0]);
    assert_eq!(index.load_rows(BusId(4), Some("1")), vec![1]);
    assert_eq!(index.load_rows(BusId(4), None), vec![0, 1]);
    assert_eq!(index.fixed_shunt_rows(BusId(3), Some("a")), vec![0]);
    assert_eq!(index.fixed_shunt_rows(BusId(3), Some("1")), vec![1]);
}

#[test]
fn the_resolution_notes_stop_at_the_budget() {
    use std::fmt::Write as _;

    let net = resolve_network();
    let mut text = String::new();
    for index in 0..20 {
        let _ = writeln!(
            text,
            "CONTINGENCY 'C{index}'\nOPEN LINE FROM BUS 800 TO BUS 900 CIRCUIT 1\nEND"
        );
    }
    text.push_str("END\n");
    let set = ContingencySet::parse(&text).expect("parse").set;

    let resolution = set.resolve(&net);
    assert_eq!(resolution.unresolved, 20);
    let notes = resolution.diagnostics();
    assert_eq!(notes.len(), 17);
    assert!(
        notes[..16]
            .iter()
            .all(|note| note.code() == "BUILD.CON.CASE_UNRESOLVED")
    );
    assert_eq!(notes[16].code(), "BUILD.CON.NOTES_TRUNCATED");
}

#[test]
fn every_reason_states_its_snake_case_name() {
    let names: Vec<&str> = [
        UnresolvedReason::NoSuchBus,
        UnresolvedReason::NoSuchBranch,
        UnresolvedReason::AmbiguousBranch { matches: 2 },
        UnresolvedReason::AmbiguousTransformer3w { matches: 2 },
        UnresolvedReason::NoSuchMachine,
        UnresolvedReason::NoSuchShunt,
        UnresolvedReason::NoSuchLoad,
        UnresolvedReason::NoSuchTransformer3w,
        UnresolvedReason::Unrecognized,
    ]
    .iter()
    .map(UnresolvedReason::name)
    .collect();
    assert_eq!(
        names,
        [
            "no_such_bus",
            "no_such_branch",
            "ambiguous_branch",
            "ambiguous_transformer_3w",
            "no_such_machine",
            "no_such_shunt",
            "no_such_load",
            "no_such_transformer_3w",
            "unrecognized",
        ]
    );
}

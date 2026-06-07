use super::parse_matpower as parse_mpc;
use super::write_matpower;
use crate::indexed::IndexedNetwork;
use crate::network::SourceFormat;

const CASE_TINY: &str = r"
function mpc = tiny
%TINY  3-bus test
mpc.version = '2';
mpc.baseMVA = 100;

% Bus matrix: standard 13-column layout
mpc.bus = [
    1  3  0     0     0   0   1  1.0  0   345  1  1.1  0.9;
    2  2  10    5     0   0   1  1.0  0   345  1  1.1  0.9;
    3  1  20    8     0   0   1  1.0  0   345  1  1.1  0.9;
];

% Branch matrix: standard 13-column layout
mpc.branch = [
    1  2  0.02  0.06  0.01  0  0  0  0  0  1  -360  360;
    2  3  0.01  0.04  0.015 0  0  0  1  0  1  -360  360;
];
";

#[test]
// base_mva is `100` verbatim in the source, so the exact compare is intended.
#[allow(clippy::float_cmp)]
fn parses_tiny_case() {
    let net = parse_mpc(CASE_TINY).expect("parse tiny");
    assert_eq!(net.base_mva, 100.0);
    assert_eq!(net.buses.len(), 3);
    assert_eq!(net.branches.len(), 2);
    // First bus has zero demand, so it produces no load; buses 2 and 3 do.
    assert_eq!(net.buses[0].id, 1);
    assert_eq!(net.loads.len(), 2);
    assert!(net.loads.iter().all(|l| l.bus != 1));
    // Branch 0: 1->2, r=0.02, x=0.06, in service.
    assert_eq!(net.branches[0].from, 1);
    assert_eq!(net.branches[0].to, 2);
    assert!((net.branches[0].r - 0.02).abs() < 1e-12);
    assert!((net.branches[0].x - 0.06).abs() < 1e-12);
    assert!(net.branches[0].in_service);
}

#[test]
fn maps_non_contiguous_bus_ids() {
    let src = r"
function mpc = sparse_ids
mpc.baseMVA = 100;
mpc.bus = [
    7   3  0  0  0  0  1  1  0  345  1  1.1  0.9;
    42  1  10 5  0  0  1  1  0  345  1  1.1  0.9;
];
mpc.branch = [
    7  42  0.01  0.05  0.02  0  0  0  0  0  1  -360  360;
];
";
    let net = parse_mpc(src).expect("parse sparse-ids");
    assert_eq!(net.buses.len(), 2);
    let g = IndexedNetwork::new(&net);
    assert_eq!(g.bus_index(7), Some(0));
    assert_eq!(g.bus_index(42), Some(1));
    assert_eq!(g.bus_index(99), None);
}

#[test]
fn rejects_short_bus_row() {
    let src = r"
mpc.baseMVA = 100;
mpc.bus = [
    1 3 0;
];
mpc.branch = [
    1 1 0.01 0.05 0.02 0 0 0 0 0 1 -360 360;
];
";
    let err = parse_mpc(src).expect_err("should fail on short bus row");
    let msg = err.to_string();
    assert!(msg.contains("bus"), "expected bus error, got: {msg}");
}

#[test]
fn handles_inline_percent_in_string() {
    let src = r"
mpc.baseMVA = 100;
mpc.version = '2 (50% capacity)';
mpc.bus = [
    1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;
];
mpc.branch = [
    1 1 0.01 0.05 0.02 0 0 0 0 0 1 -360 360;
];
";
    parse_mpc(src).expect("string with embedded % shouldn't break parse");
}

#[test]
fn parses_storage_block() {
    // Storage row values taken from pglib_opf_case5_pjm_storage (the
    // PowerModels / pglib 17-column layout), plus a second out-of-service row.
    let src = r"
mpc.baseMVA = 100;
mpc.bus = [
    1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;
    4 1 0 0 0 0 1 1 0 345 1 1.1 0.9;
];
mpc.branch = [
    1 4 0.01 0.05 0.02 0 0 0 0 0 1 -360 360;
];
mpc.storage = [
    4  0.0  0.0  1.00  600.0  300.0  216.0  0.9  0.85  1000  -1000  1000  0.1  0.01  0  0  1;
    1  0.0  0.0  0.50  200.0  100.0  100.0  0.95 0.9   500   -500   500   0.2  0.02  0  0  0;
];
";
    let net = parse_mpc(src).expect("parse storage");
    assert_eq!(net.storage.len(), 2);
    let s = &net.storage[0];
    assert_eq!(s.bus, 4);
    assert!((s.energy - 1.0).abs() < 1e-12);
    assert!((s.energy_rating - 600.0).abs() < 1e-12);
    assert!((s.charge_efficiency - 0.9).abs() < 1e-12);
    assert!((s.discharge_efficiency - 0.85).abs() < 1e-12);
    assert!((s.qmin - (-1000.0)).abs() < 1e-12);
    assert!((s.x - 0.01).abs() < 1e-12);
    assert!(s.in_service);
    assert!(!net.storage[1].in_service);
}

#[test]
fn absent_storage_is_empty() {
    let net = parse_mpc(CASE_TINY).expect("parse tiny");
    assert!(net.storage.is_empty());
}

#[test]
fn rejects_short_storage_row() {
    let src = r"
mpc.baseMVA = 100;
mpc.bus = [
    1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;
];
mpc.branch = [
    1 1 0.01 0.05 0.02 0 0 0 0 0 1 -360 360;
];
mpc.storage = [
    1 0.0 0.0 1.0;
];
";
    let err = parse_mpc(src).expect_err("short storage row should fail");
    assert!(err.to_string().contains("storage"), "got: {err}");
}

#[test]
fn rejects_unterminated_matrix() {
    // A `mpc.bus = [ … ` truncated at EOF with no closing `];`. The streaming
    // row parser must reject this (old `find_matrix` returned UnbalancedBrackets)
    // rather than silently accept the partial matrix.
    let src = "mpc.baseMVA = 100;\n\
               mpc.bus = [\n\
               \t1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;\n\
               \t2 1 0 0 0 0 1 1 0 345 1 1.1 0.9;\n";
    let err = parse_mpc(src).expect_err("unterminated bus matrix should fail");
    let msg = err.to_string();
    assert!(msg.contains("unbalanced"), "expected unbalanced error, got: {msg}");
    assert!(msg.contains("bus"), "expected bus field named, got: {msg}");
}

#[test]
fn accepts_last_row_without_trailing_semicolon() {
    // A properly closed matrix whose final row has no trailing `;` before `];`
    // must still parse — the unbalanced check keys on the missing `]`, not `;`.
    let src = "mpc.baseMVA = 100;\n\
               mpc.bus = [\n\
               \t1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;\n\
               \t2 1 0 0 0 0 1 1 0 345 1 1.1 0.9\n\
               ];\n\
               mpc.branch = [\n\
               \t1 2 0.01 0.05 0.02 0 0 0 0 0 1 -360 360;\n\
               ];\n";
    let net = parse_mpc(src).expect("closed matrix with unterminated last row");
    assert_eq!(net.buses.len(), 2);
}

#[test]
// base_mva is `100` verbatim in the source, so the exact compare is intended.
#[allow(clippy::float_cmp)]
fn parsed_case_keeps_source_in_memory_case_does_not() {
    // A network parsed from text retains its source so the writer echoes it
    // verbatim; one built in memory has no source and writes canonically.
    let parsed = parse_mpc(CASE_TINY).expect("parse tiny");
    assert_eq!(parsed.source.as_deref(), Some(CASE_TINY), "parsed network should echo its source");
    assert_eq!(write_matpower(&parsed), CASE_TINY);

    let mut built = parsed.clone();
    built.source = None;
    built.source_format = SourceFormat::InMemory;
    // Canonical output is parseable and keeps the headline values.
    let reparsed = parse_mpc(&write_matpower(&built)).expect("canonical reparses");
    assert_eq!(reparsed.base_mva, 100.0);
    assert_eq!(reparsed.buses.len(), built.buses.len());
}

#[test]
fn handles_nan_inf() {
    let src = r"
mpc.baseMVA = 100;
mpc.bus = [
    1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;
];
mpc.branch = [
    1 1 0.01 0.05 0.02 0 0 0 0 0 1 NaN Inf;
];
";
    let net = parse_mpc(src).expect("NaN/Inf should parse");
    assert!(net.branches[0].angmin.is_nan());
    assert!(net.branches[0].angmax.is_infinite());
}

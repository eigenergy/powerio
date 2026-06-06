use super::parse_matpower as parse_mpc;

const CASE_TINY: &str = r#"
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
"#;

#[test]
fn parses_tiny_case() {
    let mpc = parse_mpc(CASE_TINY).expect("parse tiny");
    assert_eq!(mpc.base_mva, 100.0);
    assert_eq!(mpc.buses.len(), 3);
    assert_eq!(mpc.branches.len(), 2);
    // First bus
    assert_eq!(mpc.buses[0].id, 1);
    assert_eq!(mpc.buses[0].pd, 0.0);
    // Branch 0: 1->2, r=0.02, x=0.06
    assert_eq!(mpc.branches[0].from_id, 1);
    assert_eq!(mpc.branches[0].to_id, 2);
    assert!((mpc.branches[0].r - 0.02).abs() < 1e-12);
    assert!((mpc.branches[0].x - 0.06).abs() < 1e-12);
    assert_eq!(mpc.branches[0].status, 1.0);
}

#[test]
fn maps_non_contiguous_bus_ids() {
    let src = r#"
function mpc = sparse_ids
mpc.baseMVA = 100;
mpc.bus = [
    7   3  0  0  0  0  1  1  0  345  1  1.1  0.9;
    42  1  10 5  0  0  1  1  0  345  1  1.1  0.9;
];
mpc.branch = [
    7  42  0.01  0.05  0.02  0  0  0  0  0  1  -360  360;
];
"#;
    let mpc = parse_mpc(src).expect("parse sparse-ids");
    assert_eq!(mpc.n(), 2);
    assert_eq!(mpc.bus_index(7), Some(0));
    assert_eq!(mpc.bus_index(42), Some(1));
    assert_eq!(mpc.bus_index(99), None);
}

#[test]
fn rejects_short_bus_row() {
    let src = r#"
mpc.baseMVA = 100;
mpc.bus = [
    1 3 0;
];
mpc.branch = [
    1 1 0.01 0.05 0.02 0 0 0 0 0 1 -360 360;
];
"#;
    let err = parse_mpc(src).expect_err("should fail on short bus row");
    let msg = err.to_string();
    assert!(msg.contains("bus"), "expected bus error, got: {msg}");
}

#[test]
fn handles_inline_percent_in_string() {
    let src = r#"
mpc.baseMVA = 100;
mpc.version = '2 (50% capacity)';
mpc.bus = [
    1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;
];
mpc.branch = [
    1 1 0.01 0.05 0.02 0 0 0 0 0 1 -360 360;
];
"#;
    parse_mpc(src).expect("string with embedded % shouldn't break parse");
}

#[test]
fn parses_storage_block() {
    // Storage row values taken from pglib_opf_case5_pjm_storage (the
    // PowerModels / pglib 17-column layout), plus a second out-of-service row.
    let src = r#"
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
"#;
    let mpc = parse_mpc(src).expect("parse storage");
    assert_eq!(mpc.storage.len(), 2);
    let s = &mpc.storage[0];
    assert_eq!(s.bus_id, 4);
    assert!((s.energy - 1.0).abs() < 1e-12);
    assert!((s.energy_rating - 600.0).abs() < 1e-12);
    assert!((s.charge_efficiency - 0.9).abs() < 1e-12);
    assert!((s.discharge_efficiency - 0.85).abs() < 1e-12);
    assert!((s.qmin - (-1000.0)).abs() < 1e-12);
    assert!((s.x - 0.01).abs() < 1e-12);
    assert!(s.is_in_service());
    assert!(!mpc.storage[1].is_in_service());
}

#[test]
fn absent_storage_is_empty() {
    let mpc = parse_mpc(CASE_TINY).expect("parse tiny");
    assert!(mpc.storage.is_empty());
}

#[test]
fn rejects_short_storage_row() {
    let src = r#"
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
"#;
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
    let mpc = parse_mpc(src).expect("closed matrix with unterminated last row");
    assert_eq!(mpc.buses.len(), 2);
}

#[test]
fn handles_nan_inf() {
    let src = r#"
mpc.baseMVA = 100;
mpc.bus = [
    1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;
];
mpc.branch = [
    1 1 0.01 0.05 0.02 0 0 0 0 0 1 NaN Inf;
];
"#;
    let mpc = parse_mpc(src).expect("NaN/Inf should parse");
    assert!(mpc.branches[0].angmin.is_nan());
    assert!(mpc.branches[0].angmax.is_infinite());
}

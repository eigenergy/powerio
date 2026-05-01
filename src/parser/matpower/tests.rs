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

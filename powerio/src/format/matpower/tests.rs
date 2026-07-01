use super::parse_matpower as parse_mpc;
use super::write_matpower;
use crate::indexed::IndexedNetwork;
use crate::network::{Branch, Bus, BusId, BusType, GenCost, Generator, Network, SourceFormat};

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
    assert_eq!(net.name, "tiny");
    assert_eq!(net.base_mva, 100.0);
    assert_eq!(net.buses.len(), 3);
    assert_eq!(net.branches.len(), 2);
    // First bus has zero demand, so it produces no load; buses 2 and 3 do.
    assert_eq!(net.buses[0].id, BusId(1));
    assert_eq!(net.loads.len(), 2);
    assert!(net.loads.iter().all(|l| l.bus != BusId(1)));
    // Branch 0: 1->2, r=0.02, x=0.06, in service.
    assert_eq!(net.branches[0].from, BusId(1));
    assert_eq!(net.branches[0].to, BusId(2));
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
    assert_eq!(g.bus_index(BusId(7)), Some(0));
    assert_eq!(g.bus_index(BusId(42)), Some(1));
    assert_eq!(g.bus_index(BusId(99)), None);
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
    assert_eq!(s.bus, BusId(4));
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
fn rejects_oversized_gencost_ncost_without_panicking() {
    // NCOST is read from the file as an f64 truncated to usize, so a huge value
    // saturates near usize::MAX. The row-width requirement (`start + 2*ncost` for
    // model 1, `start + ncost` for model 2) must be computed without overflowing:
    // a malformed NCOST has to surface as a loud `ShortRow`, not an add-overflow
    // panic (debug) or a reversed-slice panic (release). Both cost models exercise
    // a distinct saturating op (`saturating_mul` vs `saturating_add`).
    let case = |gencost: &str| {
        format!(
            "mpc.baseMVA = 100;\n\
             mpc.bus = [\n1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;\n];\n\
             mpc.gen = [\n1 0 0 100 -100 1 100 1 100 0 0 0 0 0 0 0 0 0 0 0 0;\n];\n\
             mpc.branch = [\n1 1 0.01 0.05 0.02 0 0 0 0 0 1 -360 360;\n];\n\
             mpc.gencost = [\n{gencost}\n];\n"
        )
    };
    // model 2 (polynomial): want = ncost
    let err = parse_mpc(&case("2 0 0 1e20 500 300 200;")).expect_err("huge model-2 ncost");
    assert!(err.to_string().contains("gencost"), "got: {err}");
    // model 1 (piecewise): want = 2 * ncost, the saturating_mul path
    let err = parse_mpc(&case("1 0 0 1e19 0 0 1 1;")).expect_err("huge model-1 ncost");
    assert!(err.to_string().contains("gencost"), "got: {err}");
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
    assert!(
        msg.contains("unbalanced"),
        "expected unbalanced error, got: {msg}"
    );
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
    assert_eq!(
        parsed.source.as_ref().map(|s| s.as_str()),
        Some(CASE_TINY),
        "parsed network should echo its source"
    );
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

#[test]
fn mixed_model_gencost_drops_padding_and_writes_rectangular() {
    // A gencost matrix that mixes piecewise (model 1) and polynomial (model 2)
    // rows is padded with trailing zeros to stay rectangular. The parser must take
    // each row's own values (2·ncost / ncost), not the padding, and the canonical
    // writer must pad back so the emitted matrix is rectangular (PowerModels and
    // MATPOWER both reject a ragged one). This mirrors t_case9_dcline's gencost.
    let src = r"
mpc.baseMVA = 100;
mpc.bus = [
    1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;
    2 2 0 0 0 0 1 1 0 345 1 1.1 0.9;
];
mpc.branch = [
    1 2 0.01 0.05 0.02 0 0 0 0 0 1 -360 360;
];
mpc.gen = [
    1 0 0 300 -300 1 100 1 250 10 0 0 0 0 0 0 0 0 0 0 0;
    2 0 0 300 -300 1 100 1 250 10 0 0 0 0 0 0 0 0 0 0 0;
];
mpc.gencost = [
    1 0 0 3 0 0 100 2500 200 5500;
    2 0 0 2 24.035 -403.5 0 0 0 0;
];
";
    let net = parse_mpc(src).expect("parse mixed gencost");
    let c0 = net.generators[0].cost.as_ref().unwrap();
    let c1 = net.generators[1].cost.as_ref().unwrap();
    // Piecewise: 2·ncost = 6 breakpoint values; polynomial: ncost = 2 coefficients
    // (the four padding zeros are dropped, not read as degree-2..5 terms).
    assert_eq!((c0.model, c0.coeffs.len()), (1, 6));
    assert_eq!((c1.model, c1.coeffs.len()), (2, 2));
    assert!((c1.coeffs[0] - 24.035).abs() < 1e-9 && (c1.coeffs[1] + 403.5).abs() < 1e-9);

    // Canonical write pads both gencost rows to the same width.
    let mut built = net.clone();
    built.source = None;
    built.source_format = SourceFormat::InMemory;
    let text = write_matpower(&built);
    let rows: Vec<usize> = text
        .lines()
        .skip_while(|l| !l.contains("mpc.gencost"))
        .skip(1)
        .take_while(|l| !l.contains("];"))
        .map(|l| l.matches('\t').count())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], rows[1], "gencost rows must be equal width");
    // And it re-parses to the same trimmed costs.
    let reparsed = parse_mpc(&text).expect("canonical mixed gencost reparses");
    assert_eq!(
        reparsed.generators[0].cost.as_ref().unwrap().coeffs.len(),
        6
    );
    assert_eq!(
        reparsed.generators[1].cost.as_ref().unwrap().coeffs.len(),
        2
    );
}

#[test]
fn piecewise_gencost_constructor_counts_breakpoints() {
    let mut generator = Generator::new(BusId(1));
    generator.cost = Some(GenCost::new(1, 0.0, 0.0, vec![0.0, 0.0, 1.0, 1.0]));
    let mut net = Network::in_memory(
        "pwl_cost_constructor",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
        ],
        vec![Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    net.generators = vec![generator];

    let cost = net.generators[0].cost.as_ref().unwrap();
    assert_eq!(cost.ncost, 2);
    assert_eq!(cost.coeffs, vec![0.0, 0.0, 1.0, 1.0]);

    let restored = Network::from_json(&net.to_json().unwrap()).unwrap();
    let restored_cost = restored.generators[0].cost.as_ref().unwrap();
    assert_eq!(restored_cost.ncost, 2);
    assert_eq!(restored_cost.coeffs, vec![0.0, 0.0, 1.0, 1.0]);

    let text = write_matpower(&net);
    let reparsed = parse_mpc(&text).expect("constructor PWL cost should reparse");
    let reparsed_cost = reparsed.generators[0].cost.as_ref().unwrap();
    assert_eq!(reparsed_cost.model, 1);
    assert_eq!(reparsed_cost.ncost, 2);
    assert_eq!(reparsed_cost.coeffs, vec![0.0, 0.0, 1.0, 1.0]);
}

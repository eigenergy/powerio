//! `BalancedNetwork::to_normalized`: per-unit / radians / tap / filter / source ids / bus
//! types, plus the no-false-write-back invariant and `parse_str == parse`.
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use std::path::{Path, PathBuf};

use powerio_tx::{
    BusId, BusType, Error, Generator, IndexedNetwork, NormalizeOptions, Shunt, SourceFormat,
    Storage, TargetFormat,
};

const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

fn data(case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(case)
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * (1.0 + a.abs().max(b.abs()))
}

#[test]
fn per_unit_and_radians_on_case9() {
    let raw = parse_matpower_file(data("case9.m")).unwrap();
    let base = raw.base_mva();
    let n = raw.to_normalized().unwrap();

    // case9 is all in service with a single reference bus, so nothing is dropped
    // and element order is preserved — a 1:1 comparison against the raw model.
    assert!(approx(n.base_mva(), base));
    assert_eq!(n.buses().len(), raw.buses().len());
    assert_eq!(n.generators().len(), raw.generators().len());
    assert_eq!(n.branches().len(), raw.branches().len());
    assert_eq!(n.loads().len(), raw.loads().len());

    for (g, rg) in n.generators().iter().zip(raw.generators()) {
        assert!(approx(g.pg, rg.pg / base));
        assert!(approx(g.pmax, rg.pmax / base));
        assert!(approx(g.pmin, rg.pmin / base));
        assert!(approx(g.qmax, rg.qmax / base));
    }
    for (l, rl) in n.loads().iter().zip(raw.loads()) {
        assert!(approx(l.p, rl.p / base));
        assert!(approx(l.q, rl.q / base));
    }
    for (b, rb) in n.branches().iter().zip(raw.branches()) {
        assert!(approx(b.angmin, rb.angmin * DEG_TO_RAD));
        assert!(approx(b.angmax, rb.angmax * DEG_TO_RAD));
        // A 0 tap normalizes to exactly 1; an explicit tap rides through.
        let want_tap = if rb.tap == 0.0 { 1.0 } else { rb.tap };
        assert!(approx(b.tap, want_tap), "tap 0→1 / explicit tap preserved");
    }
    // Polynomial gen cost: the p^2 coeff scales by base^2, p^1 by base, the
    // constant is unchanged.
    for (g, rg) in n.generators().iter().zip(raw.generators()) {
        if let (Some(c), Some(rc)) = (&g.cost, &rg.cost) {
            if c.model == 2 && c.coeffs.len() >= 3 && rc.coeffs.len() >= 3 {
                assert!(approx(c.coeffs[0], rc.coeffs[0] * base * base));
                assert!(approx(c.coeffs[1], rc.coeffs[1] * base));
                assert!(approx(c.coeffs[2], rc.coeffs[2]));
            }
        }
    }
}

#[test]
fn per_unit_shunts_on_case30() {
    let raw = parse_matpower_file(data("case30.m")).unwrap();
    let base = raw.base_mva();
    let n = raw.to_normalized().unwrap();
    assert!(!n.shunts().is_empty(), "case30 has shunts");
    for (s, rs) in n.shunts().iter().zip(raw.shunts()) {
        assert!(approx(s.g, rs.g / base));
        assert!(approx(s.b, rs.b / base));
    }
}

#[test]
fn per_unit_hvdc_keeps_matpower_sign() {
    // t_case9_dcline carries HVDC lines. to_normalized per-unitizes pf/pt/qf/qt
    // but must NOT flip their sign (that flip is a PowerModels-output convention,
    // not part of normalization) and leaves the aggregate pmin/pmax raw.
    let raw = parse_matpower_file(data("t_case9_dcline.m")).unwrap();
    let base = raw.base_mva();
    let n = raw.to_normalized().unwrap();

    let raw_in: Vec<_> = raw.hvdc().iter().filter(|d| d.in_service).collect();
    assert!(!raw_in.is_empty(), "fixture has in-service dclines");
    assert_eq!(n.hvdc().len(), raw_in.len());
    for (d, rd) in n.hvdc().iter().zip(raw_in) {
        assert!(approx(d.pf, rd.pf / base));
        assert!(approx(d.pt, rd.pt / base));
        assert!(approx(d.qf, rd.qf / base));
        assert!(approx(d.qt, rd.qt / base));
        // Same sign (product positive) ⇒ no flip; a negation would make it < 0.
        assert!(d.pt * rd.pt > 0.0, "pt sign preserved, no flip");
        // Aggregate bounds stay raw, matching the PowerModels per-unit convention.
        assert!(approx(d.pmin, rd.pmin));
        assert!(approx(d.pmax, rd.pmax));
    }
}

#[test]
fn no_false_write_back() {
    let src = std::fs::read_to_string(data("case9.m")).unwrap();
    let raw = parse_matpower_file(data("case9.m")).unwrap();
    let n = raw.to_normalized().unwrap();

    assert_eq!(n.source_format(), SourceFormat::Normalized);

    // Writing it serializes the per-unit/radian model, so it must NOT echo the
    // raw MATPOWER bytes.
    let out = write_network(&n, TargetFormat::Matpower).unwrap();
    assert_ne!(
        out.text.trim_end(),
        src.replace("\r\n", "\n").trim_end(),
        "normalized network must not echo the raw source"
    );
}

#[test]
fn warns_when_writing_normalized_lines_as_transformers() {
    // case9 is all lines (raw tap 0); to_normalized canonicalizes tap to 1.0,
    // which collides with the `tap != 0` transformer sentinel the PSS/E writer
    // uses to split lines from transformers. The conversion must report the lost
    // line/transformer distinction instead of relabeling silently.
    let raw = parse_matpower_file(data("case9.m")).unwrap();
    let n = raw.to_normalized().unwrap();
    assert!(
        n.branches()
            .iter()
            .all(|b| approx(b.tap, 1.0) && approx(b.shift, 0.0))
    );

    let out = write_network(&n, TargetFormat::Psse { rev: 33 }).unwrap();
    assert!(
        out.rendered_diagnostics()
            .iter()
            .any(|w| w.contains("line/transformer label is not preserved")),
        "expected a normalized line/transformer fidelity warning, got {:?}",
        out.rendered_diagnostics()
    );

    // A raw network keeps lines at tap 0, so the warning must not fire for it.
    let raw_out = write_network(&raw, TargetFormat::Psse { rev: 33 }).unwrap();
    assert!(
        !raw_out
            .rendered_diagnostics()
            .iter()
            .any(|w| w.contains("line/transformer label is not preserved")),
        "raw network must not trigger the normalized-tap warning"
    );
}

#[test]
fn filters_and_retypes_out_of_service_case() {
    let raw = parse_matpower_file(data("t_case9_oos.m")).unwrap();
    let n = raw.to_normalized().unwrap();

    // The fixture marks the bus-2 generator and branch 5-6 out of service; no
    // isolated buses, so all 9 buses survive with their source ids.
    assert_eq!(n.generators().len(), raw.generators().len() - 1);
    assert_eq!(n.branches().len(), raw.branches().len() - 1);
    assert_eq!(n.buses().len(), 9);
    for (i, b) in n.buses().iter().enumerate() {
        assert_eq!(b.id.0, i + 1);
    }
    assert_eq!(
        n.buses().iter().filter(|b| b.kind == BusType::Ref).count(),
        1,
        "exactly one reference bus"
    );
    // Bus 1 is the file reference; bus 2 lost its only (out-of-service) generator
    // so it falls to PQ; bus 3 keeps its generator and is PV.
    assert_eq!(n.buses()[0].kind, BusType::Ref);
    assert_eq!(n.buses()[1].kind, BusType::Pq);
    assert_eq!(n.buses()[2].kind, BusType::Pv);
}

#[test]
fn drops_isolated_bus_and_remaps_endpoints() {
    // Bus 2 is isolated (type 4); branch 2-3 references it (dropped), branch 1-3
    // survives, and the load on bus 3 keeps that source id.
    let src = "\
function mpc = iso
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t4\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t3\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t3\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t2\t3\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let raw = parse_str(src, "matpower").unwrap().network;
    let n = raw.to_normalized().unwrap();

    assert_eq!(n.buses().len(), 2, "isolated bus dropped");
    assert_eq!(n.buses().iter().map(|b| b.id.0).collect::<Vec<_>>(), [1, 3]);
    assert_eq!(n.branches().len(), 1, "branch on the isolated bus dropped");
    assert_eq!((n.branches()[0].from.0, n.branches()[0].to.0), (1, 3));
    assert_eq!(n.loads().len(), 1);
    assert_eq!(n.loads()[0].bus.0, 3, "load keeps the source id");
    assert_eq!(n.buses()[0].kind, BusType::Ref);
    assert_eq!(n.buses()[1].kind, BusType::Pq);
}

#[test]
fn source_rows_map_normalized_positions_to_raw_rows() {
    // Bus 2 is isolated; branch row 1 (2-3) dies with it; branch row 2 is out
    // of service; gen row 1 is out of service; the load on bus 2 dies with the
    // bus. Each surviving family position must name its raw row.
    let src = "\
function mpc = rows
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t4\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t3\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t4\t1\t20\t5\t3\t7\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
\t3\t0\t0\t100\t-100\t1\t100\t0\t200\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
\t4\t0\t0\t100\t-100\t1\t100\t1\t200\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t3\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t2\t3\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t3\t4\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t0\t-360\t360;
\t1\t4\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let raw = parse_str(src, "matpower").unwrap().network;
    let (n, rows) = raw
        .to_normalized_with_source_rows(&NormalizeOptions::default())
        .unwrap();

    assert_eq!(rows.buses, [Some(0), Some(2), Some(3)]);
    assert_eq!(rows.branches, [Some(0), Some(3)]);
    assert_eq!(rows.generators, [Some(0), Some(2)]);
    // The MATPOWER bus PD/QD columns become one load per demand-carrying bus,
    // in bus order. The load on dropped bus 2 is not a raw load here, so both
    // raw loads (buses 3 and 4) survive.
    assert_eq!(rows.loads, [Some(0), Some(1)]);
    assert_eq!(rows.shunts, [Some(0)]);

    for (pos, row) in rows.buses.iter().enumerate() {
        let row = row.expect("no star lowering in this case");
        assert_eq!(n.network.buses()[pos].id, raw.buses()[row].id);
    }
    for (pos, row) in rows.branches.iter().enumerate() {
        let row = row.expect("no star lowering in this case");
        assert_eq!(n.network.branches()[pos].from, raw.branches()[row].from);
        assert_eq!(n.network.branches()[pos].to, raw.branches()[row].to);
    }
    for (pos, row) in rows.generators.iter().enumerate() {
        let row = row.expect("no star lowering in this case");
        assert_eq!(n.network.generators()[pos].bus, raw.generators()[row].bus);
    }
    let plain = raw
        .to_normalized_with_options(&NormalizeOptions::default())
        .unwrap();
    assert_eq!(plain.network.buses().len(), n.network.buses().len());
    assert_eq!(plain.network.branches().len(), n.network.branches().len());
}

#[test]
fn preserves_sparse_source_bus_ids() {
    let src = "\
function mpc = sparseids
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t3\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t4\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t10\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t2\t3\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t3\t4\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t4\t10\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let n = parse_str(src, "matpower")
        .unwrap()
        .network
        .to_normalized()
        .unwrap();

    assert_eq!(
        n.buses().iter().map(|b| b.id.0).collect::<Vec<_>>(),
        [1, 2, 3, 4, 10]
    );
    assert_eq!(n.loads().len(), 1);
    assert_eq!(n.loads()[0].bus.0, 10);
    assert_eq!(n.branches().len(), 4);
    assert_eq!((n.branches()[3].from.0, n.branches()[3].to.0), (4, 10));

    let view = IndexedNetwork::new(&n);
    assert_eq!(view.bus_index(BusId(10)), Some(4));
    assert_eq!(view.bus_id(4), BusId(10));
}

#[test]
fn rejects_non_positive_base_mva() {
    // A zero (or negative / non-finite) base would silently divide every power
    // into NaN/Inf; to_normalized must reject it instead of returning garbage.
    let src = "\
function mpc = zerobase
mpc.version = '2';
mpc.baseMVA = 0;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
];
";
    let raw = parse_str(src, "matpower").unwrap().network;
    assert!(matches!(
        raw.to_normalized(),
        Err(Error::InvalidBaseMva { .. })
    ));
}

#[test]
fn errors_when_no_reference_can_be_chosen() {
    // No file REF (bus is PQ) and no generators to fall back to.
    let src = "\
function mpc = noref
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t1\t10\t5\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t10\t5\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let raw = parse_str(src, "matpower").unwrap().network;
    let err = raw.to_normalized().unwrap_err();
    assert!(matches!(err, Error::NoReferenceBus));
    // The refusal states the generating rule (a reference bus must host an
    // in-service generator) instead of a bus count the file can contradict.
    assert!(err.to_string().contains("in-service generator"), "{err}");
}

#[test]
fn warns_when_normalize_designates_a_slack_the_case_did_not_state() {
    // case14 with every type 3 bus demoted to type 2: the parsed network
    // states no reference, so normalize promotes the largest pmax generator's
    // bus (bus 1) and must say so through the coded channel.
    let source = std::fs::read_to_string(data("case14.m")).unwrap();
    let mut in_bus = false;
    let demoted: String = source
        .lines()
        .map(|line| {
            if line.starts_with("mpc.bus") {
                in_bus = true;
            } else if in_bus && line.contains("];") {
                in_bus = false;
            }
            let mut line = line.to_string();
            if in_bus {
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() > 2 && fields[1] == "3" {
                    line = line.replacen("\t3\t", "\t2\t", 1);
                }
            }
            line + "\n"
        })
        .collect();
    let raw = parse_str(&demoted, "matpower").unwrap().network;
    assert!(!raw.buses().iter().any(|b| b.kind == BusType::Ref));

    let out = raw
        .to_normalized_with_options(&NormalizeOptions::default())
        .unwrap();
    let designated: Vec<_> = out
        .diagnostics
        .iter()
        .filter(|d| d.code() == "CANONICALIZE.NORMALIZE.REFERENCE_DESIGNATED")
        .collect();
    assert_eq!(designated.len(), 1, "{:?}", out.warnings);
    assert!(designated[0].message().contains("bus 1"), "{designated:?}");
    assert!(
        designated[0].message().contains("designated the slack"),
        "{designated:?}"
    );
    assert_eq!(
        out.network
            .buses()
            .iter()
            .filter(|b| b.kind == BusType::Ref)
            .count(),
        1
    );
}

#[test]
fn keeps_multiple_file_refs() {
    // Two gen-backed file REF buses are kept; the consumer picks the slack.
    // The gen-less bus is PQ.
    let src = "\
function mpc = tworef
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t3\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t100\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
\t2\t0\t0\t100\t-100\t1\t100\t1\t300\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t2\t3\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let n = parse_str(src, "matpower")
        .unwrap()
        .network
        .to_normalized()
        .unwrap();
    // Both gen-backed REF buses stay REF; the gen-less load bus is PQ.
    assert_eq!(
        n.buses().iter().filter(|b| b.kind == BusType::Ref).count(),
        2
    );
    assert_eq!(n.buses()[0].kind, BusType::Ref);
    assert_eq!(n.buses()[1].kind, BusType::Ref);
    assert_eq!(n.buses()[2].kind, BusType::Pq);

    // The reference set query returns both (dense, ascending); the single-ref
    // query refuses to pick one and reports the count.
    let view = IndexedNetwork::new(&n);
    assert_eq!(view.reference_bus_indices(), vec![0, 1]);
    assert!(matches!(
        view.reference_bus_index(),
        Err(Error::ReferenceBusCount { found: 2, .. })
    ));
}

#[test]
fn promotes_largest_gen_when_no_file_ref() {
    // No file REF (the gen bus is PV): the largest-pmax in-service gen's bus is
    // promoted to slack.
    let src = "\
function mpc = norefgen
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t2\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let n = parse_str(src, "matpower")
        .unwrap()
        .network
        .to_normalized()
        .unwrap();
    assert_eq!(
        n.buses().iter().filter(|b| b.kind == BusType::Ref).count(),
        1
    );
    assert_eq!(n.buses()[0].kind, BusType::Ref, "gen bus promoted to slack");
    assert_eq!(n.buses()[1].kind, BusType::Pq);
}

#[test]
fn piecewise_cost_per_unit_through_to_normalized() {
    // Model-1 (piecewise) gen cost end to end: the MW breakpoints (even positions)
    // divide by base, the dollar costs (odd positions) stay — verified through
    // to_normalized, not just the standalone helper.
    let src = "\
function mpc = pw
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.gencost = [
\t1\t0\t0\t2\t0\t0\t100\t2000;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let n = parse_str(src, "matpower")
        .unwrap()
        .network
        .to_normalized()
        .unwrap();
    let c = n.generators()[0].cost.as_ref().unwrap();
    assert_eq!(c.model, 1);
    // [0, 0, 100, 2000] -> [0/100, 0, 100/100, 2000]
    assert!(approx(c.coeffs[0], 0.0));
    assert!(approx(c.coeffs[1], 0.0));
    assert!(approx(c.coeffs[2], 1.0));
    assert!(approx(c.coeffs[3], 2000.0));
}

#[test]
fn parse_str_matches_parse_file() {
    for case in ["case9.m", "case14.m", "case30.m"] {
        let text = std::fs::read_to_string(data(case)).unwrap();
        let from_path = parse_file(data(case), None).unwrap().network;
        let mut from_text = parse_str(&text, "matpower").unwrap().network;
        // The only legitimate difference is the network name, which `parse_file`
        // derives from the file stem and `parse_str` cannot (it has no path).
        *from_text.name_mut() = from_path.name().clone(); // to_json skips the retained source, so equal JSON means field-for-field
        // equal tables.
        assert_eq!(
            from_path.to_json().unwrap(),
            from_text.to_json().unwrap(),
            "{case}: parse_str disagrees with parse_file"
        );
    }
}

#[test]
fn parse_str_rejects_contentless_input() {
    // Empty or table-free text must not parse to a hollow network. Empty input
    // and `{}` fail on the missing required tables; a JSON carrying only
    // `baseMVA` would slip past those, so the zero-bus guard in `read_source`
    // catches it. Every format funnels through that guard.
    for fmt in ["matpower", "powermodels", "egret"] {
        assert!(parse_str("", fmt).is_err(), "{fmt}: empty input parsed");
    }
    assert!(parse_str("{}", "powermodels").is_err());
    assert!(parse_str("{}", "egret").is_err());
    // baseMVA present but no `bus` table: the zero-bus guard, not a missing
    // required field, is what rejects this.
    let err = parse_str(r#"{"baseMVA": 100}"#, "powermodels").unwrap_err();
    assert!(err.to_string().contains("no buses"), "{err}");
}

#[test]
fn nonzero_field_transforms() {
    // The vendored cases leave bus angle, branch shift, explicit tap, and ramp
    // caps at zero, so those transforms are never witnessed. This inline fixture
    // carries nonzero values in each, so a wrong factor or a dropped line fails.
    let src = "\
function mpc = nz
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t30\t230\t1\t1.1\t0.9;
\t2\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0\t0\t0\t5\t15\t0\t0\t0\t0\t60\t0\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0.978\t15\t1\t-30\t30;
];
";
    let n = parse_str(src, "matpower")
        .unwrap()
        .network
        .to_normalized()
        .unwrap();
    let base = 100.0;
    // Bus angle: degrees → radians.
    assert!(
        approx(n.buses()[0].va, 30.0 * DEG_TO_RAD),
        "va: {}",
        n.buses()[0].va
    );
    // Branch: an explicit tap rides through unscaled; shift and angle bounds go
    // to radians.
    let br = &n.branches()[0];
    assert!(approx(br.tap, 0.978), "explicit tap kept: {}", br.tap);
    assert!(approx(br.shift, 15.0 * DEG_TO_RAD), "shift: {}", br.shift);
    assert!(approx(br.angmin, -30.0 * DEG_TO_RAD));
    assert!(approx(br.angmax, 30.0 * DEG_TO_RAD));
    // Gen caps: ramp_30 (GEN_EXTRA_KEYS index 8) is per-unitized; qc1min (index
    // 2) stays raw — the split GEN_PU_KEYS encodes.
    let caps = &n.generators()[0].caps;
    assert!(
        approx(caps[8].unwrap(), 60.0 / base),
        "ramp_30 scaled: {:?}",
        caps[8]
    );
    assert!(
        approx(caps[2].unwrap(), 5.0),
        "qc1min stays raw: {:?}",
        caps[2]
    );
}

#[test]
fn largest_pmax_gen_wins_slack_among_several() {
    // No file REF; two PV gen buses. The slack promotion must pick the larger
    // pmax even when it is listed second — exercising the argmax, not first-wins.
    let src = "\
function mpc = twogen
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t2\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t2\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t3\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t100\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
\t2\t0\t0\t100\t-100\t1\t100\t1\t300\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t2\t3\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let n = parse_str(src, "matpower")
        .unwrap()
        .network
        .to_normalized()
        .unwrap();
    // Bus 2's gen has pmax 300 > bus 1's 100, so bus 2 (dense index 1) is slack.
    assert_eq!(n.buses()[0].kind, BusType::Pv);
    assert_eq!(
        n.buses()[1].kind,
        BusType::Ref,
        "largest-pmax gen bus is slack"
    );
    assert_eq!(n.buses()[2].kind, BusType::Pq);
    assert_eq!(IndexedNetwork::new(&n).reference_bus_indices(), vec![1]);
}

#[test]
fn storage_per_units_ratings_but_keeps_dispatch_setpoint() {
    // norm_storage scales energy/ratings/limits/losses by base but leaves the
    // ps/qs dispatch setpoint raw (matching PowerModels' make_per_unit!). No
    // vendored case carries storage, so attach one to a parsed 2-bus case.
    let src = "\
function mpc = st
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
";
    let mut net = parse_str(src, "matpower").unwrap().network;
    let mut storage = Storage::new(BusId(2));
    storage.ps = 5.0;
    storage.qs = 3.0;
    storage.energy = 100.0;
    storage.energy_rating = 200.0;
    storage.charge_rating = 40.0;
    storage.discharge_rating = 40.0;
    storage.charge_efficiency = 0.95;
    storage.discharge_efficiency = 0.95;
    storage.thermal_rating = 60.0;
    storage.qmin = -30.0;
    storage.qmax = 30.0;
    storage.p_loss = 2.0;
    storage.q_loss = 1.0;
    net.storage_mut().push(storage);
    let base = 100.0;
    let normalized = net.to_normalized().unwrap();
    let s = &normalized.storage()[0];
    // Energy, ratings, reactive limits, and losses divide by base...
    assert!(approx(s.energy, 100.0 / base));
    assert!(approx(s.thermal_rating, 60.0 / base));
    assert!(approx(s.charge_rating, 40.0 / base));
    assert!(approx(s.qmax, 30.0 / base));
    assert!(approx(s.p_loss, 2.0 / base));
    // ...but the ps/qs dispatch setpoint stays raw.
    assert!(approx(s.ps, 5.0), "ps stays raw: {}", s.ps);
    assert!(approx(s.qs, 3.0), "qs stays raw: {}", s.qs);
}

#[test]
fn source_rows_cover_the_star_lowered_view() {
    // `IndexedNetwork` star-lowers an in-service 3-winding transformer, which
    // appends a bus, its star branches, and a magnetizing shunt. Those have no
    // source row, so the map must still be positional over the lowered view.
    let mut raw = parse_str(EPC_3W, "pslf").unwrap().network;
    raw.buses_mut()[0].kind = BusType::Ref;
    let mut g = Generator::new(BusId(1));
    g.pmax = 100.0;
    raw.generators_mut().push(g);

    let (n, rows) = raw
        .to_normalized_with_source_rows(&NormalizeOptions::default())
        .unwrap();
    let view = IndexedNetwork::new(&n.network);

    assert_eq!(rows.buses.len(), view.n());
    assert_eq!(rows.branches.len(), view.branches().len());
    assert_eq!(rows.shunts.len(), view.network().shunts().len());

    // The star bus and its branches are the appended tail.
    assert_eq!(rows.buses[..3], [Some(0), Some(1), Some(2)]);
    assert_eq!(*rows.buses.last().unwrap(), None);
    assert!(rows.branches.iter().all(Option::is_none));
}

#[test]
fn source_rows_cover_the_magnetizing_shunt_the_lowering_appends() {
    // `source_rows_cover_the_star_lowered_view` runs on a transformer with no
    // magnetizing admittance, so its shunt assertion compares an empty map to an
    // empty table. Give the transformer a magnetizing branch and the lowering
    // appends a shunt after the real one: the map has to grow with it, or
    // reading provenance for that tail position is out of bounds.
    let mut raw = parse_str(EPC_3W, "pslf").unwrap().network;
    raw.buses_mut()[0].kind = BusType::Ref;
    let mut g = Generator::new(BusId(1));
    g.pmax = 100.0;
    raw.generators_mut().push(g);
    raw.shunts_mut().push(Shunt::new(BusId(2), 0.0, 5.0));
    raw.transformers_3w_mut()[0].mag_b = 0.01;

    let (n, rows) = raw
        .to_normalized_with_source_rows(&NormalizeOptions::default())
        .unwrap();
    let view = IndexedNetwork::new(&n.network);

    assert_eq!(
        view.network().shunts().len(),
        2,
        "real shunt, then magnetizing"
    );
    assert_eq!(rows.shunts.len(), view.network().shunts().len());
    // The real shunt keeps its raw row; the appended magnetizing shunt has none.
    assert_eq!(rows.shunts, [Some(0), None]);
    assert_eq!(rows.buses.len(), view.n());
    assert_eq!(rows.branches.len(), view.branches().len());

    // The tables read provenance by position across the lowered view, so the
    // padding has to hold for them too.
    let tables = raw.to_normalized_solver_tables().unwrap();
    assert_eq!(tables.index.shunt_source_rows, [Some(0), None]);
}

const EPC_3W: &str = r#"title
t3w
!
solution parameters
sbase 100.0000
!
bus data  [3] ty vsched volt angle ar zone vmax vmin
1 "B1          " 230.0000 : 0 1.0 1.0 0.0 1 1 1.1 0.9
2 "B2          " 138.0000 : 1 1.0 1.0 0.0 1 1 1.1 0.9
3 "B3          " 13.8000 : 1 1.0 1.0 0.0 1 1 1.1 0.9
transformer data  [1]
1 "B1          " 230.00 2 "B2          " 138.00 "1 " 1 "xf3" : 1 0 0 0 0 0 0 0 0 3 0 0 0 0 100 0.01 0.06 0.02 0.07 0.03 0.08 /
0 0 0 0 0 0 100 90 80 0 0.0 0 0 0 0 0 1.05
end
"#;

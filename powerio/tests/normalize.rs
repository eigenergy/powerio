//! `Network::to_normalized`: per-unit / radians / tap / filter / reindex / bus
//! types, plus the no-false-write-back contract and `parse_str == parse`.

use std::path::{Path, PathBuf};

use powerio::{
    BusType, Error, SourceFormat, TargetFormat, parse, parse_matpower_file, parse_str, write_as,
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
    let base = raw.base_mva;
    let n = raw.to_normalized().unwrap();

    // case9 is all in service with a single reference bus, so nothing is dropped
    // and element order is preserved — a 1:1 comparison against the raw model.
    assert!(approx(n.base_mva, base));
    assert_eq!(n.buses.len(), raw.buses.len());
    assert_eq!(n.generators.len(), raw.generators.len());
    assert_eq!(n.branches.len(), raw.branches.len());
    assert_eq!(n.loads.len(), raw.loads.len());

    for (g, rg) in n.generators.iter().zip(&raw.generators) {
        assert!(approx(g.pg, rg.pg / base));
        assert!(approx(g.pmax, rg.pmax / base));
        assert!(approx(g.pmin, rg.pmin / base));
        assert!(approx(g.qmax, rg.qmax / base));
    }
    for (l, rl) in n.loads.iter().zip(&raw.loads) {
        assert!(approx(l.p, rl.p / base));
        assert!(approx(l.q, rl.q / base));
    }
    for (b, rb) in n.branches.iter().zip(&raw.branches) {
        assert!(approx(b.angmin, rb.angmin * DEG_TO_RAD));
        assert!(approx(b.angmax, rb.angmax * DEG_TO_RAD));
        assert!(b.tap != 0.0, "tap 0 normalized to 1");
    }
    // Polynomial gen cost: the p^2 coeff scales by base^2, p^1 by base, the
    // constant is unchanged.
    for (g, rg) in n.generators.iter().zip(&raw.generators) {
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
    let base = raw.base_mva;
    let n = raw.to_normalized().unwrap();
    assert!(!n.shunts.is_empty(), "case30 has shunts");
    for (s, rs) in n.shunts.iter().zip(&raw.shunts) {
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
    let base = raw.base_mva;
    let n = raw.to_normalized().unwrap();

    let raw_in: Vec<_> = raw.hvdc.iter().filter(|d| d.in_service).collect();
    assert!(!raw_in.is_empty(), "fixture has in-service dclines");
    assert_eq!(n.hvdc.len(), raw_in.len());
    for (d, rd) in n.hvdc.iter().zip(raw_in) {
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

    // A derived product, not a source for write-back.
    assert!(n.source.is_none());
    assert_eq!(n.source_format, SourceFormat::Normalized);

    // Writing it serializes the per-unit/radian model, so it must NOT echo the
    // raw MATPOWER bytes.
    let out = write_as(&n, TargetFormat::Matpower);
    assert_ne!(
        out.text.trim_end(),
        src.replace("\r\n", "\n").trim_end(),
        "normalized network must not echo the raw source"
    );
}

#[test]
fn filters_and_retypes_out_of_service_case() {
    let raw = parse_matpower_file(data("t_case9_oos.m")).unwrap();
    let n = raw.to_normalized().unwrap();

    // The fixture marks the bus-2 generator and branch 5-6 out of service; no
    // isolated buses, so all 9 buses survive with dense 1-based ids.
    assert_eq!(n.generators.len(), raw.generators.len() - 1);
    assert_eq!(n.branches.len(), raw.branches.len() - 1);
    assert_eq!(n.buses.len(), 9);
    for (i, b) in n.buses.iter().enumerate() {
        assert_eq!(b.id.0, i + 1);
    }
    assert_eq!(
        n.buses.iter().filter(|b| b.kind == BusType::Ref).count(),
        1,
        "exactly one reference bus"
    );
    // Bus 1 is the file reference; bus 2 lost its only (out-of-service) generator
    // so it falls to PQ; bus 3 keeps its generator and is PV.
    assert_eq!(n.buses[0].kind, BusType::Ref);
    assert_eq!(n.buses[1].kind, BusType::Pq);
    assert_eq!(n.buses[2].kind, BusType::Pv);
}

#[test]
fn drops_isolated_bus_and_remaps_endpoints() {
    // Bus 2 is isolated (type 4); branch 2-3 references it (dropped), branch 1-3
    // survives and is remapped, the load on bus 3 follows the reindex.
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
    let raw = parse_str(src, "matpower").unwrap();
    let n = raw.to_normalized().unwrap();

    assert_eq!(n.buses.len(), 2, "isolated bus dropped");
    assert_eq!(n.buses.iter().map(|b| b.id.0).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(n.branches.len(), 1, "branch on the isolated bus dropped");
    assert_eq!((n.branches[0].from.0, n.branches[0].to.0), (1, 2));
    assert_eq!(n.loads.len(), 1);
    assert_eq!(n.loads[0].bus.0, 2, "load remapped to the dense id");
    assert_eq!(n.buses[0].kind, BusType::Ref);
    assert_eq!(n.buses[1].kind, BusType::Pq);
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
    let raw = parse_str(src, "matpower").unwrap();
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
    let raw = parse_str(src, "matpower").unwrap();
    assert!(matches!(
        raw.to_normalized(),
        Err(Error::ReferenceBusCount { found: 0 })
    ));
}

#[test]
fn keeps_multiple_file_refs() {
    // Two gen-backed file REF buses are kept (not collapsed to one) — the consumer
    // picks the slack, matching ExaPowerIO/PowerDiff. The gen-less bus is PQ.
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
    let n = parse_str(src, "matpower").unwrap().to_normalized().unwrap();
    // Both gen-backed REF buses stay REF; the gen-less load bus is PQ.
    assert_eq!(n.buses.iter().filter(|b| b.kind == BusType::Ref).count(), 2);
    assert_eq!(n.buses[0].kind, BusType::Ref);
    assert_eq!(n.buses[1].kind, BusType::Ref);
    assert_eq!(n.buses[2].kind, BusType::Pq);
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
    let n = parse_str(src, "matpower").unwrap().to_normalized().unwrap();
    assert_eq!(n.buses.iter().filter(|b| b.kind == BusType::Ref).count(), 1);
    assert_eq!(n.buses[0].kind, BusType::Ref, "gen bus promoted to slack");
    assert_eq!(n.buses[1].kind, BusType::Pq);
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
    let n = parse_str(src, "matpower").unwrap().to_normalized().unwrap();
    let c = n.generators[0].cost.as_ref().unwrap();
    assert_eq!(c.model, 1);
    // [0, 0, 100, 2000] -> [0/100, 0, 100/100, 2000]
    assert!(approx(c.coeffs[0], 0.0));
    assert!(approx(c.coeffs[1], 0.0));
    assert!(approx(c.coeffs[2], 1.0));
    assert!(approx(c.coeffs[3], 2000.0));
}

#[test]
fn parse_str_matches_parse() {
    for case in ["case9.m", "case14.m", "case30.m"] {
        let text = std::fs::read_to_string(data(case)).unwrap();
        let from_path = parse(data(case)).unwrap();
        let mut from_text = parse_str(&text, "matpower").unwrap();
        // The only legitimate difference is the network name, which `parse`
        // derives from the file stem and `parse_str` cannot (it has no path).
        from_text.name = from_path.name.clone();
        // to_json skips the retained source, so equal JSON means field-for-field
        // equal tables.
        assert_eq!(
            from_path.to_json().unwrap(),
            from_text.to_json().unwrap(),
            "{case}: parse_str disagrees with parse"
        );
    }
}

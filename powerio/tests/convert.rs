//! Structural tests for the format converters. PowerModels output is validated
//! value-for-value against PowerModels.jl in `benchmarks/validate_powermodels.jl`
//! (needs Julia); these tests pin the structure and the MATPOWER→hub mapping that
//! every converter shares, and run in plain `cargo test`.

use std::path::{Path, PathBuf};

use powerio::{
    SourceFormat, TargetFormat, parse_matpower, parse_matpower_file, parse_powermodels_json,
    parse_powerworld, parse_psse, write_as, write_egret_json, write_powermodels_json,
    write_powerworld, write_psse,
};
use serde_json::Value;

mod common;
use common::json_approx_eq;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

#[test]
fn powermodels_structure_and_split() {
    let case = parse_matpower_file(data("case30.m")).unwrap();
    let conv = write_powermodels_json(&case);
    assert!(
        conv.warnings.is_empty(),
        "case30 should convert cleanly: {:?}",
        conv.warnings
    );
    let v: Value = serde_json::from_str(&conv.text).unwrap();

    assert_eq!(v["per_unit"], Value::Bool(true));
    assert_eq!(v["source_type"], "matpower");
    // Buses are keyed by their MATPOWER id; loads/shunts are split out of the bus.
    assert_eq!(v["bus"].as_object().unwrap().len(), case.buses.len());
    assert_eq!(v["branch"].as_object().unwrap().len(), case.branches.len());
    assert_eq!(v["gen"].as_object().unwrap().len(), case.generators.len());

    // A load/shunt exists for each bus that carries demand / a shunt.
    let want_loads = case.loads.len();
    let want_shunts = case.shunts.len();
    assert_eq!(v["load"].as_object().unwrap().len(), want_loads);
    assert_eq!(v["shunt"].as_object().unwrap().len(), want_shunts);
    assert!(want_loads > 0, "case30 has loads");

    // A branch carries split charging and a transformer flag; no bus pd/qd leaks in.
    let b1 = &v["branch"]["1"];
    assert!(b1.get("b_fr").is_some() && b1.get("b_to").is_some());
    assert!(b1.get("transformer").unwrap().is_boolean());
    assert!(v["bus"]["1"].get("pd").is_none(), "bus must not keep pd");

    // A load points back at its bus and carries that bus's demand.
    let load1 = &v["load"]["1"];
    assert!(load1["load_bus"].is_number());
    assert!(load1.get("pd").is_some());
}

#[test]
// Detecting an explicit tap of exactly 1.0 from the file is the point, so the exact compare is intended.
#[allow(clippy::float_cmp)]
fn powermodels_transformer_flag_tracks_raw_tap() {
    // PowerModels' rule (io/matpower.jl): a branch is a transformer iff its raw tap
    // is nonzero. case57 has branches with an explicit tap of 1.0 — a transformer,
    // even though the effective ratio is 1 — while a pure phase shifter (tap 0,
    // shift ≠ 0) is a line. The writer must emit that same flag.
    let case = parse_matpower_file(data("case57.m")).unwrap();
    let v: Value = serde_json::from_str(&write_powermodels_json(&case).text).unwrap();
    let any_explicit_tap = case.branches.iter().any(|b| b.tap == 1.0);
    assert!(
        any_explicit_tap,
        "fixture expectation: case57 has an explicit tap=1 branch"
    );
    let xfmr = v["branch"]
        .as_object()
        .unwrap()
        .values()
        .filter(|b| b["transformer"] == Value::Bool(true))
        .count();
    let raw_xfmr = case.branches.iter().filter(|b| b.tap != 0.0).count();
    assert_eq!(xfmr, raw_xfmr);
}

#[test]
fn powermodels_warns_on_non_finite() {
    // pegase carries Inf reactive limits; JSON can't hold ±Inf, so we emit null
    // and must say so rather than fail silently.
    let case = parse_matpower_file(data("case2869pegase.m")).unwrap();
    let conv = write_powermodels_json(&case);
    let v: Value = serde_json::from_str(&conv.text).unwrap();
    assert!(
        conv.warnings.iter().any(|w| w.contains("non-finite")),
        "expected a non-finite warning, got: {:?}",
        conv.warnings
    );
    assert!(serde_json::to_string(&v).is_ok());
}

#[test]
fn egret_structure() {
    let case = parse_matpower_file(data("case30.m")).unwrap();
    let v: Value = serde_json::from_str(&write_egret_json(&case).text).unwrap();
    let elements = &v["elements"];
    assert_eq!(elements["bus"].as_object().unwrap().len(), case.buses.len());
    assert_eq!(
        elements["branch"].as_object().unwrap().len(),
        case.branches.len()
    );
    assert_eq!(
        elements["generator"].as_object().unwrap().len(),
        case.generators.len()
    );
    assert_eq!(v["system"]["baseMVA"], case.base_mva);
    assert!(v["system"].get("reference_bus").is_some());
    // A branch is typed line/transformer and a generator carries a cost curve.
    assert!(elements["branch"]["1"]["branch_type"].is_string());
    let g1 = &elements["generator"]["1"];
    assert_eq!(g1["p_cost"]["data_type"], "cost_curve");
}

#[test]
fn powermodels_json_reader_is_inverse_of_writer() {
    // read→write reproduces powerio's own PowerModels JSON across cases: same keys,
    // same structure, same values — proving the reader captures every field the
    // writer emits. Compared field-by-field with a float tolerance rather than
    // byte-exact, because the per-unit round-trip (÷base on write, ×base on read)
    // is not bit-exact in f64.
    for case in ["case9", "case14", "case30", "case57", "case118"] {
        let net = parse_matpower_file(data(&format!("{case}.m"))).unwrap();
        let json1 = write_powermodels_json(&net).text;
        let net2 = parse_powermodels_json(&json1).unwrap();
        let json2 = write_powermodels_json(&net2).text;
        let v1: Value = serde_json::from_str(&json1).unwrap();
        let v2: Value = serde_json::from_str(&json2).unwrap();
        assert!(
            json_approx_eq(&v1, &v2),
            "{case}: PowerModels JSON not stable through read→write"
        );
    }
}

#[test]
fn powermodels_json_same_format_is_byte_exact_echo() {
    // Same-format round-trip echoes the retained source byte-for-byte.
    let net = parse_matpower_file(data("case30.m")).unwrap();
    let json = write_powermodels_json(&net).text;
    let net2 = parse_powermodels_json(&json).unwrap();
    assert_eq!(write_as(&net2, TargetFormat::PowerModelsJson).text, json);
}

#[test]
// The hub round-trip must preserve base_mva exactly, so the exact compare is the assertion.
#[allow(clippy::float_cmp)]
fn powermodels_json_to_matpower_two_way() {
    // PowerModels JSON in → neutral hub → MATPOWER out. Proves the hub isn't
    // MATPOWER-only on the read side. Source is PowerModels, so the MATPOWER
    // target is canonical (not an echo).
    let orig = parse_matpower_file(data("case30.m")).unwrap();
    let json = write_powermodels_json(&orig).text;
    let net = parse_powermodels_json(&json).unwrap();
    assert_eq!(net.source_format, powerio::SourceFormat::PowerModelsJson);

    let reparsed = parse_matpower(&write_as(&net, TargetFormat::Matpower).text).unwrap();
    assert_eq!(reparsed.buses.len(), orig.buses.len());
    assert_eq!(reparsed.branches.len(), orig.branches.len());
    assert_eq!(reparsed.generators.len(), orig.generators.len());
    assert_eq!(reparsed.base_mva, orig.base_mva);
    // Total demand survives the bus→load split and the fold back onto the bus.
    let load_of = |c: &powerio::Network| c.loads.iter().map(|l| l.p).sum::<f64>();
    assert!((load_of(&orig) - load_of(&reparsed)).abs() < 1e-9);
}

#[test]
fn psse_reads_real_pti_files() {
    // Real PSS/E v33 files from PowerModels' PTI test suite (vendored under
    // tests/data/psse). Validates the reader against third-party input, not just
    // powerio's own round-trip. Value-vs-PowerModels lives in validate_psse.jl.
    let c14 = parse_psse(&std::fs::read_to_string(data("psse/case14.raw")).unwrap()).unwrap();
    assert_eq!(c14.buses.len(), 14);
    assert_eq!(c14.source_format, SourceFormat::Psse);
    assert!(!c14.branches.is_empty() && !c14.generators.is_empty());

    // case5 carries phase-shifting and 2-winding transformers.
    let c5 = parse_psse(&std::fs::read_to_string(data("psse/case5.raw")).unwrap()).unwrap();
    assert_eq!(c5.buses.len(), 5);
    let transformers = c5.branches.iter().filter(|b| b.is_transformer()).count();
    assert!(transformers > 0, "case5.raw should have transformers");
}

#[test]
fn hvdc_converts_and_round_trips() {
    // t_case9_dcline.m carries HVDC dclines. PowerModels JSON round-trips them;
    // EGRET/PSS-E/PowerWorld drop them, each with a warning.
    let net = parse_matpower_file(data("t_case9_dcline.m")).unwrap();
    assert!(!net.hvdc.is_empty(), "fixture should have dclines");

    let pm = write_powermodels_json(&net);
    assert!(
        pm.warnings.iter().any(|w| w.contains("dcline")),
        "PM should flag dcline best-effort"
    );
    let back = parse_powermodels_json(&pm.text).unwrap();
    assert_eq!(back.hvdc.len(), net.hvdc.len());
    assert_eq!(back.hvdc[0].from, net.hvdc[0].from);
    assert_eq!(back.hvdc[0].to, net.hvdc[0].to);

    for conv in [
        write_egret_json(&net),
        write_psse(&net),
        write_powerworld(&net),
    ] {
        assert!(
            conv.warnings.iter().any(|w| w.contains("dcline")),
            "expected a dropped-dcline warning, got {:?}",
            conv.warnings
        );
    }
}

#[test]
fn powermodels_reader_handles_per_unit_input() {
    // A per_unit=true PowerModels file: powers in p.u., angles in radians, cost
    // coefficients scaled by base powers. The reader must invert all three.
    let json = r#"{
      "baseMVA": 100.0, "per_unit": true, "name": "pu",
      "bus": {"1": {"bus_i":1,"index":1,"bus_type":3,"vm":1.0,"va":0.0,"vmax":1.1,"vmin":0.9,"base_kv":230.0,"area":1,"zone":1}},
      "branch": {"1": {"index":1,"f_bus":1,"t_bus":1,"br_r":0.0,"br_x":0.1,"b_fr":0.0,"b_to":0.0,"g_fr":0.0,"g_to":0.0,"tap":1.0,"shift":0.0,"br_status":1,"angmin":-0.5236,"angmax":0.5236,"transformer":false}},
      "gen": {"1": {"index":1,"gen_bus":1,"pg":2.0,"qg":0.0,"qmax":1.0,"qmin":-1.0,"vg":1.0,"mbase":100.0,"gen_status":1,"pmax":3.0,"pmin":0.0,"model":2,"ncost":3,"startup":0.0,"shutdown":0.0,"cost":[430.293,2000.0,0.0]}},
      "load": {}, "shunt": {}, "dcline": {}, "storage": {}
    }"#;
    let net = parse_powermodels_json(json).unwrap();
    let g = &net.generators[0];
    assert!((g.pg - 200.0).abs() < 1e-6, "pg p.u.→MW"); // 2.0 * 100
    assert!((g.pmax - 300.0).abs() < 1e-6);
    assert!((net.branches[0].angmax - 30.0).abs() < 1e-2, "rad→deg"); // 0.5236 rad
    let cost = g.cost.as_ref().unwrap();
    assert!(
        (cost.coeffs[0] - 0.043_029_3).abs() < 1e-6,
        "c2 un-scaled by base²"
    );
    assert!((cost.coeffs[1] - 20.0).abs() < 1e-6, "c1 un-scaled by base");
}

#[test]
fn readers_reject_malformed_input() {
    // Identity/structure errors must surface, not silently default.
    assert!(parse_powermodels_json("not json").is_err());
    assert!(
        parse_powermodels_json(r#"{"per_unit":false}"#).is_err(),
        "missing baseMVA"
    );
    let no_id = r#"{"baseMVA":100,"bus":{"1":{"bus_type":1,"vm":1.0}},"branch":{},"gen":{},"load":{},"shunt":{}}"#;
    assert!(
        parse_powermodels_json(no_id).is_err(),
        "bus missing id must error"
    );
    let dangling = r#"{"baseMVA":100,"bus":{"1":{"bus_i":1,"index":1,"bus_type":3,"vm":1.0,"va":0.0,"vmax":1.1,"vmin":0.9,"base_kv":1.0,"area":1,"zone":1}},
      "branch":{"1":{"index":1,"f_bus":1,"t_bus":99,"br_r":0,"br_x":0.1,"b_fr":0,"b_to":0,"tap":1,"shift":0,"br_status":1,"angmin":-1,"angmax":1,"transformer":false}},
      "gen":{},"load":{},"shunt":{}}"#;
    assert!(
        parse_powermodels_json(dangling).is_err(),
        "dangling branch ref must error"
    );
    assert!(parse_psse("").is_err(), "empty PSS/E");
    assert!(
        parse_powerworld("// only a comment\n").is_err(),
        "no DATA blocks"
    );
}

#[test]
fn matpower_target_round_trips() {
    let net = parse_matpower_file(data("case14.m")).unwrap();
    let conv = write_as(&net, TargetFormat::Matpower);
    assert!(conv.warnings.is_empty());
    // Matpower target is the lossless echo: byte-identical to the source.
    let src = std::fs::read_to_string(data("case14.m")).unwrap();
    assert_eq!(conv.text, src);
}

#[test]
fn powermodels_phase_shifter_is_a_line_not_a_transformer() {
    // A pure phase shifter (raw tap 0, nonzero shift in column 10) is a line under
    // PowerModels' rule, while its shift is preserved and converted to radians.
    // This is the case that distinguishes the raw-tap rule from the old
    // tap-or-shift one; no vendored fixture carries it.
    let src = "\
function mpc = ps
mpc.baseMVA = 100;
mpc.bus = [
\t1 3 0 0 0 0 1 1 0 345 1 1.1 0.9;
\t2 1 10 5 0 0 1 1 0 345 1 1.1 0.9;
];
mpc.branch = [
\t1 2 0.01 0.05 0.0 0 0 0 0 30 1 -360 360;
];
";
    let net = parse_matpower(src).unwrap();
    let v: Value = serde_json::from_str(&write_powermodels_json(&net).text).unwrap();
    let b1 = &v["branch"]["1"];
    assert_eq!(
        b1["transformer"],
        Value::Bool(false),
        "phase shifter must be a line"
    );
    let shift = b1["shift"].as_f64().unwrap();
    assert!(
        (shift - 30.0_f64.to_radians()).abs() < 1e-9,
        "shift converted to radians"
    );
}

#[test]
fn powermodels_dcline_flips_pt_qf_qt_sign() {
    // PowerModels stores Pt/Qf/Qt with the opposite sign to MATPOWER. The writer
    // emits the flipped sign; the reader un-flips it (so the round-trip cancels and
    // can't catch a sign error on its own — this checks the absolute sign).
    let net = parse_matpower_file(data("t_case9_dcline.m")).unwrap();
    let dc = net
        .hvdc
        .iter()
        .find(|d| d.pt != 0.0)
        .expect("a dcline with nonzero Pt");
    let v: Value = serde_json::from_str(&write_powermodels_json(&net).text).unwrap();
    let obj = v["dcline"]
        .as_object()
        .unwrap()
        .values()
        .find(|d| {
            d["f_bus"].as_u64() == Some(dc.from as u64) && d["t_bus"].as_u64() == Some(dc.to as u64)
        })
        .expect("emitted dcline with matching endpoints");
    let emitted_pt = obj["pt"].as_f64().unwrap();
    assert!(
        emitted_pt.signum() != dc.pt.signum(),
        "pt sign must flip on write"
    );
    assert!(
        (emitted_pt + dc.pt / net.base_mva).abs() < 1e-9,
        "pt = -Pt / base"
    );
}

#[test]
fn powermodels_storage_ps_qs_stay_raw() {
    // PowerModels' make_per_unit! leaves storage ps/qs as setpoints (raw) while
    // scaling energy/ratings/limits by base. The reader must mirror that split.
    // No vendored fixture has storage, so feed an inline per_unit=true record.
    let json = r#"{
      "baseMVA": 100.0, "per_unit": true, "name": "st",
      "bus": {"1": {"bus_i":1,"index":1,"bus_type":3,"vm":1.0,"va":0.0,"vmax":1.1,"vmin":0.9,"base_kv":1.0,"area":1,"zone":1}},
      "branch": {}, "gen": {}, "load": {}, "shunt": {}, "dcline": {},
      "storage": {"1": {"index":1,"storage_bus":1,"ps":0.5,"qs":0.25,"energy":1.0,"energy_rating":6.0,"charge_rating":3.0,"discharge_rating":3.0,"charge_efficiency":0.9,"discharge_efficiency":0.9,"thermal_rating":3.0,"qmin":-1.0,"qmax":1.0,"r":0.0,"x":0.0,"p_loss":0.0,"q_loss":0.0,"status":1}}
    }"#;
    let net = parse_powermodels_json(json).unwrap();
    let s = &net.storage[0];
    assert!((s.ps - 0.5).abs() < 1e-9, "ps stays raw");
    assert!((s.qs - 0.25).abs() < 1e-9, "qs stays raw");
    assert!(
        (s.energy_rating - 600.0).abs() < 1e-6,
        "energy_rating ×base"
    ); // 6.0 · 100
    assert!((s.qmax - 100.0).abs() < 1e-6, "qmax ×base"); // 1.0 · 100
}

#[test]
fn powermodels_unbounded_limit_round_trips_as_infinity() {
    // A gen qmax = Inf writes as JSON null (with a warning); the reader must read it
    // back as unbounded, not as a binding 0.0.
    let mut net = parse_matpower_file(data("case9.m")).unwrap();
    net.generators[0].qmax = f64::INFINITY;
    net.generators[0].qmin = f64::NEG_INFINITY;
    let conv = write_powermodels_json(&net);
    assert!(
        conv.warnings.iter().any(|w| w.contains("non-finite")),
        "expected null warning"
    );
    let back = parse_powermodels_json(&conv.text).unwrap();
    assert!(back.generators[0].qmax.is_infinite() && back.generators[0].qmax > 0.0);
    assert!(back.generators[0].qmin.is_infinite() && back.generators[0].qmin < 0.0);
}

#[test]
fn oos_fixture_marks_out_of_service_elements() {
    // t_case9_oos.m turns gen 2 and branch 5-6 out of service; the parse must carry
    // those in_service=false flags (the basis for ExaPowerIO filtered=true parity).
    // The fixture otherwise runs only in the Julia validators.
    let net = parse_matpower_file(data("t_case9_oos.m")).unwrap();
    assert_eq!(net.generators.iter().filter(|g| !g.in_service).count(), 1);
    let br = net
        .branches
        .iter()
        .find(|b| b.from == 5 && b.to == 6)
        .expect("branch 5-6");
    assert!(!br.in_service, "branch 5-6 is out of service");
}

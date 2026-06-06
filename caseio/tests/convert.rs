//! Structural tests for the format converters. PowerModels output is validated
//! value-for-value against PowerModels.jl in `benchmarks/validate_powermodels.jl`
//! (needs Julia); these tests pin the structure and the MATPOWER→hub mapping that
//! every converter shares, and run in plain `cargo test`.

use std::path::{Path, PathBuf};

use caseio::{
    parse_matpower, parse_matpower_file, parse_powermodels_json, parse_powerworld, parse_psse,
    write_as, write_egret_json, write_powermodels_json, write_powerworld, write_psse, SourceFormat,
    TargetFormat,
};
use serde_json::Value;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data").join(name)
}

#[test]
fn powermodels_structure_and_split() {
    let case = parse_matpower_file(data("case30.m")).unwrap();
    let conv = write_powermodels_json(&case.to_network());
    assert!(conv.warnings.is_empty(), "case30 should convert cleanly: {:?}", conv.warnings);
    let v: Value = serde_json::from_str(&conv.text).unwrap();

    assert_eq!(v["per_unit"], Value::Bool(false));
    assert_eq!(v["source_type"], "matpower");
    // Buses are keyed by their MATPOWER id; loads/shunts are split out of the bus.
    assert_eq!(v["bus"].as_object().unwrap().len(), case.buses.len());
    assert_eq!(v["branch"].as_object().unwrap().len(), case.branches.len());
    assert_eq!(v["gen"].as_object().unwrap().len(), case.gens.len());

    // A load/shunt exists for each bus that carries demand / a shunt.
    let want_loads = case.buses.iter().filter(|b| b.pd != 0.0 || b.qd != 0.0).count();
    let want_shunts = case.buses.iter().filter(|b| b.gs != 0.0 || b.bs != 0.0).count();
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
fn powermodels_transformer_flag_tracks_raw_tap() {
    // case57 has branches with an explicit tap of 1.0 — a transformer in MATPOWER,
    // even though the effective ratio is 1.
    let case = parse_matpower_file(data("case57.m")).unwrap();
    let v: Value = serde_json::from_str(&write_powermodels_json(&case.to_network()).text).unwrap();
    let any_explicit_tap = case.branches.iter().any(|b| b.tap == 1.0);
    assert!(any_explicit_tap, "fixture expectation: case57 has an explicit tap=1 branch");
    let xfmr = v["branch"]
        .as_object()
        .unwrap()
        .values()
        .filter(|b| b["transformer"] == Value::Bool(true))
        .count();
    let raw_xfmr = case.branches.iter().filter(|b| b.tap != 0.0 || b.shift != 0.0).count();
    assert_eq!(xfmr, raw_xfmr);
}

#[test]
fn powermodels_warns_on_non_finite() {
    // pegase carries Inf reactive limits; JSON can't hold ±Inf, so we emit null
    // and must say so rather than fail silently.
    let case = parse_matpower_file(data("case2869pegase.m")).unwrap();
    let conv = write_powermodels_json(&case.to_network());
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
    let v: Value = serde_json::from_str(&write_egret_json(&case.to_network()).text).unwrap();
    let elements = &v["elements"];
    assert_eq!(elements["bus"].as_object().unwrap().len(), case.buses.len());
    assert_eq!(elements["branch"].as_object().unwrap().len(), case.branches.len());
    assert_eq!(elements["generator"].as_object().unwrap().len(), case.gens.len());
    assert_eq!(v["system"]["baseMVA"], case.base_mva);
    assert!(v["system"].get("reference_bus").is_some());
    // A branch is typed line/transformer and a generator carries a cost curve.
    assert!(elements["branch"]["1"]["branch_type"].is_string());
    let g1 = &elements["generator"]["1"];
    assert_eq!(g1["p_cost"]["data_type"], "cost_curve");
}

#[test]
fn powermodels_json_reader_is_inverse_of_writer() {
    // read→write is the identity on caseio's own PowerModels JSON, across cases:
    // proves the reader captures every field the writer emits.
    for case in ["case9", "case14", "case30", "case57", "case118"] {
        let net = parse_matpower_file(data(&format!("{case}.m"))).unwrap().to_network();
        let json1 = write_powermodels_json(&net).text;
        let net2 = parse_powermodels_json(&json1).unwrap();
        let json2 = write_powermodels_json(&net2).text;
        assert_eq!(json1, json2, "{case}: PowerModels JSON not stable through read→write");
    }
}

#[test]
fn powermodels_json_same_format_is_byte_exact_echo() {
    // Same-format round-trip echoes the retained source byte-for-byte.
    let net = parse_matpower_file(data("case30.m")).unwrap().to_network();
    let json = write_powermodels_json(&net).text;
    let net2 = parse_powermodels_json(&json).unwrap();
    assert_eq!(write_as(&net2, TargetFormat::PowerModelsJson).text, json);
}

#[test]
fn powermodels_json_to_matpower_two_way() {
    // PowerModels JSON in → neutral hub → MATPOWER out. Proves the hub isn't
    // MATPOWER-only on the read side. Source is PowerModels, so the MATPOWER
    // target is canonical (not an echo).
    let orig = parse_matpower_file(data("case30.m")).unwrap();
    let json = write_powermodels_json(&orig.to_network()).text;
    let net = parse_powermodels_json(&json).unwrap();
    assert_eq!(net.source_format, caseio::SourceFormat::PowerModelsJson);

    let reparsed = parse_matpower(&write_as(&net, TargetFormat::Matpower).text).unwrap();
    assert_eq!(reparsed.buses.len(), orig.buses.len());
    assert_eq!(reparsed.branches.len(), orig.branches.len());
    assert_eq!(reparsed.gens.len(), orig.gens.len());
    assert_eq!(reparsed.base_mva, orig.base_mva);
    // Total demand survives the bus→load split and the fold back onto the bus.
    let load_of = |c: &caseio::MpcCase| c.buses.iter().map(|b| b.pd).sum::<f64>();
    assert!((load_of(&orig) - load_of(&reparsed)).abs() < 1e-9);
}

#[test]
fn psse_reads_real_pti_files() {
    // Real PSS/E v33 files from PowerModels' PTI test suite (vendored under
    // tests/data/psse). Validates the reader against third-party input, not just
    // caseio's own round-trip. Value-vs-PowerModels lives in validate_psse.jl.
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
    let net = parse_matpower_file(data("t_case9_dcline.m")).unwrap().to_network();
    assert!(!net.hvdc.is_empty(), "fixture should have dclines");

    let pm = write_powermodels_json(&net);
    assert!(pm.warnings.iter().any(|w| w.contains("dcline")), "PM should flag dcline best-effort");
    let back = parse_powermodels_json(&pm.text).unwrap();
    assert_eq!(back.hvdc.len(), net.hvdc.len());
    assert_eq!(back.hvdc[0].from, net.hvdc[0].from);
    assert_eq!(back.hvdc[0].to, net.hvdc[0].to);

    for conv in [write_egret_json(&net), write_psse(&net), write_powerworld(&net)] {
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
    assert!((cost.coeffs[0] - 0.043_029_3).abs() < 1e-6, "c2 un-scaled by base²");
    assert!((cost.coeffs[1] - 20.0).abs() < 1e-6, "c1 un-scaled by base");
}

#[test]
fn readers_reject_malformed_input() {
    // Identity/structure errors must surface, not silently default.
    assert!(parse_powermodels_json("not json").is_err());
    assert!(parse_powermodels_json(r#"{"per_unit":false}"#).is_err(), "missing baseMVA");
    let no_id = r#"{"baseMVA":100,"bus":{"1":{"bus_type":1,"vm":1.0}},"branch":{},"gen":{},"load":{},"shunt":{}}"#;
    assert!(parse_powermodels_json(no_id).is_err(), "bus missing id must error");
    let dangling = r#"{"baseMVA":100,"bus":{"1":{"bus_i":1,"index":1,"bus_type":3,"vm":1.0,"va":0.0,"vmax":1.1,"vmin":0.9,"base_kv":1.0,"area":1,"zone":1}},
      "branch":{"1":{"index":1,"f_bus":1,"t_bus":99,"br_r":0,"br_x":0.1,"b_fr":0,"b_to":0,"tap":1,"shift":0,"br_status":1,"angmin":-1,"angmax":1,"transformer":false}},
      "gen":{},"load":{},"shunt":{}}"#;
    assert!(parse_powermodels_json(dangling).is_err(), "dangling branch ref must error");
    assert!(parse_psse("").is_err(), "empty PSS/E");
    assert!(parse_powerworld("// only a comment\n").is_err(), "no DATA blocks");
}

#[test]
fn matpower_target_round_trips() {
    let net = parse_matpower_file(data("case14.m")).unwrap().to_network();
    let conv = write_as(&net, TargetFormat::Matpower);
    assert!(conv.warnings.is_empty());
    // Matpower target is the lossless echo: byte-identical to the source.
    let src = std::fs::read_to_string(data("case14.m")).unwrap();
    assert_eq!(conv.text, src);
}

//! Structural tests for the format converters. PowerModels output is validated
//! value-for-value against PowerModels.jl in `benchmarks/validate_powermodels.jl`
//! (needs Julia); these tests pin the structure and the MATPOWER→hub mapping that
//! every converter shares, and run in plain `cargo test`.

use std::path::{Path, PathBuf};

use caseio::{
    parse_matpower, parse_matpower_file, parse_powermodels_json, write_as, write_egret_json,
    write_powermodels_json, TargetFormat,
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
fn matpower_target_round_trips() {
    let net = parse_matpower_file(data("case14.m")).unwrap().to_network();
    let conv = write_as(&net, TargetFormat::Matpower);
    assert!(conv.warnings.is_empty());
    // Matpower target is the lossless echo: byte-identical to the source.
    let src = std::fs::read_to_string(data("case14.m")).unwrap();
    assert_eq!(conv.text, src);
}

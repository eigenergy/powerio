//! Structural tests for the format converters. PowerModels output is validated
//! value-for-value against PowerModels.jl in `benchmarks/validate_powermodels.jl`
//! (needs Julia); these tests pin the structure and the MATPOWER→hub mapping that
//! every converter shares, and run in plain `cargo test`.

use std::path::{Path, PathBuf};

use powerio::{
    BusId, BusType, Error, Network, SourceFormat, TargetFormat, convert_file, parse_file,
    parse_matpower, parse_matpower_file, parse_powermodels_json, parse_powerworld, parse_psse,
    read_pypsa_csv_folder, write_as, write_egret_json, write_powermodels_json, write_powerworld,
    write_psse, write_pypsa_csv_folder,
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
fn canonical_api_names_parse_and_convert() {
    let path = data("case14.m");
    let src = std::fs::read_to_string(&path).unwrap();
    let net = parse_file(&path, None).unwrap().network;

    assert_eq!(
        "powermodels-json".parse::<TargetFormat>().unwrap(),
        TargetFormat::PowerModelsJson
    );
    assert_eq!(TargetFormat::Psse.to_string(), "psse");
    assert_eq!(net.to_matpower(), src);

    let pm = net.to_format(TargetFormat::PowerModelsJson).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&pm.text).unwrap()["name"],
        "case14"
    );

    let same = convert_file(&path, TargetFormat::Matpower, None).unwrap();
    assert_eq!(same.text, src);
    assert!(same.warnings.is_empty());
}

#[derive(Debug, PartialEq)]
struct Core {
    buses: usize,
    branches: usize,
    gens: usize,
    loads: usize,
    shunts: usize,
    load_p: i64,
    load_q: i64,
    shunt_g: i64,
    shunt_b: i64,
    gen_p: i64,
    branch_r: i64,
    branch_x: i64,
    branch_b: i64,
    base_mva: i64,
}

fn core(net: &Network) -> Core {
    let r = |x: f64| (x * 1e6).round() as i64;
    Core {
        buses: net.buses.len(),
        branches: net.branches.len(),
        gens: net.generators.len(),
        loads: net.loads.len(),
        shunts: net.shunts.len(),
        load_p: r(net.loads.iter().map(|l| l.p).sum()),
        load_q: r(net.loads.iter().map(|l| l.q).sum()),
        shunt_g: r(net.shunts.iter().map(|s| s.g).sum()),
        shunt_b: r(net.shunts.iter().map(|s| s.b).sum()),
        gen_p: r(net.generators.iter().map(|g| g.pg).sum()),
        branch_r: r(net.branches.iter().map(|b| b.r).sum()),
        branch_x: r(net.branches.iter().map(|b| b.x).sum()),
        branch_b: r(net.branches.iter().map(|b| b.b).sum()),
        base_mva: r(net.base_mva),
    }
}

#[test]
fn pandapower_json_round_trips_core_and_echoes_source() {
    let net = parse_matpower_file(data("case9.m")).unwrap();
    let conv = write_as(&net, TargetFormat::PandapowerJson).unwrap();
    assert!(
        !conv.warnings.iter().any(|w| w.contains("dcline")),
        "case9 has no dclines, got warnings: {:?}",
        conv.warnings
    );
    let back = powerio::parse_str(&conv.text, "pandapower-json")
        .unwrap()
        .network;
    assert_eq!(back.source_format, SourceFormat::PandapowerJson);
    assert_eq!(core(&back), core(&net));
    assert_eq!(
        write_as(&back, TargetFormat::PandapowerJson).unwrap().text,
        conv.text
    );

    let inferred_path = tmp_path("case9-pandapower-json", "json");
    std::fs::write(&inferred_path, &conv.text).unwrap();
    let inferred = parse_file(&inferred_path, None).unwrap().network;
    assert_eq!(inferred.source_format, SourceFormat::PandapowerJson);
}

#[test]
fn pypsa_csv_folder_round_trips_core() {
    let net = parse_matpower_file(data("case9.m")).unwrap();
    let out = tmp_dir("case9-pypsa-csv");
    let written = write_pypsa_csv_folder(&net, &out).unwrap();
    let names: std::collections::BTreeSet<_> = written
        .files
        .iter()
        .filter_map(|p| p.file_name().and_then(|s| s.to_str()))
        .collect();
    for expected in [
        "network.csv",
        "snapshots.csv",
        "buses.csv",
        "generators.csv",
        "loads.csv",
        "lines.csv",
    ] {
        assert!(names.contains(expected), "missing {expected} in {names:?}");
    }
    let back = read_pypsa_csv_folder(&out).unwrap().network;
    assert_eq!(back.source_format, SourceFormat::PypsaCsv);
    assert_eq!(core(&back), core(&net));
}

#[test]
fn pypsa_csv_folder_preserves_nonnumeric_bus_names() {
    let dir = tmp_dir("pypsa-nonnumeric-buses");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.csv"), "name,srid\nnamed,4326\n").unwrap();
    std::fs::write(dir.join("snapshots.csv"), ",snapshot\n0,now\n").unwrap();
    std::fs::write(
        dir.join("buses.csv"),
        "name,v_nom,v_mag_pu_set,v_mag_pu_min,v_mag_pu_max\nalpha,110.0,1.0,0.9,1.1\nbeta,110.0,1.0,0.9,1.1\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("loads.csv"),
        "name,bus,p_set,q_set\nload_1,beta,5.0,2.0\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("lines.csv"),
        "name,bus0,bus1,r,x,b,s_nom\nline_1,alpha,beta,12.1,24.2,0.0001,100.0\n",
    )
    .unwrap();

    let net = read_pypsa_csv_folder(&dir).unwrap().network;
    assert_eq!(net.buses[0].id, BusId(1));
    assert_eq!(net.buses[0].name.as_deref(), Some("alpha"));
    assert_eq!(net.buses[1].id, BusId(2));
    assert_eq!(net.buses[1].name.as_deref(), Some("beta"));
    assert_eq!(net.loads[0].bus, BusId(2));
    assert_eq!(net.branches[0].from, BusId(1));
    assert_eq!(net.branches[0].to, BusId(2));
}

#[test]
#[allow(clippy::float_cmp)]
fn pypsa_csv_folder_reads_storage_units() {
    let dir = tmp_dir("pypsa-storage-units");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.csv"), "name,srid\nstorage,4326\n").unwrap();
    std::fs::write(
        dir.join("buses.csv"),
        "name,v_nom,v_mag_pu_set,v_mag_pu_min,v_mag_pu_max\n1,110.0,1.0,0.9,1.1\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("storage_units.csv"),
        "name,bus,p_nom,max_hours,efficiency_store,efficiency_dispatch,p_set,q_set,state_of_charge_initial,active\nstorage_1,1,25.0,4.0,0.91,0.92,3.0,1.5,20.0,false\n",
    )
    .unwrap();

    let net = read_pypsa_csv_folder(&dir).unwrap().network;
    assert_eq!(net.storage.len(), 1);
    let st = &net.storage[0];
    assert_eq!(st.bus, BusId(1));
    assert_eq!(st.energy_rating, 100.0);
    assert_eq!(st.charge_rating, 25.0);
    assert_eq!(st.discharge_rating, 25.0);
    assert_eq!(st.charge_efficiency, 0.91);
    assert_eq!(st.discharge_efficiency, 0.92);
    assert_eq!(st.ps, 3.0);
    assert_eq!(st.qs, 1.5);
    assert_eq!(st.energy, 20.0);
    assert!(!st.in_service);
}

/// Build a minimal pandapower JSON net: a `_object` map of split-orient frames
/// (each frame's payload JSON-string-encoded the way pandas writes them).
fn pp_json(tables: &[(&str, Value)]) -> String {
    let mut object = serde_json::Map::new();
    object.insert("sn_mva".into(), serde_json::json!(100.0));
    for (name, payload) in tables {
        object.insert(
            (*name).into(),
            serde_json::json!({ "_object": payload.to_string() }),
        );
    }
    serde_json::json!({ "_class": "pandapowerNet", "_object": object }).to_string()
}

fn pp_frame(columns: &[&str], index: &[usize], data: &Value) -> Value {
    serde_json::json!({ "columns": columns, "index": index, "data": data })
}

#[test]
fn pandapower_json_rejects_malformed_input() {
    let err = |text: &str| {
        powerio::parse_str(text, "pandapower-json")
            .unwrap_err()
            .to_string()
    };

    assert!(err(r#"{"_class":"NotANet","_object":{}}"#).contains("_class"));
    assert!(err(r#"{"_class":"pandapowerNet"}"#).contains("_object"));
    assert!(err(r#"{"_class":"pandapowerNet","_object":{}}"#).contains("bus"));
    // A frame whose `_object` payload is not valid JSON.
    assert!(
        err(r#"{"_class":"pandapowerNet","_object":{"bus":{"_object":"not json"}}}"#)
            .contains("bus")
    );

    // A load referencing a bus the bus table doesn't have.
    let dangling = pp_json(&[
        (
            "bus",
            pp_frame(&["vn_kv"], &[1], &serde_json::json!([[110.0]])),
        ),
        (
            "load",
            pp_frame(
                &["bus", "p_mw", "q_mvar"],
                &[1],
                &serde_json::json!([[7, 5.0, 2.0]]),
            ),
        ),
    ]);
    assert!(powerio::parse_str(&dangling, "pandapower-json").is_err());
}

#[test]
#[allow(clippy::float_cmp)]
fn pandapower_line_rating_sentinel_reads_as_unlimited() {
    let text = pp_json(&[
        (
            "bus",
            pp_frame(&["vn_kv"], &[1, 2], &serde_json::json!([[110.0], [110.0]])),
        ),
        (
            "line",
            pp_frame(
                &[
                    "from_bus",
                    "to_bus",
                    "r_ohm_per_km",
                    "x_ohm_per_km",
                    "length_km",
                    "max_i_ka",
                ],
                &[1, 2],
                &serde_json::json!([[1, 2, 1.0, 10.0, 1.0, 99999.0], [1, 2, 1.0, 10.0, 1.0, 1.0]]),
            ),
        ),
    ]);
    let net = powerio::parse_str(&text, "pandapower-json")
        .unwrap()
        .network;
    assert_eq!(
        net.branches[0].rate_a, 0.0,
        ">= 99999 kA is the unlimited sentinel"
    );
    let want = 110.0 * 3.0_f64.sqrt();
    assert!((net.branches[1].rate_a - want).abs() < 1e-9);
}

#[test]
#[allow(clippy::float_cmp)]
fn pandapower_writer_keeps_zero_rating_zero() {
    let mut net = parse_matpower_file(data("case9.m")).unwrap();
    net.branches[0].rate_a = 0.0;
    net.source = None; // force the canonical (non-echo) writer
    let conv = write_as(&net, TargetFormat::PandapowerJson).unwrap();
    let back = powerio::parse_str(&conv.text, "pandapower-json")
        .unwrap()
        .network;
    assert_eq!(back.branches[0].rate_a, 0.0);
    assert!(back.branches[1].rate_a > 0.0, "other ratings survive");
}

#[test]
fn pypsa_csv_folder_requires_buses_csv() {
    let dir = tmp_dir("pypsa-no-buses");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.csv"), "name,srid\nempty,4326\n").unwrap();
    let err = read_pypsa_csv_folder(&dir).unwrap_err().to_string();
    assert!(err.contains("buses.csv"), "got: {err}");
}

#[test]
fn pypsa_csv_quoted_fields_round_trip() {
    let mut net = parse_matpower_file(data("case9.m")).unwrap();
    net.buses[0].name = Some("weird, name\nwith \"newline\"".into());
    let dir = tmp_dir("pypsa-quoted-names");
    write_pypsa_csv_folder(&net, &dir).unwrap();
    let back = read_pypsa_csv_folder(&dir).unwrap().network;
    assert_eq!(back.buses.len(), net.buses.len());
    assert!(
        back.buses
            .iter()
            .any(|b| b.name.as_deref() == Some("weird, name\nwith \"newline\"")),
        "quoted name lost: {:?}",
        back.buses.iter().map(|b| &b.name).collect::<Vec<_>>()
    );
}

#[test]
fn parse_file_routes_pypsa_folders() {
    let net = parse_matpower_file(data("case9.m")).unwrap();
    let dir = tmp_dir("pypsa-parse-file-routing");
    write_pypsa_csv_folder(&net, &dir).unwrap();

    // Explicit format name, including alias spellings.
    for alias in ["pypsa", "PyPSA", "pypsa-csv", "pypsa_csv"] {
        let back = parse_file(&dir, Some(alias)).unwrap().network;
        assert_eq!(back.source_format, SourceFormat::PypsaCsv, "alias {alias}");
    }
    // No format: a directory with network.csv auto-detects as PyPSA.
    let back = parse_file(&dir, None).unwrap().network;
    assert_eq!(back.source_format, SourceFormat::PypsaCsv);
}

fn tmp_dir(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("powerio-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

fn tmp_path(label: &str, ext: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("powerio-{label}-{}.{}", std::process::id(), ext));
    let _ = std::fs::remove_file(&p);
    p
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
    assert_eq!(
        write_as(&net2, TargetFormat::PowerModelsJson).unwrap().text,
        json
    );
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

    let reparsed = parse_matpower(&write_as(&net, TargetFormat::Matpower).unwrap().text).unwrap();
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
    // egret/PSS-E/PowerWorld drop them, each with a warning.
    let net = parse_matpower_file(data("t_case9_dcline.m")).unwrap();
    assert!(!net.hvdc.is_empty(), "fixture should have dclines");

    let pm = write_powermodels_json(&net);
    assert!(
        pm.warnings.iter().any(|w| w.contains("dcline")),
        "PM should warn about dcline mapping"
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

    // Cross-format → MATPOWER also drops HVDC (the canonical writer emits no
    // dcline block), so it must warn too. `net` itself is MATPOWER-sourced, so
    // write_as would echo its source; convert through PowerModels first to reach
    // the canonical MATPOWER path with HVDC still present.
    assert_eq!(back.source_format, SourceFormat::PowerModelsJson);
    let to_mp = write_as(&back, TargetFormat::Matpower).unwrap();
    assert!(
        to_mp.warnings.iter().any(|w| w.contains("dcline")),
        "cross-format → MATPOWER should warn on dropped dclines, got {:?}",
        to_mp.warnings
    );
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
    let conv = write_as(&net, TargetFormat::Matpower).unwrap();
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
            d["f_bus"].as_u64() == Some(dc.from.0 as u64)
                && d["t_bus"].as_u64() == Some(dc.to.0 as u64)
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
fn parse_file_accepts_uppercase_and_mixed_case_extensions() {
    // Issue #97: .RAW / .Raw / .M / .JSON / .AUX must work identically to their
    // lowercase forms; extension detection is case-insensitive.
    let dir = std::env::temp_dir();

    let raw_src = std::fs::read_to_string(data("psse/case14.raw")).unwrap();
    for ext in ["RAW", "Raw", "rAw"] {
        let path = dir.join(format!("powerio_test_issue97.{ext}"));
        std::fs::write(&path, &raw_src).unwrap();
        let result = parse_file(&path, None);
        let _ = std::fs::remove_file(&path);
        let net = result
            .unwrap_or_else(|e| panic!(".{ext} extension should be accepted: {e}"))
            .network;
        assert_eq!(net.buses.len(), 14, ".{ext}: wrong bus count");
    }

    // One uppercase probe per remaining extension; the JSON goes through the
    // egret-vs-PowerModels sniff, the AUX through the PowerWorld reader.
    let net = parse_matpower_file(data("case14.m")).unwrap();
    let m_src = std::fs::read_to_string(data("case14.m")).unwrap();
    for (ext, src) in [
        ("M", m_src),
        ("JSON", write_powermodels_json(&net).text),
        ("AUX", write_powerworld(&net).text),
    ] {
        let path = dir.join(format!("powerio_test_issue97.{ext}"));
        std::fs::write(&path, &src).unwrap();
        let result = parse_file(&path, None);
        let _ = std::fs::remove_file(&path);
        let parsed = result
            .unwrap_or_else(|e| panic!(".{ext} extension should be accepted: {e}"))
            .network;
        assert_eq!(parsed.buses.len(), 14, ".{ext}: wrong bus count");
    }
}

#[test]
fn oos_fixture_marks_out_of_service_elements() {
    // t_case9_oos.m turns gen 2 and branch 5-6 out of service; the parse must carry
    // those in_service=false flags.
    // The fixture otherwise runs only in the Julia validators.
    let net = parse_matpower_file(data("t_case9_oos.m")).unwrap();
    assert_eq!(net.generators.iter().filter(|g| !g.in_service).count(), 1);
    let br = net
        .branches
        .iter()
        .find(|b| b.from == BusId(5) && b.to == BusId(6))
        .expect("branch 5-6");
    assert!(!br.in_service, "branch 5-6 is out of service");
}

// --- genuine third-party fixtures (tests/data/pandapower, tests/data/pypsa) ---

fn close(a: f64, b: f64) {
    assert!(
        (a - b).abs() <= 1e-9 * (1.0 + a.abs().max(b.abs())),
        "{a} vs {b}"
    );
}

#[test]
fn parse_file_dispatch_precedes_the_text_read() {
    // Format selection errors must be UnknownFormat, never the UTF-8 read
    // error a binary file would hit first: the .pwd display sibling ships
    // next to every case in the wild and gets its display API pointer, and an
    // unmapped extension errors without touching the file at all.
    let dir = std::env::temp_dir();

    let pwd = dir.join("powerio_test_dispatch.pwd");
    std::fs::write(&pwd, [0x32u8, 0, 0, 0, 0xff, 0xfe, 0x80, 0x81]).unwrap();
    let err = parse_file(&pwd, None).unwrap_err();
    let _ = std::fs::remove_file(&pwd);
    assert!(
        matches!(err, powerio::Error::UnknownFormat(_)),
        "pwd is UnknownFormat with a pointer, got: {err}"
    );
    assert!(err.to_string().contains("parse_display_file"), "{err}");

    // Unmapped extension: UnknownFormat even though the file does not exist,
    // because the extension settles the question before any read.
    let err = parse_file(dir.join("powerio_test_dispatch.xyz"), None).unwrap_err();
    assert!(
        matches!(err, powerio::Error::UnknownFormat(_)),
        "unmapped extension is UnknownFormat, got: {err}"
    );
}

#[test]
// Pass-through values compare exactly: a deviation means a column was misread.
#[allow(clippy::float_cmp)]
fn pandapower_genuine_fixture_reads() {
    // tests/data/pandapower/example.json was written by pandapower 3.2.2
    // (provenance in the directory README): example_simple() plus a storage
    // unit and a dcline. sn_mva = 1, f_hz = 50.
    let parsed = parse_file(data("pandapower/example.json"), None).unwrap();
    let net = &parsed.network;
    assert_eq!(net.source_format, SourceFormat::PandapowerJson);
    assert_eq!(net.base_mva, 1.0);

    // pandas index 0..=6 shifts to BusId 1..=7.
    let ids: Vec<usize> = net.buses.iter().map(|b| b.id.0).collect();
    assert_eq!(ids, (1..=7).collect::<Vec<_>>());
    assert_eq!(net.buses[0].name.as_deref(), Some("HV Busbar"));

    // ext_grid on pp bus 0 -> Ref; gen (slack=False) on pp bus 5 -> Pv; the
    // sgen on pp bus 6 is a PQ injection and leaves its bus kind alone.
    assert_eq!(net.buses[0].kind, BusType::Ref);
    assert_eq!(net.buses[5].kind, BusType::Pv);
    assert_eq!(net.buses[6].kind, BusType::Pq);

    // gen + ext_grid + sgen, in table order.
    assert_eq!(net.generators.len(), 3);
    let g = &net.generators[0];
    assert_eq!(g.bus, BusId(6));
    assert_eq!(g.pg, 6.0);
    assert_eq!(g.vg, 1.03);
    let ext = &net.generators[1];
    assert_eq!(ext.bus, BusId(1));
    assert_eq!(ext.vg, 1.02);
    assert_eq!(ext.pg, 0.0);
    let sgen = &net.generators[2];
    assert_eq!(sgen.bus, BusId(7));
    assert_eq!(sgen.pg, 2.0);
    assert_eq!(sgen.qg, -0.5);
    assert_eq!(sgen.pmax, 2.0); // max_p_mw absent -> defaults to p_mw

    // load scaling 0.6 applies to p and q.
    assert_eq!(net.loads.len(), 1);
    close(net.loads[0].p, 2.0 * 0.6);
    close(net.loads[0].q, 4.0 * 0.6);

    // shunt: q_mvar = -0.96 -> b = +0.96 (MATPOWER sign).
    assert_eq!(net.shunts.len(), 1);
    assert_eq!(net.shunts[0].bus, BusId(3));
    close(net.shunts[0].b, 0.96);

    // 4 lines + 1 trafo. The trafo reconstructs r/x from vkr/vk on the sn_mva
    // base: vk=12%, vkr=0.41%, sn=25, base=1.
    assert_eq!(net.branches.len(), 5);
    let xf = net
        .branches
        .iter()
        .find(|b| b.is_transformer())
        .expect("trafo");
    assert_eq!((xf.from, xf.to), (BusId(3), BusId(4)));
    let r = 0.41 * 1.0 / (25.0 * 100.0);
    let z = 12.0 * 1.0 / (25.0 * 100.0);
    close(xf.r, r);
    close(xf.x, (z * z - r * r).sqrt());
    assert_eq!(xf.rate_a, 25.0);
    assert_eq!(xf.shift, 150.0);
    // tap_pos == tap_neutral (0) on the hv side -> tap exactly 1.0, kept.
    assert_eq!(xf.tap, 1.0);

    // Line 1: 10 km at 110 kV. r/x/b all scale by length; b is
    // c_nf_per_km * length * 2*pi*f * zbase (pandapower build_branch).
    let l1 = net
        .branches
        .iter()
        .find(|b| (b.from, b.to) == (BusId(1), BusId(2)))
        .expect("Line 1");
    let zb = 110.0 * 110.0 / 1.0;
    close(l1.r, 0.06 * 10.0 / zb);
    close(l1.x, 0.144 * 10.0 / zb);
    close(
        l1.b,
        144.0e-9 * 10.0 * 2.0 * std::f64::consts::PI * 50.0 * zb,
    );

    // storage and dcline land on Network.storage / Network.hvdc.
    assert_eq!(net.storage.len(), 1);
    let st = &net.storage[0];
    assert_eq!(st.bus, BusId(7));
    assert_eq!(st.ps, 0.5);
    assert_eq!(st.qs, 0.1);
    assert_eq!(st.energy_rating, 2.0);
    assert_eq!(st.charge_rating, 0.5); // max_p_mw absent -> |ps|
    assert_eq!(st.discharge_rating, 0.5);
    assert_eq!(net.hvdc.len(), 1);
    let dc = &net.hvdc[0];
    assert_eq!((dc.from, dc.to), (BusId(4), BusId(5)));
    assert_eq!(dc.pf, 2.0);
    close(dc.pt, 2.0 - 0.05 - 2.0 * 1.0 / 100.0);
    assert_eq!(dc.loss0, 0.05);
    close(dc.loss1, 0.01);
    assert_eq!(dc.vf, 1.01);
    assert!(dc.pmax.is_infinite());

    // Exactly the 8-row switch table and the trafo magnetizing branch warn.
    assert_eq!(parsed.warnings.len(), 2, "{:?}", parsed.warnings);
    assert!(
        parsed.warnings.iter().any(|w| w
            == "`switch` table ignored (8 rows): switches are not modeled; open switches are not applied"),
        "{:?}",
        parsed.warnings
    );
    assert!(
        parsed.warnings.iter().any(|w| w
            == "`trafo`: i0_percent/pfe_kw nonzero on 1 rows; the magnetizing branch is not representable and was ignored"),
        "{:?}",
        parsed.warnings
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn pypsa_genuine_export_reads() {
    // tests/data/pypsa/example/ was written by PyPSA 1.2.2 export_to_csv_folder
    // (provenance in the directory README). A genuine export has no
    // powerio_base_mva column, so base_mva = 1. A directory with network.csv
    // parses as a PyPSA CSV folder without an explicit `from`.
    let parsed = parse_file(data("pypsa/example"), None).unwrap();
    let net = &parsed.network;
    assert_eq!(net.source_format, SourceFormat::PypsaCsv);
    assert_eq!(net.name, "example");
    assert_eq!(net.base_mva, 1.0);

    // Non-numeric names -> scheme B: positional ids, names kept.
    assert_eq!(net.buses.len(), 3);
    let names: Vec<_> = net.buses.iter().map(|b| b.name.as_deref()).collect();
    assert_eq!(names, [Some("north"), Some("south"), Some("east")]);
    assert_eq!(net.buses[0].id, BusId(1));
    assert_eq!(net.buses[2].base_kv, 20.0);

    // control Slack -> Ref, PV -> Pv, PQ leaves the bus alone.
    assert_eq!(net.buses[0].kind, BusType::Ref);
    assert_eq!(net.buses[1].kind, BusType::Pv);
    assert_eq!(net.buses[2].kind, BusType::Pq);

    assert_eq!(net.generators.len(), 3);
    let slack = &net.generators[0];
    assert_eq!(slack.bus, BusId(1));
    assert_eq!(slack.pg, 50.0);
    assert_eq!(slack.pmax, 120.0);
    let cost = slack.cost.as_ref().expect("marginal cost");
    assert_eq!(cost.coeffs, vec![0.04, 12.0, 0.0]);

    // Line l1 r/x are ohms -> per unit on zbase(110 kV, base 1); the
    // transformer t1 is per unit on its own s_nom = 60 and rebases by
    // base_mva / 60.
    assert_eq!(net.branches.len(), 2);
    let line = &net.branches[0];
    assert_eq!((line.from, line.to), (BusId(1), BusId(2)));
    let zb = 110.0 * 110.0 / 1.0;
    close(line.r, 0.5 / zb);
    close(line.x, 2.0 / zb);
    close(line.b, 1e-5 * zb);
    assert_eq!(line.rate_a, 100.0);
    let xf = &net.branches[1];
    assert!(xf.is_transformer());
    assert_eq!((xf.from, xf.to), (BusId(2), BusId(3)));
    close(xf.r, 0.01 * 1.0 / 60.0);
    close(xf.x, 0.1 * 1.0 / 60.0);
    assert_eq!(xf.rate_a, 60.0);
    assert_eq!(xf.tap, 1.05);

    assert_eq!(net.loads.len(), 1);
    assert_eq!(net.loads[0].bus, BusId(3));
    assert_eq!(net.loads[0].p, 40.0);

    // storage p_nom = 25, max_hours = 4.
    assert_eq!(net.storage.len(), 1);
    let st = &net.storage[0];
    assert_eq!(st.charge_rating, 25.0);
    assert_eq!(st.energy_rating, 100.0);
    assert_eq!(st.ps, 3.0);
    assert_eq!(st.energy, 20.0);

    // link -> Hvdc, with the one fidelity warning.
    assert_eq!(net.hvdc.len(), 1);
    let h = &net.hvdc[0];
    assert_eq!(h.pf, 10.0);
    close(h.pt, 9.7);
    assert_eq!(h.pmax, 30.0);

    assert_eq!(net.shunts.len(), 1);
    close(net.shunts[0].b, 1e-4 * zb * 1.0);

    assert_eq!(
        parsed.warnings,
        vec![
            "links.csv: 1 links read as HVDC lines; PyPSA links carry no reactive or voltage data (q limits 0, voltage setpoints 1.0)"
        ]
    );
}

#[test]
fn read_warnings_flow_through_every_channel() {
    let path = data("pandapower/example.json");
    let text = std::fs::read_to_string(&path).unwrap();

    // parse_file and parse_str report the same warnings for the same bytes.
    let from_file = parse_file(&path, None).unwrap();
    assert!(
        from_file.warnings.iter().any(|w| w.contains("`switch`")),
        "{:?}",
        from_file.warnings
    );
    let from_str = powerio::parse_str(&text, "pandapower-json").unwrap();
    assert_eq!(from_file.warnings, from_str.warnings);

    // A total reader yields no warnings.
    assert!(
        parse_file(data("case9.m"), None)
            .unwrap()
            .warnings
            .is_empty()
    );

    // convert_file folds read warnings in front of write warnings...
    let conv = convert_file(&path, TargetFormat::Matpower, None).unwrap();
    assert!(
        conv.warnings.iter().any(|w| w.contains("`switch`")),
        "{:?}",
        conv.warnings
    );

    // ...except on an echo to the same format, which reproduces the source
    // bytes and carries no fidelity loss.
    let echo = convert_file(&path, TargetFormat::PandapowerJson, None).unwrap();
    assert_eq!(echo.text, text);
    assert!(echo.warnings.is_empty(), "{:?}", echo.warnings);
}

#[test]
fn pypsa_empty_folder_rejected_via_parse_file() {
    let dir = tmp_dir("pypsa-empty-folder");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("buses.csv"), "name,v_nom\n").unwrap();
    let err = parse_file(&dir, Some("pypsa-csv")).unwrap_err();
    assert!(
        matches!(
            &err,
            Error::FormatRead { format, message }
                if *format == "PyPSA CSV" && message.contains("case has no buses")
        ),
        "expected the PyPSA no-buses error, got {err:?}"
    );
}

// Knock the 4-7 transformer and the bus 9 shunt out of service so case14
// carries transformers, a shunt, and out-of-service elements in one case,
// exercising the OOS transformer and OOS shunt write paths.
fn knock_out_case14(net: &mut Network) {
    let xf = net
        .branches
        .iter_mut()
        .find(|b| b.from == BusId(4) && b.to == BusId(7))
        .expect("case14 branch 4-7");
    assert!(xf.is_transformer(), "branch 4-7 is a transformer");
    xf.in_service = false;
    let sh = net
        .shunts
        .iter_mut()
        .find(|s| s.bus == BusId(9))
        .expect("case14 bus 9 shunt");
    sh.in_service = false;
}

#[test]
fn pandapower_json_round_trips_transformers_shunts_and_oos() {
    // case14 carries transformers (explicit taps) and a shunt, with one
    // transformer and the shunt knocked out of service; t_case9_oos an
    // out-of-service generator and line. All must survive the canonical
    // pandapower write -> read.
    for case in ["case14.m", "t_case9_oos.m"] {
        let mut net = parse_matpower_file(data(case)).unwrap();
        if case == "case14.m" {
            knock_out_case14(&mut net);
        }
        let conv = write_as(&net, TargetFormat::PandapowerJson).unwrap();
        let back = powerio::parse_str(&conv.text, "pandapower-json")
            .unwrap()
            .network;
        assert_eq!(core(&back), core(&net), "{case}");
        assert_eq!(
            back.branches.iter().filter(|b| b.is_transformer()).count(),
            net.branches.iter().filter(|b| b.is_transformer()).count(),
            "{case}: transformer count"
        );
        // The writers split lines and transformers into separate tables, so
        // branch order changes; compare keyed by endpoints (no parallel
        // branches in these cases).
        for rb in &net.branches {
            let b = back
                .branches
                .iter()
                .find(|b| b.from == rb.from && b.to == rb.to)
                .unwrap_or_else(|| panic!("{case}: branch {:?}-{:?} lost", rb.from, rb.to));
            close(b.effective_tap(), rb.effective_tap());
            assert_eq!(b.in_service, rb.in_service, "{case}: branch in_service");
        }
        for (g, rg) in back.generators.iter().zip(&net.generators) {
            assert_eq!(g.in_service, rg.in_service, "{case}: gen in_service");
        }
        for (s, rs) in back.shunts.iter().zip(&net.shunts) {
            assert_eq!(s.bus, rs.bus, "{case}: shunt bus");
            assert_eq!(s.in_service, rs.in_service, "{case}: shunt in_service");
        }
    }
}

#[test]
fn pypsa_csv_round_trips_transformers_shunts_and_oos() {
    for case in ["case14.m", "t_case9_oos.m"] {
        let mut net = parse_matpower_file(data(case)).unwrap();
        if case == "case14.m" {
            knock_out_case14(&mut net);
        }
        let out = tmp_dir(&format!("pypsa-rt-{case}"));
        let written = write_pypsa_csv_folder(&net, &out).unwrap();
        assert!(
            !written.warnings.iter().any(|w| w.contains("storage")),
            "{case}: {:?}",
            written.warnings
        );
        let back = read_pypsa_csv_folder(&out).unwrap().network;
        assert_eq!(core(&back), core(&net), "{case}");
        assert_eq!(
            back.branches.iter().filter(|b| b.is_transformer()).count(),
            net.branches.iter().filter(|b| b.is_transformer()).count(),
            "{case}: transformer count"
        );
        // The writers split lines and transformers into separate tables, so
        // branch order changes; compare keyed by endpoints (no parallel
        // branches in these cases).
        for rb in &net.branches {
            let b = back
                .branches
                .iter()
                .find(|b| b.from == rb.from && b.to == rb.to)
                .unwrap_or_else(|| panic!("{case}: branch {:?}-{:?} lost", rb.from, rb.to));
            close(b.effective_tap(), rb.effective_tap());
            assert_eq!(b.in_service, rb.in_service, "{case}: branch in_service");
        }
        for (g, rg) in back.generators.iter().zip(&net.generators) {
            assert_eq!(g.in_service, rg.in_service, "{case}: gen in_service");
        }
        for (s, rs) in back.shunts.iter().zip(&net.shunts) {
            assert_eq!(s.bus, rs.bus, "{case}: shunt bus");
            assert_eq!(s.in_service, rs.in_service, "{case}: shunt in_service");
        }
    }
}

#[test]
fn gen_costs_round_trip_through_pandapower_json() {
    // case9 costs are quadratic [c2, c1, c0] = [0.11, 5.0, 150.0]; poly_cost
    // must carry all three back without reordering (a cp0/cp2 swap fails here).
    let net = parse_matpower_file(data("case9.m")).unwrap();
    let conv = write_as(&net, TargetFormat::PandapowerJson).unwrap();
    let back = powerio::parse_str(&conv.text, "pandapower-json")
        .unwrap()
        .network;
    assert_eq!(back.generators.len(), net.generators.len());
    for (g, rg) in back.generators.iter().zip(&net.generators) {
        let got = &g.cost.as_ref().expect("cost survives").coeffs;
        let want = &rg.cost.as_ref().unwrap().coeffs;
        assert_eq!(got.len(), 3);
        for (a, b) in got.iter().zip(want) {
            close(*a, *b);
        }
    }
}

#[test]
fn gen_costs_round_trip_through_pypsa_csv() {
    // PyPSA carries marginal_cost (c1) and marginal_cost_quadratic (c2) only;
    // the constant term comes back as 0.
    let net = parse_matpower_file(data("case9.m")).unwrap();
    let out = tmp_dir("pypsa-costs");
    write_pypsa_csv_folder(&net, &out).unwrap();
    let back = read_pypsa_csv_folder(&out).unwrap().network;
    for (g, rg) in back.generators.iter().zip(&net.generators) {
        let got = &g.cost.as_ref().expect("cost survives").coeffs;
        let want = &rg.cost.as_ref().unwrap().coeffs;
        assert_eq!(got.len(), 3);
        close(got[0], want[0]); // c2
        close(got[1], want[1]); // c1
        close(got[2], 0.0); // c0 is not representable
    }
}

#[test]
fn parse_str_rejects_malformed_pandapower_frames() {
    // Regression: a frame whose index is missing (or shorter than data) used to
    // panic in Row::index_usize; malformed columns/rows used to coerce to an
    // empty table and default every field.
    let frame_no_index = r#"{"_module":"pandas.core.frame","_class":"DataFrame",
        "_object":"{\"columns\":[\"vn_kv\"],\"data\":[[110.0]]}",
        "orient":"split","is_multiindex":false,"is_multicolumn":false}"#;
    let text = format!(
        r#"{{"_module":"pandapower.auxiliary","_class":"pandapowerNet",
            "_object":{{"sn_mva":100.0,"bus":{frame_no_index}}}}}"#
    );
    let err = powerio::parse_str(&text, "pandapower-json")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("`bus` table: index length 0 does not match data length 1"),
        "{err}"
    );

    let frame_bad_columns = r#"{"_module":"pandas.core.frame","_class":"DataFrame",
        "_object":"{\"columns\":[1,2],\"index\":[0],\"data\":[[110.0,true]]}",
        "orient":"split","is_multiindex":false,"is_multicolumn":false}"#;
    let text = format!(
        r#"{{"_module":"pandapower.auxiliary","_class":"pandapowerNet",
            "_object":{{"sn_mva":100.0,"bus":{frame_bad_columns}}}}}"#
    );
    let err = powerio::parse_str(&text, "pandapower-json")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("`bus` table: column names must be strings"),
        "{err}"
    );

    // Huge indices must be a parse error, not a saturating cast or an
    // overflowing `pp_idx + 1`.
    for huge in ["1e30", "18446744073709551615"] {
        let frame_huge_index = format!(
            r#"{{"_module":"pandas.core.frame","_class":"DataFrame",
            "_object":"{{\"columns\":[\"vn_kv\"],\"index\":[{huge}],\"data\":[[110.0]]}}",
            "orient":"split","is_multiindex":false,"is_multicolumn":false}}"#
        );
        let text = format!(
            r#"{{"_module":"pandapower.auxiliary","_class":"pandapowerNet",
                "_object":{{"sn_mva":100.0,"bus":{frame_huge_index}}}}}"#
        );
        let err = powerio::parse_str(&text, "pandapower-json")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("index is not a non-negative integer"),
            "index {huge}: {err}"
        );
    }
}

#[test]
fn pypsa_written_folder_joins_on_bus_names() {
    // The PyPSA import contract: every element bus column must match a
    // buses.csv name key exactly, or PyPSA rejects the folder. Write the
    // genuine fixture with named buses back out and check the join.
    let net = parse_file(data("pypsa/example"), None).unwrap().network;
    let out = tmp_dir("pypsa-named-write");
    write_pypsa_csv_folder(&net, &out).unwrap();

    let column = |file: &str, name: &str| -> Vec<String> {
        let text =
            std::fs::read_to_string(out.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        let mut lines = text.lines();
        let headers: Vec<&str> = lines.next().unwrap().split(',').collect();
        let col = headers
            .iter()
            .position(|h| *h == name)
            .unwrap_or_else(|| panic!("{file}: no column {name}"));
        lines
            .map(|l| l.split(',').nth(col).unwrap().to_string())
            .collect()
    };

    let keys: std::collections::BTreeSet<String> =
        column("buses.csv", "name").into_iter().collect();
    assert_eq!(
        keys.iter().map(String::as_str).collect::<Vec<_>>(),
        ["east", "north", "south"]
    );
    for (file, cols) in [
        ("generators.csv", vec!["bus"]),
        ("loads.csv", vec!["bus"]),
        ("lines.csv", vec!["bus0", "bus1"]),
        ("transformers.csv", vec!["bus0", "bus1"]),
        ("shunt_impedances.csv", vec!["bus"]),
        ("storage_units.csv", vec!["bus"]),
    ] {
        for col in cols {
            for v in column(file, col) {
                assert!(keys.contains(&v), "{file} {col}: `{v}` not in buses.csv");
            }
        }
    }

    // And the folder reads back with the references resolved by name.
    let back = read_pypsa_csv_folder(&out).unwrap().network;
    assert_eq!(back.loads[0].bus, back.buses[2].id);
}

#[test]
fn slackless_network_conversion_warns_for_power_flow_targets() {
    use powerio::network::{Branch, Bus, BusType, Extras, Network};
    fn bus(id: usize, kind: BusType) -> Bus {
        Bus {
            id: BusId(id),
            kind,
            vm: 1.0,
            va: 0.0,
            base_kv: 1.0,
            vmax: 1.1,
            vmin: 0.9,
            area: 1,
            zone: 1,
            name: None,
            extras: Extras::new(),
        }
    }
    fn branch(from: usize, to: usize) -> Branch {
        Branch {
            from: BusId(from),
            to: BusId(to),
            r: 0.0,
            x: 0.1,
            b: 0.0,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
            tap: 0.0,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            extras: Extras::new(),
        }
    }
    // PowerWorld .pwb stores no slack designation; converting its network to
    // a format whose solvers need one must say so instead of silently
    // emitting a case every power flow tool rejects.
    let net = Network::in_memory(
        "noslack",
        100.0,
        vec![bus(1, BusType::Pv), bus(2, BusType::Pq)],
        vec![branch(1, 2)],
    );
    for fmt in [
        TargetFormat::Matpower,
        TargetFormat::Psse,
        TargetFormat::PowerModelsJson,
    ] {
        let conv = write_as(&net, fmt).unwrap();
        assert!(
            conv.warnings
                .iter()
                .any(|w| w.contains("reference (slack) bus")),
            "{fmt:?} missing the slackless warning: {:?}",
            conv.warnings
        );
    }
    // A network with a slack stays warning free on this dimension.
    let with_ref = Network::in_memory(
        "slack",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 2)],
    );
    assert!(
        !write_as(&with_ref, TargetFormat::Matpower)
            .unwrap()
            .warnings
            .iter()
            .any(|w| w.contains("reference (slack) bus"))
    );
}

#[test]
fn snapshot_warns_on_non_finite_and_does_not_read_back() {
    // JSON has no Inf/NaN: serde writes them as `null`, which the validating
    // reader rejects. Readers legitimately produce Inf limits and the bindings
    // materialize every network through the snapshot, so the write stays total,
    // but it must SAY what degraded (naming the field), and the no-read-back
    // consequence is pinned here so a change to either side surfaces.
    let mut net = parse_matpower_file(data("case9.m")).unwrap();
    net.branches[2].angmax = f64::INFINITY;
    let conv = write_as(&net, TargetFormat::PowerioJson).unwrap();
    assert!(
        conv.warnings
            .iter()
            .any(|w| w.contains("branches[2].angmax")),
        "the degradation warning should name the field: {:?}",
        conv.warnings
    );
    let err = powerio::parse_str(&conv.text, "powerio-json")
        .expect_err("a null-degraded snapshot must not validate");
    assert!(err.to_string().contains("null"), "got: {err}");

    // A NaN bus voltage warns the same way.
    let mut net = parse_matpower_file(data("case9.m")).unwrap();
    net.buses[0].vm = f64::NAN;
    let conv = write_as(&net, TargetFormat::PowerioJson).unwrap();
    assert!(
        conv.warnings.iter().any(|w| w.contains("buses[0].vm")),
        "got: {:?}",
        conv.warnings
    );

    // EVERY non-finite field is named, not just the first: serde writes them all
    // as null, so a caller fixing only the first-reported field would re-export
    // and still fail to read back. Both offenders must appear in one write.
    let mut net = parse_matpower_file(data("case9.m")).unwrap();
    net.buses[0].vm = f64::NAN;
    net.branches[2].angmax = f64::INFINITY;
    let conv = write_as(&net, TargetFormat::PowerioJson).unwrap();
    assert!(
        conv.warnings.iter().any(|w| w.contains("buses[0].vm"))
            && conv
                .warnings
                .iter()
                .any(|w| w.contains("branches[2].angmax")),
        "both non-finite fields must be named in one write: {:?}",
        conv.warnings
    );
}

#[test]
fn snapshot_round_trips_through_core_api() {
    // write_as -> parse_str at the core level (the C ABI test covers the same
    // path over FFI). case30 carries loads, shunts, and gen costs.
    let net = parse_matpower_file(data("case30.m")).unwrap();
    let conv = write_as(&net, TargetFormat::PowerioJson).unwrap();
    assert!(conv.warnings.is_empty(), "the snapshot writes no warnings");
    let parsed = powerio::parse_str(&conv.text, "powerio-json").unwrap();
    assert!(parsed.warnings.is_empty(), "the snapshot reads back total");
    let back = parsed.network;
    assert_eq!(back.buses.len(), net.buses.len());
    assert_eq!(back.branches.len(), net.branches.len());
    assert_eq!(back.generators.len(), net.generators.len());
    // Bit-exact: the snapshot is lossless, so even the sign of a zero survives.
    assert_eq!(back.base_mva.to_bits(), net.base_mva.to_bits());
    assert_eq!(back.source_format, net.source_format);
}

#[test]
fn snapshot_json_file_is_sniffed_without_a_format_hint() {
    // A snapshot written to disk carries the generic .json extension; the
    // sniffer must route it to the powerio-json reader (top level `buses`),
    // not the PowerModels fallback, so parse_file works with from=None.
    let net = parse_matpower_file(data("case14.m")).unwrap();
    let text = write_as(&net, TargetFormat::PowerioJson).unwrap().text;
    let path = std::env::temp_dir().join(format!(
        "powerio_snapshot_sniff_{}.json",
        std::process::id()
    ));
    std::fs::write(&path, &text).unwrap();
    let parsed = parse_file(&path, None);
    std::fs::remove_file(&path).ok();
    let back = parsed.unwrap().network;
    assert_eq!(back.buses.len(), 14);
    assert_eq!(back.source_format, SourceFormat::Matpower);
}

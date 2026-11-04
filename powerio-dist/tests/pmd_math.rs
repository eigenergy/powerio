//! The PMD MATHEMATICAL model against PowerModelsDistribution itself: the
//! writer reproduces `transform_data_model` on the vendored ENGINEERING
//! exports value by value, the reader folds PMD's own MATHEMATICAL exports
//! back to the network their ENGINEERING source gives, and the canonical
//! write is byte stable.

mod helpers;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use helpers::{Parsed, parse_file};
use powerio_dist::{DistSourceFormat, DistTargetFormat, DistWindingConn};
use serde_json::Value;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist/pmd")
        .join(rel)
}

/// The vendored (ENGINEERING, MATHEMATICAL) export pairs.
const PAIRS: [(&str, &str); 5] = [
    ("case3_balanced.json", "math/case3_balanced_math.json"),
    (
        "math/case3_balanced_cap_eng.json",
        "math/case3_balanced_cap_math.json",
    ),
    (
        "math/case3_unbalanced_switch_eng.json",
        "math/case3_unbalanced_switch_math.json",
    ),
    (
        "math/case3_unbalanced_ZIPloads_eng.json",
        "math/case3_unbalanced_ZIPloads_math.json",
    ),
    (
        "math/ut_trans_2w_yy_eng.json",
        "math/ut_trans_2w_yy_math.json",
    ),
];

fn parse_eng(rel: &str) -> Parsed {
    parse_file(fixture(rel), Some("pmd-json")).unwrap()
}

fn parse_math(rel: &str) -> Parsed {
    parse_file(fixture(rel), Some("pmd-math-json")).unwrap()
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// Two numbers agree to a relative 1e-8, `null` standing for an infinite
/// bound on both sides.
fn close(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (None, None) => a.is_null() && b.is_null(),
        (Some(x), Some(y)) => (x - y).abs() <= 1e-8 * x.abs().max(y.abs()).max(1e-12),
        _ => false,
    }
}

/// Deep numeric agreement: arrays element by element, numbers by `close`,
/// everything else by equality.
fn agree(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| agree(p, q))
        }
        (Value::Number(_) | Value::Null, Value::Number(_) | Value::Null) => close(a, b),
        _ => a == b,
    }
}

/// A MATHEMATICAL document indexed by object name within each table.
struct Math {
    doc: Value,
}

impl Math {
    fn table(&self, table: &str) -> BTreeMap<String, &Value> {
        self.doc[table]
            .as_object()
            .map(|t| {
                t.values()
                    .map(|v| (v["name"].as_str().unwrap_or_default().to_owned(), v))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The name of the bus an object field indexes.
    fn bus_name(&self, index: &Value) -> String {
        let key = index.as_i64().map(|i| i.to_string()).unwrap_or_default();
        self.doc["bus"][&key]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }
}

/// Compare the named fields of every object in `table`, matching by name,
/// and the bus references by the referenced bus's name.
fn compare_table(
    ours: &Math,
    theirs: &Math,
    table: &str,
    fields: &[&str],
    bus_fields: &[&str],
    label: &str,
) -> Vec<String> {
    let mut problems = Vec::new();
    let a = ours.table(table);
    let b = theirs.table(table);
    let names_a: BTreeSet<&String> = a.keys().collect();
    let names_b: BTreeSet<&String> = b.keys().collect();
    if names_a != names_b {
        problems.push(format!(
            "{label}: {table} names differ: ours {:?}, PMD {:?}",
            names_a.difference(&names_b).collect::<Vec<_>>(),
            names_b.difference(&names_a).collect::<Vec<_>>()
        ));
    }
    for name in names_a.intersection(&names_b) {
        let (x, y) = (a[*name], b[*name]);
        for field in fields {
            if !agree(&x[*field], &y[*field]) {
                problems.push(format!(
                    "{label}: {table} {name}.{field}: ours {} vs PMD {}",
                    x[*field], y[*field]
                ));
            }
        }
        for field in bus_fields {
            let (ba, bb) = (ours.bus_name(&x[*field]), theirs.bus_name(&y[*field]));
            if ba != bb {
                problems.push(format!(
                    "{label}: {table} {name}.{field}: bus {ba} vs PMD {bb}"
                ));
            }
        }
    }
    problems
}

#[test]
#[allow(clippy::too_many_lines)] // every ENGINEERING field the reader recovers, checked against the PMD export
fn writer_matches_pmd_transform_data_model() {
    let mut problems = Vec::new();
    for (eng, math) in PAIRS {
        let net = parse_eng(eng);
        let emitted = net.emit_value(DistTargetFormat::PmdMathJson);
        let ours = Math {
            doc: json(&emitted.text),
        };
        let theirs = Math {
            doc: json(&std::fs::read_to_string(fixture(math)).unwrap()),
        };
        let label = eng;
        if !close(
            &ours.doc["settings"]["sbase"],
            &theirs.doc["settings"]["sbase"],
        ) {
            problems.push(format!(
                "{label}: settings.sbase {} vs {}",
                ours.doc["settings"]["sbase"], theirs.doc["settings"]["sbase"]
            ));
        }
        if ours.doc["is_kron_reduced"] != theirs.doc["is_kron_reduced"] {
            problems.push(format!(
                "{label}: is_kron_reduced {} vs {}",
                ours.doc["is_kron_reduced"], theirs.doc["is_kron_reduced"]
            ));
        }
        if ours.doc["conductor_ids"] != theirs.doc["conductor_ids"] {
            problems.push(format!(
                "{label}: conductor_ids {} vs {}",
                ours.doc["conductor_ids"], theirs.doc["conductor_ids"]
            ));
        }
        problems.extend(compare_table(
            &ours,
            &theirs,
            "bus",
            &[
                "vbase",
                "terminals",
                "grounded",
                "vmin",
                "vmax",
                "bus_type",
                "vm",
                "va",
                "source_id",
            ],
            &[],
            label,
        ));
        problems.extend(compare_table(
            &ours,
            &theirs,
            "branch",
            &[
                "vbase",
                "f_connections",
                "t_connections",
                "br_r",
                "br_x",
                "g_fr",
                "g_to",
                "b_fr",
                "b_to",
                "angmin",
                "angmax",
                "c_rating_a",
                "rate_a",
                "br_status",
                "source_id",
            ],
            &["f_bus", "t_bus"],
            label,
        ));
        problems.extend(compare_table(
            &ours,
            &theirs,
            "transformer",
            &[
                "tm_nom",
                "tm_set",
                "tm_fix",
                "tm_lb",
                "tm_ub",
                "tm_step",
                "configuration",
                "polarity",
                "sm_ub",
                "status",
                "f_vbase",
                "t_vbase",
                "f_connections",
                "t_connections",
                "source_id",
            ],
            &["f_bus", "t_bus"],
            label,
        ));
        problems.extend(compare_table(
            &ours,
            &theirs,
            "gen",
            &[
                "vbase",
                "pg",
                "qg",
                "pmin",
                "pmax",
                "qmin",
                "qmax",
                "vg",
                "cost",
                "model",
                "ncost",
                "control_mode",
                "configuration",
                "connections",
                "gen_status",
                "source_id",
            ],
            &["gen_bus"],
            label,
        ));
        problems.extend(compare_table(
            &ours,
            &theirs,
            "load",
            &[
                "vbase",
                "pd",
                "qd",
                "vnom_kv",
                "model",
                "configuration",
                "connections",
                "status",
                "dispatchable",
                "source_id",
            ],
            &["load_bus"],
            label,
        ));
        problems.extend(compare_table(
            &ours,
            &theirs,
            "shunt",
            &[
                "vbase",
                "gs",
                "bs",
                "connections",
                "status",
                "dispatchable",
                "source_id",
            ],
            &["shunt_bus"],
            label,
        ));
        problems.extend(compare_table(
            &ours,
            &theirs,
            "switch",
            &[
                "vbase",
                "state",
                "dispatchable",
                "status",
                "current_rating",
                "f_connections",
                "t_connections",
                "source_id",
            ],
            &["f_bus", "t_bus"],
            label,
        ));
        // The map: the same ENGINEERING objects, each becoming the same kinds
        // of MATHEMATICAL objects.
        let map_summary = |m: &Math| -> BTreeSet<(String, String, Vec<String>)> {
            m.doc["map"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    let to = match &e["to"] {
                        Value::String(s) => vec![s.split('.').next().unwrap().to_owned()],
                        Value::Array(a) => {
                            let mut kinds: Vec<String> = a
                                .iter()
                                .map(|s| s.as_str().unwrap().split('.').next().unwrap().to_owned())
                                .collect();
                            kinds.sort();
                            kinds
                        }
                        _ => Vec::new(),
                    };
                    (
                        e["from"].as_str().unwrap_or_default().to_owned(),
                        e["unmap_function"].as_str().unwrap().to_owned(),
                        to,
                    )
                })
                .collect()
        };
        let (ma, mb) = (map_summary(&ours), map_summary(&theirs));
        if ma != mb {
            problems.push(format!(
                "{label}: map differs: ours only {:?}; PMD only {:?}",
                ma.difference(&mb).collect::<Vec<_>>(),
                mb.difference(&ma).collect::<Vec<_>>()
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Ohms of a winding's resistance and of the first pair's leakage reactance:
/// base free, so the two readers' bases need not match.
fn transformer_ohms(t: &powerio_dist::DistTransformer) -> (Vec<f64>, f64) {
    let r: Vec<f64> = t
        .windings
        .iter()
        .map(|w| w.r_pct / 100.0 * w.v_ref * w.v_ref / w.s_rating)
        .collect();
    let w1 = &t.windings[0];
    let x = t.xsc_pct.first().copied().unwrap_or(0.0) / 100.0 * w1.v_ref * w1.v_ref / w1.s_rating;
    (r, x)
}

fn rel(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-8 * a.abs().max(b.abs()).max(1e-12)
}

#[test]
#[allow(clippy::too_many_lines)] // one oracle comparison per PMD table, with the tolerances each needs
fn reader_recovers_the_engineering_network_from_pmd_exports() {
    for (eng, math) in PAIRS {
        let e = parse_eng(eng);
        let m = parse_math(math);
        let label = math;
        assert_eq!(
            *m.source_format(),
            Some(DistSourceFormat::PmdMathJson),
            "{label}"
        );
        // Buses: the same ids; the Kron reduced document has no neutral.
        let ids = |net: &powerio_dist::MulticonductorNetwork| -> BTreeSet<String> {
            net.buses()
                .iter()
                .map(|b| b.id.to_ascii_lowercase())
                .collect()
        };
        assert_eq!(ids(&m), ids(&e), "{label}: bus ids");
        // Lines: total series impedance.
        for line in e.lines() {
            let other = m
                .lines()
                .iter()
                .find(|l| l.name == line.name)
                .unwrap_or_else(|| panic!("{label}: line {}", line.name));
            let (ce, cm) = (
                e.linecode(&line.linecode).unwrap(),
                m.linecode(&other.linecode).unwrap(),
            );
            assert!(
                rel(
                    ce.r_series[0][0] * line.length,
                    cm.r_series[0][0] * other.length
                ),
                "{label}: line {} r",
                line.name
            );
            assert!(
                rel(
                    ce.x_series[0][0] * line.length,
                    cm.x_series[0][0] * other.length
                ),
                "{label}: line {} x",
                line.name
            );
            assert_eq!(
                other.bus_from.to_ascii_lowercase(),
                line.bus_from.to_ascii_lowercase(),
                "{label}: line {} from",
                line.name
            );
        }
        // Loads: the same powers, voltages, and laws.
        for load in e.loads() {
            let other = m
                .loads()
                .iter()
                .find(|l| l.name == load.name)
                .unwrap_or_else(|| panic!("{label}: load {}", load.name));
            let total = |v: &[f64]| v.iter().sum::<f64>();
            assert!(
                rel(total(&load.p_nom), total(&other.p_nom)),
                "{label}: load {} p {} vs {}",
                load.name,
                total(&load.p_nom),
                total(&other.p_nom)
            );
            assert!(
                rel(total(&load.q_nom), total(&other.q_nom)),
                "{label}: load {} q",
                load.name
            );
            assert_eq!(
                std::mem::discriminant(&load.voltage_model),
                std::mem::discriminant(&other.voltage_model),
                "{label}: load {} law",
                load.name
            );
            if let (Some(a), Some(b)) = (
                load.voltage_model.v_nom().first(),
                other.voltage_model.v_nom().first(),
            ) {
                assert!(rel(*a, *b), "{label}: load {} v_nom {a} vs {b}", load.name);
            }
        }
        // Transformers: voltages, connections, and ohms.
        for t in e.transformers() {
            let other = m
                .transformers()
                .iter()
                .find(|x| x.name == t.name)
                .unwrap_or_else(|| panic!("{label}: transformer {}", t.name));
            assert_eq!(
                t.windings.len(),
                other.windings.len(),
                "{label}: {} windings",
                t.name
            );
            for (a, b) in t.windings.iter().zip(&other.windings) {
                assert!(
                    rel(a.v_ref, b.v_ref),
                    "{label}: {} v_ref {} vs {}",
                    t.name,
                    a.v_ref,
                    b.v_ref
                );
                assert_eq!(a.conn, b.conn, "{label}: {} conn", t.name);
                assert_eq!(
                    a.terminal_map, b.terminal_map,
                    "{label}: {} winding terminals",
                    t.name
                );
                assert_eq!(
                    a.bus.to_ascii_lowercase(),
                    b.bus.to_ascii_lowercase(),
                    "{label}: {} bus",
                    t.name
                );
                assert!(rel(a.tap, b.tap), "{label}: {} tap", t.name);
            }
            let ((ra, xa), (rb, xb)) = (transformer_ohms(t), transformer_ohms(other));
            for (k, (p, q)) in ra.iter().zip(&rb).enumerate() {
                assert!(
                    rel(*p, *q),
                    "{label}: {} winding {k} r ohm {p} vs {q}",
                    t.name
                );
            }
            assert!(rel(xa, xb), "{label}: {} leakage ohm {xa} vs {xb}", t.name);
            assert_eq!(
                other.windings[0].conn == DistWindingConn::Wye,
                t.windings[0].conn == DistWindingConn::Wye
            );
        }
        // Sources: the first phase magnitude and angle.
        for s in e.sources() {
            let other = m
                .sources()
                .iter()
                .find(|x| x.name == s.name)
                .unwrap_or_else(|| panic!("{label}: source {}", s.name));
            assert!(
                rel(s.v_magnitude[0], other.v_magnitude[0]),
                "{label}: source {} vm {} vs {}",
                s.name,
                s.v_magnitude[0],
                other.v_magnitude[0]
            );
            assert!(
                (s.v_angle[0] - other.v_angle[0]).abs() < 1e-9,
                "{label}: source {} va",
                s.name
            );
            assert_eq!(
                s.bus.to_ascii_lowercase(),
                other.bus.to_ascii_lowercase(),
                "{label}: source {} bus",
                s.name
            );
        }
        for sh in e.shunts() {
            let other = m
                .shunts()
                .iter()
                .find(|x| x.name == sh.name)
                .unwrap_or_else(|| panic!("{label}: shunt {}", sh.name));
            assert!(
                rel(sh.b[0][0], other.b[0][0]),
                "{label}: shunt {} b {} vs {}",
                sh.name,
                sh.b[0][0],
                other.b[0][0]
            );
        }
        for sw in e.switches() {
            let other = m
                .switches()
                .iter()
                .find(|x| x.name == sw.name)
                .unwrap_or_else(|| panic!("{label}: switch {}", sw.name));
            assert_eq!(sw.open, other.open, "{label}: switch {} state", sw.name);
            assert_eq!(
                sw.bus_to.to_ascii_lowercase(),
                other.bus_to.to_ascii_lowercase(),
                "{label}: switch {} to",
                sw.name
            );
        }
        assert!(
            m.warnings
                .iter()
                .all(|w| w.contains("READ.PMD_MATH.") || w.contains("VALIDATE.")),
            "{label}: every finding carries a READ.PMD_MATH code: {:?}",
            m.warnings
        );
    }
}

#[test]
fn canonical_write_is_idempotent_and_echoes() {
    for (_, math) in PAIRS {
        let net = parse_math(math);
        // Same format: the retained bytes.
        assert_eq!(
            net.emit(DistTargetFormat::PmdMathJson).text,
            std::fs::read_to_string(fixture(math)).unwrap(),
            "{math}: echo"
        );
        let first = net.emit_value(DistTargetFormat::PmdMathJson);
        let back = helpers::parse_str(&first.text, "pmd-math-json")
            .unwrap_or_else(|e| panic!("{math}: reparse: {e}"));
        let second = back.emit_value(DistTargetFormat::PmdMathJson);
        assert_same_text(
            &first.text,
            &second.text,
            &format!("{math}: canonical write is not idempotent"),
        );
        assert!(
            first
                .warnings
                .iter()
                .all(|w| w.contains("EMIT.PMD_MATH.") || w.contains("TRANSFORM.")),
            "{math}: {:?}",
            first.warnings
        );
    }
}

#[test]
fn a_dss_feeder_lowers_and_reads_back() {
    let dss = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist/micro/xfmr_single_phase.dss");
    let net = parse_file(dss, None).unwrap();
    let math = net.emit_value(DistTargetFormat::PmdMathJson);
    let doc = json(&math.text);
    assert_eq!(doc["data_model"], "MATHEMATICAL");
    assert_eq!(doc["per_unit"], true);
    let back = helpers::parse_str(&math.text, "pmd-math-json").unwrap();
    assert_eq!(back.transformers().len(), net.transformers().len());
    assert_eq!(back.loads().len(), net.loads().len());
    assert_eq!(back.sources().len(), net.sources().len());
}

#[test]
fn the_engineering_reader_points_at_the_mathematical_format() {
    let text = std::fs::read_to_string(fixture("math/case3_balanced_math.json")).unwrap();
    let err = helpers::parse_str(&text, "pmd-json")
        .unwrap_err()
        .to_string();
    assert!(err.contains("pmd-math-json"), "{err}");
    assert_eq!(
        powerio_dist::classify_distribution_json(&text).unwrap(),
        DistTargetFormat::PmdMathJson
    );
    for alias in ["pmd-math-json", "pmd-math", "mathematical", "PMD_MATH_JSON"] {
        assert_eq!(
            powerio_dist::parse_dist_target_format(alias),
            Some(DistTargetFormat::PmdMathJson),
            "{alias}"
        );
    }
}

#[test]
fn kron_reduced_wye_transformer_keeps_its_grounded_return_path() {
    let net = parse_math("math/ut_trans_2w_yy_math.json");
    for winding in &net.transformers()[0].windings {
        assert_eq!(winding.terminal_map, ["1", "2", "3", "4"]);
        let bus = net.buses().iter().find(|b| b.id == winding.bus).unwrap();
        assert!(bus.grounded.iter().any(|t| t == "4"));
    }
    let output = net.emit_value(DistTargetFormat::BmopfJson);
    assert!(
        !output
            .warnings
            .iter()
            .any(|w| w.contains("TRANSFORMER_UNSUPPORTED"))
    );
    assert_eq!(
        json(&output.text)["transformer"]["single_phase"]
            .as_object()
            .unwrap()
            .len(),
        3
    );
}

/// A reduced extract of PMD 0.16.0's transform_data_model output for a
/// direct ideal unit with tm_nom=30, tm_set=[1.05], polarity=-1,
/// `vbases_default["hi"]=12`, `sbase_default=100`, `kron_reduce=false`.
fn direct_transformer_math() -> Value {
    serde_json::json!({
        "data_model": "MATHEMATICAL", "per_unit": true, "is_kron_reduced": false,
        "settings": {"sbase": 100.0, "voltage_scale_factor": 1000.0, "power_scale_factor": 1000.0},
        "bus": {
            "1": {"name": "lo", "vbase": 0.4, "terminals": [1,4], "grounded": [false,true]},
            "2": {"name": "hi", "vbase": 12.0, "terminals": [1,4], "grounded": [false,true]}
        },
        "transformer": {"1": {
            "index": 1, "name": "tx", "f_bus": 2, "t_bus": 1,
            "f_vbase": 12.0, "t_vbase": 0.4, "tm_nom": 1.0, "tm_set": [1.05],
            "tm_fix": [true], "polarity": -1, "sm_ub": 1.0,
            "configuration": "WYE", "f_connections": [1,4], "t_connections": [1,4]
        }},
        "map": [{"from": "tx", "to": ["transformer.1"], "unmap_function": "_map_math2eng_transformer!"}]
    })
}

#[test]
fn direct_transformer_ratio_tap_and_polarity_act_once() {
    for delta in [false, true] {
        let mut doc = direct_transformer_math();
        if delta {
            doc["transformer"]["1"]["configuration"] = serde_json::json!("DELTA");
            doc["transformer"]["1"]["f_connections"] = serde_json::json!([1, 2]);
            doc["bus"]["2"]["terminals"] = serde_json::json!([1, 2]);
            doc["bus"]["2"]["grounded"] = serde_json::json!([false, false]);
        }
        let net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
        let xf = &net.transformers()[0];
        let factor = if delta { 3f64.sqrt() } else { 1.0 };
        assert!(rel(xf.windings[0].v_ref * factor, 12_000.0));
        assert!(rel(xf.windings[1].v_ref, 400.0));
        assert_eq!(xf.windings[1].conn, DistWindingConn::Wye);
        assert_eq!(xf.windings[0].tap, 1.05);
        assert_eq!(xf.windings[1].tap, 1.0);
        assert_eq!(xf.extras["pmd_polarity"], serde_json::json!([-1, 1]));
        let output = net.emit_value(DistTargetFormat::PmdMathJson);
        let back = helpers::parse_str(&output.text, "pmd-math-json").unwrap();
        let recovered = &back.transformers()[0];
        assert!(rel(
            recovered.windings[0].v_ref / recovered.windings[1].v_ref,
            xf.windings[0].v_ref / xf.windings[1].v_ref
        ));
        assert_eq!(recovered.windings[0].tap, 1.05);
        assert_eq!(recovered.windings[1].tap, 1.0);
    }
}

#[test]
fn non_per_unit_mathematical_angles_are_degrees() {
    // PMD 0.16.0 transform_data_model(case3_balanced; make_pu=false)
    // emits the source bus va as [0,-120,120], unlike its per-unit radians.
    let doc = serde_json::json!({
        "data_model": "MATHEMATICAL", "per_unit": false,
        "settings": {"voltage_scale_factor": 1000.0, "power_scale_factor": 1000.0},
        "bus": {"1": {"name": "source", "terminals": [1,2,3],
            "grounded": [false,false,false], "vm": [0.23,0.23,0.23], "va": [0,-120,120]}},
        "gen": {"1": {"name": "source", "gen_bus": 1, "connections": [1,2,3], "control_mode": 1}},
        "map": [{"from": "source", "to": "gen.1", "unmap_function": "_map_math2eng_voltage_source!"}]
    });
    let net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    assert!(rel(net.sources()[0].v_angle[1], -120f64.to_radians()));
    let output = json(&net.emit_value(DistTargetFormat::PmdJson).text);
    assert!(rel(
        output["voltage_source"]["source"]["va"][1]
            .as_f64()
            .unwrap(),
        -120.0
    ));
}

#[test]
fn switch_conductance_survives_canonical_mathematical_emission() {
    let mut doc =
        json(&std::fs::read_to_string(fixture("math/case3_unbalanced_switch_math.json")).unwrap());
    let branch = doc["branch"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .find(|b| b["name"] == "_virtual_branch.switch.switch1")
        .unwrap();
    // PMD 0.16.0 output for ENGINEERING switch1.g_fr=2I at 50 Hz,
    // length=.001. All four admittance fields use 2*pi*f/1e9.
    let expected = 6.702_064_327_658_226e-11;
    branch["g_fr"][0][0] = serde_json::json!(expected);
    let net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    let output = json(&net.emit_value(DistTargetFormat::PmdMathJson).text);
    let branch = output["branch"]
        .as_object()
        .unwrap()
        .values()
        .find(|b| b["name"] == "_virtual_branch.switch.switch1")
        .unwrap();
    assert!(rel(branch["g_fr"][0][0].as_f64().unwrap(), expected));
    let engineering = net.emit_value(DistTargetFormat::PmdJson);
    let engineering = helpers::parse_str(&engineering.text, "pmd-json").unwrap();
    let output = json(&engineering.emit_value(DistTargetFormat::PmdMathJson).text);
    let branch = output["branch"]
        .as_object()
        .unwrap()
        .values()
        .find(|b| b["name"] == "_virtual_branch.switch.switch1")
        .unwrap();
    assert!(rel(branch["g_fr"][0][0].as_f64().unwrap(), expected));
}

#[test]
fn engineering_switch_canonical_write_keeps_admittance_and_length() {
    let net = parse_eng("math/case3_unbalanced_switch_eng.json");
    let first = json(&net.emit_value(DistTargetFormat::PmdMathJson).text);
    let engineering = net.emit_value(DistTargetFormat::PmdJson);
    let engineering = helpers::parse_str(&engineering.text, "pmd-json").unwrap();
    let second = json(&engineering.emit_value(DistTargetFormat::PmdMathJson).text);
    for name in [
        "_virtual_branch.switch.switch1",
        "_virtual_branch.switch.switch2",
    ] {
        let branch = |doc: &Value| {
            doc["branch"]
                .as_object()
                .unwrap()
                .values()
                .find(|b| b["name"] == name)
                .unwrap()
                .clone()
        };
        let (a, b) = (branch(&first), branch(&second));
        for key in ["br_r", "br_x", "g_fr", "g_to", "b_fr", "b_to"] {
            assert!(
                agree(&a[key], &b[key]),
                "{name}.{key}: {} != {}",
                a[key],
                b[key]
            );
        }
    }
}

#[test]
fn branch_angle_limits_survive_both_pmd_models() {
    let mut doc = json(&std::fs::read_to_string(fixture("math/case3_balanced_math.json")).unwrap());
    let branch = doc["branch"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .find(|b| b["name"] == "quad")
        .unwrap();
    branch["angmin"] = serde_json::json!([-0.1, -0.2, -0.3]);
    branch["angmax"] = serde_json::json!([0.1, 0.2, 0.3]);
    let net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    for via_engineering in [false, true] {
        let output = if via_engineering {
            let engineering = net.emit_value(DistTargetFormat::PmdJson);
            helpers::parse_str(&engineering.text, "pmd-json")
                .unwrap()
                .emit_value(DistTargetFormat::PmdMathJson)
        } else {
            net.emit_value(DistTargetFormat::PmdMathJson)
        };
        let output = json(&output.text);
        let branch = output["branch"]
            .as_object()
            .unwrap()
            .values()
            .find(|b| b["name"] == "quad")
            .unwrap();
        assert!(agree(
            &branch["angmin"],
            &serde_json::json!([-0.1, -0.2, -0.3])
        ));
        assert!(agree(
            &branch["angmax"],
            &serde_json::json!([0.1, 0.2, 0.3])
        ));
    }
}

#[test]
fn open_phase_bounds_do_not_erase_other_phase_constraints() {
    let mut doc = direct_transformer_math();
    doc["bus"]["2"]["terminals"] = serde_json::json!([1, 2, 3, 4]);
    doc["bus"]["2"]["grounded"] = serde_json::json!([false, false, false, true]);
    doc["bus"]["2"]["vmin"] = serde_json::json!([0.9, 0.0, 0.8, 0.0]);
    doc["bus"]["2"]["vmax"] = serde_json::json!([1.1, null, 1.2, null]);
    let net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    let bus = net.buses().iter().find(|b| b.id == "hi").unwrap();
    assert!(
        bus.v_min_phase
            .as_ref()
            .unwrap()
            .iter()
            .zip([10_800.0, 0.0, 9_600.0])
            .all(|(actual, expected)| rel(*actual, expected))
    );
    assert_eq!(bus.v_max_phase.as_ref().unwrap()[1], f64::INFINITY);
    let output = json(&net.emit_value(DistTargetFormat::PmdMathJson).text);
    let bus = output["bus"]
        .as_object()
        .unwrap()
        .values()
        .find(|b| b["name"] == "hi")
        .unwrap();
    assert!(agree(&bus["vmin"], &serde_json::json!([0.9, 0.0, 0.8])));
    assert!(agree(&bus["vmax"], &serde_json::json!([1.1, null, 1.2])));
}

#[test]
fn single_phase_bus_bounds_exclude_the_grounded_neutral() {
    let mut doc = direct_transformer_math();
    doc["bus"]["2"]["terminals"] = serde_json::json!([3, 4]);
    doc["bus"]["2"]["vmin"] = serde_json::json!([0.9, 0.0]);
    doc["bus"]["2"]["vmax"] = serde_json::json!([1.1, 0.0]);
    doc["transformer"]["1"]["f_connections"] = serde_json::json!([3, 4]);
    let net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    let output = net.emit_value(DistTargetFormat::BmopfJson);
    let doc = json(&output.text);
    assert_eq!(doc["bus"]["hi"]["v_min"], serde_json::json!([10800.0]));
    let back = helpers::parse_str(&output.text, "bmopf-json").unwrap();
    assert!(
        !back.warnings.iter().any(|w| w.contains("SEMANTIC_INVALID")),
        "{:?}",
        back.warnings
    );
}

#[test]
fn stated_numeric_bases_must_be_positive_and_finite() {
    for bad in [
        serde_json::json!(0),
        serde_json::json!(-1),
        Value::Null,
        serde_json::json!("NaN"),
    ] {
        for (table, key) in [
            ("settings", "sbase"),
            ("settings", "voltage_scale_factor"),
            ("settings", "power_scale_factor"),
            ("bus", "vbase"),
            ("transformer", "t_vbase"),
        ] {
            let mut doc = direct_transformer_math();
            if table == "settings" {
                doc[table][key] = bad.clone();
            } else {
                doc[table]["1"][key] = bad.clone();
            }
            let error = helpers::parse_str(&doc.to_string(), "pmd-math-json")
                .unwrap_err()
                .to_string();
            assert!(error.contains("finite positive"), "{table}.{key}: {error}");
        }
    }
}

#[test]
fn zero_winding_resistance_does_not_consume_the_leakage_branch() {
    let mut doc = json(&std::fs::read_to_string(fixture("math/ut_trans_2w_yy_math.json")).unwrap());
    // PMD 0.16.0 transform_data_model(ut_trans_2w_yy; rw[1]=0)
    // removes edge _1 and merges its two buses. This is the same native
    // graph operation on the existing oracle, keeping its numbering.
    let branches = doc["branch"].as_object_mut().unwrap();
    let key = branches
        .iter()
        .find(|(_, b)| b["name"] == "_virtual_branch.transformer.tx1_1")
        .unwrap()
        .0
        .clone();
    let removed = branches.remove(&key).unwrap();
    let (keep, drop) = (removed["f_bus"].clone(), removed["t_bus"].clone());
    for branch in branches.values_mut() {
        for side in ["f_bus", "t_bus"] {
            if branch[side] == drop {
                branch[side] = keep.clone();
            }
        }
    }
    let bus_key = drop.as_i64().unwrap().to_string();
    doc["bus"].as_object_mut().unwrap().remove(&bus_key);
    for entry in doc["map"].as_array_mut().unwrap() {
        if let Some(to) = entry.get_mut("to").and_then(Value::as_array_mut) {
            to.retain(|id| id != &format!("branch.{key}") && id != &format!("bus.{bus_key}"));
        }
    }
    let net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    let xf = &net.transformers()[0];
    assert_eq!(xf.windings[0].r_pct, 0.0);
    assert!(rel(transformer_ohms(xf).1, 12.1));
    let output = net.emit_value(DistTargetFormat::PmdMathJson);
    let back = helpers::parse_str(&output.text, "pmd-math-json").unwrap();
    assert!(rel(transformer_ohms(&back.transformers()[0]).1, 12.1));
    let units = doc["transformer"].as_object_mut().unwrap();
    let mut first = units.remove("1").unwrap();
    let mut second = units.remove("2").unwrap();
    first["index"] = serde_json::json!(2);
    second["index"] = serde_json::json!(1);
    units.insert("2".into(), first);
    units.insert("1".into(), second);
    let renumbered = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    let actual = transformer_ohms(&renumbered.transformers()[0]);
    let expected = transformer_ohms(xf);
    assert!(actual.0.iter().zip(&expected.0).all(|(a, b)| rel(*a, *b)));
    assert!(rel(actual.1, expected.1));
}

#[test]
fn unbounded_transformer_capacity_keeps_finite_loss_impedance() {
    let mut doc = json(&std::fs::read_to_string(fixture("math/ut_trans_2w_yy_math.json")).unwrap());
    for unit in doc["transformer"].as_object_mut().unwrap().values_mut() {
        unit["sm_ub"] = Value::Null;
    }
    let original = parse_math("math/ut_trans_2w_yy_math.json");
    let expected = transformer_ohms(&original.transformers()[0]);
    let net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    let actual = transformer_ohms(&net.transformers()[0]);
    assert!(actual.0.iter().zip(&expected.0).all(|(a, b)| rel(*a, *b)));
    assert!(rel(actual.1, expected.1));
    assert!(net.warnings.iter().any(|w| w.contains("system power base")));
    for target in [DistTargetFormat::PmdMathJson, DistTargetFormat::PmdJson] {
        let output = net.emit_value(target);
        let result = json(&output.text);
        assert!(
            result["transformer"]
                .as_object()
                .unwrap()
                .values()
                .all(|t| t["sm_ub"].is_null())
        );
        let back = helpers::parse_str(&output.text, target.name()).unwrap();
        let actual = transformer_ohms(&back.transformers()[0]);
        assert!(actual.0.iter().zip(&expected.0).all(|(a, b)| rel(*a, *b)));
        assert!(rel(actual.1, expected.1));
    }
}

#[test]
fn transformer_emergency_rating_stash_is_kva_on_a_watt_power_base() {
    // PMD 0.16.0 native transform_data_model of the direct fixture with
    // power_scale_factor=1, sbase_default=100000 and sm_ub=100000 emits
    // this sbase and sm_ub=1. The physical capacity remains 100 kVA.
    let mut doc = direct_transformer_math();
    doc["settings"]["power_scale_factor"] = serde_json::json!(1.0);
    doc["settings"]["sbase"] = serde_json::json!(100_000.0);
    let net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    let transformer = &net.transformers()[0];
    assert_eq!(transformer.windings[0].s_rating, 100_000.0);
    assert_eq!(transformer.extras["emerghkva"], serde_json::json!(100.0));
    let emitted = net.emit_value(DistTargetFormat::PmdMathJson);
    let output = json(&emitted.text);
    for unit in output["transformer"].as_object().unwrap().values() {
        assert!(rel(unit["sm_ub"].as_f64().unwrap(), 1.0));
    }
    let back = helpers::parse_str(&emitted.text, "pmd-math-json").unwrap();
    assert_eq!(
        back.transformers()[0].extras["emerghkva"],
        serde_json::json!(100.0)
    );
    assert_eq!(back.transformers()[0].windings[0].s_rating, 100_000.0);
}

#[test]
fn fractional_and_overflowing_bus_references_are_rejected() {
    for bad in [serde_json::json!(1.9), serde_json::json!(1e30), Value::Null] {
        let mut doc = direct_transformer_math();
        doc["transformer"]["1"]["f_bus"] = bad;
        let error = helpers::parse_str(&doc.to_string(), "pmd-math-json")
            .unwrap_err()
            .to_string();
        assert!(error.contains("f_bus must be an integer"), "{error}");
    }
    let mut doc = direct_transformer_math();
    doc["transformer"]["1"]["f_bus"] = serde_json::json!(2.0);
    let net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    assert_eq!(net.transformers()[0].windings[0].bus, "hi");
}

fn native_reduced_center_tap() -> Value {
    // Lossless extract of PMD 0.16.0 transform_data_model on the micro
    // xfmr_center_tap.dss fixture: its third reduced unit keeps polarity -1
    // after dropping the first terminal of [4,2]. Zero losses keep this
    // regression focused on winding orientation and independent of folding.
    let mut doc = serde_json::json!({
        "data_model": "MATHEMATICAL", "per_unit": true, "is_kron_reduced": true,
        "settings": {"sbase": 100.0, "voltage_scale_factor": 1000.0, "power_scale_factor": 1000.0},
        "bus": {
            "1": {"name": "sourcebus", "vbase": 7.2, "terminals": [1], "grounded": [false]},
            "2": {"name": "secondary", "vbase": 0.12, "terminals": [1,2], "grounded": [false,false]},
            "3": {"name": "_virtual_bus.transformer.t1_1", "vbase": 1.0,
                "terminals": [1], "grounded": [false]}
        },
        "transformer": {},
        "map": [{"from": "t1", "to": ["bus.3", "transformer.1", "transformer.2", "transformer.3"],
            "unmap_function": "_map_math2eng_transformer!"}]
    });
    for w in 1..=3 {
        doc["transformer"][w.to_string()] = serde_json::json!({
            "index": w, "name": format!("_virtual_transformer.t1.{w}"),
            "f_bus": if w == 1 {1} else {2}, "t_bus": 3,
            "f_connections": [if w == 3 {2} else {1}], "t_connections": [1],
            "configuration": "WYE", "polarity": if w == 3 {-1} else {1},
            "tm_nom": 1.0, "tm_set": [1.0], "tm_fix": [true], "sm_ub": 0.375
        });
    }
    doc
}

#[test]
fn reduced_center_tap_restores_physical_winding_orientation() {
    let doc = native_reduced_center_tap();
    let mut net = helpers::parse_str(&doc.to_string(), "pmd-math-json").unwrap();
    assert_eq!(net.transformers()[0].windings[2].terminal_map, ["4", "2"]);
    // PMD metadata cannot carry the sign into another format. The typed
    // winding must retain it when all format-specific polarity is absent.
    net.transformers_mut()[0].extras.remove("pmd_polarity");
    for target in [DistTargetFormat::Dss, DistTargetFormat::BmopfJson] {
        let output = net.emit_value(target);
        let back = helpers::parse_str(&output.text, target.name()).unwrap();
        assert_eq!(back.transformers()[0].windings[2].terminal_map, ["4", "2"]);
    }
    let output = net.emit_value(DistTargetFormat::PmdMathJson);
    let document = json(&output.text);
    assert_eq!(document["is_kron_reduced"], true);
    let third = document["transformer"]
        .as_object()
        .unwrap()
        .values()
        .find(|unit| unit["name"] == "_virtual_transformer.t1.3")
        .unwrap();
    assert_eq!(third["f_connections"], serde_json::json!([2]));
    assert_eq!(third["polarity"], -1);
    let back = helpers::parse_str(&output.text, "pmd-math-json").unwrap();
    assert_eq!(back.transformers()[0].windings[2].terminal_map, ["4", "2"]);
}

/// Byte equality with a readable failure: the first differing line and its
/// neighbours, not two whole documents.
fn assert_same_text(a: &str, b: &str, what: &str) {
    if a == b {
        return;
    }
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let at = la
        .iter()
        .zip(&lb)
        .position(|(x, y)| x != y)
        .unwrap_or(la.len().min(lb.len()));
    let window = |lines: &[&str]| lines[at.saturating_sub(4)..(at + 5).min(lines.len())].join("\n");
    panic!(
        "{what} at line {}:\n--- first\n{}\n--- second\n{}",
        at + 1,
        window(&la),
        window(&lb)
    );
}

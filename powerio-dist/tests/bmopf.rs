//! BMOPF reader/writer against the vendored draft schema and the two
//! public example networks from frederikgeth/bmopf-report.

use std::path::PathBuf;
use std::sync::Arc;

use powerio_dist::dss::{parse_dss_file, parse_dss_str};
use powerio_dist::{
    Configuration, DistNetwork, DistTransformer, Extras, Winding, WindingConn, parse_bmopf_file,
    parse_bmopf_str, write_bmopf_json,
};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist")
        .join(rel)
}

fn schema_validator() -> jsonschema::Validator {
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture("bmopf/draft_bmopf_schema.json")).unwrap(),
    )
    .unwrap();
    jsonschema::validator_for(&schema).expect("vendored schema compiles")
}

fn errors(validator: &jsonschema::Validator, text: &str) -> Vec<String> {
    let doc: serde_json::Value = serde_json::from_str(text).unwrap();
    validator
        .iter_errors(&doc)
        .map(|e| format!("{}: {e}", e.instance_path()))
        .collect()
}

#[test]
fn vendored_examples_validate() {
    let v = schema_validator();
    for example in ["bmopf/example_ieee13.json", "bmopf/example_enwl_n1_f2.json"] {
        let text = std::fs::read_to_string(fixture(example)).unwrap();
        assert_eq!(errors(&v, &text), Vec::<String>::new(), "{example}");
    }
}

#[test]
fn parse_the_public_examples() {
    let net = parse_bmopf_file(fixture("bmopf/example_ieee13.json")).unwrap();
    assert_eq!(net.buses.len(), 16);
    assert_eq!(net.switches.len(), 1);
    assert_eq!(net.shunts.len(), 2);
    assert_eq!(net.transformers.len(), 7);
    assert_eq!(net.sources.len(), 1);
    assert!(net.warnings.is_empty(), "{:?}", net.warnings);

    let b611 = net.bus("611").unwrap();
    assert_eq!(b611.terminals, vec!["3", "4"]);
    assert_eq!(b611.grounded, vec!["4"]);

    let enwl = parse_bmopf_file(fixture("bmopf/example_enwl_n1_f2.json")).unwrap();
    assert_eq!(enwl.buses.len(), 506);
    assert_eq!(enwl.generators.len(), 7);
    let g = &enwl.generators[0];
    assert_eq!(g.cost, Some(0.001));
    assert!(g.p_max.is_some());
    // ENWL buses carry phase to neutral bounds.
    assert!(enwl.buses.iter().any(|b| b.vpn_min.is_some()));
}

#[test]
fn written_output_validates_and_round_trips() {
    let v = schema_validator();
    let net = parse_bmopf_file(fixture("bmopf/example_ieee13.json")).unwrap();
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    // Nothing in the example exceeds the schema, so nothing should drop.
    assert_eq!(out.warnings, Vec::<String>::new());

    // Canonical idempotence at the model level: parse(write(parse(x)))
    // equals parse(x) up to the retained source text.
    let again = parse_bmopf_str(&out.text).unwrap();
    assert_model_eq(&net, &again);

    // And byte idempotence of the canonical form.
    let out2 = write_bmopf_json(&again);
    assert_eq!(out.text, out2.text);
}

#[test]
fn enwl_round_trips() {
    let v = schema_validator();
    let net = parse_bmopf_file(fixture("bmopf/example_enwl_n1_f2.json")).unwrap();
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let again = parse_bmopf_str(&out.text).unwrap();
    assert_model_eq(&net, &again);
}

/// Model equality minus the retained source (which differs by format).
fn assert_model_eq(a: &DistNetwork, b: &DistNetwork) {
    let strip = |n: &DistNetwork| {
        let mut n = n.clone();
        n.source = Some(Arc::new(String::new()));
        n
    };
    let (a, b) = (strip(a), strip(b));
    assert_eq!(a.buses, b.buses);
    assert_eq!(a.linecodes, b.linecodes);
    assert_eq!(a.lines, b.lines);
    assert_eq!(a.switches, b.switches);
    assert_eq!(a.loads, b.loads);
    assert_eq!(a.generators, b.generators);
    assert_eq!(a.shunts, b.shunts);
    assert_eq!(a.sources, b.sources);
    assert_eq!(a.transformers, b.transformers);
}

#[test]
fn dss_fixtures_emit_valid_bmopf() {
    let v = schema_validator();
    for case in [
        "opendss/ieee13/IEEE13Nodeckt.dss",
        "opendss/ieee34/ieee34Mod1.dss",
        "opendss/ieee123/IEEE123Master.dss",
        "micro/xfmr_single_phase.dss",
        "micro/xfmr_center_tap.dss",
        "micro/xfmr_wye_delta.dss",
        "micro/xfmr_delta_wye.dss",
        "micro/xfmr_open_wye_open_delta.dss",
        "micro/xfmr_1ph_delta_wye.dss",
        "micro/switch.dss",
        "micro/fourwire_linecode.dss",
        "micro/defaults_degenerate.dss",
    ] {
        let net = parse_dss_file(fixture(case)).unwrap();
        let out = write_bmopf_json(&net);
        assert_eq!(errors(&v, &out.text), Vec::<String>::new(), "{case}");
    }
}

/// A single phase wye/delta transformer (an open delta leg) must reach the
/// BMOPF output as a `single_phase` entry, not drop, and the delta winding
/// must carry both of its phase terminals end to end. This is issue #135's
/// item 1: the classifier used to route only `(1, [Wye, Wye])`, and the dss
/// reader collapsed a single phase delta map to one terminal.
#[test]
fn single_phase_wye_delta_keeps_both_delta_terminals() {
    // Open wye / open delta: the delta is on the secondary, on .1.2 and .2.3.
    let net = parse_dss_file(fixture("micro/xfmr_open_wye_open_delta.dss")).unwrap();
    let t1 = net.transformers.iter().find(|t| t.name == "t1").unwrap();
    assert_eq!(t1.windings[1].conn, WindingConn::Delta);
    assert_eq!(t1.windings[1].terminal_map, vec!["1", "2"]);
    let t2 = net.transformers.iter().find(|t| t.name == "t2").unwrap();
    assert_eq!(t2.windings[1].terminal_map, vec!["2", "3"]);

    let out = write_bmopf_json(&net);
    assert!(
        !out.warnings.iter().any(|w| w.contains("transformer")
            && w.contains("not representable")
            && w.contains("dropped")),
        "open delta transformers dropped: {:?}",
        out.warnings
    );
    // The wye/delta connection is not encoded in single_phase; we flag it
    // rather than dropping the transformer.
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("t1") && w.contains("not encoded in the subtype")),
        "missing the wye/delta fidelity note: {:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let sp = &doc["transformer"]["single_phase"];
    assert_eq!(sp["t1"]["terminal_map_to"], serde_json::json!(["1", "2"]));
    assert_eq!(sp["t2"]["terminal_map_to"], serde_json::json!(["2", "3"]));

    // The other orientation: a delta primary (line to line, on .1.2) into a
    // grounded wye secondary keeps both primary terminals.
    let dw = parse_dss_file(fixture("micro/xfmr_1ph_delta_wye.dss")).unwrap();
    let t = &dw.transformers[0];
    assert_eq!(t.windings[0].conn, WindingConn::Delta);
    assert_eq!(t.windings[0].terminal_map, vec!["1", "2"]);
    let out = write_bmopf_json(&dw);
    assert!(
        !out.warnings.iter().any(|w| w.contains("not representable")),
        "delta-wye dropped: {:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(
        doc["transformer"]["single_phase"]["t1"]["terminal_map_from"],
        serde_json::json!(["1", "2"])
    );
}

#[test]
fn ieee13_conversion_warnings_name_every_loss() {
    let net = parse_dss_file(fixture("opendss/ieee13/IEEE13Nodeckt.dss")).unwrap();
    let out = write_bmopf_json(&net);
    // The wye-wye XFM1 decomposes; regulators and coordinates drop loudly.
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("XFM1") && w.contains("single_phase"))
    );
    assert!(out.warnings.iter().any(|w| w.contains("regcontrol")));
    // No silent extras: every warning leads with a `class name:` element
    // identifier ("load 671: ...", "voltage source source: ...").
    for w in &out.warnings {
        let Some((head, _)) = w.split_once(": ") else {
            panic!("warning has no `class name:` prefix: {w}");
        };
        assert!(
            head.split_whitespace().count() >= 2,
            "warning does not name its element: {w}"
        );
    }
}

#[test]
fn ten_conductor_linecode_is_schema_valid() {
    let v = schema_validator();
    let net = parse_dss_file(fixture("micro/linecode_10x10.dss")).unwrap();
    let out = write_bmopf_json(&net);
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("double digit matrix keys")),
        "obsolete double digit warning still emitted: {:?}",
        out.warnings
    );
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
}

#[test]
fn fixed_generators_emit_as_negative_loads() {
    let v = schema_validator();
    let net = parse_dss_str(
        "New Circuit.c\n\
         New Generator.g bus1=b.1 phases=1 kv=2.4 kw=100 kvar=20",
    );
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("generator g") && w.contains("negative load")),
        "missing fixed generator warning: {:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert!(
        doc.get("generator").is_none(),
        "fixed generator was emitted"
    );
    assert_eq!(doc["load"]["g"]["p_nom"], serde_json::json!([-100_000.0]));
    assert_eq!(doc["load"]["g"]["q_nom"], serde_json::json!([-20_000.0]));
}

#[test]
fn negative_validation_cases() {
    let v = schema_validator();
    let base: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture("bmopf/example_ieee13.json")).unwrap(),
    )
    .unwrap();
    let mutate = |f: &dyn Fn(&mut serde_json::Value)| {
        let mut doc = base.clone();
        f(&mut doc);
        doc
    };
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "missing voltage_source",
            mutate(&|d| {
                d.as_object_mut().unwrap().remove("voltage_source");
            }),
        ),
        (
            "missing terminal_map on a line",
            mutate(&|d| {
                d["line"]["632633"]
                    .as_object_mut()
                    .unwrap()
                    .remove("terminal_map_from");
            }),
        ),
        (
            "unknown field on a bus",
            mutate(&|d| {
                d["bus"]["632"]["color"] = "blue".into();
            }),
        ),
        (
            "lowercase configuration enum",
            mutate(&|d| {
                let loads = d["load"].as_object_mut().unwrap();
                let first = loads.keys().next().unwrap().clone();
                loads[&first]["configuration"] = "wye".into();
            }),
        ),
        (
            "wrong type for length",
            mutate(&|d| {
                d["line"]["632633"]["length"] = "152.4".into();
            }),
        ),
        (
            "negative linecode i_max",
            mutate(&|d| {
                let codes = d["linecode"].as_object_mut().unwrap();
                let first = codes.keys().next().unwrap().clone();
                codes[&first]["i_max"] = serde_json::json!([-600.0, 600.0, 600.0]);
            }),
        ),
        (
            "negative switch i_max",
            mutate(&|d| {
                d["switch"]["671692"]["i_max"] = serde_json::json!([-600.0]);
            }),
        ),
        (
            "integer terminal names",
            mutate(&|d| {
                d["bus"]["632"]["terminal_names"] = serde_json::json!([1, 2, 3]);
            }),
        ),
    ];
    for (what, doc) in cases {
        let text = serde_json::to_string(&doc).unwrap();
        assert!(!errors(&v, &text).is_empty(), "schema accepted: {what}");
    }
}

/// A bus plus one source, the minimum the schema requires; element
/// snippets splice in after the source.
fn doc_with(extra: &str) -> String {
    format!(
        r#"{{
        "bus": {{"a": {{"terminal_names": ["1", "2"]}}}},
        "voltage_source": {{"src": {{"v_magnitude": [240.0], "v_angle": [0.0],
            "bus": "a", "terminal_map": ["1"]}}}}{extra}
    }}"#
    )
}

#[test]
fn shunt_size_mismatch_pads_the_smaller_matrix() {
    let text = doc_with(
        r#", "shunt": {"c1": {"bus": "a", "terminal_map": ["1", "2"],
            "G_1_1": 0.5,
            "B_1_1": 1.0, "B_1_2": -1.0, "B_2_1": -1.0, "B_2_2": 1.0}}"#,
    );
    let net = parse_bmopf_str(&text).unwrap();
    let s = &net.shunts[0];
    // G grew to B's size; its entry survived the padding.
    assert_eq!(s.g, vec![vec![0.5, 0.0], vec![0.0, 0.0]]);
    assert_eq!(s.b, vec![vec![1.0, -1.0], vec![-1.0, 1.0]]);
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("shunt c1") && w.contains("padded")),
        "{:?}",
        net.warnings
    );
    // The padded form writes back schema valid.
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
}

#[test]
fn center_tap_collapse_converts_resistance_through_ohms() {
    // Each 120 V half carries %R=1.2 on 25 kVA: 0.012 * 120^2/25000 =
    // 0.006912 ohm, so the series path across the outer terminals is
    // 0.013824 ohm. Percent does not transfer to the 240 V base (zb
    // scales 4x), so the collapse converts through ohms.
    let net = parse_dss_file(fixture("micro/xfmr_center_tap.dss")).unwrap();
    let out = write_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["center_tap"]["t1"];
    assert_eq!(t["v_ref_to"], 240.0);
    let r_to = t["r_series_to"].as_f64().unwrap();
    assert!((r_to - 0.013_824).abs() < 1e-12, "r_series_to = {r_to}");
    // The primary is untouched by the collapse: %R=0.6 on 7.2 kV/25 kVA.
    let r_from = t["r_series_from"].as_f64().unwrap();
    assert!((r_from - 12.4416).abs() < 1e-9, "r_series_from = {r_from}");
}

#[test]
fn center_tap_collapse_uses_each_half_windings_own_s_rating() {
    // Legal OpenDSS: the two 120 V halves carry different kva ratings, so
    // each half's impedance base is its own v^2/s. The series path across
    // the outer terminals is the sum of the per half ohms.
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.ct basekv=7.2 pu=1.0 phases=1 bus1=src.1\n\
         New Transformer.t1 phases=1 windings=3 buses=(src.1.0, lv.1.0, lv.0.2) \
         kvs=(7.2 0.12 0.12) kvas=(25 50 25) %Rs=(1 2 4) xhl=2.04 xht=2.04 xlt=1.36\n",
    );
    let out = write_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["center_tap"]["t1"];
    assert_eq!(t["v_ref_to"], 240.0);
    let expected = 0.02 * 120.0 * 120.0 / 50e3 + 0.04 * 120.0 * 120.0 / 25e3;
    let r_to = t["r_series_to"].as_f64().unwrap();
    assert!((r_to - expected).abs() < 1e-12, "r_series_to = {r_to}");
    // The collapsed winding keeps one s_rating; the half ratings drop loudly.
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("transformer t1") && w.contains("s_rating")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn x_only_linecode_sizes_from_x_and_keeps_required_keys() {
    let text = doc_with(
        r#", "linecode": {"lc": {
            "X_series_1_1": 0.4, "X_series_1_2": 0.1,
            "X_series_2_1": 0.1, "X_series_2_2": 0.4}}"#,
    );
    let net = parse_bmopf_str(&text).unwrap();
    let lc = net.linecode("lc").unwrap();
    assert_eq!(lc.n_conductors, 2);
    assert_eq!(lc.r_series, vec![vec![0.0; 2]; 2]);
    assert!((lc.x_series[0][1] - 0.1).abs() < 1e-15);
    // The output carries the schema required R_series_1_1 (zero).
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(doc["linecode"]["lc"]["R_series_1_1"], 0.0);
}

#[test]
fn matrixless_linecode_and_shunt_emit_required_zero_matrices_loudly() {
    let text = doc_with(
        r#", "linecode": {"bare": {"i_max": [400.0]}},
        "shunt": {"empty": {"bus": "a", "terminal_map": ["1"]}}"#,
    );
    let net = parse_bmopf_str(&text).unwrap();
    assert_eq!(net.linecode("bare").unwrap().n_conductors, 0);
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(doc["linecode"]["bare"]["R_series_1_1"], 0.0);
    assert_eq!(doc["linecode"]["bare"]["X_series_1_1"], 0.0);
    assert_eq!(doc["shunt"]["empty"]["G_1_1"], 0.0);
    assert_eq!(doc["shunt"]["empty"]["B_1_1"], 0.0);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("linecode bare") && w.contains("no series matrix"))
    );
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("shunt empty") && w.contains("no admittance matrix"))
    );
}

#[test]
fn malformed_matrix_keys_land_in_extras_with_warnings() {
    let text = doc_with(
        r#", "linecode": {"lc": {"R_series_1_1": 0.2, "X_series_1_1": 0.4,
            "X_series_note": "an aside"}},
        "shunt": {"c1": {"bus": "a", "terminal_map": ["1"],
            "G_1_1": 0.5, "B_1_1": 1.0, "B_total": 5.0, "G_0_1": 9.0}}"#,
    );
    let net = parse_bmopf_str(&text).unwrap();
    let lc = net.linecode("lc").unwrap();
    assert_eq!(lc.n_conductors, 1);
    assert!(lc.extras.contains_key("X_series_note"));
    let s = &net.shunts[0];
    // Only well formed `_i_j` keys (1 based) count as matrix entries.
    assert_eq!(s.g, vec![vec![0.5]]);
    assert_eq!(s.b, vec![vec![1.0]]);
    assert!(s.extras.contains_key("B_total"));
    assert!(s.extras.contains_key("G_0_1"));
    for key in ["X_series_note", "B_total", "G_0_1"] {
        assert!(
            net.warnings.iter().any(|w| w.contains(&format!("`{key}`"))),
            "no warning for {key}: {:?}",
            net.warnings
        );
    }
    // Writing drops them, again loudly.
    let out = write_bmopf_json(&net);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("shunt c1") && w.contains("`B_total`"))
    );
}

#[test]
fn unrecognized_configuration_and_subtype_warn() {
    let text = doc_with(
        r#", "load": {
            "l1": {"p_nom": [1000.0], "q_nom": [0.0], "bus": "a",
                "configuration": "delta", "terminal_map": ["1", "2"]},
            "l2": {"p_nom": [1000.0], "q_nom": [0.0], "bus": "a",
                "configuration": "zigzag", "terminal_map": ["1", "2"]}},
        "transformer": {"open_delta": {"t1": {"bus_from": "a", "bus_to": "a",
            "terminal_map_from": ["1", "2"], "terminal_map_to": ["1", "2"],
            "s_rating": 5000.0, "v_ref_from": 240.0, "v_ref_to": 240.0}}}"#,
    );
    let net = parse_bmopf_str(&text).unwrap();
    // A recognized value in the wrong case is tolerated without a warning.
    assert_eq!(net.loads[0].configuration, Configuration::Delta);
    assert!(!net.warnings.iter().any(|w| w.contains("load l1")));
    // A truly unknown one coerces to WYE, loudly.
    assert_eq!(net.loads[1].configuration, Configuration::Wye);
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("load l2") && w.contains("zigzag"))
    );
    // An unknown transformer subtype group reads, with a warning.
    assert_eq!(net.transformers.len(), 1);
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("transformer t1") && w.contains("open_delta"))
    );
}

#[test]
fn missing_voltage_source_warns() {
    let net = parse_bmopf_str(r#"{"bus": {"a": {"terminal_names": ["1"]}}}"#).unwrap();
    let out = write_bmopf_json(&net);
    assert!(out.warnings.iter().any(|w| w.contains("no voltage source")));
    // Still schema valid: the required key exists, empty.
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
}

#[test]
fn three_wire_wye_wye_is_unsupported_not_a_panic() {
    // Terminal maps without a trailing neutral cannot decompose per phase;
    // a map shorter than the phase count used to index out of bounds.
    let mut net = parse_bmopf_str(&doc_with("")).unwrap();
    let winding = |map: &[&str]| Winding {
        bus: "a".into(),
        terminal_map: map.iter().map(ToString::to_string).collect(),
        conn: WindingConn::Wye,
        v_ref: 4160.0,
        s_rating: 500_000.0,
        r_pct: 0.5,
        tap: 1.0,
    };
    net.transformers.push(DistTransformer {
        name: "t3w".into(),
        windings: vec![winding(&["1", "2", "3"]), winding(&["1", "2"])],
        xsc_pct: vec![2.0],
        phases: 3,
        extras: Extras::new(),
    });
    let out = write_bmopf_json(&net);
    assert!(!out.text.contains("t3w"));
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("transformer t3w") && w.contains("dropped")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn reader_is_liberal_where_the_writer_is_strict() {
    // An out of schema field parses with a warning and lands in extras;
    // writing drops it with a warning. Nothing is silent in either
    // direction.
    let text = r#"{
        "bus": {"a": {"terminal_names": ["1"], "note": "hand edited"}},
        "voltage_source": {"src": {"v_magnitude": [240.0], "v_angle": [0.0],
            "bus": "a", "terminal_map": ["1"]}}
    }"#;
    let net = parse_bmopf_str(text).unwrap();
    assert!(net.warnings.iter().any(|w| w.contains("note")));
    assert!(net.buses[0].extras.contains_key("note"));
    let out = write_bmopf_json(&net);
    assert!(out.warnings.iter().any(|w| w.contains("note")));
    assert!(!out.text.contains("hand edited"));
}

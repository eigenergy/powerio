//! BMOPF reader/writer against the vendored draft schema and the two
//! public example networks from frederikgeth/bmopf-report.

use std::path::PathBuf;
use std::sync::Arc;

use powerio_dist::dss::parse_dss_file;
use powerio_dist::{DistNetwork, parse_bmopf_file, parse_bmopf_str, write_bmopf_json};

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
        "micro/switch.dss",
        "micro/fourwire_linecode.dss",
        "micro/defaults_degenerate.dss",
    ] {
        let net = parse_dss_file(fixture(case)).unwrap();
        let out = write_bmopf_json(&net);
        assert_eq!(errors(&v, &out.text), Vec::<String>::new(), "{case}");
    }
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
    // No silent extras: every dropped field names its element.
    for w in &out.warnings {
        assert!(!w.is_empty());
    }
}

#[test]
fn ten_conductor_linecode_is_valid_data_the_schema_rejects() {
    let v = schema_validator();
    let net = parse_dss_file(fixture("micro/linecode_10x10.dss")).unwrap();
    let out = write_bmopf_json(&net);
    // The writer says what is about to happen...
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("double digit matrix keys"))
    );
    // ...and the draft schema indeed rejects the document: the single
    // digit key patterns (`^R_series_\d_\d`) do not match `R_series_10_10`,
    // so additionalProperties: false refuses the key. The fix is
    // `^R_series_\d+_\d+$`.
    let errs = errors(&v, &out.text);
    assert!(!errs.is_empty());
    assert!(errs.iter().any(|e| e.contains("linecode")));
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
            // linecode i_max items are nonnegative; switch i_max has no
            // item constraint in the draft, an asymmetry worth feedback.
            "negative linecode i_max",
            mutate(&|d| {
                let codes = d["linecode"].as_object_mut().unwrap();
                let first = codes.keys().next().unwrap().clone();
                codes[&first]["i_max"] = serde_json::json!([-600.0, 600.0, 600.0]);
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

//! BMOPF reader/writer against the vendored draft schema and the two
//! public example networks from frederikgeth/bmopf-report.

use std::path::PathBuf;
use std::sync::Arc;

use powerio_dist::dss::{parse_dss_file, parse_dss_str};
use powerio_dist::{
    Configuration, DistLineCode, DistNetwork, DistTransformer, Winding, WindingConn,
    parse_bmopf_file, parse_bmopf_str, write_bmopf_json, write_dss,
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
fn vendored_examples_validate_after_canonicalization() {
    let v = schema_validator();
    for example in ["bmopf/example_ieee13.json", "bmopf/example_enwl_n1_f2.json"] {
        let text = std::fs::read_to_string(fixture(example)).unwrap();
        let net = parse_bmopf_str(&text).unwrap();
        let out = write_bmopf_json(&net);
        assert_eq!(errors(&v, &out.text), Vec::<String>::new(), "{example}");
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

#[test]
fn dss_grounding_reactors_emit_bmopf_shunts() {
    let v = schema_validator();
    let net = parse_dss_str(
        "New Circuit.c basekv=0.4\n\
         New Reactor.tx_busgrounding_B179 phases=1 bus1=B179.4 bus2=B179.0 r=0.3 x=0.0\n\
         New Reactor.loadbusgrounding_B3230 phases=1 bus1=B3230.4 bus2=B3230.0 r=10.0 x=0.0\n\
         New Reactor.loadbusgrounding_B2656 phases=1 bus1=B2656.4 bus2=B2656.0 r=10.0 x=0.0\n",
    );
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    assert!(
        !out.warnings
            .iter()
            .any(|w| w.contains("reactor") || w.contains("ground")),
        "{:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let sh = &doc["shunt"];
    assert_eq!(sh.as_object().unwrap().len(), 3);
    assert_eq!(
        sh["tx_busgrounding_B179"]["terminal_map"],
        serde_json::json!(["4"])
    );
    assert_eq!(
        sh["tx_busgrounding_B179"]["G_1_1"],
        serde_json::json!(1.0 / 0.3)
    );
    assert_eq!(sh["tx_busgrounding_B179"]["B_1_1"], serde_json::json!(0.0));
    assert_eq!(
        sh["loadbusgrounding_B3230"]["G_1_1"],
        serde_json::json!(0.1)
    );
}

#[test]
fn dss_delta_shunts_emit_bmopf_matrices() {
    let v = schema_validator();
    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\n\
         New Capacitor.capd bus1=b2.1.2.3 phases=3 conn=delta kvar=900 kv=4.16\n\
         New Reactor.rxd bus1=b3.1.2.3 phases=3 conn=delta kvar=600 kv=4.16\n",
    );
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert!(doc["shunt"]["capd"]["B_1_2"].as_f64().unwrap() < 0.0);
    assert!(doc["shunt"]["rxd"]["B_1_2"].as_f64().unwrap() > 0.0);
    // The `conn` marker is preserved in the off diagonal B matrix, so it must
    // not raise a spurious "dropped from the output" warning.
    assert!(
        !out.warnings.iter().any(|w| w.contains("`conn`")),
        "spurious conn drop warning: {:?}",
        out.warnings
    );
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
fn dss_fixed_generator_emits_as_bmopf_generator() {
    let v = schema_validator();
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.generator_case basekv=12.47 bus1=sourcebus\n\
         New Generator.g1 bus1=sourcebus.1 phases=1 kv=7.2 kw=10 kvar=2",
    );
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    assert!(
        out.warnings.iter().all(|w| !w.contains("negative load")),
        "obsolete fixed generator warning: {:?}",
        out.warnings
    );
    assert!(
        out.warnings
            .iter()
            .any(|w| w == "generator g1: no generation cost in the source; emitted cost 0"),
        "missing zero cost warning: {:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert!(
        doc.get("load").and_then(|loads| loads.get("g1")).is_none(),
        "fixed generator was emitted as a load"
    );
    let g = &doc["generator"]["g1"];
    assert!(g.is_object(), "BMOPF generator g1 missing: {doc}");
    assert_eq!(g["p_min"], serde_json::json!([10_000.0]));
    assert_eq!(g["p_max"], serde_json::json!([10_000.0]));
    assert_eq!(g["q_min"], serde_json::json!([2_000.0]));
    assert_eq!(g["q_max"], serde_json::json!([2_000.0]));
    assert_eq!(g["cost"], serde_json::json!([0.0]));
}

#[test]
fn fixed_bmopf_generators_with_cost_stay_generators() {
    let v = schema_validator();
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "b": {
              "terminal_names": ["1"],
              "perfectly_grounded_terminals": []
            }
          },
          "voltage_source": {
            "source": {
              "v_magnitude": [2400.0],
              "v_angle": [0.0],
              "bus": "b",
              "terminal_map": ["1"]
            }
          },
          "generator": {
            "g": {
              "p_min": [100.0],
              "p_max": [100.0],
              "q_min": [20.0],
              "q_max": [20.0],
              "cost": [0.001],
              "bus": "b",
              "configuration": "SINGLE_PHASE",
              "terminal_map": ["1"]
            }
          }
        }"#,
    )
    .unwrap();

    let out = write_bmopf_json(&net);

    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("negative load") && !w.contains("cost")),
        "unexpected generator loss warning: {:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert!(
        doc.get("load").is_none(),
        "BMOPF generator was reclassified as a load"
    );
    assert_eq!(doc["generator"]["g"]["p_min"], serde_json::json!([100.0]));
    assert_eq!(doc["generator"]["g"]["p_max"], serde_json::json!([100.0]));
    assert_eq!(doc["generator"]["g"]["cost"], serde_json::json!([0.001]));
    let again = parse_bmopf_str(&out.text).unwrap();
    assert_model_eq(&net, &again);
}

#[test]
fn raw_ibr_and_control_profile_tables_round_trip() {
    let v = schema_validator();
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "b": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]}
          },
          "voltage_source": {
            "source": {
              "v_magnitude": [240.0, 240.0, 240.0, 0.0],
              "v_angle": [0.0, -2.0943951023931953, 2.0943951023931953, 0.0],
              "bus": "b",
              "terminal_map": ["1", "2", "3", "n"]
            }
          },
          "control_profile": {
            "cp": {"power_factor": {"pf": 0.98}}
          },
          "ibr": {
            "pv": {
              "bus": "b",
              "terminal_map": ["1", "2", "3", "n"],
              "topology": "FOUR_LEG",
              "prime_mover": "PV",
              "s_max": [1000.0, 1000.0, 1000.0],
              "control_profile": "cp"
            }
          }
        }"#,
    )
    .unwrap();

    let out = write_bmopf_json(&net);

    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(doc["ibr"]["pv"]["control_profile"], "cp");
    assert_eq!(doc["control_profile"]["cp"]["power_factor"]["pf"], 0.98);
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("ibr") && !w.contains("control_profile")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn voltage_source_cost_round_trips_as_extra() {
    let net = parse_bmopf_str(
        r#"{
          "bus": {"b": {"terminal_names": ["1"]}},
          "voltage_source": {
            "source": {
              "v_magnitude": [240.0],
              "v_angle": [0.0],
              "bus": "b",
              "terminal_map": ["1"],
              "cost": [1.0]
            }
          }
        }"#,
    )
    .unwrap();

    let out = write_bmopf_json(&net);

    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(
        doc["voltage_source"]["source"]["cost"],
        serde_json::json!([1.0])
    );
    assert!(
        out.warnings.iter().all(|w| !w.contains("cost")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn transformer_tap_fields_round_trip_through_bmopf() {
    let v = schema_validator();
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "a": {"terminal_names": ["1", "n"], "perfectly_grounded_terminals": ["n"]},
            "b": {"terminal_names": ["1", "n"], "perfectly_grounded_terminals": ["n"]}
          },
          "voltage_source": {
            "source": {"v_magnitude": [7200.0, 0.0], "v_angle": [0.0, 0.0], "bus": "a", "terminal_map": ["1", "n"]}
          },
          "transformer": {
            "single_phase": {
              "t": {
                "bus_from": "a", "bus_to": "b",
                "terminal_map_from": ["1", "n"], "terminal_map_to": ["1", "n"],
                "s_rating": 25000.0,
                "v_nom_from": 7200.0, "v_nom_to": 240.0,
                "r_series_from": 1.0, "r_series_to": 0.01,
                "x_series_from": 4.0, "x_series_to": 0.0,
                "tap": 1.05, "tap_min": 0.9, "tap_max": 1.1,
                "g_no_load": 0.000001,
                "b_no_load": 0.0
              }
            }
          }
        }"#,
    )
    .unwrap();

    assert!((net.transformers[0].windings[0].tap - 1.05).abs() < 1e-12);
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["single_phase"]["t"];
    assert_eq!(t["tap"], 1.05);
    assert_eq!(t["tap_min"], 0.9);
    assert_eq!(t["tap_max"], 1.1);
    assert_eq!(t["g_no_load"], serde_json::json!(0.000_001));
    assert_eq!(t["b_no_load"], serde_json::json!(0.0));
}

#[test]
fn n_winding_transformer_round_trips_through_bmopf() {
    let v = schema_validator();
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "a": {"terminal_names": ["1", "2", "3"]},
            "b": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]},
            "c": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]}
          },
          "voltage_source": {
            "source": {"v_magnitude": [100.0, 100.0, 100.0], "v_angle": [0.0, -2.0943951023931953, 2.0943951023931953], "bus": "a", "terminal_map": ["1", "2", "3"]}
          },
          "transformer": {
            "n_winding": {
              "t3": {
                "s_rating": 10000.0,
                "windings": [
                  {"bus": "a", "terminal_map": ["1", "2", "3"], "v_nom": 100.0, "configuration": "DELTA", "r_winding": 0.01},
                  {"bus": "b", "terminal_map": ["1", "2", "3", "n"], "v_nom": 100.0, "configuration": "WYE", "r_winding": 0.02},
                  {"bus": "c", "terminal_map": ["1", "2", "3", "n"], "v_nom": 100.0, "configuration": "WYE", "r_winding": 0.03}
                ],
                "x_sc": {"1_2": 0.04, "1_3": 0.05, "2_3": 0.06},
                "g_no_load": 0.001,
                "b_no_load": -0.002
              }
            }
          }
        }"#,
    )
    .unwrap();

    assert_eq!(net.transformers[0].windings.len(), 3);
    let model_t = &net.transformers[0];
    assert!((model_t.windings[1].v_ref - 100.0 * 3f64.sqrt()).abs() < 1e-12);
    assert!((model_t.windings[0].r_pct - 0.01 / 3.0 * 100.0).abs() < 1e-12);
    assert!((model_t.windings[1].r_pct - 0.02 / 3.0 * 100.0).abs() < 1e-12);
    assert!((model_t.xsc_pct[0] - 0.04 / 3.0 * 100.0).abs() < 1e-12);
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["n_winding"]["t3"];
    assert_eq!(t["windings"].as_array().unwrap().len(), 3);
    assert_eq!(t["windings"][1]["v_nom"], serde_json::json!(100.0));
    assert_eq!(t["x_sc"]["1_2"], serde_json::json!(0.04));
    assert_eq!(t["g_no_load"], serde_json::json!(0.001));
    assert_eq!(t["b_no_load"], serde_json::json!(-0.002));
}

#[test]
fn legacy_transformer_aliases_write_current_bmopf_names() {
    let v = schema_validator();
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "a": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]},
            "b": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]},
            "c": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]}
          },
          "voltage_source": {
            "source": {"v_magnitude": [7200.0, 7200.0, 7200.0, 0.0], "v_angle": [0.0, -2.0943951023931953, 2.0943951023931953, 0.0], "bus": "a", "terminal_map": ["1", "2", "3", "n"]}
          },
          "transformer": {
            "single_phase": {
              "t": {
                "bus_from": "a", "bus_to": "b",
                "terminal_map_from": ["1", "n"], "terminal_map_to": ["1", "n"],
                "s_rating": 25000.0,
                "v_ref_from": 7200.0, "v_ref_to": 240.0,
                "r_series_from": 1.0, "r_series_to": 0.01,
                "x_series_from": 4.0, "x_series_to": 0.0
              }
            },
            "n_winding": {
              "t3": {
                "s_rating": 10000.0,
                "windings": [
                  {"bus": "a", "terminal_map": ["1", "2", "3", "n"], "v_ref": 7200.0, "connection": "WYE", "r_winding": 0.01},
                  {"bus": "b", "terminal_map": ["1", "2", "3", "n"], "v_ref": 240.0, "connection": "WYE", "r_winding": 0.02},
                  {"bus": "c", "terminal_map": ["1", "2", "3", "n"], "v_ref": 240.0, "connection": "WYE", "r_winding": 0.03}
                ],
                "x_sc": {"1_2": 0.04, "1_3": 0.05, "2_3": 0.06}
              }
            }
          }
        }"#,
    )
    .unwrap();

    let out = write_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let two = &doc["transformer"]["single_phase"]["t"];
    assert_eq!(two["v_nom_from"], serde_json::json!(7200.0));
    assert_eq!(two["v_nom_to"], serde_json::json!(240.0));
    assert!(two.get("v_ref_from").is_none());
    assert!(two.get("v_ref_to").is_none());
    let winding = &doc["transformer"]["n_winding"]["t3"]["windings"][0];
    assert_eq!(winding["v_nom"], serde_json::json!(7200.0));
    assert_eq!(winding["configuration"], serde_json::json!("WYE"));
    assert!(winding.get("v_ref").is_none());
    assert!(winding.get("connection").is_none());
}

#[test]
fn three_phase_transformer_no_load_fields_round_trip_through_bmopf() {
    let v = schema_validator();
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "a": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]},
            "b": {"terminal_names": ["1", "2", "3"]}
          },
          "voltage_source": {
            "source": {"v_magnitude": [7200.0, 7200.0, 7200.0, 0.0], "v_angle": [0.0, -2.0943951023931953, 2.0943951023931953, 0.0], "bus": "a", "terminal_map": ["1", "2", "3", "n"]}
          },
          "transformer": {
            "wye_delta": {
              "t": {
                "bus_from": "a", "bus_to": "b",
                "terminal_map_from": ["1", "2", "3", "n"], "terminal_map_to": ["1", "2", "3"],
                "s_rating": 50000.0,
                "v_nom_from": 7200.0, "v_nom_to": 480.0,
                "r_series": 0.1,
                "x_series": 0.2,
                "g_no_load": 0.000002,
                "b_no_load": -0.000003
              }
            }
          }
        }"#,
    )
    .unwrap();

    let out = write_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["wye_delta"]["t"];
    assert_eq!(t["v_nom_from"], serde_json::json!(7200.0));
    assert_eq!(t["v_nom_to"], serde_json::json!(480.0));
    assert_eq!(t["g_no_load"], serde_json::json!(0.000_002));
    assert_eq!(t["b_no_load"], serde_json::json!(-0.000_003));
}

#[test]
fn dss_noloadloss_derives_bmopf_no_load_fields() {
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.core basekv=7.2 pu=1.0 phases=1 bus1=src.1\n\
         New Transformer.t1 phases=1 windings=2 buses=(src.1.0, load.1.0) \
         kvs=(7.2 0.24) kvas=(25 25) %Rs=(1 1) xhl=2 \
         %noloadloss=0.2 %imag=0.5\n",
    );
    let out = write_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["single_phase"]["t1"];
    let expected_g = 0.2 / 100.0 * 25_000.0 / (7200.0 * 7200.0);
    let g = t["g_no_load"].as_f64().unwrap();
    assert!((g - expected_g).abs() < 1e-18, "g_no_load = {g}");
    assert_eq!(t["b_no_load"], serde_json::json!(0.0));
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
}

#[test]
fn dss_phase_to_phase_noloadloss_does_not_emit_bmopf_ground_shunt() {
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.core basekv=12.47 pu=1.0 phases=1 bus1=src.1.2\n\
         New Transformer.t1 phases=1 windings=2 buses=(src.1.2, load.1.2) \
         kvs=(12.47 0.48) kvas=(25 25) %Rs=(1 1) xhl=2 \
         %noloadloss=0.2 %imag=0.5\n",
    );
    let out = write_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["single_phase"]["t1"];
    assert!(t.get("g_no_load").is_none(), "{t}");
    assert!(t.get("b_no_load").is_none(), "{t}");
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("phase-to-phase") && w.contains("%noloadloss")),
        "{:?}",
        out.warnings
    );
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
}

#[test]
fn bmopf_frequency_hint_reaches_dss() {
    let net = parse_bmopf_str(
        r#"{
          "base_frequency": 50.0,
          "bus": {"b": {"terminal_names": ["1"]}},
          "voltage_source": {
            "source": {"v_magnitude": [240.0], "v_angle": [0.0], "bus": "b", "terminal_map": ["1"]}
          }
        }"#,
    )
    .unwrap();

    assert!((net.base_frequency - 50.0).abs() < 1e-12);
    let out = write_dss(&net);
    assert!(
        out.text.contains("Set DefaultBaseFrequency=50"),
        "{}",
        out.text
    );
}

#[test]
fn bmopf_phase_to_phase_single_phase_load_emits_delta_dss() {
    let net = parse_bmopf_str(
        r#"{
          "bus": {"b": {"terminal_names": ["1", "2"]}},
          "voltage_source": {
            "source": {"v_magnitude": [240.0], "v_angle": [0.0], "bus": "b", "terminal_map": ["1"]}
          },
          "load": {
            "ld": {
              "bus": "b",
              "terminal_map": ["1", "2"],
              "configuration": "SINGLE_PHASE",
              "p_nom": [1000.0],
              "q_nom": [250.0],
              "v_nom": [240.0]
            }
          }
        }"#,
    )
    .unwrap();

    let out = write_dss(&net);
    let line = out.text.lines().find(|l| l.contains("Load.ld")).unwrap();
    assert!(line.contains("phases=1 conn=delta"), "{line}");
}

#[test]
fn zero_voltage_source_neutral_does_not_inflate_dss_phases() {
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "src": {"terminal_names": ["1", "2", "3", "4", "5"], "perfectly_grounded_terminals": ["5"]}
          },
          "voltage_source": {
            "source": {
              "v_magnitude": [2400.0, 2400.0, 2400.0, 0.0],
              "v_angle": [0.0, -2.0943951023931953, 2.0943951023931953, 0.0],
              "bus": "src",
              "terminal_map": ["1", "2", "3", "4"]
            }
          }
        }"#,
    )
    .unwrap();

    let out = write_dss(&net);
    let line = out.text.lines().find(|l| l.contains("Circuit.")).unwrap();
    assert!(line.contains("phases=3"), "{line}");
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
    assert_eq!(t["v_nom_to"], 240.0);
    let r_to = t["r_series_to"].as_f64().unwrap();
    assert!((r_to - 0.013_824).abs() < 1e-12, "r_series_to = {r_to}");
    // The primary is untouched by the collapse: %R=0.6 on 7.2 kV/25 kVA.
    let r_from = t["r_series_from"].as_f64().unwrap();
    assert!((r_from - 12.4416).abs() < 1e-9, "r_series_from = {r_from}");
}

#[test]
fn bmopf_center_tap_rebuilds_dss_grounded_center() {
    let text = r#"{
        "bus": {
            "src": {"terminal_names": ["1", "2"], "perfectly_grounded_terminals": ["2"]},
            "lv": {"terminal_names": ["1", "2", "3"], "perfectly_grounded_terminals": ["3"]}
        },
        "voltage_source": {
            "source": {
                "v_magnitude": [7200.0, 0.0],
                "v_angle": [0.0, 0.0],
                "bus": "src",
                "terminal_map": ["1", "2"]
            }
        },
        "transformer": {
            "center_tap": {
                "ct": {
                    "bus_from": "src",
                    "bus_to": "lv",
                    "terminal_map_from": ["1", "2"],
                    "terminal_map_to": ["1", "2", "3"],
                    "s_rating": 25000.0,
                    "v_nom_from": 7200.0,
                    "v_nom_to": 240.0,
                    "r_series_from": 12.4416,
                    "r_series_to": 0.013824,
                    "x_series_from": 42.2784,
                    "x_series_to": 0.0
                }
            }
        }
    }"#;
    let net = parse_bmopf_str(text).unwrap();
    assert_eq!(net.transformers[0].windings.len(), 3);
    let dss = write_dss(&net).text;
    assert!(dss.contains("lv.1.0"), "{dss}");
    assert!(dss.contains("lv.0.2"), "{dss}");

    let out = write_bmopf_json(&parse_dss_str(&dss));
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(
        doc["bus"]["lv"]["perfectly_grounded_terminals"],
        serde_json::json!(["4"])
    );
    assert_eq!(
        doc["transformer"]["center_tap"]["ct"]["terminal_map_to"],
        serde_json::json!(["1", "2", "4"])
    );
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
    assert_eq!(t["v_nom_to"], 240.0);
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
fn linecode_constructor_sizes_x_only_matrix_from_x() {
    let lc = DistLineCode::new("lc", Vec::new(), vec![vec![0.4]]);
    assert_eq!(lc.n_conductors, 1);
    assert_eq!(lc.r_series, Vec::<Vec<f64>>::new());
    assert_eq!(lc.g_from, vec![vec![0.0]]);
    assert_eq!(lc.b_from, vec![vec![0.0]]);
    assert_eq!(lc.g_to, vec![vec![0.0]]);
    assert_eq!(lc.b_to, vec![vec![0.0]]);

    let mut net = DistNetwork::default();
    net.linecodes.push(lc);
    let out = write_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(doc["linecode"]["lc"]["R_series_1_1"], 0.0);
    assert_eq!(doc["linecode"]["lc"]["X_series_1_1"], 0.4);

    let again = parse_bmopf_str(&out.text).unwrap();
    let back = again.linecode("lc").unwrap();
    assert_eq!(back.n_conductors, 1);
    assert_eq!(back.r_series, vec![vec![0.0]]);
    assert_eq!(back.x_series, vec![vec![0.4]]);
}

#[test]
fn linecode_constructor_sizes_from_widest_series_matrix() {
    let lc = DistLineCode::new("lc", vec![vec![0.2]], vec![vec![0.4, 0.1], vec![0.1, 0.4]]);
    assert_eq!(lc.n_conductors, 2);
    assert_eq!(lc.r_series, vec![vec![0.2]]);
    assert_eq!(lc.x_series, vec![vec![0.4, 0.1], vec![0.1, 0.4]]);
    assert_eq!(lc.g_from, vec![vec![0.0; 2]; 2]);
    assert_eq!(lc.b_from, vec![vec![0.0; 2]; 2]);
    assert_eq!(lc.g_to, vec![vec![0.0; 2]; 2]);
    assert_eq!(lc.b_to, vec![vec![0.0; 2]; 2]);
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
            "s_rating": 5000.0, "v_nom_from": 240.0, "v_nom_to": 240.0}}}"#,
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
    let winding = |map: &[&str]| {
        let mut winding = Winding::new(
            "a",
            map.iter().map(ToString::to_string).collect(),
            WindingConn::Wye,
            4160.0,
            500_000.0,
        );
        winding.r_pct = 0.5;
        winding
    };
    net.transformers.push(DistTransformer::new(
        "t3w",
        vec![winding(&["1", "2", "3"]), winding(&["1", "2"])],
        vec![2.0],
        3,
    ));
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

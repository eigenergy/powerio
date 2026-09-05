//! BMOPF reader/writer against the two vendored schema versions and the two
//! public example networks from distribution-system-opt/bmopf-resources.

use std::path::PathBuf;

use powerio_dist::{
    BmopfEmitOptions, BmopfProfile, Configuration, CoordinateSpace, DiagnosticSeverity,
    DiagnosticStage, DistBus, DistCoordsKind, DistGeoMeta, DistLineCode, DistLocation,
    DistTransformer, DistWinding, DistWindingConn, Extras, MulticonductorNetwork, VoltageSource,
};

mod helpers;
use helpers::{
    emit_bmopf_json, emit_bmopf_json_with_options, emit_dss, parse_bmopf_file, parse_bmopf_str,
    parse_dss_file, parse_dss_str, parse_pmd_str,
};

/// The findings beside the schema version notice.
///
/// A document that states no `meta.$schema` names no schema version, which
/// the reader reports; the inline documents below state none, so every one of
/// them carries that finding and it is never what a test is about.
fn beside_schema_version(warnings: &[String]) -> Vec<&String> {
    warnings
        .iter()
        .filter(|w| !w.starts_with("READ.BMOPF.SCHEMA_"))
        .collect()
}

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist")
        .join(rel)
}

#[test]
fn nonuniform_phase_bounds_survive_value_serialization_and_emission() {
    let input = r#"{"bus":{"b":{"terminal_names":["a","b","c","n"],"v_min":[210,215,220],"v_max":[240,245,250]}},"voltage_source":{"s":{"bus":"b","terminal_map":["a","b","c","n"],"v_magnitude":[230,230,230,0],"v_angle":[0,-2.094,2.094,0]}}}"#;
    let net = parse_bmopf_str(input).unwrap();
    assert_eq!(net.buses()[0].v_min, None);
    assert_eq!(
        net.buses()[0].v_min_phase.as_deref(),
        Some([210.0, 215.0, 220.0].as_slice())
    );
    let encoded = serde_json::to_string(&*net).unwrap();
    let restored: MulticonductorNetwork = serde_json::from_str(&encoded).unwrap();
    let output = emit_bmopf_json(&restored);
    let doc: serde_json::Value = serde_json::from_str(&output.text).unwrap();
    assert_eq!(
        doc["bus"]["b"]["v_min"],
        serde_json::json!([210.0, 215.0, 220.0])
    );
    assert_eq!(
        doc["bus"]["b"]["v_max"],
        serde_json::json!([240.0, 245.0, 250.0])
    );
    assert!(schema_validator().is_valid(&doc));
}

#[test]
fn contradictory_schema_versions_are_reported_as_errors() {
    let input = format!(
        r#"{{"meta":{{"$schema":"{}","schema_version":"0.1.0"}},"bus":{{}}}}"#,
        BmopfProfile::Bmopf020.retrieval_url()
    );
    let parsed = parse_bmopf_str(&input).unwrap();
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.code() == "READ.BMOPF.SCHEMA_MISMATCH"
                && d.severity() == DiagnosticSeverity::Error)
    );
}

/// The validator for one vendored schema version.
fn schema_validator_for(profile: BmopfProfile) -> jsonschema::Validator {
    let file = match profile {
        BmopfProfile::Bmopf010 => "bmopf/draft_bmopf_schema.json",
        BmopfProfile::Bmopf020 => "bmopf/bmopf-0.2.0.schema.json",
        _ => unreachable!("every schema version has a vendored document"),
    };
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture(file)).unwrap()).unwrap();
    jsonschema::validator_for(&schema).expect("vendored schema compiles")
}

/// The validator for the version the writer targets by default.
fn schema_validator() -> jsonschema::Validator {
    schema_validator_for(BmopfProfile::default())
}

fn errors(validator: &jsonschema::Validator, text: &str) -> Vec<String> {
    let doc: serde_json::Value = serde_json::from_str(text).unwrap();
    validator
        .iter_errors(&doc)
        .map(|e| format!("{}: {e}", e.instance_path()))
        .collect()
}

#[test]
fn bmopf_sideloaded_coordinates_promote_to_locations() {
    let net = parse_bmopf_str(
        r#"{
  "bus": {
    "b1": {
      "terminal_names": ["1", "2", "3", "4"],
      "perfectly_grounded_terminals": ["4"],
      "longitude": -80.0,
      "latitude": 35.0
    }
  }
}"#,
    )
    .unwrap();
    assert!(
        beside_schema_version(&net.warnings).is_empty(),
        "{:?}",
        net.warnings
    );
    let geo = net.geo().as_ref().expect("geo metadata");
    assert_eq!(geo.space, CoordinateSpace::Geographic { crs: None });
    assert_eq!(geo.kind, Some(DistCoordsKind::Source));
    let bus = &net.buses()[0];
    assert_eq!(bus.location.unwrap().x.to_bits(), (-80.0f64).to_bits());
    assert_eq!(bus.location.unwrap().y.to_bits(), 35.0f64.to_bits());
    assert!(!bus.extras.contains_key("longitude"));
    assert!(!bus.extras.contains_key("latitude"));
}

fn diagnostic<'a>(
    conv: &'a helpers::Conv,
    code: &str,
    element_path: &str,
) -> &'a powerio_dist::Diagnostic {
    conv.diagnostics
        .iter()
        .find(|d| d.code() == code && d.target() == Some(element_path))
        .unwrap_or_else(|| panic!("missing diagnostic {code} for {element_path}: {conv:?}"))
}

#[test]
fn vendored_examples_validate_after_canonicalization() {
    let v = schema_validator();
    for example in ["bmopf/example_ieee13.json", "bmopf/example_enwl_n1_f2.json"] {
        let text = std::fs::read_to_string(fixture(example)).unwrap();
        let net = parse_bmopf_str(&text).unwrap();
        let out = emit_bmopf_json(&net);
        assert_eq!(errors(&v, &out.text), Vec::<String>::new(), "{example}");
    }
}

/// Both vendored fixtures are schema 0.1.0 documents and validate as shipped
/// against that version, so any raw validation failure is a real schema
/// mismatch.
#[test]
fn vendored_examples_raw_validation_is_known_and_bounded() {
    let v = schema_validator_for(BmopfProfile::Bmopf010);
    for example in ["bmopf/example_ieee13.json", "bmopf/example_enwl_n1_f2.json"] {
        let text = std::fs::read_to_string(fixture(example)).unwrap();
        assert_eq!(errors(&v, &text), Vec::<String>::new(), "{example}");
    }
}

/// BMOPFTools spells symmetric matrices as the upper triangle only; the
/// reader mirrors the unspelled transpose cells.
#[test]
fn one_triangle_matrix_spelling_mirrors_on_read() {
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "a": {"terminal_names": ["1", "2"]},
            "b": {"terminal_names": ["1", "2"]}
          },
          "voltage_source": {
            "s": {"v_magnitude": [240.0, 240.0], "v_angle": [0.0, 0.0], "bus": "a", "terminal_map": ["1", "2"]}
          },
          "linecode": {
            "lc": {
              "R_series_1_1": 1.0, "R_series_1_2": 0.25, "R_series_2_2": 1.0,
              "X_series_1_1": 2.0, "X_series_1_2": 0.5, "X_series_2_2": 2.0
            }
          },
          "line": {
            "l": {"bus_from": "a", "bus_to": "b", "linecode": "lc", "length": 10.0,
                  "terminal_map_from": ["1", "2"], "terminal_map_to": ["1", "2"]}
          }
        }"#,
    )
    .unwrap();
    let lc = &net.line_codes()[0];
    assert_eq!(lc.r_series[0][1].to_bits(), 0.25f64.to_bits());
    assert_eq!(lc.r_series[1][0].to_bits(), 0.25f64.to_bits());
    assert_eq!(lc.x_series[1][0].to_bits(), 0.5f64.to_bits());
}

#[test]
fn parse_the_public_examples() {
    let net = parse_bmopf_file(fixture("bmopf/example_ieee13.json")).unwrap();
    assert_eq!(net.buses().len(), 7);
    assert_eq!(net.lines().len(), 4);
    assert_eq!(net.loads().len(), 5);
    assert_eq!(net.generators().len(), 1);
    assert_eq!(net.transformers().len(), 2);
    assert_eq!(net.sources().len(), 1);
    assert!(net.warnings.is_empty(), "{:?}", net.warnings);

    let enwl = parse_bmopf_file(fixture("bmopf/example_enwl_n1_f2.json")).unwrap();
    assert_eq!(enwl.buses().len(), 506);
    assert_eq!(enwl.lines().len(), 505);
    assert_eq!(enwl.loads().len(), 31);
    assert_eq!(enwl.shunts().len(), 1);
    // The ENWL example routes OpenDSS earth terminals to the bus neutral;
    // buses carry grounded terminals.
    assert!(enwl.buses().iter().any(|b| !b.grounded.is_empty()));
}

#[test]
fn written_output_validates_and_round_trips() {
    let v = schema_validator();
    let net = parse_bmopf_file(fixture("bmopf/example_ieee13.json")).unwrap();
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    // All bus terminals remain available for subsequent interventions.
    assert!(
        out.warnings
            .iter()
            .all(|w| w.contains("not referenced by emitted BMOPF elements")),
        "{:?}",
        out.warnings
    );

    // Canonical output preserves the electrical model on each round trip.
    let canonical = parse_bmopf_str(&out.text).unwrap();
    let out2 = emit_bmopf_json(&canonical);
    assert_eq!(errors(&v, &out2.text), Vec::<String>::new());
    let again = parse_bmopf_str(&out2.text).unwrap();
    assert_model_eq(&canonical, &again);

    // And byte idempotence of the canonical form.
    let out3 = emit_bmopf_json(&again);
    assert_eq!(out2.text, out3.text);
}

#[test]
fn enwl_round_trips() {
    let v = schema_validator();
    let net = parse_bmopf_file(fixture("bmopf/example_enwl_n1_f2.json")).unwrap();
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    // Canonical emission is idempotent.
    let canonical = parse_bmopf_str(&out.text).unwrap();
    let out2 = emit_bmopf_json(&canonical);
    let again = parse_bmopf_str(&out2.text).unwrap();
    assert_model_eq(&canonical, &again);
}

/// Model equality minus the retained source (which differs by format) and
/// the stashed `meta` block (the writer regenerates its own provenance, so a
/// round trip replaces the source document's).
fn assert_model_eq(a: &MulticonductorNetwork, b: &MulticonductorNetwork) {
    let strip = |n: &MulticonductorNetwork| {
        let mut n = n.clone();
        n.extras_mut().remove("bmopf_meta");
        n
    };
    let (a, b) = (strip(a), strip(b));
    assert_eq!(a.buses(), b.buses());
    assert_eq!(a.line_codes(), b.line_codes());
    assert_eq!(a.lines(), b.lines());
    assert_eq!(a.switches(), b.switches());
    assert_eq!(a.loads(), b.loads());
    assert_eq!(a.generators(), b.generators());
    assert_eq!(a.ibrs(), b.ibrs());
    assert_eq!(a.control_profiles(), b.control_profiles());
    assert_eq!(a.shunts(), b.shunts());
    assert_eq!(a.sources(), b.sources());
    assert_eq!(a.transformers(), b.transformers());
}

/// Every distribution case fixture, whatever its source format, emits a
/// document that the vendored schema 0.1.0 accepts. The emitted document
/// must also read back, re-emit valid, and re-emit byte identical, so a
/// schema break shows up here whether it is in the writer or the reader.
#[test]
fn every_dist_fixture_emits_valid_bmopf() {
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
        "micro/ibr_pv_control.dss",
        "micro/linecode_10x10.dss",
        "micro/neutral_grounding_reactor.dss",
        "micro/onephase_cvr_load.dss",
        "micro/onephase_zip_load.dss",
        "pmd/ieee13.json",
        "pmd/fourwire_linecode.json",
        "bmopf/example_ieee13.json",
        "bmopf/example_enwl_n1_f2.json",
    ] {
        let net = helpers::parse_file(fixture(case), None).unwrap();
        let out = emit_bmopf_json(&net);
        assert_eq!(errors(&v, &out.text), Vec::<String>::new(), "{case}");
        let again = parse_bmopf_str(&out.text).unwrap();
        let twice = emit_bmopf_json(&again);
        assert_eq!(
            errors(&v, &twice.text),
            Vec::<String>::new(),
            "{case} round trip"
        );
        assert_eq!(out.text, twice.text, "{case} is not idempotent");
    }
}

/// A reference to a bus that does not exist warns instead of parsing
/// silently: the graph projection would synthesize a phantom bus for it.
#[test]
fn bmopf_dangling_bus_reference_warns() {
    let net = parse_bmopf_str(
        r#"{
        "bus": {"a": {"terminal_names": ["p1", "n"]}},
        "voltage_source": {"src": {"bus": "a", "terminal_map": ["p1", "n"],
            "v_magnitude": [240.0], "v_angle": [0.0]}},
        "load": {"ld1": {"bus": "typo", "terminal_map": ["p1", "n"],
            "configuration": "SINGLE_PHASE", "p_nom": [1000.0], "q_nom": [0.0],
            "model": "CONSTANT_POWER"}}
    }"#,
    )
    .unwrap();
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("load ld1") && w.contains("undefined bus `typo`")),
        "{:?}",
        net.warnings
    );
}

/// An ibr or control profile declared twice under one class, differing only
/// by case, is the schema 0.1.0 top level plus `extras` migration overlap:
/// the second copy drops with a warning rather than doubling the count.
#[test]
fn an_ibr_declared_twice_differing_only_by_case_drops_the_second() {
    let net = parse_bmopf_str(
        r#"{
        "bus": {"a": {"terminal_names": ["p1", "n"]}},
        "ibr": {
            "Gen1": {"bus": "a", "terminal_map": ["p1", "n"]},
            "gen1": {"bus": "a", "terminal_map": ["p1", "n"]}
        }
    }"#,
    )
    .unwrap();
    assert_eq!(net.ibrs().len(), 1, "{:?}", net.ibrs());
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("ibr gen1") && w.contains("second copy is dropped")),
        "{:?}",
        net.warnings
    );
}

#[test]
fn a_control_profile_declared_twice_differing_only_by_case_drops_the_second() {
    let net = parse_bmopf_str(
        r#"{
        "control_profile": {
            "Cp1": {"power_factor": {"pf": 0.95}},
            "cp1": {"power_factor": {"pf": 0.9}}
        }
    }"#,
    )
    .unwrap();
    assert_eq!(
        net.control_profiles().len(),
        1,
        "{:?}",
        net.control_profiles()
    );
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("control_profile cp1") && w.contains("second copy is dropped")),
        "{:?}",
        net.warnings
    );
}

/// PMD spells an unbounded phase as JSON null, which restores as Inf.
/// BMOPF has no unbounded spelling: the rating field drops with a warning
/// instead of the zero fallback turning "no limit" into a zero limit, and
/// the finite sibling field survives.
#[test]
fn nonfinite_line_ratings_drop_instead_of_zeroing() {
    let text = r#"{
        "data_model": "ENGINEERING",
        "bus": {
            "b1": {"terminals": [1], "grounded": [], "rg": [], "xg": [], "status": "ENABLED"},
            "b2": {"terminals": [1], "grounded": [], "rg": [], "xg": [], "status": "ENABLED"}
        },
        "linecode": {"lc": {"rs": [[0.1]], "xs": [[0.1]],
            "g_fr": [[0.0]], "g_to": [[0.0]], "b_fr": [[0.0]], "b_to": [[0.0]]}},
        "line": {"ln1": {"f_bus": "b1", "t_bus": "b2",
            "f_connections": [1], "t_connections": [1], "length": 10.0,
            "linecode": "lc", "cm_ub": [null], "sm_ub": [600.0],
            "status": "ENABLED"}}
    }"#;
    let net = parse_pmd_str(text).unwrap();
    let out = emit_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert!(doc["line"]["ln1"].get("i_max").is_none());
    assert_eq!(doc["line"]["ln1"]["s_max"], serde_json::json!([600.0]));
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("i_max") && w.contains("dropped")),
        "{:?}",
        out.warnings
    );
    assert!(
        !out.warnings.iter().any(|w| w.contains("emitted as 0")),
        "{:?}",
        out.warnings
    );
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
    let out = emit_bmopf_json(&net);
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
    let out = emit_bmopf_json(&net);
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
    let t1 = net.transformers().iter().find(|t| t.name == "t1").unwrap();
    assert_eq!(t1.windings[1].conn, DistWindingConn::Delta);
    assert_eq!(t1.windings[1].terminal_map, vec!["1", "2"]);
    let t2 = net.transformers().iter().find(|t| t.name == "t2").unwrap();
    assert_eq!(t2.windings[1].terminal_map, vec!["2", "3"]);

    let out = emit_bmopf_json(&net);
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
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("EMIT.BMOPF.TRANSFORMER_CONNECTION_LOSSY")),
        "missing the wye/delta diagnostic code: {:?}",
        out.warnings
    );
    let diag = diagnostic(
        &out,
        "EMIT.BMOPF.TRANSFORMER_CONNECTION_LOSSY",
        "transformer t1",
    );
    assert_eq!(diag.severity(), DiagnosticSeverity::Warning);
    assert_eq!(diag.stage(), Some(DiagnosticStage::Emit));
    assert_eq!(diag.details()["transformer"], serde_json::json!("t1"));
    assert_eq!(diag.details()["connection"], serde_json::json!("wye/delta"));
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let sp = &doc["transformer"]["single_phase"];
    assert_eq!(sp["t1"]["terminal_map_to"], serde_json::json!(["1", "2"]));
    assert_eq!(sp["t2"]["terminal_map_to"], serde_json::json!(["2", "3"]));

    // The other orientation: a delta primary (line to line, on .1.2) into a
    // grounded wye secondary keeps both primary terminals.
    let dw = parse_dss_file(fixture("micro/xfmr_1ph_delta_wye.dss")).unwrap();
    let t = &dw.transformers()[0];
    assert_eq!(t.windings[0].conn, DistWindingConn::Delta);
    assert_eq!(t.windings[0].terminal_map, vec!["1", "2"]);
    let out = emit_bmopf_json(&dw);
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
    assert!(
        !out.text.contains("EMIT.BMOPF"),
        "diagnostic codes must stay out of BMOPF JSON: {}",
        out.text
    );
}

#[test]
fn delta_wye_leakage_uses_each_winding_base() {
    let v = schema_validator();
    let net = parse_dss_file(fixture("micro/xfmr_delta_wye.dss")).unwrap();
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());

    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["delta_wye"]["t1"];
    // Schema 0.1.0 three phase transformers carry one lumped pair on the wye
    // side; the split `_from`/`_to` fields lost their slots.
    assert!(t.get("r_series_from").is_none(), "{t:?}");
    assert!(t.get("x_series_from").is_none(), "{t:?}");

    let z_wye = 208.0 * 208.0 / 300_000.0;
    let r = t["r_series"].as_f64().unwrap();
    let x = t["x_series"].as_f64().unwrap();
    assert!((r - 0.01 * z_wye).abs() < 1e-12, "r_series = {r}");
    assert!((x - 0.0575 * z_wye).abs() < 1e-12, "x_series = {x}");

    let round_trip = parse_bmopf_str(&out.text).unwrap();
    let t = round_trip
        .transformers()
        .iter()
        .find(|t| t.name == "t1")
        .unwrap();
    assert_eq!(t.windings[0].conn, DistWindingConn::Delta);
    assert_eq!(t.windings[1].conn, DistWindingConn::Wye);
    assert!((t.windings[0].r_pct - 0.5).abs() < 1e-12);
    assert!((t.windings[1].r_pct - 0.5).abs() < 1e-12);
    assert_eq!(t.xsc_pct.len(), 1);
    assert!((t.xsc_pct[0] - 5.75).abs() < 1e-12);
}

#[test]
fn delta_wye_split_leakage_uses_each_winding_rating() {
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.dw basekv=12.47 pu=1.0 phases=3 bus1=sourcebus\n\
         New Transformer.t1 phases=3 windings=2 buses=(sourcebus, secondary) \
         conns=(delta, wye) kvs=(12.47, 0.208) kvas=(500, 300) \
         xhl=5.75 %Rs=(0.5, 0.7)\n",
    );
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());

    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["delta_wye"]["t1"];
    // The lumped wye-side pair refers each winding's percent resistance to
    // its own rating base before summing; XHL is on the first winding's base.
    let v_wye2 = 208.0 * 208.0;
    let r = t["r_series"].as_f64().unwrap();
    let x = t["x_series"].as_f64().unwrap();
    let expect_r = (0.005 / 500_000.0 + 0.007 / 300_000.0) * v_wye2;
    let expect_x = 0.0575 * v_wye2 / 500_000.0;
    assert!((r - expect_r).abs() < 1e-12, "r_series = {r}");
    assert!((x - expect_x).abs() < 1e-12, "x_series = {x}");
}

#[test]
fn wye_delta_leakage_stays_on_legacy_wye_side_fields() {
    let v = schema_validator();
    let net = parse_dss_file(fixture("micro/xfmr_wye_delta.dss")).unwrap();
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());

    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["wye_delta"]["t1"];
    assert!(t.get("r_series_from").is_none(), "{t:?}");
    assert!(t.get("r_series_to").is_none(), "{t:?}");
    assert!(t.get("x_series_from").is_none(), "{t:?}");
    assert!(t.get("x_series_to").is_none(), "{t:?}");

    let z_wye = 12_470.0 * 12_470.0 / 500_000.0;
    let r = t["r_series"].as_f64().unwrap();
    let x = t["x_series"].as_f64().unwrap();
    assert!((r - 0.01 * z_wye).abs() < 1e-12, "r_series = {r}");
    assert!((x - 0.0575 * z_wye).abs() < 1e-12, "x_series = {x}");
}

#[test]
fn ieee13_conversion_warnings_name_every_loss() {
    let net = parse_dss_file(fixture("opendss/ieee13/IEEE13Nodeckt.dss")).unwrap();
    let out = emit_bmopf_json(&net);
    // The wye-wye XFM1 decomposes; regulators and coordinates drop loudly.
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("XFM1") && w.contains("single_phase"))
    );
    assert!(out.warnings.iter().any(|w| w.contains("regcontrol")));
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("EMIT.BMOPF.REGCONTROL_DROPPED")),
        "missing regcontrol diagnostic code: {:?}",
        out.warnings
    );
    let diag = diagnostic(&out, "EMIT.BMOPF.REGCONTROL_DROPPED", "regcontrol Reg1");
    assert_eq!(diag.severity(), DiagnosticSeverity::Warning);
    assert_eq!(diag.stage(), Some(DiagnosticStage::Emit));
    assert_eq!(diag.details()["class"], serde_json::json!("regcontrol"));
    // No silent extras: every message leads with a `class name:` element
    // identifier ("load 671: ...", "voltage source source: ..."), or is an
    // aggregated per-field record naming its elements in details (#377). The
    // line the text channel carries leads with the code, so the check reads
    // the record.
    for d in &out.diagnostics {
        if let Some(elements) = d.details().get("elements") {
            let elements = elements.as_array().expect("elements is a list");
            assert!(
                !elements.is_empty()
                    && elements
                        .iter()
                        .all(|e| e.as_str().is_some_and(|e| e.contains(' '))),
            );
            continue;
        }
        let Some((head, _)) = d.message().split_once(": ") else {
            panic!("warning has no `class name:` prefix: {}", d.message());
        };
        assert!(
            head.split_whitespace().count() >= 2,
            "warning does not name its element: {}",
            d.message()
        );
    }
}

#[test]
fn ten_conductor_linecode_is_schema_valid() {
    let v = schema_validator();
    let net = parse_dss_file(fixture("micro/linecode_10x10.dss")).unwrap();
    let out = emit_bmopf_json(&net);
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
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    assert!(
        out.warnings.iter().all(|w| !w.contains("negative load")),
        "obsolete fixed generator warning: {:?}",
        out.warnings
    );
    assert!(
        out.diagnostics.iter().any(
            |d| d.message() == "generator g1: no generation cost in the source; emitted cost 0"
        ),
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

    let out = emit_bmopf_json(&net);

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
            "cp": {
              "power_factor": {"pf": 0.98},
              "volt_var": {
                "voltage_reference": "PN_PER_PHASE",
                "breakpoints": [216.0, 228.0, 252.0, 264.0],
                "q_limits": [-0.44, 0.44],
                "q_unit": "VA_FRACTION",
                "q_ref": "VAR_MAX",
                "p_min_for_q": 10.0,
                "p_min_for_q_max": 50.0
              }
            }
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

    assert_eq!(net.ibrs().len(), 1);
    assert_eq!(net.control_profiles().len(), 1);
    let ibr = &net.ibrs()[0];
    assert_eq!(ibr.name, "pv");
    assert_eq!(ibr.control_profile.as_deref(), Some("cp"));
    let cp = &net.control_profiles()[0];
    let vv = cp.volt_var.as_ref().expect("volt var typed");
    assert_eq!(vv.q_limits, vec![-0.44, 0.44]);
    assert_eq!(vv.p_min_for_q, Some(10.0));
    assert_eq!(vv.p_min_for_q_max, Some(50.0));

    let out = emit_bmopf_json(&net);

    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(doc["ibr"]["pv"]["control_profile"], "cp");
    assert_eq!(doc["control_profile"]["cp"]["power_factor"]["pf"], 0.98);
    assert_eq!(
        doc["control_profile"]["cp"]["volt_var"]["voltage_reference"],
        "PN_PER_PHASE"
    );
    assert_eq!(doc["control_profile"]["cp"]["volt_var"]["q_ref"], "VAR_MAX");
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("ibr") && !w.contains("control_profile")),
        "{:?}",
        out.warnings
    );
    let again = parse_bmopf_str(&out.text).unwrap();
    assert_model_eq(&net, &again);
}

#[test]
fn bmopf_fixed_dispatch_ibr_emits_dss_generator() {
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
          "ibr": {
            "pv": {
              "bus": "b",
              "terminal_map": ["1", "2", "3", "n"],
              "topology": "FOUR_LEG",
              "prime_mover": "PV",
              "s_max": [10000.0, 10000.0, 10000.0],
              "p_min": [8000.0, 8000.0, 8000.0],
              "p_max": [8000.0, 8000.0, 8000.0],
              "q_min": [0.0, 0.0, 0.0],
              "q_max": [0.0, 0.0, 0.0]
            }
          }
        }"#,
    )
    .unwrap();

    let out = emit_dss(&net);

    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let line = out
        .text
        .lines()
        .find(|l| l.starts_with("New Generator.pv"))
        .unwrap();
    assert!(line.contains("model=1 vminpu=0 vmaxpu=2"), "{line}");
    assert!(line.contains("kw=24"), "{line}");
    assert!(!out.text.contains("PVSystem.pv"), "{}", out.text);
}

#[test]
fn bmopf_volt_var_ibr_emits_dss_pvsystem_control() {
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
            "cp": {
              "volt_var": {
                "voltage_reference": "PG_AVERAGED",
                "breakpoints": [220.8, 235.2, 244.8, 259.2],
                "q_limits": [-0.44, 0.44],
                "q_unit": "VA_FRACTION",
                "q_ref": "VAR_MAX"
              }
            }
          },
          "ibr": {
            "pv": {
              "bus": "b",
              "terminal_map": ["1", "2", "3", "n"],
              "topology": "FOUR_LEG",
              "prime_mover": "PV",
              "s_max": [10000.0, 10000.0, 10000.0],
              "p_avail": 24000.0,
              "q_min": [-4000.0, -4000.0, -4000.0],
              "q_max": [4000.0, 4000.0, 4000.0],
              "control_profile": "cp"
            }
          }
        }"#,
    )
    .unwrap();

    let out = emit_dss(&net);

    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert!(out.text.contains("New PVSystem.pv"), "{}", out.text);
    assert!(out.text.contains("New XYcurve.vv_pv"), "{}", out.text);
    assert!(out.text.contains("New InvControl.ivc_pv"), "{}", out.text);
    assert!(out.text.contains("mode=VOLTVAR"), "{}", out.text);
    assert!(out.text.contains("RefReactivePower=VARMAX"), "{}", out.text);
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

    let out = emit_bmopf_json(&net);

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

fn single_phase_source(name: &str, phase: &str, angle: f64, extras: Extras) -> VoltageSource {
    let mut source = VoltageSource::new(
        name,
        "sourcebus",
        vec![phase.into(), "4".into()],
        vec![7200.0, 0.0],
        vec![angle, 0.0],
    );
    source.extras = extras;
    source
}

fn split_source_network(sources: Vec<VoltageSource>) -> MulticonductorNetwork {
    let mut bus = DistBus::new(
        "sourcebus",
        vec!["1".into(), "2".into(), "3".into(), "4".into()],
    );
    bus.grounded = vec!["4".into()];
    let mut net = MulticonductorNetwork::default();
    *net.name_mut() = Some("split".into());
    *net.buses_mut() = vec![bus];
    *net.sources_mut() = sources;
    net
}

#[test]
fn bmopf_coordinates_are_strict_by_default_and_opt_in_as_sideloads() {
    let mut net =
        split_source_network(vec![single_phase_source("source", "1", 0.0, Extras::new())]);
    *net.geo_mut() = Some(DistGeoMeta {
        space: CoordinateSpace::Geographic { crs: None },
        kind: Some(DistCoordsKind::Source),
    });
    net.buses_mut()[0].location = Some(DistLocation {
        x: -80.0,
        y: 35.0,
        kind: None,
    });

    let strict = emit_bmopf_json(&net);
    let strict_doc: serde_json::Value = serde_json::from_str(&strict.text).unwrap();
    assert!(strict_doc["bus"]["sourcebus"].get("longitude").is_none());
    assert!(
        strict
            .warnings
            .iter()
            .any(|w| w.contains("EMIT.BMOPF.BUS_LOCATION_DROPPED")),
        "{:?}",
        strict.warnings
    );
    assert_eq!(
        diagnostic(&strict, "EMIT.BMOPF.BUS_LOCATION_DROPPED", "bus sourcebus").severity(),
        DiagnosticSeverity::Warning
    );

    let mut options = BmopfEmitOptions::default();
    options.sideload_coordinates = true;
    let sideloaded = emit_bmopf_json_with_options(&net, options);
    let doc: serde_json::Value = serde_json::from_str(&sideloaded.text).unwrap();
    assert_eq!(
        doc["bus"]["sourcebus"]["longitude"],
        serde_json::json!(-80.0)
    );
    assert_eq!(doc["bus"]["sourcebus"]["latitude"], serde_json::json!(35.0));
    assert!(
        sideloaded
            .warnings
            .iter()
            .all(|w| !w.contains("BUS_LOCATION_DROPPED")),
        "{:?}",
        sideloaded.warnings
    );

    *net.geo_mut() = Some(DistGeoMeta {
        space: CoordinateSpace::Unknown,
        kind: Some(DistCoordsKind::Source),
    });
    let unknown = emit_bmopf_json_with_options(&net, options);
    let doc: serde_json::Value = serde_json::from_str(&unknown.text).unwrap();
    assert!(doc["bus"]["sourcebus"].get("longitude").is_none());
    assert!(
        unknown
            .warnings
            .iter()
            .any(|w| w.contains("EMIT.BMOPF.BUS_LOCATION_DROPPED")),
        "{:?}",
        unknown.warnings
    );
}

#[test]
fn opendss_split_voltage_sources_merge_in_bmopf() {
    let third = 2.0 * std::f64::consts::FRAC_PI_3;
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.split phases=1 basekv=7.2 pu=1.0 angle=0 bus1=SourceBus.1\n\
         New Vsource.phb phases=1 basekv=7.2 pu=1.0 angle=-120 bus1=sourcebus.2\n\
         New Vsource.phc phases=1 basekv=7.2 pu=1.0 angle=120 bus1=SOURCEBUS.3\n",
    );
    assert_eq!(net.sources().len(), 3);

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("expects exactly one source")),
        "{:?}",
        out.warnings
    );
    // The per-field aggregation (#377) carries the elements in details.
    let by_field = |field: &str| {
        out.diagnostics
            .iter()
            .find(|d| {
                d.code() == "EMIT.BMOPF.FIELD_DROPPED"
                    && d.details().get("field") == Some(&serde_json::json!(field))
            })
            .unwrap_or_else(|| panic!("missing aggregated drop for {field}: {out:?}"))
    };
    let angle = by_field("angle");
    assert!(
        angle.details()["elements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "voltage source phb"),
        "{:?}",
        angle.details()
    );
    let basekv = by_field("basekv");
    assert!(
        basekv.details()["elements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "voltage source phc"),
        "{:?}",
        basekv.details()
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert!(doc["bus"].get("SourceBus").is_some(), "{}", out.text);
    let sources = doc["voltage_source"].as_object().unwrap();
    assert_eq!(sources.len(), 1, "{sources:?}");
    let source = &sources["source"];
    assert_eq!(source["bus"], serde_json::json!("SourceBus"));
    assert_eq!(
        source["terminal_map"],
        serde_json::json!(["1", "2", "3", "4"])
    );
    assert_eq!(
        source["v_magnitude"],
        serde_json::json!([7200.0, 7200.0, 7200.0, 0.0])
    );
    let angles = source["v_angle"].as_array().unwrap();
    assert_eq!(angles.len(), 4);
    for (actual, expected) in angles.iter().zip([0.0, -third, third, 0.0]) {
        let actual = actual.as_f64().unwrap();
        assert!(
            (actual - expected).abs() < 1e-12,
            "v_angle entry = {actual}, expected {expected}"
        );
    }
}

#[test]
fn two_phase_split_voltage_sources_merge_in_bmopf() {
    let third = 2.0 * std::f64::consts::FRAC_PI_3;
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.split phases=1 basekv=7.2 pu=1.0 angle=0 bus1=sourcebus.1\n\
         New Vsource.phb phases=1 basekv=7.2 pu=1.0 angle=-120 bus1=sourcebus.2\n",
    );
    assert_eq!(net.sources().len(), 2);

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("expects exactly one source")),
        "{:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let sources = doc["voltage_source"].as_object().unwrap();
    assert_eq!(sources.len(), 1, "{sources:?}");
    let source = &sources["source"];
    assert_eq!(source["terminal_map"], serde_json::json!(["1", "2", "4"]));
    assert_eq!(
        source["v_magnitude"],
        serde_json::json!([7200.0, 7200.0, 0.0])
    );
    let angles = source["v_angle"].as_array().unwrap();
    assert_eq!(angles.len(), 3);
    for (actual, expected) in angles.iter().zip([0.0, -third, 0.0]) {
        let actual = actual.as_f64().unwrap();
        assert!(
            (actual - expected).abs() < 1e-12,
            "v_angle entry = {actual}, expected {expected}"
        );
    }
}

#[test]
fn split_voltage_source_merge_declines_ambiguous_banks() {
    let third = 2.0 * std::f64::consts::FRAC_PI_3;
    let cases = [
        (
            "quadrature",
            vec![
                single_phase_source("source", "1", 0.0, Extras::new()),
                single_phase_source("phb", "2", std::f64::consts::FRAC_PI_2, Extras::new()),
            ],
        ),
        (
            "anti phase",
            vec![
                single_phase_source("source", "1", 0.0, Extras::new()),
                single_phase_source("phb", "2", std::f64::consts::PI, Extras::new()),
            ],
        ),
        (
            "incoherent",
            vec![
                single_phase_source("source", "1", 0.0, Extras::new()),
                single_phase_source("phb", "2", -third, Extras::new()),
                single_phase_source("phc", "3", 0.25, Extras::new()),
            ],
        ),
        (
            "phase conflict",
            vec![
                single_phase_source("source", "1", 0.0, Extras::new()),
                single_phase_source("other", "1", -third, Extras::new()),
            ],
        ),
        ("bounded", {
            let mut extras = Extras::new();
            extras.insert("p_min".into(), serde_json::json!([-1.0]));
            vec![
                single_phase_source("source", "1", 0.0, extras),
                single_phase_source("phb", "2", -third, Extras::new()),
                single_phase_source("phc", "3", third, Extras::new()),
            ]
        }),
        ("priced", {
            let mut extras = Extras::new();
            extras.insert("cost".into(), serde_json::json!([1.0]));
            vec![
                single_phase_source("source", "1", 0.0, extras),
                single_phase_source("phb", "2", -third, Extras::new()),
                single_phase_source("phc", "3", third, Extras::new()),
            ]
        }),
    ];

    for (case, sources) in cases {
        let source_count = sources.len();
        let out = emit_bmopf_json(&split_source_network(sources));
        if case != "priced" {
            assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
        }
        let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
        let emitted = doc["voltage_source"].as_object().unwrap();
        assert_eq!(emitted.len(), source_count, "{case}: {emitted:?}");
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("expects exactly one source")),
            "{case}: {:?}",
            out.warnings
        );
    }
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

    assert!((net.transformers()[0].windings[0].tap - 1.05).abs() < 1e-12);
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["single_phase"]["t"];
    assert_eq!(t["tap_ratio"], 1.05);
    assert_eq!(t["tap_ratio_min"], 0.9);
    assert_eq!(t["tap_ratio_max"], 1.1);
    assert_eq!(t["g_no_load"], serde_json::json!(0.000_001));
    assert_eq!(t["b_no_load"], serde_json::json!(0.0));

    let back = parse_bmopf_str(&out.text).unwrap();
    assert!((back.transformers()[0].windings[0].tap - 1.05).abs() < 1e-12);
    let second = emit_bmopf_json(&back);
    let document: serde_json::Value = serde_json::from_str(&second.text).unwrap();
    let transformer = &document["transformer"]["single_phase"]["t"];
    assert_eq!(transformer["tap_ratio_min"], 0.9);
    assert_eq!(transformer["tap_ratio_max"], 1.1);
}

#[test]
fn dss_fixed_to_side_tap_emits_bmopf_ratio_without_bounds() {
    let v = schema_validator();
    let net = parse_dss_str(
        "New Circuit.tap basekv=7.2 pu=1.0 phases=1 bus1=src.1\n\
         New Transformer.t1 phases=1 windings=2 buses=(src.1, load.1) \
         conns=(wye, wye) kvs=(7.2, 0.24) kvas=(25, 25) taps=(1.0, 1.05) \
         mintap=0.9 maxtap=1.1 numtaps=32 xhl=2.0 %Rs=(0.5, 0.5)\n",
    );
    assert!((net.transformers()[0].windings[1].tap - 1.05).abs() < 1e-12);

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    assert!(
        out.warnings.iter().all(|w| !w.contains("TAP_DROPPED")),
        "{:?}",
        out.warnings
    );
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("mintap") && !w.contains("maxtap") && !w.contains("numtaps")),
        "{:?}",
        out.warnings
    );

    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["single_phase"]["t1"];
    assert!((t["tap_ratio"].as_f64().unwrap() - (1.0 / 1.05)).abs() < 1e-12);
    assert!(t.get("tap_min").is_none(), "{t:?}");
    assert!(t.get("tap_max").is_none(), "{t:?}");
}

#[test]
fn dss_center_tap_uses_first_secondary_tap_and_warns_if_halves_differ() {
    let net = parse_dss_str(
        "New Circuit.ct basekv=7.2 pu=1.0 phases=1 bus1=sourcebus.1\n\
         New Transformer.t1 phases=1 windings=3\n\
         ~ wdg=1 bus=sourcebus.1 kv=7.2 kva=25 conn=wye tap=1.02 %R=0.6\n\
         ~ wdg=2 bus=secondary.1.0 kv=0.12 kva=25 conn=wye tap=1.01 %R=1.2\n\
         ~ wdg=3 bus=secondary.0.2 kv=0.12 kva=25 conn=wye tap=1.03 %R=1.2\n\
         ~ xhl=2.04 xht=2.04 xlt=1.36\n",
    );

    let out = emit_bmopf_json(&net);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("EMIT.BMOPF.TRANSFORMER_CENTER_TAP_TAP_COLLAPSED")),
        "{:?}",
        out.warnings
    );
    let diag = diagnostic(
        &out,
        "EMIT.BMOPF.TRANSFORMER_CENTER_TAP_TAP_COLLAPSED",
        "transformer t1",
    );
    assert_eq!(
        diag.details()["secondary_taps"],
        serde_json::json!([1.01, 1.03])
    );

    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["center_tap"]["t1"];
    assert!((t["tap_ratio"].as_f64().unwrap() - (1.02 / 1.01)).abs() < 1e-12);
}

#[test]
fn pmd_uniform_per_phase_taps_emit_ratio_without_warning() {
    let net = parse_pmd_str(
        r#"{
          "data_model": "ENGINEERING",
          "transformer": {"reg": {"bus": ["b1", "b2"],
            "connections": [[1, 2, 3, 4], [1, 2, 3, 4]],
            "configuration": ["WYE", "WYE"], "polarity": [1, 1],
            "rw": [0.005, 0.005], "xsc": [0.01],
            "sm_nom": [1666.0, 1666.0], "vm_nom": [2.4, 2.4],
            "tm_set": [[1.0, 1.0, 1.0], [1.05, 1.05, 1.05]],
            "tm_lb": [[0.9, 0.9, 0.9], [0.9, 0.9, 0.9]],
            "tm_ub": [[1.1, 1.1, 1.1], [1.1, 1.1, 1.1]],
            "tm_fix": [[true, true, true], [false, false, false]],
            "tm_step": [[0.03125, 0.03125, 0.03125], [0.03125, 0.03125, 0.03125]],
            "status": "ENABLED"}}
        }"#,
    )
    .unwrap();

    let out = emit_bmopf_json(&net);
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("TRANSFORMER_PER_PHASE_TAP_COLLAPSED")),
        "{:?}",
        out.warnings
    );
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("TRANSFORMER_EXTRA_DROPPED") && !w.contains("pmd_tm_")),
        "{:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["single_phase"]["reg_1"];
    assert!((t["tap_ratio"].as_f64().unwrap() - (1.0 / 1.05)).abs() < 1e-12);
    assert!(t.get("tap_min").is_none());
    assert!(t.get("tap_max").is_none());
}

#[test]
fn pmd_nonuniform_per_phase_taps_warn_with_stable_code() {
    let net = parse_pmd_str(
        r#"{
          "data_model": "ENGINEERING",
          "transformer": {"reg": {"bus": ["b1", "b2"],
            "connections": [[1, 2, 3, 4], [1, 2, 3, 4]],
            "configuration": ["WYE", "WYE"], "polarity": [1, 1],
            "rw": [0.005, 0.005], "xsc": [0.01],
            "sm_nom": [1666.0, 1666.0], "vm_nom": [2.4, 2.4],
            "tm_set": [[1.0, 1.0, 1.0], [1.05, 1.04, 1.05]],
            "status": "ENABLED"}}
        }"#,
    )
    .unwrap();

    let out = emit_bmopf_json(&net);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("EMIT.BMOPF.TRANSFORMER_PER_PHASE_TAP_COLLAPSED")),
        "{:?}",
        out.warnings
    );
    let diag = diagnostic(
        &out,
        "EMIT.BMOPF.TRANSFORMER_PER_PHASE_TAP_COLLAPSED",
        "transformer reg",
    );
    assert_eq!(diag.severity(), DiagnosticSeverity::Warning);
    assert_eq!(diag.stage(), Some(DiagnosticStage::Emit));
    assert_eq!(diag.details()["winding"], serde_json::json!(2));
    assert_eq!(
        diag.details()["source_taps"],
        serde_json::json!([1.05, 1.04, 1.05])
    );

    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["single_phase"]["reg_1"];
    assert!((t["tap_ratio"].as_f64().unwrap() - (1.0 / 1.05)).abs() < 1e-12);
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

    assert_eq!(net.transformers()[0].windings.len(), 3);
    let model_t = &net.transformers()[0];
    assert!((model_t.windings[1].v_ref - 100.0 * 3f64.sqrt()).abs() < 1e-12);
    assert!((model_t.windings[0].r_pct - 0.01 / 3.0 * 100.0).abs() < 1e-12);
    assert!((model_t.windings[1].r_pct - 0.02 / 3.0 * 100.0).abs() < 1e-12);
    assert!((model_t.xsc_pct[0] - 0.04 / 3.0 * 100.0).abs() < 1e-12);
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["n_winding"]["t3"];
    assert_eq!(t["windings"].as_array().unwrap().len(), 3);
    assert_eq!(t["windings"][0]["delta_roll"], serde_json::json!(-1));
    assert!(t["windings"][1].get("delta_roll").is_none(), "{t:?}");
    assert!(t["windings"][2].get("delta_roll").is_none(), "{t:?}");
    assert_eq!(t["windings"][1]["v_nom"], serde_json::json!(100.0));
    assert_eq!(t["x_sc"]["1_2"], serde_json::json!(0.04));
    assert_eq!(t["g_no_load"], serde_json::json!(0.001));
    assert_eq!(t["b_no_load"], serde_json::json!(-0.002));
}

#[test]
fn n_winding_explicit_delta_roll_round_trips_through_bmopf() {
    let v = schema_validator();
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "a": {"terminal_names": ["1", "2", "3"]},
            "b": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]},
            "c": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]}
          },
          "transformer": {
            "n_winding": {
              "t3": {
                "s_rating": 10000.0,
                "windings": [
                  {"bus": "a", "terminal_map": ["1", "2", "3"], "v_nom": 100.0, "configuration": "DELTA", "r_winding": 0.01, "delta_roll": 1},
                  {"bus": "b", "terminal_map": ["1", "2", "3", "n"], "v_nom": 100.0, "configuration": "WYE", "r_winding": 0.02},
                  {"bus": "c", "terminal_map": ["1", "2", "3", "n"], "v_nom": 100.0, "configuration": "WYE", "r_winding": 0.03}
                ],
                "x_sc": {"1_2": 0.04, "1_3": 0.05, "2_3": 0.06}
              }
            }
          }
        }"#,
    )
    .unwrap();

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("bmopf_delta_rolls")),
        "{:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["n_winding"]["t3"];
    assert_eq!(t["windings"][0]["delta_roll"], serde_json::json!(1));

    let again = parse_bmopf_str(&out.text).unwrap();
    let out2 = emit_bmopf_json(&again);
    assert_eq!(out.text, out2.text);
}

#[test]
fn opendss_n_winding_delta_emits_delta_roll() {
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.dyn basekv=12.47 pu=1.0 phases=3 bus1=sourcebus\n\
         New Transformer.t1 phases=3 windings=3 buses=(sourcebus, mid, low) \
         conns=(delta, wye, wye) kvs=(12.47, 4.16, 0.48) \
         kvas=(1000, 1000, 1000) %Rs=(0.5, 0.5, 0.5) xhl=5 xht=6 xlt=4\n",
    );
    let t = &net.transformers()[0];
    assert_eq!(t.windings.len(), 3);
    assert_eq!(t.windings[0].conn, DistWindingConn::Delta);

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["n_winding"]["t1"];
    assert_eq!(
        t["windings"][0]["configuration"],
        serde_json::json!("DELTA")
    );
    assert_eq!(t["windings"][0]["delta_roll"], serde_json::json!(-1));
    assert!(t["windings"][1].get("delta_roll").is_none(), "{t:?}");
    assert!(t["windings"][2].get("delta_roll").is_none(), "{t:?}");
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

    let out = emit_bmopf_json(&net);
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

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["wye_delta"]["t"];
    assert_eq!(t["v_nom_from"], serde_json::json!(7200.0));
    assert_eq!(t["v_nom_to"], serde_json::json!(480.0));
    let ox = &doc["transformer"]["wye_delta"]["t"];
    assert_eq!(ox["g_no_load"], serde_json::json!(0.000_002));
    assert_eq!(ox["b_no_load"], serde_json::json!(-0.000_003));
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
    let out = emit_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["single_phase"]["t1"];
    let expected_g = 0.2 / 100.0 * 25_000.0 / (7200.0 * 7200.0);
    let g = t["g_no_load"].as_f64().unwrap();
    assert!((g - expected_g).abs() < 1e-18, "g_no_load = {g}");
    let expected_b = 0.5 / 100.0 * 25_000.0 / (7200.0 * 7200.0);
    let b = t["b_no_load"].as_f64().unwrap();
    assert!((b - expected_b).abs() < 1e-18, "b_no_load = {b}");
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
}

#[test]
fn transformer_neutral_impedance_round_trips_dss_and_bmopf() {
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.neutral basekv=7.2 pu=1.0 phases=1 bus1=src.1\n\
         New Transformer.t1 phases=1 windings=2 buses=(src.1.0, load.1.0) \
         kvs=(7.2 0.24) kvas=(25 25) %Rs=(1 1) xhl=2\n\
         ~ wdg=1 rneut=5 xneut=6\n\
         ~ wdg=2 rneut=7 xneut=8\n",
    );
    let t = &net.transformers()[0];
    assert_eq!(t.windings[0].r_neutral, Some(5.0));
    assert_eq!(t.windings[0].x_neutral, Some(6.0));
    assert_eq!(t.windings[1].r_neutral, Some(7.0));
    assert_eq!(t.windings[1].x_neutral, Some(8.0));

    let dss = emit_dss(&net).text;
    assert!(!dss.contains("NaN"), "{dss}");
    assert!(dss.contains("kvs=(7.2, 0.24)"), "{dss}");
    assert!(dss.contains("~ wdg=1 rneut=5 xneut=6"), "{dss}");
    assert!(dss.contains("~ wdg=2 rneut=7 xneut=8"), "{dss}");

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["single_phase"]["t1"];
    assert_eq!(t["r_neutral_from"], serde_json::json!(5.0));
    assert_eq!(t["x_neutral_from"], serde_json::json!(6.0));
    assert_eq!(t["r_neutral_to"], serde_json::json!(7.0));
    assert_eq!(t["x_neutral_to"], serde_json::json!(8.0));

    let reparsed = parse_bmopf_str(&out.text).unwrap();
    let rt = &reparsed.transformers()[0];
    assert_eq!(rt.windings[0].r_neutral, Some(5.0));
    assert_eq!(rt.windings[0].x_neutral, Some(6.0));
    assert_eq!(rt.windings[1].r_neutral, Some(7.0));
    assert_eq!(rt.windings[1].x_neutral, Some(8.0));

    let net = parse_dss_str(
        "Clear\n\
         New Circuit.neutral basekv=7.2 pu=1.0 phases=1 bus1=src.1\n\
         New Transformer.t1 phases=1 windings=2 buses=(src.1.0, load.1.0) \
         kvs=(7.2 0.24) kvas=(25 25) %Rs=(1 1) xhl=2\n\
         ~ wdg=1 rneut=-1 xneut=-2\n",
    );
    let dss = emit_dss(&net).text;
    assert!(dss.contains("~ wdg=1 rneut=-1 xneut=-2"), "{dss}");
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    assert!(!out.text.contains("r_neutral_from"), "{out:?}");
    assert!(!out.text.contains("x_neutral_from"), "{out:?}");
    assert!(
        out.warnings.iter().any(|w| w.contains("r_neutral_from=-1")),
        "{:?}",
        out.warnings
    );
    assert!(
        out.warnings.iter().any(|w| w.contains("x_neutral_from=-2")),
        "{:?}",
        out.warnings
    );
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
    let out = emit_bmopf_json(&net);
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
fn wye_wye_3_extras_drop_warns_once_not_per_phase() {
    let from = DistWinding::new(
        "a",
        vec!["1".into(), "2".into(), "3".into(), "n".into()],
        DistWindingConn::Wye,
        7200.0,
        25_000.0,
    );
    let to = DistWinding::new(
        "b",
        vec!["1".into(), "2".into(), "3".into(), "n".into()],
        DistWindingConn::Wye,
        240.0,
        25_000.0,
    );
    let mut t = DistTransformer::new("t", vec![from, to], vec![4.0], 3);
    t.extras
        .insert("unknown_key".into(), serde_json::json!("x"));
    let mut net = MulticonductorNetwork::default();
    net.transformers_mut().push(t);

    let out = emit_bmopf_json(&net);
    let count = out
        .warnings
        .iter()
        .filter(|w| w.contains("unknown_key"))
        .count();
    assert_eq!(count, 1, "{:?}", out.warnings);
}

#[test]
fn wye_wye_3_neutral_grounding_decomposes_once_not_per_phase() {
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.wyewye basekv=7.2 pu=1.0 phases=3 bus1=src.1.2.3.0\n\
         New Transformer.t phases=3 windings=2 buses=(src.1.2.3.0, load.1.2.3.0) \
         conns=(wye wye) kvs=(12.47 0.48) kvas=(75 75) %Rs=(1 1) xhl=2 \
         %noloadloss=0.3 %imag=0.6\n\
         ~ wdg=1 rneut=5 xneut=6\n\
         ~ wdg=2 rneut=7 xneut=8\n",
    );
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let sp = doc["transformer"]["single_phase"].as_object().unwrap();
    assert_eq!(sp.len(), 3);
    let sp = doc["transformer"]["single_phase"].as_object().unwrap();
    assert_eq!(sp["t_1"]["r_neutral_from"], serde_json::json!(5.0));
    assert_eq!(sp["t_1"]["x_neutral_from"], serde_json::json!(6.0));
    assert_eq!(sp["t_1"]["r_neutral_to"], serde_json::json!(7.0));
    assert_eq!(sp["t_1"]["x_neutral_to"], serde_json::json!(8.0));
    let expected_g = 0.3 / 100.0 * (75_000.0 / 3.0) / ((12_470.0 / 3f64.sqrt()).powi(2));
    let expected_b = 0.6 / 100.0 * (75_000.0 / 3.0) / ((12_470.0 / 3f64.sqrt()).powi(2));
    for name in ["t_1", "t_2", "t_3"] {
        let t = &sp[name];
        let g = t["g_no_load"].as_f64().unwrap();
        assert!((g - expected_g).abs() < 1e-18, "{name} g_no_load = {g}");
        let b = t["b_no_load"].as_f64().unwrap();
        assert!((b - expected_b).abs() < 1e-18, "{name} b_no_load = {b}");
    }
    for name in ["t_2", "t_3"] {
        let t = &sp[name];
        assert!(t.get("r_neutral_from").is_none(), "{name}: {t}");
        assert!(t.get("x_neutral_from").is_none(), "{name}: {t}");
        assert!(t.get("r_neutral_to").is_none(), "{name}: {t}");
        assert!(t.get("x_neutral_to").is_none(), "{name}: {t}");
    }
}

#[test]
fn wye_wye_3_raw_no_load_splits_across_decomposition() {
    let from = DistWinding::new(
        "a",
        vec!["1".into(), "2".into(), "3".into(), "n".into()],
        DistWindingConn::Wye,
        7200.0,
        30_000.0,
    );
    let to = DistWinding::new(
        "b",
        vec!["1".into(), "2".into(), "3".into(), "n".into()],
        DistWindingConn::Wye,
        240.0,
        30_000.0,
    );
    let mut t = DistTransformer::new("t", vec![from, to], vec![4.0], 3);
    t.extras
        .insert("g_no_load".into(), serde_json::json!(0.000_009));
    t.extras
        .insert("b_no_load".into(), serde_json::json!(-0.000_012));

    let mut net = MulticonductorNetwork::default();
    *net.buses_mut() = vec![
        DistBus::new("a", vec!["1".into(), "2".into(), "3".into(), "n".into()]),
        DistBus::new("b", vec!["1".into(), "2".into(), "3".into(), "n".into()]),
    ];
    net.transformers_mut().push(t);

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let sp = doc["transformer"]["single_phase"].as_object().unwrap();
    assert_eq!(sp.len(), 3);
    let ox = doc["transformer"]["single_phase"].as_object().unwrap();
    let mut g_sum = 0.0;
    let mut b_sum = 0.0;
    for name in ["t_1", "t_2", "t_3"] {
        let t = &ox[name];
        assert_eq!(t["g_no_load"], serde_json::json!(0.000_003));
        assert_eq!(t["b_no_load"], serde_json::json!(-0.000_004));
        g_sum += t["g_no_load"].as_f64().unwrap();
        b_sum += t["b_no_load"].as_f64().unwrap();
    }
    assert!((g_sum - 0.000_009).abs() < 1e-18);
    assert!((b_sum + 0.000_012).abs() < 1e-18);
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

    assert!((net.base_frequency() - 50.0).abs() < 1e-12);
    let out = emit_dss(&net);
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

    let out = emit_dss(&net);
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

    let out = emit_dss(&net);
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
                d["line"]["l632671"]
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
                d["line"]["l632671"]["length"] = "152.4".into();
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
            "negative line i_max",
            mutate(&|d| {
                d["line"]["l671611"]["i_max"] = serde_json::json!([-600.0]);
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
    let s = &net.shunts()[0];
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
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
}

#[test]
fn center_tap_emits_bmopf_convention_and_star_leakage() {
    // Each 120 V half carries %R=1.2 on 25 kVA:
    // 0.012 * 120^2 / 25000 = 0.006912 ohm. BMOPF stores the per leg
    // voltage and per leg to side series arm, not the full 240 V path.
    let net = parse_dss_file(fixture("micro/xfmr_center_tap.dss")).unwrap();
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    assert!(
        out.warnings
            .iter()
            .all(|w| !w.contains("EMIT.BMOPF.TRANSFORMER_CENTER_TAP_COLLAPSED")),
        "{:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["center_tap"]["t1"];
    assert_eq!(t["terminal_map_to"], serde_json::json!(["1", "4", "2"]));
    assert_eq!(t["v_nom_to"], 120.0);
    let r_to = t["r_series_to"].as_f64().unwrap();
    assert!((r_to - 0.006_912).abs() < 1e-12, "r_series_to = {r_to}");
    // The primary is untouched: %R=0.6 on 7.2 kV/25 kVA.
    let r_from = t["r_series_from"].as_f64().unwrap();
    assert!((r_from - 12.4416).abs() < 1e-9, "r_series_from = {r_from}");
    // xhl=2.04, xht=2.04, xlt=1.36 gives symmetric star arms:
    // x_from=(xhl+xht-xlt)/2=1.36%, x_to=(xhl+xlt-xht)/2=0.68%.
    let x_from = t["x_series_from"].as_f64().unwrap();
    assert!(
        (x_from - 28.200_96).abs() < 1e-9,
        "x_series_from = {x_from}"
    );
    let x_to = t["x_series_to"].as_f64().unwrap();
    assert!((x_to - 0.003_916_8).abs() < 1e-12, "x_series_to = {x_to}");
}

#[test]
fn bmopf_center_tap_canonical_order_rebuilds_dss_grounded_center() {
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
                    "terminal_map_to": ["1", "3", "2"],
                    "s_rating": 25000.0,
                    "v_nom_from": 7200.0,
                    "v_nom_to": 120.0,
                    "r_series_from": 12.4416,
                    "r_series_to": 0.006912,
                    "x_series_from": 28.20096,
                    "x_series_to": 0.0039168
                }
            }
        }
    }"#;
    let net = parse_bmopf_str(text).unwrap();
    assert_eq!(net.transformers()[0].windings.len(), 3);
    let dss = emit_dss(&net).text;
    assert!(dss.contains("lv.1.0"), "{dss}");
    assert!(dss.contains("lv.0.2"), "{dss}");

    let out = emit_bmopf_json(&parse_dss_str(&dss));
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(
        doc["bus"]["lv"]["perfectly_grounded_terminals"],
        serde_json::json!(["4"])
    );
    assert_eq!(
        doc["transformer"]["center_tap"]["ct"]["terminal_map_to"],
        serde_json::json!(["1", "4", "2"])
    );
}

#[test]
fn bmopf_center_tap_service_exports_solvable_dss() {
    // PowerIO.jl#79, reduced to one consumer. The load is balanced across the
    // two legs, so the imbalance split never fires; the star has no secondary
    // leakage, so it back solves to xlt=0; and `v_nom_to` states the full
    // span, which the bus's own phase to neutral band contradicts.
    let text = r#"{
        "meta": {"frequency": 50.0},
        "bus": {
            "hv": {"terminal_names": ["1", "2"], "perfectly_grounded_terminals": ["2"]},
            "lv": {
                "terminal_names": ["1", "2", "3"],
                "perfectly_grounded_terminals": ["2"],
                "vpn_min": [225.6, 225.6],
                "vpn_max": [254.4, 254.4]
            }
        },
        "voltage_source": {
            "source": {
                "v_magnitude": [19000.0, 0.0],
                "v_angle": [0.0, 0.0],
                "bus": "hv",
                "terminal_map": ["1", "2"]
            }
        },
        "transformer": {
            "center_tap": {
                "tx": {
                    "bus_from": "hv",
                    "bus_to": "lv",
                    "terminal_map_from": ["1", "2"],
                    "terminal_map_to": ["1", "2", "3"],
                    "s_rating": 25000.0,
                    "v_nom_from": 19000.0,
                    "v_nom_to": 480.0,
                    "r_series_from": 0.5,
                    "x_series_from": 2.5
                }
            }
        },
        "load": {
            "ld": {
                "bus": "lv",
                "terminal_map": ["1", "2", "3"],
                "p_nom": [1304.0, 1304.0],
                "q_nom": [978.0, 978.0],
                "model": "constant_impedance"
            }
        }
    }"#;
    let net = parse_bmopf_str(text).unwrap();
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("v_nom_to 480") && w.contains("per leg")),
        "{:?}",
        net.warnings
    );

    let out = emit_dss(&net);
    // One Load per leg, each on its own hot node and the grounded return.
    let loads: Vec<&str> = out
        .text
        .lines()
        .filter(|l| l.contains("New Load."))
        .collect();
    assert_eq!(loads.len(), 2, "{}", out.text);
    assert!(loads.iter().all(|l| l.contains("phases=1")), "{loads:?}");
    assert!(loads.iter().all(|l| l.contains("kw=1.304")), "{loads:?}");
    assert!(
        loads.iter().any(|l| l.contains("bus1=lv.1.0 ")),
        "{loads:?}"
    );
    assert!(
        loads.iter().any(|l| l.contains("bus1=lv.3.0 ")),
        "{loads:?}"
    );

    // A star dss can solve, and the warning names the substitution.
    let tx = out
        .text
        .lines()
        .find(|l| l.contains("New Transformer.tx"))
        .unwrap_or_else(|| panic!("no transformer: {}", out.text));
    let arm = |key: &str| -> f64 {
        tx.split_whitespace()
            .find_map(|t| t.strip_prefix(key))
            .unwrap_or_else(|| panic!("no {key} in {tx}"))
            .parse()
            .unwrap()
    };
    let (xhl, xlt) = (arm("xhl="), arm("xlt="));
    assert!(xhl > 0.0, "{tx}");
    assert!((xlt - 2.0 / 3.0 * xhl).abs() < 1e-12 * xhl, "{tx}");
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("collapsed secondary")),
        "{:?}",
        out.warnings
    );
    // The stated frequency reaches the deck, so the charging conversion holds.
    assert!(
        out.text.contains("Set DefaultBaseFrequency=50"),
        "{}",
        out.text
    );
}

#[test]
fn bmopf_center_tap_neutral_grounding_rebuilds_once() {
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
                    "x_series_to": 0.0,
                    "r_neutral_to": 5.0,
                    "x_neutral_to": 6.0
                }
            }
        }
    }"#;
    let net = parse_bmopf_str(text).unwrap();
    let t = &net.transformers()[0];
    assert_eq!(t.windings[1].r_neutral, Some(5.0));
    assert_eq!(t.windings[1].x_neutral, Some(6.0));
    assert_eq!(t.windings[2].r_neutral, None);
    assert_eq!(t.windings[2].x_neutral, None);

    let dss = emit_dss(&net).text;
    assert_eq!(dss.matches("rneut=5").count(), 1, "{dss}");
    assert_eq!(dss.matches("xneut=6").count(), 1, "{dss}");

    let out = emit_bmopf_json(&parse_dss_str(&dss));
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["center_tap"]["ct"];
    assert_eq!(t["r_neutral_to"], serde_json::json!(5.0));
    assert_eq!(t["x_neutral_to"], serde_json::json!(6.0));
}

#[test]
fn center_tap_collapse_uses_first_secondary_half_rating() {
    // BMOPF carries one to side series arm, so unequal half ratings collapse
    // to the first secondary half rating with a warning.
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.ct basekv=7.2 pu=1.0 phases=1 bus1=src.1\n\
         New Transformer.t1 phases=1 windings=3 buses=(src.1.0, lv.1.0, lv.0.2) \
         kvs=(7.2 0.12 0.12) kvas=(25 50 25) %Rs=(1 2 4) xhl=2.04 xht=2.04 xlt=1.36\n",
    );
    let out = emit_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["center_tap"]["t1"];
    assert_eq!(t["v_nom_to"], 120.0);
    let expected = 0.02 * 120.0 * 120.0 / 50e3;
    let r_to = t["r_series_to"].as_f64().unwrap();
    assert!((r_to - expected).abs() < 1e-12, "r_series_to = {r_to}");
    assert!(
        out.warnings.iter().any(|w| w
            .contains("EMIT.BMOPF.TRANSFORMER_CENTER_TAP_RATING_COLLAPSED")
            && w.contains("first secondary half rating")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn center_tap_negative_star_arm_warns_and_falls_back_schema_valid() {
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.ct basekv=7.2 pu=1.0 phases=1 bus1=src.1\n\
         New Transformer.t1 phases=1 windings=3 buses=(src.1.0, lv.1.0, lv.0.2) \
         kvs=(7.2 0.12 0.12) kvas=(25 25 25) %Rs=(1 2 2) xhl=2 xht=10 xlt=2\n",
    );
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let diag = diagnostic(
        &out,
        "EMIT.BMOPF.TRANSFORMER_CENTER_TAP_LEAKAGE_UNREPRESENTABLE",
        "transformer t1",
    );
    assert_eq!(
        diag.details()["xsc_pct"],
        serde_json::json!([2.0, 10.0, 2.0])
    );
    assert_eq!(
        diag.details()["emitted_percentages"],
        serde_json::json!([2.0, 0.0])
    );

    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["center_tap"]["t1"];
    let x_from = t["x_series_from"].as_f64().unwrap();
    assert!((x_from - 41.472).abs() < 1e-12, "x_series_from = {x_from}");
    assert_eq!(t["x_series_to"], serde_json::json!(0.0));
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
    let out = emit_bmopf_json(&net);
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

    let mut net = MulticonductorNetwork::default();
    net.line_codes_mut().push(lc);
    let out = emit_bmopf_json(&net);
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
fn linecode_constructor_sizes_from_matrix_row_width() {
    let lc = DistLineCode::new("lc", Vec::new(), vec![vec![0.4, 0.1]]);
    assert_eq!(lc.n_conductors, 2);
    assert_eq!(lc.r_series, Vec::<Vec<f64>>::new());
    assert_eq!(lc.x_series, vec![vec![0.4, 0.1]]);
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
    let out = emit_bmopf_json(&net);
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

/// `serde_json::Map` iterates in key order, so `line` comes before
/// `linecode`. A line named like a declared linecode must not take that name
/// for its synthesized inline code, or every line that names the declared
/// linecode resolves to the line's own impedance instead. The match is
/// case-insensitive, like `MulticonductorNetwork::linecode`.
#[test]
fn synthesized_inline_linecode_never_shadows_a_declared_one() {
    let text = doc_with(
        r#", "linecode": {"LC": {"R_series_1_1": 9.0, "X_series_1_1": 9.0}},
        "line": {
            "lc": {"bus_from": "a", "bus_to": "a", "terminal_map_from": ["1"],
                   "terminal_map_to": ["1"], "R_series_1_1": 1.0, "X_series_1_1": 1.0},
            "user": {"bus_from": "a", "bus_to": "a", "terminal_map_from": ["1"],
                     "terminal_map_to": ["1"], "linecode": "LC", "length": 5.0}
        }"#,
    );
    let net = parse_bmopf_str(&text).unwrap();
    assert_eq!(net.line_codes().len(), 2);
    let series = |name: &str| net.linecode(name).unwrap().r_series[0][0].to_bits();
    // The declared code keeps its name and its impedance.
    assert_eq!(series("LC"), 9.0f64.to_bits());
    let user = net.lines().iter().find(|l| l.name == "user").unwrap();
    assert_eq!(series(&user.linecode), 9.0f64.to_bits());
    // The inline line takes a suffixed name and keeps its own impedance.
    let inline = net.lines().iter().find(|l| l.name == "lc").unwrap();
    assert_eq!(inline.linecode, "lc_");
    assert_eq!(series(&inline.linecode), 1.0f64.to_bits());
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("line lc") && w.contains("synthesized linecode `lc_`")),
        "{:?}",
        net.warnings
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
    let s = &net.shunts()[0];
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
    let out = emit_bmopf_json(&net);
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
    assert_eq!(net.loads()[0].configuration, Configuration::Delta);
    assert!(!net.warnings.iter().any(|w| w.contains("load l1")));
    // A truly unknown one coerces to WYE, loudly.
    assert_eq!(net.loads()[1].configuration, Configuration::Wye);
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("load l2") && w.contains("zigzag"))
    );
    // An unknown transformer subtype group reads, with a warning.
    assert_eq!(net.transformers().len(), 1);
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("transformer t1") && w.contains("open_delta"))
    );
}

#[test]
fn missing_voltage_source_warns() {
    let net = parse_bmopf_str(r#"{"bus": {"a": {"terminal_names": ["1"]}}}"#).unwrap();
    let out = emit_bmopf_json(&net);
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
        let mut winding = DistWinding::new(
            "a",
            map.iter().map(ToString::to_string).collect(),
            DistWindingConn::Wye,
            4160.0,
            500_000.0,
        );
        winding.r_pct = 0.5;
        winding
    };
    net.transformers_mut().push(DistTransformer::new(
        "t3w",
        vec![winding(&["1", "2", "3"]), winding(&["1", "2"])],
        vec![2.0],
        3,
    ));
    let out = emit_bmopf_json(&net);
    assert!(!out.text.contains("t3w"));
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("transformer t3w") && w.contains("dropped")),
        "{:?}",
        out.warnings
    );
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("EMIT.BMOPF.TRANSFORMER_UNSUPPORTED")),
        "missing unsupported transformer diagnostic code: {:?}",
        out.warnings
    );
    let diag = diagnostic(
        &out,
        "EMIT.BMOPF.TRANSFORMER_UNSUPPORTED",
        "transformer t3w",
    );
    assert_eq!(diag.severity(), DiagnosticSeverity::Warning);
    assert_eq!(diag.stage(), Some(DiagnosticStage::Emit));
    assert_eq!(diag.details()["transformer"], serde_json::json!("t3w"));
    assert_eq!(diag.details()["phases"], serde_json::json!(3));
}

#[test]
fn dss_autotransformer_drop_has_stable_diagnostic() {
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.auto basekv=12.47 bus1=sourcebus.1\n\
         New AutoTrans.at1 phases=1 windings=2 buses=(sourcebus.1.0, loadbus.1.0) \
         kvs=(7.2 0.24) kvas=(25 25) xhl=2\n",
    );
    assert_eq!(
        net.untyped_objects().len(),
        1,
        "{:?}",
        net.untyped_objects()
    );
    assert_eq!(net.untyped_objects()[0].class, "autotrans");

    let out = emit_bmopf_json(&net);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("EMIT.BMOPF.AUTOTRANSFORMER_DROPPED")),
        "missing autotransformer diagnostic code: {:?}",
        out.warnings
    );
    let diag = diagnostic(&out, "EMIT.BMOPF.AUTOTRANSFORMER_DROPPED", "autotrans at1");
    assert_eq!(diag.severity(), DiagnosticSeverity::Warning);
    assert_eq!(diag.stage(), Some(DiagnosticStage::Emit));
    assert_eq!(diag.details()["class"], serde_json::json!("autotrans"));
    assert_eq!(diag.details()["name"], serde_json::json!("at1"));
    assert!(
        !out.text.contains("at1"),
        "untyped AutoTrans must not silently enter BMOPF JSON: {}",
        out.text
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
    assert!(net.buses()[0].extras.contains_key("note"));
    let out = emit_bmopf_json(&net);
    assert!(out.warnings.iter().any(|w| w.contains("note")));
    assert!(!out.text.contains("hand edited"));
}

/// The writer seeds its `extras` block from the source document's own
/// `extras` object, then, writing schema 0.1.0, files the tables that version
/// has no top-level slot for into the same block. A source value that is not
/// a table has no slot for a named entry, so it must not reach the write as
/// one.
#[test]
fn source_extras_value_that_is_not_a_table_does_not_panic() {
    for class in ["time_series", "dc_bus", "dc_branch", "dc_load", "dc_source"] {
        let text = doc_with(&format!(
            r#", "extras": {{"{class}": 1}}, "{class}": {{"t1": {{"values": [1.0]}}}}"#
        ));
        let net = parse_bmopf_str(&text).unwrap();
        let out = emit_bmopf_json_with_options(
            &net,
            BmopfEmitOptions::default().with_profile(BmopfProfile::Bmopf010),
        );
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains(&format!("extras `{class}`")) && w.contains("not a table")),
            "{class}: {:?}",
            out.warnings
        );
        // The top-level objects survive into the relocated table.
        let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
        assert!(
            doc["extras"][class]["t1"].is_object(),
            "{class}: {}",
            out.text
        );
    }
}

/// Writing schema 0.1.0, the same `extras` slot takes entries from two
/// places: the source `extras` block and the source top-level table. A name in
/// both is a replacement, and the two tier fidelity rule says a replacement is
/// never silent.
#[test]
fn source_extras_entry_replaced_by_the_top_level_table_warns() {
    let text = doc_with(
        r#", "extras": {"time_series": {"t1": {"values": [9.0]}}},
        "time_series": {"t1": {"values": [1.0]}}"#,
    );
    let net = parse_bmopf_str(&text).unwrap();
    let out = emit_bmopf_json_with_options(
        &net,
        BmopfEmitOptions::default().with_profile(BmopfProfile::Bmopf010),
    );
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("time_series t1") && w.contains("replaced")),
        "{:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(doc["extras"]["time_series"]["t1"]["values"][0], 1.0);
}

#[test]
fn oversized_matrix_key_is_bounded_and_warned() {
    // The largest matrix index seen sizes a dense allocation; an unbounded
    // key would demand gigabytes from a few bytes of JSON. Out-of-bounds
    // indices fall out of the matrix grammar and land in extras, warned.
    let net = helpers::parse_str(
        r#"{"linecode":{"lc":{"R_series_100000_1":1.0}}}"#,
        "bmopf-json",
    )
    .unwrap();
    assert!(
        net.warnings.iter().any(|w| w.contains("R_series_100000_1")),
        "warnings: {:?}",
        net.warnings
    );
}

#[test]
fn winding_count_is_bounded() {
    let windings: Vec<serde_json::Value> = (0..65)
        .map(|i| {
            serde_json::json!({
                "bus": format!("b{i}"), "terminal_map": ["1", "2", "3"], "v_nom": 1000.0
            })
        })
        .collect();
    let doc = serde_json::json!({
        "transformer": {"n_winding": {"t1": {"windings": windings, "s_rating": 1000.0}}}
    });
    let net = helpers::parse_str(&doc.to_string(), "bmopf-json").unwrap();
    assert!(
        net.warnings.iter().any(|w| w.contains("supported maximum")),
        "warnings: {:?}",
        net.warnings
    );
    assert_eq!(net.transformers().len(), 1);
}

#[test]
fn meta_block_is_kept_in_extras() {
    let net = helpers::parse_str(
        r#"{"bus":{"b1":{}},"meta":{"license":"CC-BY-4.0"}}"#,
        "bmopf-json",
    )
    .unwrap();
    assert!(
        net.extras().contains_key("bmopf_meta"),
        "{:?}",
        net.extras()
    );
}

/// Schema 0.1.0 calls `length` "purely descriptive" on the inline impedance
/// branch: the matrices are the whole impedance of the line, in ohms. A
/// reader that scales them by a declared length multiplies the impedance.
#[test]
fn inline_impedance_is_not_scaled_by_a_descriptive_length() {
    let text = doc_with(
        r#", "line": {"l": {"bus_from": "a", "bus_to": "a", "terminal_map_from": ["1"],
            "terminal_map_to": ["1"], "length": 100.0,
            "R_series_1_1": 1.0, "X_series_1_1": 2.0}}"#,
    );
    let net = parse_bmopf_str(&text).unwrap();
    let line = &net.lines()[0];
    let code = net.linecode(&line.linecode).unwrap();
    assert_eq!(line.length.to_bits(), 1.0f64.to_bits());
    assert_eq!(
        (code.r_series[0][0] * line.length).to_bits(),
        1.0f64.to_bits()
    );
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("line l") && w.contains("does not scale")),
        "{:?}",
        net.warnings
    );
}

/// Schema 0.1.0 moved the IBR and control profile tables under `extras`, and
/// the reader accepts both places. A part-migrated document that declares a
/// name twice must not produce two elements of that name.
#[test]
fn a_name_declared_at_the_top_level_and_under_extras_reads_once() {
    let ibr = r#"{"pv1": {"bus": "a", "terminal_map": ["1"], "p_max": 1000.0}}"#;
    let profile = r#"{"cp1": {"power_factor": {"pf": 0.95}}}"#;
    let text = doc_with(&format!(
        r#", "ibr": {ibr}, "control_profile": {profile},
        "extras": {{"ibr": {ibr}, "control_profile": {profile}}}"#
    ));
    let net = parse_bmopf_str(&text).unwrap();
    assert_eq!(net.ibrs().len(), 1);
    assert_eq!(net.control_profiles().len(), 1);
    for class in ["ibr pv1", "control_profile cp1"] {
        assert!(
            net.warnings
                .iter()
                .any(|w| w.contains(class) && w.contains("second copy is dropped")),
            "{class}: {:?}",
            net.warnings
        );
    }
}

/// The transformer arm folds `extras.transformer` back onto the raw
/// transformer objects. An overlay with no transformer table to attach to
/// has no reader, so it must not disappear without a word.
#[test]
fn an_orphan_transformer_overlay_warns() {
    let text = doc_with(r#", "extras": {"transformer": {"single_phase": {"t": {"tap": 1.05}}}}"#);
    let net = parse_bmopf_str(&text).unwrap();
    assert!(net.transformers().is_empty());
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("`extras.transformer`") && w.contains("dropped")),
        "{:?}",
        net.warnings
    );
}

/// An OpenDSS capacitor the dss reader could not type stays untyped. The
/// typed `capacitor` table is strict, so the raw properties re-emit under
/// `extras`, which the schema leaves free-form.
#[test]
fn an_untyped_capacitor_re_emits_under_extras() {
    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\n\
         New Capacitor.cap bus1=b.1 phases=0 kv=7.2 kvar=300\n",
    );
    assert!(net.untyped_objects().iter().any(|u| u.class == "capacitor"));
    let out = emit_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert!(
        doc["extras"]["capacitor"]["cap"].is_object(),
        "{}",
        out.text
    );
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
}

/// Each winding holds its percent resistance on its own rating base. A
/// rating that is not positive has no base to refer to, so that term drops
/// with a warning; an infinity would otherwise reach `num` and emit as a
/// lossless zero.
#[test]
fn a_winding_rating_that_is_not_positive_drops_only_its_own_resistance_term() {
    let names = || vec!["1".to_string(), "2".to_string(), "3".to_string()];
    let mut from = DistWinding::new("a", names(), DistWindingConn::Delta, 416.0, 1000.0);
    from.r_pct = 1.0;
    let mut to = DistWinding::new("a", names(), DistWindingConn::Wye, 240.0, 0.0);
    to.r_pct = 1.0;
    let mut net = MulticonductorNetwork::default();
    net.transformers_mut()
        .push(DistTransformer::new("t", vec![from, to], vec![4.0], 3));

    let out = emit_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let r = doc["transformer"]["delta_wye"]["t"]["r_series"]
        .as_f64()
        .expect("r_series is a number");
    // The `from` term survives. An infinity from the zero rating would have
    // reached `num` and emitted as a lossless 0.
    assert!(r > 0.0 && r.is_finite(), "r_series = {r}");
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("`to` winding rating is not positive")),
        "{:?}",
        out.warnings
    );
}

/// XHL is a percent on the first winding's rating base, the same base the
/// resistance terms use. A rating that is not positive leaves the reactance
/// undefined, and emitting it as 0 would model the transformer as a short
/// circuit, so the field drops with a warning instead. The schema leaves
/// `x_series` optional, so an absent field reads as unknown.
#[test]
fn a_from_rating_that_is_not_positive_drops_the_reactance_instead_of_zeroing_it() {
    let names = || vec!["1".to_string(), "2".to_string(), "3".to_string()];
    let from = DistWinding::new("a", names(), DistWindingConn::Delta, 416.0, 0.0);
    let to = DistWinding::new("a", names(), DistWindingConn::Wye, 240.0, 1000.0);
    let mut net = MulticonductorNetwork::default();
    net.transformers_mut()
        .push(DistTransformer::new("t", vec![from, to], vec![4.0], 3));

    let out = emit_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let t = &doc["transformer"]["delta_wye"]["t"];
    assert!(
        t.get("x_series").is_none(),
        "x_series must be absent, not a zero short circuit: {}",
        out.text
    );
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("`x_series` is dropped")),
        "{:?}",
        out.warnings
    );
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
}

/// The re-vendored example networks carry no generator `cost` and no bus
/// `vpn_min`, so the assertions that used to cover both read paths went with
/// the old fixtures. A synthetic document keeps the coverage: the per-phase
/// cost array is kept exactly as stated, and the phase to neutral bound
/// arrays survive read and write.
#[test]
fn generator_cost_and_phase_neutral_bounds_survive_a_round_trip() {
    let text = r#"{
      "bus": {"a": {"terminal_names": ["1", "2", "3"],
        "vpn_min": [220.0, 220.0, 220.0], "vpn_max": [260.0, 260.0, 260.0]}},
      "voltage_source": {"s": {"v_magnitude": [240.0], "v_angle": [0.0],
        "bus": "a", "terminal_map": ["1"]}},
      "generator": {"g": {"bus": "a", "terminal_map": ["1", "2", "3"],
        "configuration": "WYE", "p_max": [1000.0], "cost": [0.12, 0.12, 0.12]}}
    }"#;
    let net = parse_bmopf_str(text).unwrap();

    let bus = net.bus("a").unwrap();
    assert_eq!(
        bus.vpn_min.as_deref(),
        Some([220.0, 220.0, 220.0].as_slice())
    );
    assert_eq!(
        bus.vpn_max.as_deref(),
        Some([260.0, 260.0, 260.0].as_slice())
    );
    let generator = &net.generators()[0];
    assert_eq!(
        generator.cost.as_deref(),
        Some([0.12, 0.12, 0.12].as_slice())
    );

    // Both survive the write and read back.
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let again = parse_bmopf_str(&out.text).unwrap();
    assert_eq!(again.bus("a").unwrap().vpn_min, bus.vpn_min);
    assert_eq!(again.generators()[0].cost, generator.cost);
}

/// Schema 0.1.0 defines generator `s_max`/`i_max`, linecode `source`, and the
/// `meta` provenance fields. Each is read into the model without an
/// outside-the-schema warning, survives the write, and validates; `meta`
/// keeps the source provenance while the writer owns `$schema`, `frequency`,
/// and `case_study_generator`.
#[test]
fn schema_fields_survive_a_round_trip_without_wrong_warnings() {
    let text = r#"{
      "meta": {"title": "synthetic feeder", "license": "CC-BY-4.0",
        "authors": [{"name": "task force"}],
        "data_sources": [{"name": "measurement campaign"}],
        "created": "2026-01-01", "provenance": {"note": "synthetic"},
        "frequency": 50.0,
        "case_study_generator": {"tool": "elsewhere", "version": "0.0.0"}},
      "bus": {"a": {"terminal_names": ["1", "2", "3"]}},
      "voltage_source": {"s": {"v_magnitude": [240.0], "v_angle": [0.0],
        "bus": "a", "terminal_map": ["1"]}},
      "linecode": {"lc": {"R_series_1_1": 0.1, "X_series_1_1": 0.2,
        "source": "datasheet"}},
      "line": {"l": {"bus_from": "a", "bus_to": "a",
        "terminal_map_from": ["1"], "terminal_map_to": ["2"],
        "linecode": "lc", "length": 10.0}},
      "generator": {"g": {"bus": "a", "terminal_map": ["1", "2", "3"],
        "configuration": "WYE", "cost": [0.1, 0.1, 0.1],
        "s_max": [5000.0, 5000.0, 5000.0], "i_max": [20.0, 20.0, 20.0]}}
    }"#;
    let net = parse_bmopf_str(text).unwrap();
    assert!(
        net.warnings
            .iter()
            .all(|w| !w.contains("outside the schema")),
        "schema fields must not warn as outside the schema: {:?}",
        net.warnings
    );
    let generator = &net.generators()[0];
    assert_eq!(generator.s_max.as_deref(), Some([5000.0; 3].as_slice()));
    assert_eq!(generator.i_max.as_deref(), Some([20.0; 3].as_slice()));
    assert_eq!(net.line_codes()[0].source.as_deref(), Some("datasheet"));

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let meta = &doc["meta"];
    assert_eq!(meta["title"], serde_json::json!("synthetic feeder"));
    assert_eq!(meta["license"], serde_json::json!("CC-BY-4.0"));
    assert_eq!(meta["authors"], serde_json::json!([{"name": "task force"}]));
    assert_eq!(
        meta["data_sources"],
        serde_json::json!([{"name": "measurement campaign"}])
    );
    assert_eq!(meta["created"], serde_json::json!("2026-01-01"));
    assert_eq!(meta["provenance"]["note"], "synthetic");
    // Writer-owned: powerio's stamp and the model frequency, whatever the
    // source declared.
    assert_eq!(meta["case_study_generator"]["tool"], "powerio");
    assert_eq!(meta["frequency"], serde_json::json!(50.0));
    assert_eq!(
        doc["generator"]["g"]["s_max"],
        serde_json::json!([5000.0, 5000.0, 5000.0])
    );
    assert_eq!(
        doc["generator"]["g"]["i_max"],
        serde_json::json!([20.0, 20.0, 20.0])
    );
    assert_eq!(
        doc["linecode"]["lc"]["source"],
        serde_json::json!("datasheet")
    );

    // The write is a fixed point: a second read and write changes nothing.
    let again = parse_bmopf_str(&out.text).unwrap();
    assert_eq!(emit_bmopf_json(&again).text, out.text);
}

#[test]
fn bmopf_transformer_without_v_nom_never_writes_nan_kv() {
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "hv": {"terminal_names": ["1", "2", "3"]},
            "lv": {"terminal_names": ["1", "2", "3", "4"], "perfectly_grounded_terminals": ["4"]}
          },
          "voltage_source": {
            "source": {"v_magnitude": [6350.9, 6350.9, 6350.9], "v_angle": [0.0, -2.0943951023931953, 2.0943951023931953], "bus": "hv", "terminal_map": ["1", "2", "3"]}
          },
          "transformer": {
            "delta_wye": {
              "t1": {
                "bus_from": "hv", "bus_to": "lv",
                "terminal_map_from": ["1", "2", "3"], "terminal_map_to": ["1", "2", "3", "4"],
                "s_rating": 500000.0,
                "r_series": 0.001, "x_series": 0.002
              }
            }
          }
        }"#,
    )
    .unwrap();
    let out = emit_dss(&net);
    assert!(!out.text.contains("NaN"), "{}", out.text);
    let t = out
        .text
        .lines()
        .find(|l| l.contains("Transformer.t1 "))
        .unwrap();
    assert!(!t.contains("kvs="), "{t}");
    assert!(t.contains("kvas=(500, 500)"), "{t}");
    // The hv winding derives from the source bus estimate. The lv bus has no
    // estimate.
    let expected_kv = 6350.9 * 3f64.sqrt() / 1e3;
    let edit = out
        .text
        .lines()
        .find(|l| l.starts_with("~ wdg=1 "))
        .unwrap();
    assert_eq!(edit, format!("~ wdg=1 kv={expected_kv}"));
    assert!(!out.text.contains("wdg=2 kv="), "{}", out.text);
    assert!(
        out.warnings.iter().any(|w| w.contains("transformer t1")
            && w.contains("winding 1")
            && w.contains("derived")),
        "{:?}",
        out.warnings
    );
    assert!(
        out.warnings.iter().any(|w| w.contains("transformer t1")
            && w.contains("winding 2")
            && w.contains("kv not emitted")),
        "{:?}",
        out.warnings
    );
    // The deck must read back, with the derived hv winding voltage.
    let reparsed = parse_dss_str(&out.text);
    let rt = &reparsed.transformers()[0];
    assert!((rt.windings[0].v_ref - expected_kv * 1e3).abs() < 1e-9);
}

#[test]
fn bmopf_transformer_without_s_rating_never_writes_nan_kva() {
    let net = parse_bmopf_str(
        r#"{
          "bus": {
            "hv": {"terminal_names": ["1", "2", "3"]},
            "lv": {"terminal_names": ["1", "2", "3", "4"], "perfectly_grounded_terminals": ["4"]}
          },
          "voltage_source": {
            "source": {"v_magnitude": [6350.9, 6350.9, 6350.9], "v_angle": [0.0, -2.0943951023931953, 2.0943951023931953], "bus": "hv", "terminal_map": ["1", "2", "3"]}
          },
          "transformer": {
            "delta_wye": {
              "t1": {
                "bus_from": "hv", "bus_to": "lv",
                "terminal_map_from": ["1", "2", "3"], "terminal_map_to": ["1", "2", "3", "4"],
                "v_nom_from": 11000.0, "v_nom_to": 415.0,
                "r_series": 0.001, "x_series": 0.002
              }
            }
          }
        }"#,
    )
    .unwrap();
    let out = emit_dss(&net);
    assert!(!out.text.contains("NaN"), "{}", out.text);
    let t = out
        .text
        .lines()
        .find(|l| l.contains("Transformer.t1 "))
        .unwrap();
    assert!(t.contains("kvs=(11, 0.415)"), "{t}");
    assert!(!t.contains("kvas="), "{t}");
    assert!(!out.text.contains("kva="), "{}", out.text);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("transformer t1") && w.contains("kva not emitted")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn dss_network_authors_terminal_conventions_from_its_naming() {
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.tc basekv=12.47 pu=1.0 phases=3 bus1=src.1.2.3\n\
         New Line.l1 bus1=src.1.2.3 bus2=mid.1.2.3 phases=3 r1=0.1 x1=0.2 length=10 units=m\n",
    );
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    // The source uses terminal 4 as its grounded neutral.
    assert_eq!(
        doc["terminal_conventions"],
        serde_json::json!({"phase": ["1", "2", "3"], "neutral": ["4"]})
    );
    assert!(
        !out.warnings
            .iter()
            .any(|w| w.contains("terminal_conventions")),
        "{:?}",
        out.warnings
    );
    // The reader keeps the block, and the next write gives it again.
    let again = parse_bmopf_str(&out.text).unwrap();
    assert_eq!(emit_bmopf_json(&again).text, out.text);
}

#[test]
fn source_terminal_conventions_pass_through_verbatim() {
    let net = parse_bmopf_str(
        r#"{
          "terminal_conventions": {"phase": ["a", "b", "c"], "neutral": ["n"]},
          "bus": {"b": {"terminal_names": ["1"]}},
          "voltage_source": {
            "s": {"v_magnitude": [240.0], "v_angle": [0.0], "bus": "b", "terminal_map": ["1"]}
          }
        }"#,
    )
    .unwrap();
    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&schema_validator(), &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    // The source block wins over the bus naming.
    assert_eq!(
        doc["terminal_conventions"],
        serde_json::json!({"phase": ["a", "b", "c"], "neutral": ["n"]})
    );
}

/// A line with no `linecode=` carries its rating on the linecode synthesized
/// for it, not on both. Emitting it twice reads as a per line override of a
/// shared default, and `_line_*` is a stand-in that belongs to this line alone.
#[test]
fn an_inline_line_rating_is_not_repeated_on_its_synthetic_linecode() {
    let src = "New Circuit.c1\n\
               New Line.l1 bus1=a bus2=b phases=3 r1=0.1 x1=0.2 length=10 units=m emergamps=250\n";
    let net = parse_dss_str(src);
    let line = &net.lines()[0];
    let code = net
        .line_codes()
        .iter()
        .find(|c| c.name == line.linecode)
        .expect("synthetic linecode");

    assert_eq!(
        code.i_max.as_deref(),
        Some([250.0, 250.0, 250.0].as_slice())
    );
    assert!(
        line.i_max.is_none(),
        "the rating belongs to the linecode alone: {:?}",
        line.i_max
    );

    let text = emit_dss(&net).text;
    assert_eq!(text.matches("emergamps=250").count(), 1, "{text}");
}

#[test]
fn a_non_numeric_bmopf_field_is_refused_rather_than_read_as_nan() {
    // #299: every one of these is schema invalid, and each used to read as
    // NaN, which serializes on as `null` and restores as an unbounded limit.
    for (spelling, what) in [
        (r#""not a number""#, "a string"),
        ("null", "null"),
        ("{}", "an object"),
        (
            r#"[400.0, "x"]"#,
            "an array holding a value that is not a number",
        ),
    ] {
        let text = format!(
            r#"{{
                "bus": {{"b": {{"terminal_names": ["1"], "perfectly_grounded_terminals": []}}}},
                "linecode": {{"lc": {{"i_max": {spelling}}}}}
            }}"#
        );
        let net = parse_bmopf_str(&text).unwrap();
        let found: Vec<_> = net
            .diagnostics
            .iter()
            .filter(|d| d.code() == "READ.BMOPF.FIELD_NOT_A_NUMBER")
            .collect();
        assert_eq!(found.len(), 1, "{spelling}: {:?}", net.diagnostics);
        assert_eq!(found[0].severity(), DiagnosticSeverity::Error, "{spelling}");
        assert_eq!(found[0].stage(), Some(DiagnosticStage::Read), "{spelling}");
        assert_eq!(found[0].target(), Some("/linecode/lc/i_max"), "{spelling}");
        assert!(
            found[0].message().contains(what),
            "{spelling}: {}",
            found[0].message()
        );
    }
}

#[test]
fn reporting_a_malformed_field_does_not_disturb_the_read() {
    // The report used to remove the field. `R_series_1_1` is the presence
    // check that picks the inline branch of the line `oneOt`, so removing it
    // sent the whole line down the linecode branch and every other matrix
    // entry landed in extras as "outside the schema".
    let text = r#"{
        "bus": {
            "a": {"terminal_names": ["1", "2", "3"], "perfectly_grounded_terminals": []},
            "b": {"terminal_names": ["1", "2", "3"], "perfectly_grounded_terminals": []}
        },
        "line": {
            "ln1": {
                "bus_from": "a", "bus_to": "b",
                "terminal_map_from": ["1", "2", "3"], "terminal_map_to": ["1", "2", "3"],
                "R_series_1_1": null,
                "R_series_2_2": 0.3, "R_series_3_3": 0.3,
                "X_series_1_1": 0.6, "X_series_2_2": 0.6, "X_series_3_3": 0.6
            }
        }
    }"#;
    let net = parse_bmopf_str(text).unwrap();
    assert_eq!(
        net.diagnostics
            .iter()
            .filter(|d| d.code() == "READ.BMOPF.FIELD_NOT_A_NUMBER")
            .count(),
        1,
        "{:?}",
        net.diagnostics
    );
    // The line keeps its inline matrices: one synthesized linecode, with the
    // sound entries intact and only the malformed cell undefined.
    assert_eq!(net.line_codes().len(), 1, "{:?}", net.warnings);
    let lc = &net.line_codes()[0];
    assert!(lc.r_series[0][0].is_nan(), "{:?}", lc.r_series);
    for (got, want) in [
        (lc.r_series[1][1], 0.3),
        (lc.r_series[2][2], 0.3),
        (lc.x_series[0][0], 0.6),
    ] {
        assert!((got - want).abs() < 1e-12, "{got} != {want}");
    }
    assert!(
        !net.warnings
            .iter()
            .any(|w| w.contains("outside the schema")),
        "{:?}",
        net.warnings
    );
}

#[test]
fn consumer_extras_are_not_read_as_schema_fields() {
    // `extras` round trips arbitrary consumer JSON, where a name like `cost`
    // carries no schema meaning.
    let text = r#"{
        "bus": {"b": {"terminal_names": ["1"], "perfectly_grounded_terminals": []}},
        "extras": {"anything": {"cost": "free", "v_nom": null}}
    }"#;
    let net = parse_bmopf_str(text).unwrap();
    let beside: Vec<_> = net
        .diagnostics
        .iter()
        .filter(|d| !d.code().starts_with("READ.BMOPF.SCHEMA_"))
        .collect();
    assert!(beside.is_empty(), "{:?}", net.diagnostics);
}

// ----- regulator subtypes -------------------------------------------------

#[test]
fn dss_single_phase_regulator_emits_the_autotransformer_subtype() {
    let v = schema_validator();
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.regcase basekv=2.4 bus1=650.1.2.3\n\
         New Transformer.vreg phases=1 windings=2 buses=(650.1 rg60.1) \
         kvs=(2.4 2.4) kvas=(1666 1666) xhl=0.01 %loadloss=0.01 taps=(1.0 1.05)\n\
         New RegControl.cvreg transformer=vreg winding=2 vreg=122 band=2 ptratio=20",
    );
    let t = &net.transformers()[0];
    assert_eq!(
        t.extras.get("bmopf_subtype"),
        Some(&serde_json::json!("single_phase_autotransformer"))
    );

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let reg = &doc["transformer"]["single_phase_autotransformer"]["vreg"];
    assert_eq!(reg["bus_from"], "650");
    assert_eq!(reg["bus_to"], "rg60");
    assert_eq!(reg["s_rating"], serde_json::json!(1_666_000.0));
    // %loadloss 0.01 splits into 0.005% per winding on zb = 2400^2/1666000.
    let zb = 2400.0 * 2400.0 / 1_666_000.0;
    assert!((reg["r_series_from"].as_f64().unwrap() - 0.005 / 100.0 * zb).abs() < 1e-15);
    assert!((reg["x_series_from"].as_f64().unwrap() - 0.01 / 100.0 * zb).abs() < 1e-15);
    assert_eq!(reg["x_series_to"], serde_json::json!(0.0));
    assert_eq!(reg["tap_ratio"], serde_json::json!(1.05));
    // The subtype has no voltage fields; the ratio is tap_ratio alone.
    assert!(reg.get("v_nom_from").is_none(), "{reg}");
    assert!(reg.get("v_nom_to").is_none(), "{reg}");

    // The classification marker is converter bookkeeping; the dss echo
    // neither prints it nor warns about it.
    let dss_out = emit_dss(&net);
    assert!(!dss_out.text.contains("bmopf_subtype"), "{}", dss_out.text);
    assert!(
        dss_out
            .warnings
            .iter()
            .all(|w| !w.contains("bmopf_subtype")),
        "{:?}",
        dss_out.warnings
    );
}

#[test]
fn dss_open_delta_regulator_pair_merges_into_one_object() {
    let v = schema_validator();
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.odcase basekv=4.16 bus1=busa.1.2.3\n\
         New Transformer.rega phases=1 windings=2 buses=(busa.1.2 busb.1.2) \
         kvs=(4.16 4.16) kvas=(1000 1000) xhl=1 %loadloss=0.2 taps=(1.0 1.03)\n\
         New RegControl.crega transformer=rega winding=2 vreg=120 band=2 ptratio=34.7\n\
         New Transformer.regb phases=1 windings=2 buses=(busa.2.3 busb.2.3) \
         kvs=(4.16 4.16) kvas=(1000 1000) xhl=1 %loadloss=0.2 taps=(1.0 1.06)\n\
         New RegControl.cregb transformer=regb winding=2 vreg=120 band=2 ptratio=34.7",
    );
    for t in net.transformers() {
        assert_eq!(
            t.extras.get("bmopf_subtype"),
            Some(&serde_json::json!("open_delta_regulator")),
            "{}",
            t.name
        );
    }

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let table = doc["transformer"]["open_delta_regulator"]
        .as_object()
        .unwrap();
    assert_eq!(table.keys().collect::<Vec<_>>(), ["rega"]);
    let reg = &table["rega"];
    assert_eq!(reg["connection"], "ABBC");
    assert_eq!(reg["bus_from"], "busa");
    assert_eq!(reg["bus_to"], "busb");
    assert_eq!(reg["terminal_map_from"], serde_json::json!(["1", "2", "3"]));
    assert_eq!(reg["terminal_map_to"], serde_json::json!(["1", "2", "3"]));
    assert_eq!(reg["tap_ratio"], serde_json::json!([1.03, 1.06]));
    let zb = 4160.0 * 4160.0 / 1_000_000.0;
    assert!((reg["r_series_from"].as_f64().unwrap() - 0.1 / 100.0 * zb).abs() < 1e-12);
    assert!((reg["x_series_from"].as_f64().unwrap() - 1.0 / 100.0 * zb).abs() < 1e-12);
    let merge = diagnostic(
        &out,
        "EMIT.BMOPF.TRANSFORMER_OPEN_DELTA_MERGED",
        "transformer rega",
    );
    assert_eq!(merge.details()["merged_leg"], serde_json::json!("regb"));

    // The emitted document reads back untyped and re-emits byte identical.
    let again = parse_bmopf_str(&out.text).unwrap();
    let twice = emit_bmopf_json(&again);
    assert_eq!(out.text, twice.text);
}

#[test]
fn unregulated_and_mismatched_transformers_keep_their_subtype() {
    // No regcontrol: a plain two winding unit stays single_phase.
    let plain = parse_dss_str(
        "Clear\n\
         New Circuit.plaincase basekv=2.4 bus1=650.1.2.3\n\
         New Transformer.t1 phases=1 windings=2 buses=(650.1 651.1) \
         kvs=(2.4 2.4) kvas=(100 100) xhl=2",
    );
    assert!(!plain.transformers()[0].extras.contains_key("bmopf_subtype"));
    let doc: serde_json::Value = serde_json::from_str(&emit_bmopf_json(&plain).text).unwrap();
    assert!(doc["transformer"]["single_phase"].get("t1").is_some());
    assert!(
        doc["transformer"]
            .get("single_phase_autotransformer")
            .is_none()
    );

    // A regcontrol on a voltage changing transformer is an LTC, and its
    // winding voltages rule the series regulator reading out.
    let ltc = parse_dss_str(
        "Clear\n\
         New Circuit.ltccase basekv=2.4 bus1=650.1.2.3\n\
         New Transformer.t2 phases=1 windings=2 buses=(650.1 lv.1) \
         kvs=(2.4 0.24) kvas=(100 100) xhl=2\n\
         New RegControl.ct2 transformer=t2 winding=2 vreg=122 band=2 ptratio=20",
    );
    assert!(!ltc.transformers()[0].extras.contains_key("bmopf_subtype"));
    let doc: serde_json::Value = serde_json::from_str(&emit_bmopf_json(&ltc).text).unwrap();
    assert!(doc["transformer"]["single_phase"].get("t2").is_some());
}

#[test]
fn open_delta_legs_with_different_impedance_stay_single_units() {
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.odcase basekv=4.16 bus1=busa.1.2.3\n\
         New Transformer.rega phases=1 windings=2 buses=(busa.1.2 busb.1.2) \
         kvs=(4.16 4.16) kvas=(1000 1000) xhl=1\n\
         New RegControl.crega transformer=rega winding=2 vreg=120 band=2 ptratio=34.7\n\
         New Transformer.regb phases=1 windings=2 buses=(busa.2.3 busb.2.3) \
         kvs=(4.16 4.16) kvas=(1000 1000) xhl=2\n\
         New RegControl.cregb transformer=regb winding=2 vreg=120 band=2 ptratio=34.7",
    );
    for t in net.transformers() {
        assert_eq!(
            t.extras.get("bmopf_subtype"),
            Some(&serde_json::json!("single_phase_autotransformer")),
            "{}",
            t.name
        );
    }
    let doc: serde_json::Value = serde_json::from_str(&emit_bmopf_json(&net).text).unwrap();
    let table = doc["transformer"]["single_phase_autotransformer"]
        .as_object()
        .unwrap();
    assert_eq!(table.len(), 2);
    assert!(doc["transformer"].get("open_delta_regulator").is_none());
}

#[test]
fn open_delta_leg_without_its_partner_falls_back_to_a_single_unit() {
    let mut net = parse_dss_str(
        "Clear\n\
         New Circuit.odcase basekv=4.16 bus1=busa.1.2.3\n\
         New Transformer.rega phases=1 windings=2 buses=(busa.1.2 busb.1.2) \
         kvs=(4.16 4.16) kvas=(1000 1000) xhl=1\n\
         New RegControl.crega transformer=rega winding=2 vreg=120 band=2 ptratio=34.7\n\
         New Transformer.regb phases=1 windings=2 buses=(busa.2.3 busb.2.3) \
         kvs=(4.16 4.16) kvas=(1000 1000) xhl=1\n\
         New RegControl.cregb transformer=regb winding=2 vreg=120 band=2 ptratio=34.7",
    );
    net.transformers_mut().retain(|t| t.name == "rega");
    let out = emit_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert!(
        doc["transformer"]["single_phase_autotransformer"]
            .get("rega")
            .is_some()
    );
    assert!(doc["transformer"].get("open_delta_regulator").is_none());
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("rega") && w.contains("no matching partner")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn bmopf_regulator_subtypes_round_trip_verbatim() {
    let v = schema_validator();
    let text = r#"{
        "bus": {
            "b1": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]},
            "b2": {"terminal_names": ["1", "2", "3", "n"], "perfectly_grounded_terminals": ["n"]}
        },
        "voltage_source": {"src": {"bus": "b1", "terminal_map": ["1", "n"],
            "v_magnitude": [2400.0], "v_angle": [0.0]}},
        "transformer": {
            "single_phase_autotransformer": {
                "svr": {
                    "bus_from": "b1", "bus_to": "b2",
                    "terminal_map_from": ["1", "n"], "terminal_map_to": ["1", "n"],
                    "s_rating": 1666000.0,
                    "r_series_from": 0.001, "x_series_from": 0.002,
                    "r_series_to": 0.001, "x_series_to": 0.0,
                    "g_no_load": 0.0001, "b_no_load": -0.0002,
                    "tap_ratio": 1.05, "tap_ratio_min": 0.9, "tap_ratio_max": 1.1,
                    "regulator_type": "B",
                    "i_max_from": [668.0], "i_max_to": [668.0]
                }
            },
            "open_delta_regulator": {
                "odr": {
                    "bus_from": "b1", "bus_to": "b2",
                    "terminal_map_from": ["1", "2", "3"], "terminal_map_to": ["1", "2", "3"],
                    "s_rating": 1000000.0,
                    "r_series_from": 0.01, "x_series_from": 0.02,
                    "connection": "BCAC",
                    "tap_ratio": [1.03, 1.06],
                    "regulator_type": "A"
                }
            }
        }
    }"#;
    let net = parse_bmopf_str(text).unwrap();
    // Typed reads: the autotransformer is one row, the open delta bank its
    // two legs, none of them untyped.
    assert!(net.untyped_objects().is_empty());
    let by_name = |name: &str| {
        net.transformers()
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} missing"))
    };
    assert_eq!(net.transformers().len(), 3);
    let svr = by_name("svr");
    assert_eq!(
        svr.extras.get("bmopf_subtype").and_then(|v| v.as_str()),
        Some("single_phase_autotransformer")
    );
    // tap_ratio regulates the to side: to tap over from tap.
    assert!((svr.windings[1].tap / svr.windings[0].tap - 1.05).abs() < 1e-12);
    // The BCAC connection names legs on phases (2,3) and (1,3).
    let odr = by_name("odr");
    assert_eq!(odr.windings[0].terminal_map, ["2", "3"]);
    assert_eq!(by_name("odr:leg2").windings[0].terminal_map, ["1", "3"]);
    assert!((odr.windings[1].tap / odr.windings[0].tap - 1.03).abs() < 1e-12);

    let out = emit_bmopf_json(&net);
    assert_eq!(errors(&v, &out.text), Vec::<String>::new());
    let source: serde_json::Value = serde_json::from_str(text).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(
        doc["transformer"]["single_phase_autotransformer"]["svr"],
        source["transformer"]["single_phase_autotransformer"]["svr"]
    );
    assert_eq!(
        doc["transformer"]["open_delta_regulator"]["odr"],
        source["transformer"]["open_delta_regulator"]["odr"]
    );
    let again = parse_bmopf_str(&out.text).unwrap();
    let twice = emit_bmopf_json(&again);
    assert_eq!(out.text, twice.text);
}

#[test]
fn a_fixed_tap_open_delta_bank_emits_two_single_phase_units() {
    // Two 1 phase Delta/Delta legs across A-B and B-C between one bus pair: a
    // fixed tap open delta bank with no RegControl. Each leg is a line to
    // line unit the single_phase shape holds; through 0.9.0 the [Delta,
    // Delta] pairing had no classify arm and both legs dropped silently.
    let net = parse_dss_str(
        "Clear\n\
         New Circuit.odb basekv=12.47 bus1=sourcebus.1.2.3\n\
         New Transformer.leg_ab phases=1 windings=2 buses=(sourcebus.1.2, loadbus.1.2) \
         conns=(delta delta) kvs=(12.47 12.47) kvas=(100 100) xhl=1\n\
         New Transformer.leg_bc phases=1 windings=2 buses=(sourcebus.2.3, loadbus.2.3) \
         conns=(delta delta) kvs=(12.47 12.47) kvas=(100 100) xhl=1\n",
    );
    assert_eq!(net.transformers().len(), 2, "{:?}", net.transformers());

    let out = emit_bmopf_json(&net);
    assert!(
        !out.warnings
            .iter()
            .any(|w| w.contains("EMIT.BMOPF.TRANSFORMER_UNSUPPORTED")),
        "the bank must not drop: {:?}",
        out.warnings
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    let single_phase = &doc["transformer"]["single_phase"];
    assert!(
        single_phase.get("leg_ab").is_some() && single_phase.get("leg_bc").is_some(),
        "both legs emit as single_phase: {}",
        doc["transformer"]
    );
}

/// #383: a classified regulator bank read back out of a BMOPF document is
/// typed transformer rows, so a dss regeneration keeps the bank instead of
/// dropping untyped objects on both legs.
#[test]
fn regulator_banks_survive_a_dss_bmopf_dss_round_trip() {
    let net = parse_dss_file(fixture("opendss/ieee13/IEEE13Nodeckt.dss")).unwrap();
    let names_before: Vec<String> = net
        .transformers()
        .iter()
        .map(|t| t.name.to_ascii_lowercase())
        .collect();
    assert!(
        names_before.iter().any(|n| n.starts_with("reg")),
        "{names_before:?}"
    );

    let bmopf = emit_bmopf_json(&net);
    let again = parse_bmopf_str(&bmopf.text).unwrap();
    assert!(
        again.untyped_objects().is_empty(),
        "untyped rows survived: {:?}",
        again
            .untyped_objects()
            .iter()
            .map(|u| &u.class)
            .collect::<Vec<_>>()
    );
    let regs: Vec<&str> = again
        .transformers()
        .iter()
        .map(|t| t.name.as_str())
        .filter(|n| n.to_ascii_lowercase().starts_with("reg"))
        .collect();
    assert!(!regs.is_empty(), "no regulator rows read back");

    let dss = emit_dss(&again);
    for reg in regs {
        assert!(
            dss.text.contains(&format!("Transformer.{reg} ")),
            "regulator {reg} missing from the regenerated dss:\n{}",
            dss.text
        );
    }
    assert!(
        !dss.warnings
            .iter()
            .any(|w| w.contains("RECORD_DROPPED") && w.contains("transformer")),
        "{:?}",
        dss.warnings
    );
}

// ----- schema versions ----------------------------------------------------

/// The writer stamps the version it targets into `meta`, and the reader
/// resolves that same value back to the version.
#[test]
fn the_written_schema_version_reads_back_as_the_version_written() {
    for profile in [BmopfProfile::Bmopf010, BmopfProfile::Bmopf020] {
        let net = parse_bmopf_file(fixture("bmopf/example_ieee13.json")).unwrap();
        let out =
            emit_bmopf_json_with_options(&net, BmopfEmitOptions::default().with_profile(profile));
        assert_eq!(
            errors(&schema_validator_for(profile), &out.text),
            Vec::<String>::new(),
            "{}",
            profile.version()
        );
        let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
        assert_eq!(doc["meta"]["$schema"], profile.retrieval_url());
        assert_eq!(
            BmopfProfile::from_schema_id(doc["meta"]["$schema"].as_str().unwrap()),
            Some(profile)
        );
        // Only 0.2.0 declares the version field, and its `meta` rejects what
        // it does not declare.
        match profile {
            BmopfProfile::Bmopf010 => assert!(doc["meta"]["schema_version"].is_null()),
            _ => assert_eq!(doc["meta"]["schema_version"], profile.version()),
        }
        let reparsed = parse_bmopf_str(&out.text).unwrap();
        assert!(
            !reparsed
                .diagnostics
                .iter()
                .any(|d| d.code().starts_with("READ.BMOPF.SCHEMA_")),
            "{:?}",
            reparsed.diagnostics
        );
    }
}

/// A document that states no schema names no version, and one that states a
/// location naming no version is no better. Both are reported, and both parse.
#[test]
fn a_document_naming_no_schema_version_is_reported() {
    let absent = parse_bmopf_str(&doc_with("")).unwrap();
    assert!(
        absent
            .diagnostics
            .iter()
            .any(|d| d.code() == "READ.BMOPF.SCHEMA_ABSENT"),
        "{:?}",
        absent.diagnostics
    );

    let unknown = parse_bmopf_str(&doc_with(
        r#", "meta": {"$schema": "https://example.org/bmopf/schema/v1/bmopf.json"}"#,
    ))
    .unwrap();
    assert!(
        unknown
            .diagnostics
            .iter()
            .any(|d| d.code() == "READ.BMOPF.SCHEMA_UNKNOWN"),
        "{:?}",
        unknown.diagnostics
    );

    // The two published examples state the draft schema location of the
    // repository that only ever held 0.1.0, so they resolve.
    for example in ["bmopf/example_ieee13.json", "bmopf/example_enwl_n1_f2.json"] {
        let net = parse_bmopf_file(fixture(example)).unwrap();
        assert!(
            !net.diagnostics
                .iter()
                .any(|d| d.code().starts_with("READ.BMOPF.SCHEMA_")),
            "{example}: {:?}",
            net.diagnostics
        );
    }
}

/// Schema 0.2.0 declares the IBR, control profile, DC and time series tables
/// at the top level, so writing it relocates nothing; schema 0.1.0 has no
/// table for them and files each under `extras`.
#[test]
fn the_tables_schema_0_1_0_relocates_stay_in_place_under_0_2_0() {
    let text = doc_with(
        r#", "ibr": {"pv": {"bus": "a", "terminal_map": ["1", "2"],
            "topology": "SINGLE_PHASE", "prime_mover": "PV", "s_max": [5000.0]}},
        "time_series": {"t1": {"values": [1.0, 0.5]}},
        "dc_bus": {"d1": {"terminal_names": ["p", "r"]}}"#,
    );
    let net = parse_bmopf_str(&text).unwrap();

    let out = emit_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    for table in ["ibr", "time_series", "dc_bus"] {
        assert!(doc[table].is_object(), "{table}: {}", out.text);
        assert!(doc["extras"][table].is_null(), "{table}: {}", out.text);
    }
    assert_eq!(
        errors(&schema_validator(), &out.text),
        Vec::<String>::new(),
        "{}",
        out.text
    );

    let out = emit_bmopf_json_with_options(
        &net,
        BmopfEmitOptions::default().with_profile(BmopfProfile::Bmopf010),
    );
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    for table in ["ibr", "time_series", "dc_bus"] {
        assert!(doc["extras"][table].is_object(), "{table}: {}", out.text);
        assert!(doc[table].is_null(), "{table}: {}", out.text);
    }
    assert_eq!(
        errors(&schema_validator_for(BmopfProfile::Bmopf010), &out.text),
        Vec::<String>::new(),
        "{}",
        out.text
    );
}

#[test]
fn n_winding_ratings_taps_neutrals_and_limits_survive_serialization() {
    let input = serde_json::json!({
        "bus": {"p": {"terminal_names": ["a", "n"]}, "s": {"terminal_names": ["a", "n"]}},
        "transformer": {"n_winding": {"t": {
            "s_rating": 10000.0,
            "windings": [
                {"bus": "p", "terminal_map": ["a", "n"], "configuration": "WYE", "v_nom": 1000.0,
                 "s_rating": 9000.0, "r_winding": 2.0, "tap_ratio": 1.02, "r_neutral": 5.0,
                 "x_neutral": 1.0, "i_max": 9.0, "tap_ratio_min": 0.95, "tap_ratio_max": 1.05},
                {"bus": "s", "terminal_map": ["a", "n"], "configuration": "WYE", "v_nom": 100.0,
                 "s_rating": 8000.0, "r_winding": 0.03, "tap_ratio": 0.98}
            ], "x_sc": {"1_2": 1.5}
        }}}
    });
    let parsed = parse_bmopf_str(&input.to_string()).unwrap();
    let restored: MulticonductorNetwork =
        serde_json::from_str(&serde_json::to_string(&*parsed).unwrap()).unwrap();
    for profile in [BmopfProfile::Bmopf020, BmopfProfile::Bmopf010] {
        let mut options = BmopfEmitOptions::default();
        options.profile = profile;
        let output = emit_bmopf_json_with_options(&restored, options);
        let doc: serde_json::Value = serde_json::from_str(&output.text).unwrap();
        assert!(schema_validator_for(profile).is_valid(&doc), "{doc}");
        let encoded = if profile == BmopfProfile::Bmopf020 {
            &doc["transformer"]["n_winding"]["t"]
        } else {
            &doc["extras"]["transformer"]["n_winding"]["t"]
        };
        assert_eq!(encoded["windings"][0]["i_max"], 9.0);
        assert_eq!(encoded["windings"][0]["tap_ratio_min"], 0.95);
        let back = parse_bmopf_str(&output.text).unwrap();
        let transformer = &back.transformers()[0];
        assert_eq!(
            transformer.windings[0].s_rating.to_bits(),
            9000.0_f64.to_bits()
        );
        assert_eq!(
            transformer.windings[1].s_rating.to_bits(),
            8000.0_f64.to_bits()
        );
        assert_eq!(transformer.windings[0].tap.to_bits(), 1.02_f64.to_bits());
        assert_eq!(transformer.windings[0].r_neutral, Some(5.0));
        assert!(
            (transformer.windings[0].r_pct - restored.transformers()[0].windings[0].r_pct).abs()
                < 1e-12
        );
        assert_eq!(encoded["x_sc"]["1_2"], 1.5);
    }
}

#[test]
fn proposal_provenance_is_pinned_and_preserves_existing_keys() {
    use powerio_dist::bmopf::{BMOPF_PROPOSAL_COMMIT, BMOPF_PROPOSAL_SHA256, BMOPF_PROPOSAL_URL};
    let mut net = parse_bmopf_file(fixture("bmopf/example_ieee13.json")).unwrap();
    net.extras_mut().insert(
        "bmopf_meta".into(),
        serde_json::json!({"provenance":{"powerio_bmopf":{"note":"keep"}}}),
    );
    let out = emit_bmopf_json(&net);
    let doc: serde_json::Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(doc["meta"]["$schema"], BMOPF_PROPOSAL_URL);
    let provenance = &doc["meta"]["provenance"];
    assert_eq!(provenance["powerio_bmopf"]["note"], "keep");
    assert_eq!(provenance["powerio_bmopf_1"]["schema_status"], "proposal");
    assert_eq!(
        provenance["powerio_bmopf_1"]["schema_sha256"],
        BMOPF_PROPOSAL_SHA256
    );
    assert_eq!(
        provenance["powerio_bmopf_1"]["schema_commit"],
        BMOPF_PROPOSAL_COMMIT
    );
    assert_eq!(
        emit_bmopf_json(&parse_bmopf_str(&out.text).unwrap()).text,
        out.text
    );
}

#[test]
fn emitted_micro_cases_keep_valid_conductor_references() {
    for name in [
        "xfmr_single_phase.dss",
        "xfmr_center_tap.dss",
        "switch.dss",
        "fourwire_linecode.dss",
        "linecode_10x10.dss",
    ] {
        let net =
            parse_dss_str(&std::fs::read_to_string(fixture(&format!("micro/{name}"))).unwrap());
        let text = emit_bmopf_json(&net).text;
        let parsed = parse_bmopf_str(&text).unwrap();
        assert!(
            !parsed
                .warnings
                .iter()
                .any(|d| d.contains("SEMANTIC_INVALID")),
            "{name}: {:?}",
            parsed.warnings
        );
    }
}

#[test]
fn three_phase_generator_without_neutral_keeps_phase_count_in_dss() {
    let net = parse_bmopf_file(fixture("bmopf/example_ieee13.json")).unwrap();
    let dss = emit_dss(&net);
    let generator = dss
        .text
        .lines()
        .find(|line| line.starts_with("New Generator.gen_634 "))
        .unwrap();
    assert!(generator.contains("phases=3"), "{generator}");
    let reread = parse_dss_str(&dss.text);
    let output = emit_bmopf_json(&reread);
    let parsed = parse_bmopf_str(&output.text).unwrap();
    assert!(
        !parsed
            .warnings
            .iter()
            .any(|d| d.contains("SEMANTIC_INVALID")),
        "{:?}",
        parsed.warnings
    );
}

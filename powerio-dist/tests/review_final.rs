//! Fresh-path regressions for the final RAVENS / PMD review.
mod helpers;
use powerio_dist::{DistTargetFormat, MulticonductorNetwork};
use serde_json::{Value, json};
fn transformer() -> MulticonductorNetwork {
    (*helpers::parse_dss_str("New Circuit.c basekv=11 bus1=p\nNew Transformer.t phases=3 windings=2 buses=(p.1.2.3.0,s.1.2.3.0) conns=(wye,wye) kvs=(11,0.4) kvas=(100,100) %rs=(0.5,0.5) xhl=2")).clone()
}
#[test]
fn invalid_transformer_bases_cannot_silently_remove_impedance() {
    for invalid in [0.0, -1.0, f64::INFINITY, f64::NAN] {
        for side in 0..2 {
            let mut n = transformer();
            n.transformers_mut()[0].windings[side].s_rating = invalid;
            let error = powerio_dist::emit(
                &powerio_core::PioModule::new(n),
                DistTargetFormat::RavensJson,
                powerio_core::Destination::memory("review").unwrap(),
            )
            .expect_err("undefined impedance base must be refused");
            assert!(
                error.to_string().contains("transformer t: winding"),
                "{error}"
            );
            assert!(error.to_string().contains("preserve impedance"), "{error}");
        }
    }
    let n = transformer();
    let out = helpers::emit_value(&n, DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    for w in &back.transformers()[0].windings {
        assert!((w.r_pct - 0.5).abs() < 1e-9);
    }
    assert!((back.transformers()[0].xsc_pct[0] - 2.0).abs() < 1e-9);
    // No physical impedance needs a fabricated base for an already ideal unit.
    let mut n = transformer();
    for w in &mut n.transformers_mut()[0].windings {
        w.s_rating = 0.0;
        w.r_pct = 0.0;
    }
    n.transformers_mut()[0].xsc_pct = vec![0.0];
    let out = helpers::emit_value(&n, DistTargetFormat::RavensJson);
    let back = helpers::parse_str(&out.text, "ravens-json").unwrap();
    assert_eq!(back.transformers()[0].xsc_pct, vec![0.0]);
}
fn delta_network(pair: [i64; 2], zip: bool, partial_bus: bool) -> MulticonductorNetwork {
    let mut input = json!({"data_model":"ENGINEERING","settings":{"vbases_default":{"b":0.23}},"bus":{"b":{"terminals":[1,2,3,4],"grounded":[4]}},"load":{"l":{"bus":"b","configuration":"DELTA","connections":pair,"pd_nom":[10],"qd_nom":[2],"vm_nom":0.4}}});
    if partial_bus {
        input["bus"]["b"]["terminals"] = json!([pair[0], pair[1], 4]);
        input["bus"]["other"] = json!({"terminals":[1,2,3,4],"grounded":[4]});
        input["settings"]["vbases_default"]["other"] = json!(0.23);
    }
    if zip {
        input["load"]["l"]["model"] = json!("ZIP");
        input["load"]["l"]["zipv"] = json!([0.2, 0.3, 0.5, 0.1, 0.2, 0.7, 0.0]);
    }
    let mut net = (*helpers::parse_str(&input.to_string(), "pmd-json").unwrap()).clone();
    // The synthetic partial bus explicitly declares terminal 4 as neutral.
    net.extras_mut().insert(
        "bmopf_terminal_conventions".into(),
        json!({"neutral":["4"]}),
    );
    net
}
#[test]
fn kron_delta_projection_keeps_each_physical_pair_and_zip_part() {
    for pair in [[1, 2], [2, 3], [3, 1], [2, 1], [3, 2], [1, 3]] {
        for zip in [false, true] {
            for partial in [false, true] {
                let network = delta_network(pair, zip, partial);
                let out = helpers::emit_value(&network, DistTargetFormat::PmdMathJson);
                let document: Value = serde_json::from_str(&out.text).unwrap();
                assert_eq!(
                    document["is_kron_reduced"], true,
                    "pair={pair:?} zip={zip} partial={partial} warnings={:?}",
                    out.warnings
                );
                assert_eq!(document["is_projected"], true);
                let third = (1..=3).find(|total_p| !pair.contains(total_p)).unwrap();
                let loads = document["load"].as_object().unwrap();
                assert_eq!(loads.len(), if zip { 3 } else { 1 });
                let mut total_p = 0.0;
                let mut total_q = 0.0;
                for load in loads.values() {
                    assert_eq!(load["connections"], json!([pair[0], pair[1], third]));
                    for key in ["pd", "qd"] {
                        let values = load[key].as_array().unwrap();
                        assert_eq!(values.len(), 3);
                        assert_eq!(values[1], 0.0);
                        assert_eq!(values[2], 0.0);
                    }
                    total_p += load["pd"][0].as_f64().unwrap();
                    total_q += load["qd"][0].as_f64().unwrap();
                    let bus = &document["bus"][load["load_bus"].as_i64().unwrap().to_string()];
                    let count = bus["terminals"].as_array().unwrap().len();
                    for key in ["grounded", "vmin", "vmax"] {
                        assert_eq!(bus[key].as_array().unwrap().len(), count);
                    }
                    let terminals = bus["terminals"].as_array().unwrap();
                    assert!(terminals.contains(&json!(third)));
                    if partial {
                        let at = terminals.iter().position(|t| t == &json!(third)).unwrap();
                        assert_eq!(bus["vmax"][at], 0.0);
                    }
                }
                assert!((total_p - 10.0).abs() < 1e-9);
                assert!((total_q - 2.0).abs() < 1e-9);
                let back = helpers::parse_str(&out.text, "pmd-math-json").unwrap();
                assert_eq!(
                    out.text,
                    helpers::emit_value(&back, DistTargetFormat::PmdMathJson).text
                );
            }
        }
    }
}
#[test]
fn unsupported_delta_phase_basis_keeps_explicit_conductors() {
    let mut n = delta_network([1, 2], false, false);
    n.buses_mut()[0].terminals = vec!["1".into(), "2".into(), "4".into()];
    let out = helpers::emit_value(&n, DistTargetFormat::PmdMathJson);
    let d: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(d["is_kron_reduced"], false);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("Kron reduction skipped") && w.contains("three-conductor")),
        "{:?}",
        out.warnings
    );
    let mut repeated = delta_network([1, 2], false, false);
    repeated.loads_mut()[0].terminal_map = vec!["1".into(), "1".into()];
    let output = helpers::emit_value(&repeated, DistTargetFormat::PmdMathJson);
    let document: Value = serde_json::from_str(&output.text).unwrap();
    assert_eq!(document["is_kron_reduced"], false);
    let mut options = powerio_dist::EmitOptions::default();
    options.pmd_math.kron_reduce = false;
    let result = powerio_dist::emit_with_options(
        &powerio_core::PioModule::new(delta_network([1, 2], false, false)),
        DistTargetFormat::PmdMathJson,
        &options,
        powerio_core::Destination::memory("out").unwrap(),
    )
    .unwrap();
    let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!()
    };
    let d: Value = serde_json::from_slice(&artifacts.pop().unwrap().into_bytes()).unwrap();
    assert_eq!(d["is_kron_reduced"], false);
    assert_eq!(d["load"]["1"]["connections"], json!([1, 2]));
}

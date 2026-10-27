use powerio_core::Source;
mod helpers;

fn source(name: &str, text: &str) -> Source {
    Source::from_memory(name, text.as_bytes()).unwrap()
}

#[test]
fn code_binding_keeps_historical_state_and_missing_references_fail() {
    let script = "new circuit.c\nnew xfmrcode.c phases=1 kvs=[12 .48] xhl=5\nnew transformer.first xfmrcode=c buses=[sourcebus end]\nedit xfmrcode.c kvs=[12 .24]\nnew transformer.second xfmrcode=c buses=[sourcebus other]\n";
    let module = powerio_dist::parse(source("case.dss", script)).unwrap();
    assert_eq!(
        module.value().transformers()[0].windings[1].v_ref.to_bits(),
        480.0_f64.to_bits()
    );
    assert_eq!(
        module.value().transformers()[1].windings[1].v_ref.to_bits(),
        240.0_f64.to_bits()
    );
    assert!(
        powerio_dist::parse(source(
            "missing.dss",
            "new circuit.c\nnew transformer.t xfmrcode=later\nnew xfmrcode.later\n"
        ))
        .is_err()
    );
}

#[test]
#[ignore = "requires pinned native transformer-code observations"]
fn pinned_transformer_code_assignments() {
    let path = std::env::var("POWERIO_XFMRCODE_ORACLE").expect("native observations");
    let cases: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for (name, case) in cases.as_object().unwrap() {
        let module = powerio_dist::parse(source("case.dss", case["script"].as_str().unwrap()))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        for t in module.value().transformers() {
            let expected = &case["transformers"][&t.name];
            assert_eq!(
                t.phases,
                expected["phases"].as_u64().unwrap() as usize,
                "{name}"
            );
            let windings = expected["windings"].as_array().unwrap();
            assert_eq!(t.windings.len(), windings.len(), "{name}");
            for (i, (w, e)) in t.windings.iter().zip(windings).enumerate() {
                for (key, actual) in [
                    ("kv", w.v_ref / 1000.0),
                    ("kva", w.s_rating / 1000.0),
                    ("tap", w.tap),
                    ("r", w.r_pct),
                    ("rneut", w.r_neutral.unwrap_or(-1.0)),
                    ("xneut", w.x_neutral.unwrap_or(0.0)),
                ] {
                    let expected = e[key].as_f64().unwrap();
                    assert!(
                        (actual - expected).abs() <= 1e-10,
                        "{name}/{}/{i}/{key}: {actual} != {expected}",
                        t.name
                    );
                }
                assert_eq!(
                    w.conn == powerio_dist::DistWindingConn::Delta,
                    e["delta"].as_bool().unwrap(),
                    "{name}"
                );
            }
            let xsc = expected["xsc"].as_array().unwrap();
            assert_eq!(t.xsc_pct.len(), xsc.len(), "{name}");
            for (a, b) in t.xsc_pct.iter().zip(xsc) {
                assert!(
                    (a - b.as_f64().unwrap()).abs() <= 1e-10,
                    "{name}: {a} != {b}"
                );
            }
            for (key, e) in expected["extras"].as_object().unwrap() {
                let a = t.extras[key].as_str().unwrap().parse::<f64>().unwrap();
                assert!(
                    (a - e.as_f64().unwrap()).abs() <= 1e-10,
                    "{name}/{key}: {a} != {e}"
                );
            }
        }
    }
}

#[test]
fn canonical_output_preserves_unequal_and_edited_winding_resistances() {
    let module = powerio_dist::parse(source("case.dss", "new circuit.c\nnew xfmrcode.c phases=1 windings=3 kvs=[7.2 .12 .12] kvas=[2000 2000 2000] %rs=[.6 1.2 1.2] xhl=2.04 xht=2.04 xlt=1.36\nnew transformer.t xfmrcode=c buses=[sourcebus house.1.0 house.0.2]\n")).unwrap();
    let mut network = module.value().clone();
    network.transformers_mut()[0].windings[0].r_pct = 0.7;
    let output = helpers::emit_dss(&network);
    let restored = powerio_dist::parse(source("out.dss", &output.text)).unwrap();
    let resistance: Vec<_> = restored.value().transformers()[0]
        .windings
        .iter()
        .map(|w| w.r_pct)
        .collect();
    assert_eq!(resistance, [0.7, 1.2, 1.2]);
}

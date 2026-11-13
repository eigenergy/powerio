use powerio::{Destination, EmittedOutput, PioModule, PioValue, Source};
use serde_json::{Value, json};

fn frame(columns: &[&str], index: &Value, data: &Value) -> Value {
    json!({
        "_module": "pandas.core.frame", "_class": "DataFrame", "orient": "split",
        "_object": json!({"columns": columns, "index": index, "data": data}).to_string(),
        "dtype": {}, "is_multiindex": false, "is_multicolumn": false
    })
}

fn document() -> Value {
    json!({"_module": "pandapower.auxiliary", "_class": "pandapowerNet", "_object": {
        "sn_mva": 100.0, "f_hz": 50.0,
        "bus": frame(&["vn_kv", "in_service"], &json!([42, 7, 19]), &json!([
            [110.0, true], [110.0, true], [110.0, false]
        ])),
        "gen": frame(&["bus", "vm_pu", "p_mw", "max_q_mvar", "min_q_mvar"],
            &json!([4]), &json!([[42, 1.05, 10.0, 50.0, -50.0]])),
        "res_bus": frame(&["vm_pu", "va_degree"], &json!([19, 7, 42]), &json!([
            [0.0, 0.0], [1.02, -3.5], [0.98, 2.5]
        ]))
    }})
}

fn parse(bytes: Vec<u8>) -> PioModule<PioValue> {
    powerio::parse(Source::from_memory("case.pandapower.json", bytes).unwrap()).unwrap()
}

fn bytes(result: powerio::EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { artifacts } = result.into_output() else {
        panic!("memory output")
    };
    artifacts[0].bytes().to_vec()
}

fn through_ir(module: &PioModule<PioValue>) -> PioModule<PioValue> {
    let ir =
        bytes(powerio::serialize(module, Destination::memory("case.pio.json").unwrap()).unwrap());
    powerio::deserialize(Source::from_memory("case.pio.json", ir).unwrap()).unwrap()
}

fn network(module: &PioModule<PioValue>) -> &powerio::BalancedNetwork {
    let PioValue::BalancedNetwork(net) = module.value() else {
        panic!("balanced network")
    };
    net
}

#[test]
fn native_bus_results_survive_ir_and_canonical_emission() {
    let original = document().to_string().into_bytes();
    let parsed = parse(original.clone());
    let same = powerio::emit(
        &parsed,
        "pandapower-json",
        Destination::memory("case.json").unwrap(),
    )
    .unwrap();
    assert_eq!(bytes(same), original);

    let module = through_ir(&parsed);
    let emitted = powerio::emit(
        &module,
        "pandapower-json",
        Destination::memory("case.json").unwrap(),
    )
    .unwrap();
    assert!(
        !emitted
            .diagnostics()
            .iter()
            .any(|finding| finding.message().contains("bus voltage state(s)"))
    );
    let output = bytes(emitted);
    let json: Value = serde_json::from_slice(&output).unwrap();
    let results: Value =
        serde_json::from_str(json["_object"]["res_bus"]["_object"].as_str().unwrap()).unwrap();
    assert_eq!(results["index"], json!([42, 7, 19]));
    assert_eq!(results["columns"], json!(["vm_pu", "va_degree"]));
    assert_eq!(
        results["data"],
        json!([[0.98, 2.5], [1.02, -3.5], [0.0, 0.0]])
    );
    assert!(json["_object"].get("converged").is_none());
    let restored = parse(output);
    for (left, right) in network(&module)
        .buses()
        .iter()
        .zip(network(&restored).buses())
    {
        assert_eq!(
            (left.id, left.vm, left.va, left.kind),
            (right.id, right.vm, right.va, right.kind)
        );
    }
    assert_eq!(network(&restored).generators()[0].vg, 1.05);
}

#[test]
fn unmapped_bus_result_fields_remain_diagnosed_after_ir() {
    let mut input = document();
    input["_object"]["res_bus"] = frame(
        &["vm_pu", "va_degree", "p_mw", "q_mvar"],
        &json!([42]),
        &json!([[0.98, 2.5, -10.0, 3.0]]),
    );
    let module = through_ir(&parse(input.to_string().into_bytes()));
    let emitted = powerio::emit(
        &module,
        "pandapower-json",
        Destination::memory("case.json").unwrap(),
    )
    .unwrap();
    for column in ["p_mw", "q_mvar"] {
        assert!(module.diagnostics().iter().any(|finding| {
            finding.code() == "READ.PANDAPOWER.FIELD_DROPPED"
                && finding.message().contains(&format!("`res_bus.{column}`"))
        }));
    }
    let output: Value = serde_json::from_slice(&bytes(emitted)).unwrap();
    let results: Value =
        serde_json::from_str(output["_object"]["res_bus"]["_object"].as_str().unwrap()).unwrap();
    assert_eq!(results["columns"], json!(["vm_pu", "va_degree"]));
}

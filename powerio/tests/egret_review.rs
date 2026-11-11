use powerio::{Destination, EmittedOutput, PioModule, PioValue, Source};
use serde_json::{Value, json};

fn document() -> Value {
    json!({"system": {"baseMVA": 100}, "elements": {
        "bus": {"7": {"matpower_bustype": "ref"}, "43": {}},
        "branch": {"1": {"from_bus": "7", "to_bus": "43", "reactance": 0.1}},
        "shunt": {"shunt_1": {"bus": "43", "gs": 2.0, "bs": 4.0, "in_service": false}}
    }})
}

fn parse(document: &Value) -> PioModule<PioValue> {
    powerio::parse(
        Source::from_memory("case.egret.json", document.to_string().into_bytes()).unwrap(),
    )
    .unwrap()
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

fn emit(module: &PioModule<PioValue>, target: &str) -> powerio::EmitResult {
    powerio::emit(module, target, Destination::memory("case.json").unwrap()).unwrap()
}

fn network(module: &PioModule<PioValue>) -> &powerio::BalancedNetwork {
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("balanced network")
    };
    network
}

#[test]
fn scalar_shunt_status_survives_ir_and_canonical_egret() {
    let module = through_ir(&parse(&document()));
    let output: Value = serde_json::from_slice(&bytes(emit(&module, "egret-json"))).unwrap();
    assert_eq!(output["elements"]["shunt"]["shunt_1"]["in_service"], false);
    let restored = parse(&output);
    let shunt = &network(&restored).shunts()[0];
    assert!(!shunt.in_service);
    assert_eq!((shunt.g, shunt.b), (2.0, 4.0));
}

#[test]
fn varying_shunt_status_survives_ir_and_canonical_egret() {
    let mut input = document();
    input["system"]["time_keys"] = json!(["on", "off", "on-again"]);
    input["elements"]["shunt"]["shunt_1"]["in_service"] =
        json!({"data_type": "time_series", "values": [true, false, true]});
    let module = through_ir(&parse(&input));
    let output: Value = serde_json::from_slice(&bytes(emit(&module, "egret-json"))).unwrap();
    assert_eq!(
        output["elements"]["shunt"]["shunt_1"]["in_service"]["values"],
        json!([true, false, true])
    );
    let restored = parse(&output);
    let PioValue::TimeSeries(series) = restored.value() else {
        panic!("time series")
    };
    for (index, expected) in [true, false, true].into_iter().enumerate() {
        let Some(PioValue::BalancedNetwork(point)) = series.get(index) else {
            panic!("balanced point")
        };
        assert_eq!(point.shunts()[0].in_service, expected);
    }
}

#[test]
fn foreign_extras_cannot_populate_optional_powermodels_native_fields() {
    let mut input = document();
    let branch = input["elements"]["branch"]["1"].as_object_mut().unwrap();
    branch.insert("pf".into(), json!(50.0)); // Egret's MW, not PowerModels' per unit.
    branch.insert("c_rating_a".into(), json!(5.0));
    branch.insert("rate_a".into(), json!(20.0));
    branch.insert("other_tool_metadata".into(), json!({"label": "kept"}));
    let mut parsed = parse(&input).sever_source();
    let PioValue::BalancedNetwork(net) = parsed.value_mut() else {
        panic!("balanced network")
    };
    net.buses_mut()[0].name = None;
    net.buses_mut()[0]
        .extras
        .insert("name".into(), json!({"foreign": "name shape"}));
    let module = through_ir(&parsed);
    assert!(network(&module).branches()[0].solution.is_none());
    let emitted = emit(&module, "powermodels-json");
    let dropped: Vec<_> = emitted
        .diagnostics()
        .iter()
        .filter(|finding| finding.code() == "EMIT.POWERMODELS.FIELD_DROPPED")
        .map(|finding| finding.message().to_owned())
        .collect();
    for key in ["pf", "c_rating_a", "rate_a", "name"] {
        assert!(
            dropped
                .iter()
                .any(|message| message.contains(&format!("`{key}`"))),
            "{dropped:?}"
        );
    }
    let output: Value = serde_json::from_slice(&bytes(emitted)).unwrap();
    let branch = &output["branch"]["1"];
    for key in ["pf", "qf", "pt", "qt", "c_rating_a", "rate_a"] {
        assert!(branch.get(key).is_none(), "{key}: {branch}");
    }
    assert_eq!(branch["other_tool_metadata"], json!({"label": "kept"}));
    let restored =
        powerio::parse(Source::from_memory("case.json", output.to_string().into_bytes()).unwrap())
            .unwrap();
    assert!(network(&restored).branches()[0].solution.is_none());
    assert!(network(&restored).branches()[0].current_ratings.is_none());
    assert_eq!(network(&restored).branches()[0].rate_a, 0.0);
}

#[test]
fn complete_typed_branch_results_are_scaled_once_for_powermodels() {
    let mut input = document();
    for (key, value) in [("pf", 50.0), ("qf", -2.0), ("pt", -49.0), ("qt", 3.0)] {
        input["elements"]["branch"]["1"][key] = json!(value);
    }
    let module = through_ir(&parse(&input));
    let output: Value = serde_json::from_slice(&bytes(emit(&module, "powermodels-json"))).unwrap();
    assert_eq!(output["branch"]["1"]["pf"], 0.5);
    let restored =
        powerio::parse(Source::from_memory("case.json", output.to_string().into_bytes()).unwrap())
            .unwrap();
    assert_eq!(
        network(&restored).branches()[0].solution,
        network(&module).branches()[0].solution
    );
}

#[test]
fn foreign_branch_result_shapes_remain_readable_extras() {
    for value in [
        json!("tool-specific"),
        json!({"expression": "flow"}),
        json!([1, 2]),
    ] {
        let mut input = document();
        input["elements"]["branch"]["1"]["pf"] = value.clone();
        let module = through_ir(&parse(&input));
        assert!(network(&module).branches()[0].solution.is_none());
        assert_eq!(network(&module).branches()[0].extras["pf"], value);
        let output: Value = serde_json::from_slice(&bytes(emit(&module, "egret-json"))).unwrap();
        assert_eq!(output["elements"]["branch"]["1"]["pf"], value);
    }
}

#[test]
fn foreign_dc_cost_shapes_remain_readable_extras() {
    let mut input = document();
    input["elements"]["dc_branch"] = json!({"1": {
        "from_bus": "7", "to_bus": "43", "pf": 5,
        "p_cost": {"foreign_curve": [1, 2]}, "startup_cost": "tool-specific"
    }});
    let module = through_ir(&parse(&input));
    assert!(network(&module).hvdc()[0].cost.is_none());
    let output: Value = serde_json::from_slice(&bytes(emit(&module, "egret-json"))).unwrap();
    for key in ["p_cost", "startup_cost"] {
        assert_eq!(
            output["elements"]["dc_branch"]["1"][key],
            input["elements"]["dc_branch"]["1"][key]
        );
    }
}

#[test]
fn omitted_switches_are_diagnosed_with_or_without_source_identity() {
    for uid in [None, Some("source-switch".to_owned())] {
        let mut module = parse(&document()).sever_source();
        let PioValue::BalancedNetwork(net) = module.value_mut() else {
            panic!("balanced network")
        };
        let mut switch = powerio::Switch::new(powerio::BusId(7), powerio::BusId(43), true);
        switch.uid = uid.clone();
        net.switches_mut().push(switch);
        assert_eq!(net.switches()[0].uid, uid);
        let emitted = emit(&module, "egret-json");
        assert!(emitted.diagnostics().iter().any(|finding| {
            finding.code() == "EMIT.EGRET.RECORD_DROPPED"
                && finding.message().contains("1 switch(es) dropped")
        }));
        let output: Value = serde_json::from_slice(&bytes(emitted)).unwrap();
        assert!(output["elements"].get("switch").is_none());
        assert_eq!(network(&parse(&output)).switches().as_slice(), []);
    }
}

#[test]
fn omitted_static_var_compensators_are_diagnosed_without_source_identity() {
    let mut module = parse(&document()).sever_source();
    let PioValue::BalancedNetwork(net) = module.value_mut() else {
        panic!("balanced network")
    };
    net.static_var_compensators_mut()
        .push(powerio::network::StaticVarCompensator::new(
            powerio::BusId(43),
            -0.02,
            0.03,
        ));
    assert!(net.static_var_compensators()[0].uid.is_none());
    let emitted = emit(&module, "egret-json");
    assert!(emitted.diagnostics().iter().any(|finding| {
        finding.code() == "EMIT.EGRET.RECORD_DROPPED"
            && finding
                .message()
                .contains("1 static VAR compensator(s) dropped")
    }));
    let output: Value = serde_json::from_slice(&bytes(emitted)).unwrap();
    assert_eq!(
        network(&parse(&output))
            .static_var_compensators()
            .as_slice(),
        []
    );
}

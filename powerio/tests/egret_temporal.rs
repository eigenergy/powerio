use powerio::{Destination, EmittedOutput, PioModule, PioValue, Source};
use serde_json::{Value, json};

fn document() -> Value {
    json!({
        "system": {"baseMVA": 100.0, "time_keys": ["morning", "evening"], "time_period_length_minutes": 15},
        "elements": {
            "bus": {"7": {"matpower_bustype": "ref", "base_kv": 110.0}, "43": {"base_kv": 110.0}},
            "load": {"load_1": {"bus": "43", "p_load": {"data_type": "time_series", "values": [10.0, 20.0]}, "q_load": 2.0}},
            "generator": {"1": {"bus": "7", "pg": 11.0, "p_max": 100.0}},
            "branch": {"1": {"from_bus": "7", "to_bus": "43", "reactance": 0.1,
                "pf": {"data_type": "time_series", "values": [10.0, -3.0]},
                "qf": {"data_type": "time_series", "values": [-2.0, 1.0]},
                "pt": {"data_type": "time_series", "values": [-9.8, 3.1]},
                "qt": {"data_type": "time_series", "values": [2.2, -0.8]},
                "rating_long_term": {"data_type": "time_series", "values": [0.0, 80.0]}
            }}
        }
    })
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

#[test]
fn electrical_time_series_survives_ir_and_canonical_egret_emission() {
    let input = document();
    let module = parse(&input);
    assert_eq!(
        bytes(powerio::emit(&module, "egret-json", Destination::memory("case").unwrap()).unwrap()),
        input.to_string().as_bytes()
    );
    let ir =
        bytes(powerio::serialize(&module, Destination::memory("case.pio.json").unwrap()).unwrap());
    let restored = powerio::deserialize(Source::from_memory("case.pio.json", ir).unwrap()).unwrap();
    let output = bytes(
        powerio::emit(
            &restored,
            "egret-json",
            Destination::memory("case.json").unwrap(),
        )
        .unwrap(),
    );
    let native: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(native["system"]["time_keys"], json!(["morning", "evening"]));
    assert_eq!(native["system"]["time_period_length_minutes"], 15.0);
    assert_eq!(
        native["elements"]["branch"]["1"]["pf"]["values"],
        json!([10.0, -3.0])
    );
    let reread = powerio::parse(Source::from_memory("case.json", output).unwrap()).unwrap();
    let PioValue::TimeSeries(series) = reread.value() else {
        panic!("time series")
    };
    assert_eq!(series.time_points()[1].duration().unwrap().as_secs(), 900);
    let Some(PioValue::BalancedNetwork(network)) = series.get(1) else {
        panic!("network")
    };
    assert_eq!(network.loads()[0].p, 20.0);
    assert_eq!(
        network.branches()[0].solution,
        Some(powerio::BranchSolution::new(-3.0, 1.0, 3.1, -0.8))
    );
    assert_eq!(network.branches()[0].rate_a, 80.0);
}

#[test]
fn unsupported_variation_is_checked_even_at_a_single_point() {
    let input = json!({"system": {"baseMVA": 100, "time_keys": ["only"]}, "elements": {
        "bus": {"7": {"matpower_bustype": "ref"}},
        "load": {"load_1": {"bus": "7", "p_load": 1, "area": {"data_type": "time_series", "values": [2]}}}
    }});
    let error =
        powerio::parse(Source::from_memory("case.json", input.to_string().into_bytes()).unwrap())
            .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("outside the supported scalar network profile")
    );
}

#[test]
fn invalid_time_axes_and_intervals_are_refused() {
    for interval in [json!(0), json!(-1), json!("hour"), json!(1e300)] {
        let mut input = document();
        input["system"]["time_period_length_minutes"] = interval;
        assert!(
            powerio::parse(
                Source::from_memory("case.json", input.to_string().into_bytes()).unwrap()
            )
            .is_err()
        );
    }
    let mut input = document();
    input["system"]["time_keys"] = json!(["same", "same"]);
    assert!(
        powerio::parse(Source::from_memory("case.json", input.to_string().into_bytes()).unwrap())
            .is_err()
    );
}

#[test]
fn canonical_emission_refuses_topology_changes() {
    let mut module = parse(&document()).sever_source();
    let PioValue::TimeSeries(series) = module.value_mut() else {
        panic!("time series")
    };
    let Some(PioValue::BalancedNetwork(network)) = series.get_mut(1) else {
        panic!("network")
    };
    network.branches_mut()[0].to = powerio::BusId(7);
    let error =
        powerio::emit(&module, "egret-json", Destination::memory("case").unwrap()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("outside the supported electrical time-series profile")
    );
}

#[test]
fn partial_native_branch_results_do_not_fabricate_an_ac_solution() {
    let input = json!({"system": {"baseMVA": 100}, "elements": {
        "bus": {"7": {"matpower_bustype": "ref"}, "43": {}},
        "branch": {"1": {"from_bus": "7", "to_bus": "43", "pf": 12.0, "qf": null, "pt": null, "qt": null}}
    }});
    let module = parse(&input);
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("network")
    };
    assert!(network.branches()[0].solution.is_none());
    assert_eq!(network.branches()[0].extras["pf"], 12.0);
    assert!(!network.branches()[0].extras.contains_key("qf"));
}

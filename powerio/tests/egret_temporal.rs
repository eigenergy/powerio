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
                "rating_long_term": {"data_type": "time_series", "values": [null, 80.0]}
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
    assert_eq!(
        native["elements"]["branch"]["1"]["rating_long_term"]["values"],
        json!([null, 80.0])
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
fn every_rating_preserves_unlimited_points_through_ir_and_export() {
    for field in ["rating_long_term", "rating_short_term", "rating_emergency"] {
        for values in [
            json!([null, 150.0]),
            json!([150.0, null]),
            json!([null, null]),
        ] {
            let mut input = document();
            input["elements"]["branch"]["1"]
                .as_object_mut()
                .unwrap()
                .remove("rating_long_term");
            input["elements"]["branch"]["1"][field] =
                json!({"data_type": "time_series", "values": values});
            let module = parse(&input);
            let ir = bytes(
                powerio::serialize(&module, Destination::memory("case.pio.json").unwrap()).unwrap(),
            );
            let restored =
                powerio::deserialize(Source::from_memory("case.pio.json", ir).unwrap()).unwrap();
            let output = bytes(
                powerio::emit(
                    &restored,
                    "egret-json",
                    Destination::memory("case.json").unwrap(),
                )
                .unwrap(),
            );
            let native: Value = serde_json::from_slice(&output).unwrap();
            if values == json!([null, null]) {
                assert!(native["elements"]["branch"]["1"].get(field).is_none());
            } else {
                assert_eq!(native["elements"]["branch"]["1"][field]["values"], values);
            }
            let reread = parse(&native);
            let PioValue::TimeSeries(series) = reread.value() else {
                panic!("series")
            };
            for (point, expected) in values.as_array().unwrap().iter().enumerate() {
                let PioValue::BalancedNetwork(network) = series.get(point).unwrap() else {
                    panic!("network")
                };
                let branch = &network.branches()[0];
                let rating = match field {
                    "rating_long_term" => branch.rate_a,
                    "rating_short_term" => branch.rate_b,
                    _ => branch.rate_c,
                };
                assert_eq!(rating, expected.as_f64().unwrap_or(0.0));
            }
        }
    }
}

#[test]
fn native_zero_thermal_limits_still_read_as_unlimited_in_scalar_and_every_series_position() {
    // 0.11.x keeps reading a native Egret numeric zero as PowerIO's unlimited
    // rating, as 0.11.10 did; refusing it is planned for 0.12 (#828).
    for field in ["rating_long_term", "rating_short_term", "rating_emergency"] {
        for zero in [0.0, -0.0] {
            for values in [json!(zero), json!([zero, 150.0]), json!([150.0, zero])] {
                let mut input = document();
                let branch = &mut input["elements"]["branch"]["1"];
                branch.as_object_mut().unwrap().remove("rating_long_term");
                if values.is_array() {
                    branch[field] = json!({"data_type": "time_series", "values": values});
                } else {
                    branch[field] = values.clone();
                    for flow in ["pf", "qf", "pt", "qt"] {
                        branch.as_object_mut().unwrap().remove(flow);
                    }
                    input["system"].as_object_mut().unwrap().remove("time_keys");
                    input["elements"]["load"]["load_1"]["p_load"] = json!(10.0);
                }
                let module = parse(&input);
                let networks: Vec<_> = match module.value() {
                    PioValue::TimeSeries(series) => (0..series.len())
                        .map(|point| match series.get(point).unwrap() {
                            PioValue::BalancedNetwork(network) => network.clone(),
                            _ => panic!("network"),
                        })
                        .collect(),
                    PioValue::BalancedNetwork(network) => vec![network.clone()],
                    _ => panic!("network or series"),
                };
                let expected: Vec<f64> = match values.as_array() {
                    Some(points) => points.iter().map(|v| v.as_f64().unwrap()).collect(),
                    None => vec![zero],
                };
                assert_eq!(networks.len(), expected.len());
                for (network, expected) in networks.iter().zip(expected) {
                    let branch = &network.branches()[0];
                    let rating = match field {
                        "rating_long_term" => branch.rate_a,
                        "rating_short_term" => branch.rate_b,
                        _ => branch.rate_c,
                    };
                    assert_eq!(rating, expected, "{field} {values}");
                }
            }
        }
    }
}

#[test]
fn scalar_absent_and_null_ratings_export_as_unlimited() {
    for field in ["rating_long_term", "rating_short_term", "rating_emergency"] {
        for stated_null in [false, true] {
            let mut input = json!({"system": {"baseMVA": 100}, "elements": {
                "bus": {"1": {"matpower_bustype": "ref"}, "2": {}},
                "branch": {"1": {"from_bus": "1", "to_bus": "2", "reactance": 0.1}}
            }});
            if stated_null {
                input["elements"]["branch"]["1"][field] = Value::Null;
            }
            let module = parse(&input).sever_source();
            let output = bytes(
                powerio::emit(&module, "egret-json", Destination::memory("case").unwrap()).unwrap(),
            );
            let native: Value = serde_json::from_slice(&output).unwrap();
            assert!(native["elements"]["branch"]["1"].get(field).is_none());
        }
    }
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

fn first_network() -> powerio::BalancedNetwork {
    let module = parse(&document());
    let PioValue::TimeSeries(series) = module.value() else {
        panic!("series")
    };
    let PioValue::BalancedNetwork(network) = series.get(0).unwrap() else {
        panic!("network")
    };
    network.clone()
}

#[test]
fn operating_point_overrides_cannot_reuse_base_branch_results() {
    use powerio::TimePoint;
    use powerio_prob::BalancedOperatingPointBuilder;
    let base = first_network();
    let untouched = BalancedOperatingPointBuilder::for_point(base.clone())
        .build_point()
        .unwrap();
    assert_eq!(
        powerio::network_with_operating_point(&untouched).branches()[0].solution,
        base.branches()[0].solution
    );
    let points = BalancedOperatingPointBuilder::new(
        base.clone(),
        vec![
            TimePoint::new("unchanged", None).unwrap(),
            TimePoint::new("changed", None).unwrap(),
        ],
    )
    .bus_voltage_magnitudes(vec![1.0, 1.0, 1.05, 0.95])
    .build()
    .unwrap();
    for point in points.values() {
        assert!(
            powerio::network_with_operating_point(point).branches()[0]
                .solution
                .is_none()
        );
    }
    let result = powerio::emit(
        &PioModule::new(points),
        "egret-json",
        Destination::memory("series.json").unwrap(),
    )
    .unwrap();
    assert_eq!(
        result
            .diagnostics()
            .iter()
            .filter(|d| d
                .message()
                .contains("base-network branch results were cleared"))
            .count(),
        1
    );
    let emitted: Value = serde_json::from_slice(&bytes(result)).unwrap();
    assert!(emitted["elements"]["branch"]["1"].get("pf").is_none());
    assert_eq!(
        emitted["elements"]["bus"]["7"]["vm"]["values"],
        json!([1.0, 1.05])
    );
    assert!(
        base.branches()[0].solution.is_some(),
        "shared base stays intact"
    );
}

#[test]
fn series_keeps_scalar_loss_checks_and_deduplicates_identical_findings() {
    use powerio::{TimePoint, TimeSeries};
    let mut base = first_network();
    *base.source_format_mut() = powerio::SourceFormat::Sincal;
    let mut solver = powerio::SolverParams::new();
    solver.max_iterations = Some(11);
    *base.solver_mut() = Some(solver);
    let mut switch = powerio::Switch::new(powerio::BusId(7), powerio::BusId(43), true);
    switch.uid = Some("source-switch".to_owned());
    base.switches_mut().push(switch);
    let scalar = powerio::emit(
        &PioModule::new(base.clone()),
        "egret-json",
        Destination::memory("scalar").unwrap(),
    )
    .unwrap();
    let mut later = base.clone();
    later.loads_mut()[0].p = 24.0;
    let series = TimeSeries::new(
        vec![
            TimePoint::new("a", None).unwrap(),
            TimePoint::new("b", None).unwrap(),
            TimePoint::new("c", None).unwrap(),
        ],
        vec![base.clone(), later, base],
    )
    .unwrap();
    let output = powerio::emit(
        &PioModule::new(series),
        "egret-json",
        Destination::memory("series").unwrap(),
    )
    .unwrap();
    for finding in scalar.diagnostics() {
        assert_eq!(
            output
                .diagnostics()
                .iter()
                .filter(
                    |other| other.code() == finding.code() && other.message() == finding.message()
                )
                .count(),
            1,
            "{}",
            finding.message()
        );
    }
    assert!(
        output
            .diagnostics()
            .iter()
            .any(|d| d.message().contains("source-neutral grid exchange context"))
    );
    assert!(
        output
            .diagnostics()
            .iter()
            .any(|d| d.message().contains("absent electrical rows"))
    );
    assert!(
        output
            .diagnostics()
            .iter()
            .any(|d| d.code() == "EMIT.SINCAL.RETAINED_SOURCE_OMITTED")
    );
}

#[test]
fn hourly_series_preserves_late_variation_and_static_payload_once() {
    use powerio::{TimePoint, TimeSeries};
    let mut base = first_network();
    base.loads_mut()[0]
        .extras
        .insert("study_note".to_owned(), json!("x".repeat(8192)));
    let count = 8760;
    let mut values = vec![base; count];
    values[count - 1].loads_mut()[0].p = 99.0;
    let times = (0..count)
        .map(|n| TimePoint::new(n.to_string(), None).unwrap())
        .collect();
    let series = TimeSeries::new(times, values).unwrap();
    let output = bytes(
        powerio::emit(
            &PioModule::new(series),
            "egret-json",
            Destination::memory("series").unwrap(),
        )
        .unwrap(),
    );
    let native: Value = serde_json::from_slice(&output).unwrap();
    let loads = native["elements"]["load"].as_object().unwrap();
    let load = loads.values().next().unwrap();
    let samples = load["p_load"]["values"].as_array().unwrap();
    assert_eq!(samples.len(), count);
    assert_eq!(samples[0], 10.0);
    assert_eq!(samples[count - 1], 99.0);
    assert!(
        output.len() < 1_000_000,
        "static extras must not be repeated for each hour"
    );
    let reparsed = powerio::parse(Source::from_memory("series.json", output).unwrap()).unwrap();
    let PioValue::TimeSeries(series) = reparsed.value() else {
        panic!("series")
    };
    let PioValue::BalancedNetwork(last) = series.get(count - 1).unwrap() else {
        panic!("network")
    };
    assert_eq!(last.loads()[0].p, 99.0);
}

#[test]
fn operating_point_also_clears_partial_branch_result_extras() {
    let mut base = first_network();
    base.branches_mut()[0].solution = None;
    base.branches_mut()[0]
        .extras
        .insert("pf".to_owned(), json!(50.0));
    base.branches_mut()[0]
        .extras
        .insert("comment".to_owned(), json!("preserve metadata"));
    let point = powerio_prob::BalancedOperatingPointBuilder::for_point(base.clone())
        .bus_voltage_magnitudes(vec![1.05, 0.95])
        .build_point()
        .unwrap();
    let derived = powerio::network_with_operating_point(&point);
    assert!(!derived.branches()[0].extras.contains_key("pf"));
    assert_eq!(derived.branches()[0].extras["comment"], "preserve metadata");
    assert_eq!(base.branches()[0].extras["pf"], 50.0);
    let output = powerio::emit(
        &PioModule::new(point),
        "egret-json",
        Destination::memory("point").unwrap(),
    )
    .unwrap();
    assert!(output.diagnostics().iter().any(|d| {
        d.message()
            .contains("base-network branch results were cleared")
    }));
    let native: Value = serde_json::from_slice(&bytes(output)).unwrap();
    assert!(native["elements"]["branch"]["1"].get("pf").is_none());
}

#[test]
fn a_nonfinite_temporal_value_still_reports_its_native_field() {
    use powerio::{TimePoint, TimeSeries};
    let mut first = first_network();
    first.loads_mut()[0].p = f64::NAN;
    let mut next = first.clone();
    next.loads_mut()[0].p = 20.0;
    let series = TimeSeries::new(
        vec![
            TimePoint::new("a", None).unwrap(),
            TimePoint::new("b", None).unwrap(),
        ],
        vec![first, next],
    )
    .unwrap();
    let output = powerio::emit(
        &PioModule::new(series),
        "egret-json",
        Destination::memory("series").unwrap(),
    )
    .unwrap();
    assert_eq!(
        output
            .diagnostics()
            .iter()
            .filter(|d| d.code() == "EMIT.EGRET.NOT_A_NUMBER" && d.message().contains("p_load"))
            .count(),
        1
    );
    let native: Value = serde_json::from_slice(&bytes(output)).unwrap();
    let load = native["elements"]["load"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(load["p_load"]["values"], json!([null, 20.0]));
}

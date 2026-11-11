//! Numeric ratings and the older document version's zero convention.
mod helpers;
use helpers::{deserialize_module_text, serialize_module_text};
use powerio::{PioModule, PioValue};
use powerio_tx::{BranchRatingSet, BusId, Impedance, Transformer3W, Winding};

#[test]
fn thermal_limit_records_survive_the_module_envelope() {
    let module = powerio::parse(
        powerio::Source::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/data/case9.m"
        ))
        .unwrap(),
    )
    .unwrap();
    let PioValue::BalancedNetwork(base) = module.into_value() else {
        panic!("network")
    };
    for kind in [
        "legacy",
        "branch-zero",
        "named-zero",
        "winding-zero",
        "winding-named-zero",
        "partial-zero",
    ] {
        let mut net = base.clone();
        net.branches_mut()[0].rate_b = f64::INFINITY;
        match kind {
            "branch-zero" => net.branches_mut()[0].rate_a = 0.0,
            "named-zero" => net.branches_mut()[0]
                .rating_sets
                .push(BranchRatingSet::new("winter", 0.0)),
            "winding-zero" | "winding-named-zero" | "partial-zero" => {
                let mut t = Transformer3W::new(
                    [
                        Winding::new(BusId(1)),
                        Winding::new(BusId(2)),
                        Winding::new(BusId(3)),
                    ],
                    [Impedance::new(0.01, 0.1, 100.0); 3],
                );
                if kind == "winding-named-zero" {
                    t.set_winding_rating_sets(1, vec![BranchRatingSet::new("winter", 0.0)]);
                } else {
                    t.windings[0].rate_a = 0.0;
                }
                if kind == "partial-zero" {
                    t.set_winding_in_service([true, false, true]);
                }
                net.transformers_3w_mut().push(t);
            }
            "legacy" => (),
            _ => unreachable!(),
        }
        net.assign_missing_component_ids();
        let module = PioModule::new(PioValue::BalancedNetwork(net));
        let text = serialize_module_text(&module).unwrap();
        let restored = deserialize_module_text(&text).unwrap();
        assert_eq!(serialize_module_text(&restored).unwrap(), text);
        let (PioValue::BalancedNetwork(before), PioValue::BalancedNetwork(after)) =
            (module.value(), restored.value())
        else {
            panic!("network")
        };
        assert_eq!(before.branches(), after.branches());
        assert_eq!(before.transformers_3w(), after.transformers_3w());
        assert_eq!(before.buses(), after.buses());
        assert!(!text.contains("WithThermalLimits"));
        assert!(text.contains("\"Infinity\""));
        if let Some(dir) = std::env::var_os("POWERIO_THERMAL_IR_OUTPUT") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                std::path::Path::new(&dir).join(format!("{kind}.pio.json")),
                text,
            )
            .unwrap();
        }
    }
}

#[test]
fn old_document_zero_ratings_read_as_infinity_without_rewriting_other_zeros() {
    let mut net = helpers::parse_matpower_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/case9.m"
    ))
    .unwrap();
    net.branches_mut()[0].rate_a = 0.0;
    net.branches_mut()[0].rate_b = 150.0;
    net.branches_mut()[0].rate_c = f64::INFINITY;
    net.branches_mut()[0]
        .extras
        .insert("rate_a".into(), serde_json::json!(0));
    net.branches_mut()[0]
        .rating_sets
        .push(BranchRatingSet::new("winter", 0.0));
    let mut t = Transformer3W::new(
        [
            Winding::new(BusId(1)),
            Winding::new(BusId(2)),
            Winding::new(BusId(3)),
        ],
        [Impedance::new(0.01, 0.1, 100.0); 3],
    );
    t.windings[0].rate_a = 0.0;
    t.set_winding_rating_sets(1, vec![BranchRatingSet::new("winter", 0.0)]);
    net.transformers_3w_mut().push(t);
    let current = serialize_module_text(&PioModule::new(PioValue::BalancedNetwork(net))).unwrap();
    let mut old: serde_json::Value = serde_json::from_str(&current).unwrap();
    assert_eq!(old["value"]["data"]["branches"][0]["rate_a"], 0.0);
    assert_eq!(old["value"]["data"]["branches"][0]["rate_c"], "Infinity");
    old["version"] = serde_json::json!(2);
    let restored = deserialize_module_text(&old.to_string()).unwrap();
    let PioValue::BalancedNetwork(net) = restored.value() else {
        panic!("network")
    };
    assert_eq!(net.branches()[0].rate_a, f64::INFINITY);
    assert_eq!(net.branches()[0].rate_b, 150.0);
    assert_eq!(net.branches()[0].rate_c, f64::INFINITY);
    assert_eq!(net.branches()[0].rating_sets[0].rate_mva, f64::INFINITY);
    assert_eq!(net.branches()[0].extras["rate_a"], 0);
    assert_eq!(net.transformers_3w()[0].windings[0].rate_a, f64::INFINITY);
    assert_eq!(
        net.transformers_3w()[0].winding_rating_sets(1)[0].rate_mva,
        f64::INFINITY
    );
    let current = deserialize_module_text(&current).unwrap();
    let PioValue::BalancedNetwork(net) = current.value() else {
        panic!("network")
    };
    assert_eq!(net.branches()[0].rate_a, 0.0);
    assert_eq!(net.transformers_3w()[0].windings[0].rate_a, 0.0);
}

#[test]
fn legacy_ratings_reach_every_balanced_collection_network() {
    use powerio_core::{Scenario, ScenarioId, ScenarioSet, TimePoint, TimeSeries};
    let net = helpers::parse_matpower_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/case9.m"
    ))
    .unwrap();
    let times = vec![
        TimePoint::new("a", None).unwrap(),
        TimePoint::new("b", None).unwrap(),
    ];
    let series = TimeSeries::new(times.clone(), vec![net.clone(), net.clone()]).unwrap();
    let points = powerio_prob::BalancedOperatingPointBuilder::new(net.clone(), times)
        .build()
        .unwrap();
    let point = powerio_prob::BalancedOperatingPointBuilder::for_point(net.clone())
        .build_point()
        .unwrap();
    macro_rules! scenarios {
        ($value:expr) => {
            ScenarioSet::new(vec![
                Scenario::new(ScenarioId::new("base").unwrap(), None, $value.clone()),
                Scenario::new(ScenarioId::new("other").unwrap(), None, $value.clone()),
            ])
            .unwrap()
        };
    }
    let cases: Vec<(PioValue, Vec<&str>)> = vec![
        (net.clone().into(), vec!["/value/data"]),
        (point.clone().into(), vec!["/value/data/network"]),
        (
            series.clone().into(),
            vec!["/value/data/values/0", "/value/data/values/1"],
        ),
        (points.clone().into(), vec!["/value/data/network"]),
        (
            scenarios!(net).into(),
            vec![
                "/value/data/scenarios/0/value",
                "/value/data/scenarios/1/value",
            ],
        ),
        (scenarios!(point).into(), vec!["/value/data/network"]),
        (
            scenarios!(series).into(),
            vec![
                "/value/data/scenarios/0/value/values/0",
                "/value/data/scenarios/0/value/values/1",
                "/value/data/scenarios/1/value/values/0",
                "/value/data/scenarios/1/value/values/1",
            ],
        ),
        (
            scenarios!(points).into(),
            vec![
                "/value/data/scenarios/0/value/network",
                "/value/data/scenarios/1/value/network",
            ],
        ),
    ];
    for (value, paths) in cases {
        let text = serialize_module_text(&PioModule::new(value)).unwrap();
        helpers::thermal_migration::check(&text, &paths);
    }
}

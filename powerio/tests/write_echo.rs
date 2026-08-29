//! The byte exact echo tier extends past the two network kinds: any module
//! that retains a source already in the requested format writes it back
//! unchanged, instead of refusing every kind but `BalancedNetwork` and
//! `MulticonductorNetwork` with `REQUEST.WRITE.UNSUPPORTED_VALUE_KIND`.

use powerio::{PioValue, write_module_str};

/// A minimal Egret `ModelData` document with `system.time_keys`, so
/// `powerio::parse` routes it to the time series reader instead of the
/// balanced hub. No declared format: the common case for a bare `.json`
/// source (`Source::open`/`Source::from_bytes` with no `.with_format`),
/// which is also the reclassify-by-content branch of the echo check.
const EGRET_TIME_SERIES: &str = r#"{
    "elements": {
        "bus": {"1": {"matpower_bustype": "ref", "base_kv": 138.0},
                "2": {"matpower_bustype": "PQ", "base_kv": 138.0}},
        "load": {"load_1": {"bus": "2",
            "p_load": {"data_type": "time_series", "values": [10.0, 20.0]},
            "q_load": 3.0}},
        "generator": {"1": {"bus": "1", "pg": 12.0, "qg": 0.0,
            "p_min": 0.0, "p_max": 50.0, "q_min": -10.0, "q_max": 10.0}},
        "branch": {"1": {"from_bus": "1", "to_bus": "2",
            "resistance": 0.01, "reactance": 0.1, "charging_susceptance": 0.0,
            "rating_long_term": 100.0, "rating_short_term": 100.0,
            "rating_emergency": 100.0, "transformer_phase_shift": 0.0}}
    },
    "system": {"baseMVA": 100.0, "time_keys": ["t1", "t2"]}
}"#;

#[test]
fn a_time_series_kind_echoes_its_retained_source_on_a_same_format_write() {
    let source =
        powerio_core::Source::from_bytes("case.json", EGRET_TIME_SERIES.as_bytes().to_vec())
            .unwrap();
    let module = powerio::parse(source).unwrap();
    assert!(
        matches!(module.value(), PioValue::BalancedNetworkTimeSeries(_)),
        "the time_keys document should route to the time series kind"
    );

    let (text, diagnostics) = write_module_str(&module, "egret-json").unwrap();
    assert_eq!(
        text, EGRET_TIME_SERIES,
        "the write must echo the source byte for byte"
    );
    assert!(diagnostics.is_empty());
}

#[test]
fn a_time_series_kind_still_refuses_a_format_its_retained_source_is_not() {
    let source =
        powerio_core::Source::from_bytes("case.json", EGRET_TIME_SERIES.as_bytes().to_vec())
            .unwrap();
    let module = powerio::parse(source).unwrap();

    let error = write_module_str(&module, "matpower").unwrap_err();
    assert!(error.to_string().contains("no matpower writer"), "{error}");
}

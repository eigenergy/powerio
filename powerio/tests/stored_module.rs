//! The `.pio.json` version 1 wire: round trips, refusals, and the one way
//! 0.9 upgrade.

use powerio::stored::{read_module, write_module};
use powerio::{BalancedNetwork, PioValue};
use powerio_core::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, HistoryEntry, HistoryId, HistoryKind,
    PioModule, Producer, SourceDescriptor, SourceId, SourceMapEntry, SourceRelation, SourceSpan,
    TimePoint,
};
use powerio_tx::{Bus, BusId, BusType, Generator, Load};

fn small_network() -> BalancedNetwork {
    let mut bus1 = Bus::new(BusId(1), BusType::Ref, 345.0);
    bus1.vm = 1.02;
    let mut bus2 = Bus::new(BusId(2), BusType::Pq, 345.0);
    // A genuine open bound: the stored form must carry it as "Infinity".
    bus2.vmax = f64::INFINITY;
    bus2.va = 12.0;
    let mut network = BalancedNetwork::in_memory(
        "stored-roundtrip",
        100.0,
        vec![bus1, bus2],
        vec![powerio_tx::Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    network.loads_mut().push(Load::new(BusId(2), 40.0, 10.0));
    let mut generator = Generator::new(BusId(1));
    generator.pg = 42.0;
    network.generators_mut().push(generator);
    network
}

fn module_with_records() -> PioModule<PioValue> {
    let mut module = PioModule::new(PioValue::BalancedNetwork(small_network()))
        .with_producer(Producer::new("powerio", "1.0.0-test").unwrap());
    let source = SourceDescriptor::new(SourceId::new("s1").unwrap(), "case.m", 128).unwrap();
    module.add_source_descriptor(source).unwrap();
    module
        .add_source_map_entry(
            SourceMapEntry::new(
                "/buses/0/vm",
                SourceRelation::Exact,
                vec![SourceSpan::new(SourceId::new("s1").unwrap(), 10, 20).unwrap()],
            )
            .unwrap(),
        )
        .unwrap();
    module
        .add_diagnostic(Diagnostic::new(
            DiagnosticCode::new("READ.TEST.NOTE").unwrap(),
            DiagnosticSeverity::Note,
            "a stored finding",
        ))
        .unwrap();
    module
        .add_history_entry(
            HistoryEntry::new(
                HistoryId::new("h1").unwrap(),
                HistoryKind::Parse,
                "parse_matpower",
            )
            .unwrap(),
        )
        .unwrap();
    module
        .insert_extension("org.example.note".to_string(), serde_json::json!({"x": 1}))
        .unwrap();
    module
}

#[test]
fn version_one_round_trips_with_records_and_nonfinite_bounds() {
    let module = module_with_records();
    let text = write_module(&module).unwrap();

    // The exact top level identity and the stored nonfinite spelling.
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["schema"], "powerio.module");
    assert_eq!(raw["version"], 1);
    assert_eq!(raw["value"]["kind"], "balanced_network");
    assert_eq!(raw["value"]["data"]["buses"][1]["vmax"], "Infinity");

    let back = read_module(&text).unwrap();
    let PioValue::BalancedNetwork(network) = back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(network.buses().len(), 2);
    assert!(network.buses()[1].vmax.is_infinite());
    assert_eq!(back.sources().len(), 1);
    assert_eq!(back.source_map().len(), 1);
    assert_eq!(back.diagnostics().len(), 1);
    assert_eq!(back.history().len(), 1);
    assert!(back.extensions().contains_key("org.example.note"));

    // Writing again reproduces the document byte for byte.
    assert_eq!(write_module(&back).unwrap(), text);
}

#[test]
fn operating_point_series_round_trips_typed() {
    let network = small_network();
    let time_points = vec![
        TimePoint::new("h0", Some(std::time::Duration::from_secs(3600))).unwrap(),
        TimePoint::new("h1", Some(std::time::Duration::from_secs(3600))).unwrap(),
    ];
    let series = powerio_prob::BalancedStateBuilder::new(network, time_points)
        .load_active_powers(vec![40.0, 55.0])
        .generator_active_powers(vec![42.0, 61.5])
        .build()
        .unwrap();
    let module = PioModule::new(PioValue::BalancedOperatingPointTimeSeries(series));
    let text = write_module(&module).unwrap();
    let back = read_module(&text).unwrap();
    let PioValue::BalancedOperatingPointTimeSeries(series) = back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(series.len(), 2);
    assert_eq!(series.values()[1].load_active_power("loads:0"), Some(55.0));
    assert_eq!(
        series.values()[1].generator_active_power("generators:0"),
        Some(61.5)
    );
    assert_eq!(
        series.time_points()[0].duration(),
        Some(std::time::Duration::from_secs(3600))
    );
}

#[test]
fn unknown_semantic_fields_are_refused() {
    let module = module_with_records();
    let text = write_module(&module).unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    raw["surprise"] = serde_json::json!(true);
    let error = read_module(&raw.to_string()).unwrap_err().to_string();
    assert!(error.contains("surprise"), "{error}");
}

#[test]
fn unsupported_versions_are_refused_with_their_identity() {
    let error = read_module(r#"{"schema": "powerio.module", "version": 2}"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("version 2"), "{error}");
    let error = read_module(r#"{"schema": "someone.else", "version": 1}"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("someone.else"), "{error}");
}

#[test]
fn pre_09_lineage_is_refused() {
    let error = read_module(r#"{"schema_version": "0.2.0", "model": {}}"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("regenerated"), "{error}");
}

/// The frozen 0.9 upgrade fixture: a released-shape package committed as a
/// file, upgrading forever. Regenerating it requires a deliberate decision,
/// never a drive-by.
#[test]
fn the_frozen_09_fixture_upgrades() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/package/frozen-0.9-series.pio.json"
    );
    let text = std::fs::read_to_string(path).unwrap();
    let module = read_module(&text).unwrap();
    let PioValue::BalancedOperatingPointTimeSeries(series) = module.value() else {
        panic!("expected the series value");
    };
    assert_eq!(series.len(), 2);
    assert_eq!(series.values()[1].load_active_power("loads:0"), Some(75.0));
    let updated = series.values()[1].bus_voltage_angle(BusId(2)).unwrap();
    assert!((updated - 30.0_f64.to_radians()).abs() < 1e-12, "{updated}");
}

/// The other released 0.9 shapes, frozen as files: a static balanced package
/// and a static multiconductor package.
#[test]
fn the_frozen_09_static_fixtures_upgrade() {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/package");
    let text = std::fs::read_to_string(format!("{base}/frozen-0.9-balanced.pio.json")).unwrap();
    let module = read_module(&text).unwrap();
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("expected the balanced value");
    };
    assert_eq!(network.buses().len(), 2);

    let text =
        std::fs::read_to_string(format!("{base}/frozen-0.9-multiconductor.pio.json")).unwrap();
    let module = read_module(&text).unwrap();
    let PioValue::MulticonductorNetwork(network) = module.value() else {
        panic!("expected the multiconductor value");
    };
    assert_eq!(network.buses().len(), 1);
}

// ---- the remaining promoted kinds round trip ---------------------------------

#[test]
fn multiconductor_network_round_trips() {
    let mut network = powerio_dist::MulticonductorNetwork::named("mc-roundtrip");
    network.buses_mut().push(powerio_dist::DistBus::new(
        "b1",
        vec!["1".into(), "2".into()],
    ));
    let module = PioModule::new(PioValue::MulticonductorNetwork(network));
    let text = write_module(&module).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["value"]["kind"], "multiconductor_network");
    let back = read_module(&text).unwrap();
    let PioValue::MulticonductorNetwork(network) = back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(network.buses().len(), 1);
    assert_eq!(write_module(&back).unwrap(), text);
}

#[test]
fn balanced_network_time_series_round_trips() {
    let mut second = small_network();
    second.loads_mut()[0].p = 55.0;
    let series = powerio_core::TimeSeries::new(
        vec![
            TimePoint::new("h0", Some(std::time::Duration::from_secs(3600))).unwrap(),
            TimePoint::new("h1", Some(std::time::Duration::from_secs(3600))).unwrap(),
        ],
        vec![small_network(), second],
    )
    .unwrap();
    let module = PioModule::new(PioValue::BalancedNetworkTimeSeries(series));
    let text = write_module(&module).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["value"]["kind"], "balanced_network_time_series");
    let back = read_module(&text).unwrap();
    let PioValue::BalancedNetworkTimeSeries(series) = back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(series.len(), 2);
    assert!((series.values()[1].loads()[0].p - 55.0).abs() < 1e-12);
    assert_eq!(write_module(&back).unwrap(), text);
}

#[test]
fn balanced_network_scenario_set_round_trips() {
    use powerio_core::{Scenario, ScenarioId, ScenarioSet};
    let set = ScenarioSet::new(vec![
        Scenario::new(ScenarioId::new("base").unwrap(), Some(0.6), small_network()),
        Scenario::new(ScenarioId::new("peak").unwrap(), Some(0.4), small_network()),
    ])
    .unwrap();
    let module = PioModule::new(PioValue::BalancedNetworkScenarioSet(set));
    let text = write_module(&module).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["value"]["kind"], "balanced_network_scenario_set");
    let back = read_module(&text).unwrap();
    let PioValue::BalancedNetworkScenarioSet(set) = back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(set.len(), 2);
    assert_eq!(set.get("peak").unwrap().probability(), Some(0.4));
    assert_eq!(write_module(&back).unwrap(), text);
}

// ---- reference validation ----------------------------------------------------

#[test]
fn a_span_past_its_source_is_refused() {
    let module = module_with_records();
    let text = write_module(&module).unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    raw["source_map"][0]["spans"][0]["byte_end"] = serde_json::json!(999);
    let error = read_module(&raw.to_string()).unwrap_err().to_string();
    assert!(error.contains("exceeds source length"), "{error}");
}

#[test]
fn an_unnamespaced_extension_is_refused() {
    let module = module_with_records();
    let text = write_module(&module).unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    raw["extensions"] = serde_json::json!({"note": 1});
    let error = read_module(&raw.to_string()).unwrap_err().to_string();
    assert!(error.contains("not namespaced"), "{error}");
}

#[test]
fn suggested_action_survives_the_stored_round_trip() {
    let mut module = powerio_core::PioModule::new(PioValue::BalancedNetwork(small_network()));
    let diagnostic = powerio_core::Diagnostic::of(
        &powerio::write::codes::REQUEST_WRITE_UNKNOWN_FORMAT,
        "a finding with an action",
    )
    .with_suggested_action("rerun with --strict");
    module
        .add_diagnostic(diagnostic)
        .expect("diagnostic attaches");
    let text = powerio::stored::write_module(&module).expect("writes");
    assert!(
        text.contains("\"suggested_action\"") && text.contains("rerun with --strict"),
        "the stored document carries the action: {text}"
    );
    let reread = powerio::stored::read_module(&text).expect("reads back");
    assert_eq!(
        reread.diagnostics()[0].suggested_action(),
        Some("rerun with --strict"),
        "the action survives the round trip"
    );
}

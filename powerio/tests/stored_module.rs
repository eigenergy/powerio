//! The `.pio.json` version 1 wire: round trips, refusals, and the one way
//! 0.9 upgrade.

use std::collections::BTreeMap;

use powerio::stored::{read_module, write_module};
use powerio::{BalancedNetwork, PioValue};
use powerio_core::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, HistoryEntry, HistoryId, HistoryKind,
    PioModule, Producer, SourceDescriptor, SourceId, SourceMapEntry, SourceRelation, SourceSpan,
    TimePoint,
};
use powerio_tx::{Bus, BusId, BusType, Generator, Load, repair_values};

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

/// A repair finding's target is an RFC 6901 pointer into `value.data`, not
/// the `element#field` locator the writer refuses (READ.MODULE.INVALID: the
/// stored document target grammar allows only a leading `/`).
#[test]
fn a_repair_finding_target_is_a_pointer_the_writer_accepts() {
    let mut network = small_network();
    // small_network's generator otherwise leaves the constructor's default
    // `mbase: 0.0`, which is itself out of domain and would add a second,
    // unrelated finding; give it a valid base so only the bus repair below
    // fires.
    network.generators_mut()[0].mbase = 100.0;
    network.buses_mut()[1].vm = 0.0; // outside [0, 2] p.u.: triggers a repair
    let module = PioModule::new(network);
    let module = repair_values(module).unwrap();
    assert_eq!(module.diagnostics().len(), 1);
    let diagnostic = &module.diagnostics()[0];
    assert!(
        diagnostic.target_is_pointer(),
        "not an RFC 6901 pointer: {:?}",
        diagnostic.target()
    );
    let target = diagnostic.target().unwrap().to_owned();

    let module = module.map_value(PioValue::BalancedNetwork);
    let text = write_module(&module).unwrap();

    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        raw["value"].pointer(&format!("/data{target}")),
        Some(&serde_json::json!(1.0)),
        "target `{target}` does not resolve to the repaired bus's stored vm"
    );

    // read_module runs the same target validation the write did; a round
    // trip through the reader is one more proof the target is accepted.
    read_module(&text).unwrap();
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

// ---- the one way 0.9 upgrade ------------------------------------------------

fn legacy_package_text(with_series: bool, with_study: bool) -> String {
    use powerio::package::{
        ElementRef, ElementUpdate, NetworkPackage, OperatingPoint, OperatingPointSeries, TimeAxis,
    };
    let mut package = NetworkPackage::from_balanced(small_network());
    if with_series {
        let mut update_fields = BTreeMap::new();
        update_fields.insert("p".to_string(), serde_json::json!(75.0));
        let mut axis = TimeAxis::new(2);
        axis.duration_hours = vec![1.0, 1.0];
        axis.labels = vec!["h0".into(), "h1".into()];
        let mut angle_fields = BTreeMap::new();
        angle_fields.insert("va".to_string(), serde_json::json!(30.0));
        let mut second = OperatingPoint::new(1);
        second.updates = vec![
            ElementUpdate::new(ElementRef::new("loads", 0), update_fields),
            ElementUpdate::new(ElementRef::new("buses", 1), angle_fields),
        ];
        package.operating_points = Some(OperatingPointSeries::new(
            axis,
            vec![OperatingPoint::new(0), second],
        ));
    }
    let mut text = package.to_json().unwrap();
    if with_study {
        // Inject the study block at the JSON level: the runtime constructors
        // are additive-only, and the upgrade must refuse the stored shape
        // regardless of how it was produced.
        let mut raw: serde_json::Value = serde_json::from_str(&text).unwrap();
        raw["study"] = serde_json::json!({
            "label": "scenario a",
            "commits": [{"id": "c1"}]
        });
        text = raw.to_string();
    }
    text
}

#[test]
fn a_released_09_package_upgrades_one_way() {
    let text = legacy_package_text(false, false);
    let module = read_module(&text).unwrap();
    assert!(matches!(module.value(), PioValue::BalancedNetwork(_)));
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.MODULE.UPGRADED"),
        "{:?}",
        module.diagnostics()
    );
    assert!(
        module
            .history()
            .iter()
            .any(|entry| entry.kind() == HistoryKind::Upgrade)
    );
}

#[test]
fn legacy_operating_points_become_the_primary_series_value() {
    let text = legacy_package_text(true, false);
    let module = read_module(&text).unwrap();
    let PioValue::BalancedOperatingPointTimeSeries(series) = module.value() else {
        panic!("expected the series value, got {:?}", module.value().kind());
    };
    assert_eq!(series.len(), 2);
    // Point 0 keeps the static payload's load; point 1 carries the update.
    assert_eq!(series.values()[0].load_active_power("loads:0"), Some(40.0));
    assert_eq!(series.values()[1].load_active_power("loads:0"), Some(75.0));
    // Legacy angles were degrees; the state vocabulary stores radians. The
    // base row carries the static payload's angle, the update its own.
    let base = series.values()[0].bus_voltage_angle(BusId(2)).unwrap();
    assert!((base - 12.0_f64.to_radians()).abs() < 1e-12, "{base}");
    let updated = series.values()[1].bus_voltage_angle(BusId(2)).unwrap();
    assert!((updated - 30.0_f64.to_radians()).abs() < 1e-12, "{updated}");
}

#[test]
fn a_nonempty_legacy_study_is_refused_with_direction() {
    let text = legacy_package_text(false, true);
    let error = read_module(&text).unwrap_err().to_string();
    assert!(error.contains("materialize"), "{error}");
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

#[test]
#[ignore = "fixture generator"]
fn generate_frozen_fixtures() {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/package");
    std::fs::write(
        format!("{base}/frozen-0.9-series.pio.json"),
        legacy_package_text(true, false),
    )
    .unwrap();
    std::fs::write(
        format!("{base}/frozen-0.9-balanced.pio.json"),
        legacy_package_text(false, false),
    )
    .unwrap();
    let mut network = powerio_dist::MulticonductorNetwork::named("frozen-mc");
    network.buses_mut().push(powerio_dist::DistBus::new(
        "b1",
        vec!["1".into(), "2".into()],
    ));
    let package = powerio::package::NetworkPackage::from_multiconductor(network);
    std::fs::write(
        format!("{base}/frozen-0.9-multiconductor.pio.json"),
        package.to_json().unwrap(),
    )
    .unwrap();
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

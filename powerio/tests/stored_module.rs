//! The `.pio.json` version 1 wire: round trips, refusals, and the one way
//! 0.9 upgrade.

use std::collections::BTreeMap;

use powerio::stored::{read_module, write_module};
use powerio::{BalancedNetwork, PioValue};
use powerio_core::{
    Diagnostic, DiagnosticCode, DiagnosticId, DiagnosticSeverity, HistoryEntry, HistoryId,
    HistoryKind, PioModule, Producer, SourceDescriptor, SourceId, SourceMapEntry, SourceRelation,
    SourceSpan, TimePoint,
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

/// Build a module carrying the diagnostics `d0` and `d1` (`d1` referencing
/// `d0`), inserted in `order`, so the position based id this test guards
/// against depends only on order, never content.
fn module_with_d0_and_d1(order: [&str; 2]) -> PioModule<PioValue> {
    let mut module = PioModule::new(PioValue::BalancedNetwork(small_network()));
    let d0 = Diagnostic::new(
        DiagnosticCode::new("READ.TEST.NOTE").unwrap(),
        DiagnosticSeverity::Note,
        "first",
    )
    .with_id(DiagnosticId::new("d0").unwrap());
    let d1 = Diagnostic::new(
        DiagnosticCode::new("READ.TEST.NOTE").unwrap(),
        DiagnosticSeverity::Note,
        "second",
    )
    .with_id(DiagnosticId::new("d1").unwrap())
    .with_related(DiagnosticId::new("d0").unwrap())
    .unwrap();
    for name in order {
        module
            .add_diagnostic(if name == "d0" { d0.clone() } else { d1.clone() })
            .unwrap();
    }
    module
}

/// After a write/read round trip, `d1`'s `related` still names `d0`, and the
/// diagnostic actually holding id `d0` is the one whose message is `"first"`
/// (not some other record that ended up sharing the id).
fn assert_d1_still_resolves_to_d0(module: &PioModule<PioValue>) {
    let d1 = module
        .diagnostics()
        .iter()
        .find(|d| d.id().is_some_and(|id| id.as_str() == "d1"))
        .expect("d1 present");
    assert_eq!(d1.related().first().map(DiagnosticId::as_str), Some("d0"));
    let d0 = module
        .diagnostics()
        .iter()
        .find(|d| d.id().is_some_and(|id| id.as_str() == "d0"))
        .expect("d0 present");
    assert_eq!(d0.message(), "first");
}

/// A diagnostic appended with no explicit id (as a repair pass does, e.g.
/// `powerio_tx::network::ValueFinding::into_diagnostic`) must not collide
/// with an id an external document already claimed explicitly, and the
/// module must still write successfully. Checked at two different
/// insertion orders for `d0`/`d1`, since a position derived id is exactly
/// what a reorder would have broken.
#[test]
fn an_id_synthesized_for_an_unlabeled_diagnostic_never_collides_with_an_explicit_one() {
    for order in [["d0", "d1"], ["d1", "d0"]] {
        // "a .pio.json ... reads": round trip through real stored text first,
        // the same as any external document would arrive.
        let starting_text = write_module(&module_with_d0_and_d1(order)).unwrap();
        let mut module = read_module(&starting_text).unwrap();

        // Appended with no id of its own, matching the repair pass shape.
        module
            .add_diagnostic(Diagnostic::new(
                DiagnosticCode::new("READ.TEST.NOTE").unwrap(),
                DiagnosticSeverity::Note,
                "appended without an id",
            ))
            .unwrap();

        let text = write_module(&module).unwrap();
        let back = read_module(&text).unwrap();

        assert_eq!(back.diagnostics().len(), 3, "order {order:?}");
        let ids: std::collections::HashSet<&str> = back
            .diagnostics()
            .iter()
            .map(|d| d.id().unwrap().as_str())
            .collect();
        assert_eq!(ids.len(), 3, "order {order:?}: a synthesized id collided");
        assert_d1_still_resolves_to_d0(&back);
    }
}

/// SEC-9: the writer used to panic on an unmapped `SourceRelation` or
/// `HistoryKind`, folding that case together with every currently known
/// variant into one wildcard arm it could never actually prove reachable.
/// Splitting them apart must not disturb the known variants: every one
/// still has to round trip under its own stored spelling.
#[test]
fn every_source_relation_and_history_kind_round_trips() {
    let relations = [
        SourceRelation::Exact,
        SourceRelation::Defaulted,
        SourceRelation::Inferred,
        SourceRelation::ConvertedUnits,
        SourceRelation::Aggregated,
        SourceRelation::Split,
        SourceRelation::Synthetic,
        SourceRelation::Transformed,
        SourceRelation::RetainedExtra,
    ];
    let mut module = PioModule::new(PioValue::BalancedNetwork(small_network()));
    module
        .add_source_descriptor(
            SourceDescriptor::new(SourceId::new("s1").unwrap(), "case.m", 64).unwrap(),
        )
        .unwrap();
    for relation in relations {
        let span = SourceSpan::new(SourceId::new("s1").unwrap(), 0, 1).unwrap();
        module
            .add_source_map_entry(SourceMapEntry::new("/buses/0/vm", relation, vec![span]).unwrap())
            .unwrap();
    }
    for (index, kind) in [
        HistoryKind::Parse,
        HistoryKind::Upgrade,
        HistoryKind::Transform,
        HistoryKind::Edit,
        HistoryKind::Repair,
    ]
    .into_iter()
    .enumerate()
    {
        module
            .add_history_entry(
                HistoryEntry::new(HistoryId::new(format!("h{index}")).unwrap(), kind, "op")
                    .unwrap(),
            )
            .unwrap();
    }

    let text = write_module(&module).unwrap();
    let back = read_module(&text).unwrap();
    assert_eq!(
        back.source_map()
            .iter()
            .map(SourceMapEntry::relation)
            .collect::<Vec<_>>(),
        relations
    );
    assert_eq!(
        back.history()
            .iter()
            .map(HistoryEntry::kind)
            .collect::<Vec<_>>(),
        [
            HistoryKind::Parse,
            HistoryKind::Upgrade,
            HistoryKind::Transform,
            HistoryKind::Edit,
            HistoryKind::Repair,
        ]
    );
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

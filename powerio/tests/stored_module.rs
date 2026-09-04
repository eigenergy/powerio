//! PowerIO IR round trips and refusals.

use powerio::{BalancedNetwork, PioValue};
use powerio_core::{
    Diagnostic, DiagnosticCode, DiagnosticId, DiagnosticSeverity, HistoryEntry, HistoryId,
    HistoryKind, PioModule, Producer, SourceDescriptor, SourceId, SourceMapEntry, SourceRelation,
    SourceSpan, TimePoint,
};
use powerio_tx::{
    Bus, BusId, BusType, Generator, Load, TerminalReference, TransformerControl,
    TransformerControlMode, repair_values,
};

mod helpers;
use helpers::{deserialize_module_text, serialize_module_text};

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
    let branch = &mut network.branches_mut()[0];
    branch.name = Some("stored-transformer".to_owned());
    branch.tap = 1.0;
    let mut control = TransformerControl::new(TransformerControlMode::AsymmetricActiveFlow);
    control.enabled = false;
    control.controlled_bus = Some(BusId(2));
    control.controlled_bus_on_winding_side = true;
    control.regulating_terminal = Some(
        serde_json::from_value(serde_json::json!({
            "equipment": {
                "component_type": "transformer",
                "local_id": "stored-transformer"
            },
            "terminal": 2
        }))
        .unwrap(),
    );
    control.tap_min = 0.92;
    control.tap_max = 1.08;
    control.ntp = 17;
    control.winding_connection_angle = Some(12.5);
    branch.control = Some(control);
    let mut generator = Generator::new(BusId(1));
    generator.pg = 42.0;
    generator.voltage_regulation_on = false;
    generator.regulated_bus = Some(BusId(2));
    generator.regulating_terminal = Some(
        serde_json::from_value::<TerminalReference>(serde_json::json!({
            "equipment": {
                "component_type": "load",
                "local_id": "remote-load"
            },
            "terminal": 1
        }))
        .unwrap(),
    );
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
fn current_ir_round_trips_with_records_and_nonfinite_bounds() {
    let module = module_with_records();
    let text = serialize_module_text(&module).unwrap();

    // The exact top level identity and the stored nonfinite spelling.
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["schema"], powerio::IR_SCHEMA_NAME);
    assert_eq!(raw["version"], powerio::IR_VERSION);
    assert_eq!(raw["value"]["type"], "powerio.BalancedNetwork");
    assert_eq!(raw["value"]["data"]["buses"][1]["vmax"], "Infinity");

    let back = deserialize_module_text(&text).unwrap();
    let PioValue::BalancedNetwork(network) = &back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(network.buses().len(), 2);
    assert!(network.buses()[1].vmax.is_infinite());
    assert_eq!(
        network.branches()[0].name.as_deref(),
        Some("stored-transformer")
    );
    let control = network.branches()[0].control.as_ref().unwrap();
    assert_eq!(control.mode, TransformerControlMode::AsymmetricActiveFlow);
    assert!(!control.enabled);
    assert_eq!(control.controlled_bus, Some(BusId(2)));
    assert!(control.controlled_bus_on_winding_side);
    assert_eq!(
        control
            .regulating_terminal
            .as_ref()
            .unwrap()
            .equipment
            .local_id(),
        "stored-transformer"
    );
    assert_eq!(control.regulating_terminal.as_ref().unwrap().terminal, 2);
    assert_eq!(control.ntp, 17);
    assert_eq!(control.winding_connection_angle, Some(12.5));
    assert!(!network.generators()[0].voltage_regulation_on);
    assert_eq!(network.generators()[0].regulated_bus, Some(BusId(2)));
    assert_eq!(
        network.generators()[0]
            .regulating_terminal
            .as_ref()
            .unwrap()
            .equipment
            .local_id(),
        "remote-load"
    );
    assert_eq!(back.sources().len(), 1);
    assert_eq!(back.source_map().len(), 1);
    assert_eq!(back.diagnostics().len(), 1);
    assert_eq!(back.history().len(), 1);
    assert!(back.extensions().contains_key("org.example.note"));

    // Writing again reproduces the document byte for byte.
    assert_eq!(serialize_module_text(&back).unwrap(), text);
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
    let text = serialize_module_text(&module).unwrap();

    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        raw["value"].pointer(&format!("/data{target}")),
        Some(&serde_json::json!(1.0)),
        "target `{target}` does not resolve to the repaired bus's stored vm"
    );

    // `deserialize` runs the same target validation as `serialize`; a round
    // trip is one more proof the target is accepted.
    deserialize_module_text(&text).unwrap();
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
        let starting_text = serialize_module_text(&module_with_d0_and_d1(order)).unwrap();
        let mut module = deserialize_module_text(&starting_text).unwrap();

        // Appended with no id of its own, matching the repair pass shape.
        module
            .add_diagnostic(Diagnostic::new(
                DiagnosticCode::new("READ.TEST.NOTE").unwrap(),
                DiagnosticSeverity::Note,
                "appended without an id",
            ))
            .unwrap();

        let text = serialize_module_text(&module).unwrap();
        let back = deserialize_module_text(&text).unwrap();

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
        HistoryKind::Transform,
        HistoryKind::Edit,
        HistoryKind::Repair,
        HistoryKind::Solve,
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

    let text = serialize_module_text(&module).unwrap();
    let back = deserialize_module_text(&text).unwrap();
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
            HistoryKind::Transform,
            HistoryKind::Edit,
            HistoryKind::Repair,
            HistoryKind::Solve,
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
    let series = powerio_prob::BalancedOperatingPointBuilder::new(network, time_points)
        .load_active_powers(vec![40.0, 55.0])
        .generator_active_powers(vec![42.0, 61.5])
        .build()
        .unwrap();
    let module = PioModule::new(PioValue::from(series));
    let text = serialize_module_text(&module).unwrap();
    let back = deserialize_module_text(&text).unwrap();
    let PioValue::TimeSeries(series) = &back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(series.len(), 2);
    let PioValue::BalancedOperatingPoint(point) = series.get(1).unwrap() else {
        panic!("wrong element type");
    };
    let load_id = point.network().loads()[0].uid.as_deref().unwrap();
    let generator_id = point.network().generators()[0].uid.as_deref().unwrap();
    assert_eq!(point.load_active_power(load_id), Some(55.0));
    assert_eq!(point.generator_active_power(generator_id), Some(61.5));
    assert_eq!(
        series.time_points()[0].duration(),
        Some(std::time::Duration::from_secs(3600))
    );
}

#[test]
fn unknown_semantic_fields_are_refused() {
    let module = module_with_records();
    let text = serialize_module_text(&module).unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    raw["surprise"] = serde_json::json!(true);
    let error = deserialize_module_text(&raw.to_string())
        .unwrap_err()
        .to_string();
    assert!(error.contains("surprise"), "{error}");
}

/// The refusal names the generation found and says what to do.
#[test]
fn an_unreadable_powerio_ir_generation_is_refused_with_the_remedy() {
    let header = |version: serde_json::Value| {
        serde_json::json!({ "schema": powerio::IR_SCHEMA_NAME, "version": version }).to_string()
    };

    let newer = powerio::IR_VERSION + 1;
    let error = deserialize_module_text(&header(newer.into()))
        .unwrap_err()
        .to_string();
    assert!(error.contains(&format!("version {newer}")), "{error}");
    assert!(error.contains("upgrade PowerIO"), "{error}");

    // A non-integer spelling is not a released PowerIO IR generation.
    let error = deserialize_module_text(&header("0.11.0".into()))
        .unwrap_err()
        .to_string();
    assert!(error.contains("version \"0.11.0\""), "{error}");
    assert!(error.contains("regenerate this document"), "{error}");

    // The v0.10.0 document's integer version is named as written.
    let error = deserialize_module_text(&header(1.into()))
        .unwrap_err()
        .to_string();
    assert!(error.contains("version 1"), "{error}");

    // The producer a document states is named, so the reader a user needs is
    // clear from the refusal alone.
    let text = serde_json::json!({
        "schema": powerio::IR_SCHEMA_NAME,
        "version": newer,
        "producer": { "name": "powerio", "version": "9.0.0" },
    })
    .to_string();
    let error = deserialize_module_text(&text).unwrap_err().to_string();
    assert!(error.contains("written by powerio 9.0.0"), "{error}");
    assert!(error.contains("upgrade PowerIO"), "{error}");

    let text = serde_json::json!({
        "schema": "someone.else",
        "version": powerio::IR_VERSION,
    })
    .to_string();
    let error = deserialize_module_text(&text).unwrap_err().to_string();
    assert!(error.contains("someone.else"), "{error}");
    assert!(error.contains("regenerate this document"), "{error}");
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
    let text = serialize_module_text(&module).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["value"]["type"], "powerio.MulticonductorNetwork");
    let back = deserialize_module_text(&text).unwrap();
    let PioValue::MulticonductorNetwork(network) = &back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(network.buses().len(), 1);
    assert_eq!(serialize_module_text(&back).unwrap(), text);
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
    let module = PioModule::new(PioValue::from(series));
    let text = serialize_module_text(&module).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        raw["value"]["type"],
        "powerio.TimeSeries<powerio.BalancedNetwork>"
    );
    let back = deserialize_module_text(&text).unwrap();
    let PioValue::TimeSeries(series) = &back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(series.len(), 2);
    let PioValue::BalancedNetwork(network) = series.get(1).unwrap() else {
        panic!("wrong element type");
    };
    assert!((network.loads()[0].p - 55.0).abs() < 1e-12);
    assert_eq!(serialize_module_text(&back).unwrap(), text);
}

#[test]
fn balanced_network_scenario_set_round_trips() {
    use powerio_core::{Scenario, ScenarioId, ScenarioSet};
    let set = ScenarioSet::new(vec![
        Scenario::new(ScenarioId::new("base").unwrap(), Some(0.6), small_network()),
        Scenario::new(ScenarioId::new("peak").unwrap(), Some(0.4), small_network()),
    ])
    .unwrap();
    let module = PioModule::new(PioValue::from(set));
    let text = serialize_module_text(&module).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        raw["value"]["type"],
        "powerio.ScenarioSet<powerio.BalancedNetwork>"
    );
    let back = deserialize_module_text(&text).unwrap();
    let PioValue::ScenarioSet(set) = &back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(set.len(), 2);
    let peak = set
        .iter()
        .find(|scenario| scenario.id().as_str() == "peak")
        .unwrap();
    assert_eq!(peak.probability(), Some(0.4));
    assert_eq!(serialize_module_text(&back).unwrap(), text);
}

// ---- determinism of `serialize` ---------------------------------------------

fn fixture_path(relative: &str) -> String {
    format!("{}/../tests/data/{relative}", env!("CARGO_MANIFEST_DIR"))
}

fn parse_fixture(relative: &str, format: &str) -> PioModule<PioValue> {
    powerio::parse_with_options(
        powerio::Source::open(fixture_path(relative)).unwrap(),
        &powerio::ParseOptions::default().format(format).unwrap(),
    )
    .unwrap_or_else(|error| panic!("{relative}: {error}"))
}

/// Serializing a module twice gives identical text, and serializing what
/// that text deserializes to gives the same text again.
fn assert_serialization_is_stable(module: &PioModule<PioValue>, label: &str) {
    let first = serialize_module_text(module).unwrap();
    let second = serialize_module_text(module).unwrap();
    assert_eq!(
        first, second,
        "{label}: two serializations of one module differ"
    );
    let reread = deserialize_module_text(&first).unwrap();
    let third = serialize_module_text(&reread).unwrap();
    assert_eq!(
        first, third,
        "{label}: serialize, deserialize, serialize is not identity"
    );
}

#[test]
fn serialization_of_the_fixture_cases_is_deterministic() {
    for (relative, format) in [
        ("case9.m", "matpower"),
        ("case14.m", "matpower"),
        ("dist/bmopf/example_ieee13.json", "bmopf-json"),
        ("dist/opendss/ieee13/IEEE13Nodeckt.dss", "dss"),
    ] {
        assert_serialization_is_stable(&parse_fixture(relative, format), relative);
    }
}

#[test]
fn serialization_of_records_and_collections_is_deterministic() {
    use powerio_core::{Scenario, ScenarioId, ScenarioSet};

    assert_serialization_is_stable(&module_with_records(), "records");

    let mut second = small_network();
    second.loads_mut()[0].p = 55.0;
    let series = powerio_core::TimeSeries::new(
        vec![
            TimePoint::new("h0", Some(std::time::Duration::from_secs(3600))).unwrap(),
            TimePoint::new("h1", None).unwrap(),
        ],
        vec![small_network(), second],
    )
    .unwrap();
    assert_serialization_is_stable(&PioModule::new(PioValue::from(series)), "time series");

    let set = ScenarioSet::new(vec![
        Scenario::new(ScenarioId::new("peak").unwrap(), Some(0.4), small_network()),
        Scenario::new(ScenarioId::new("base").unwrap(), Some(0.6), small_network()),
    ])
    .unwrap();
    assert_serialization_is_stable(&PioModule::new(PioValue::from(set)), "scenario set");

    let points = powerio_prob::BalancedOperatingPointBuilder::new(
        small_network(),
        vec![
            TimePoint::new("h0", None).unwrap(),
            TimePoint::new("h1", None).unwrap(),
        ],
    )
    .load_active_powers(vec![40.0, 55.0])
    .generator_active_powers(vec![42.0, 61.5])
    .build()
    .unwrap();
    assert_serialization_is_stable(
        &PioModule::new(PioValue::from(points)),
        "operating point series",
    );
}

// ---- reference validation ----------------------------------------------------

#[test]
fn a_span_past_its_source_is_refused() {
    let module = module_with_records();
    let text = serialize_module_text(&module).unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    raw["source_map"][0]["spans"][0]["byte_end"] = serde_json::json!(999);
    let error = deserialize_module_text(&raw.to_string())
        .unwrap_err()
        .to_string();
    assert!(error.contains("exceeds source length"), "{error}");
}

#[test]
fn an_unnamespaced_extension_is_refused() {
    let module = module_with_records();
    let text = serialize_module_text(&module).unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    raw["extensions"] = serde_json::json!({"note": 1});
    let error = deserialize_module_text(&raw.to_string())
        .unwrap_err()
        .to_string();
    assert!(error.contains("not namespaced"), "{error}");
}

#[test]
fn suggested_action_survives_the_stored_round_trip() {
    let mut module = powerio_core::PioModule::new(PioValue::BalancedNetwork(small_network()));
    let diagnostic = powerio_core::Diagnostic::of(
        &powerio::codes::READ_MODULE_INVALID,
        "a finding with an action",
    )
    .with_suggested_action("rerun with --strict");
    module
        .add_diagnostic(diagnostic)
        .expect("diagnostic attaches");
    let text = serialize_module_text(&module).expect("serializes");
    assert!(
        text.contains("\"suggested_action\"") && text.contains("rerun with --strict"),
        "the stored document carries the action: {text}"
    );
    let reread = deserialize_module_text(&text).expect("deserializes");
    assert_eq!(
        reread.diagnostics()[0].suggested_action(),
        Some("rerun with --strict"),
        "the action survives the round trip"
    );
}

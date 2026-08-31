//! Typed inventory, selection, and explicit export over `PioValue`.

use powerio::select::{
    ScenarioEntry, SelectedState, StateInventory, StateSelector, export_module_state, export_state,
    list_states, select_state,
};
use powerio::{BalancedNetwork, PioValue};
use powerio_core::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, HistoryEntry, HistoryId, HistoryKind,
    PioModule, Producer, Scenario, ScenarioId, ScenarioSet, Source, SourceDescriptor,
    SourceMapEntry, SourceRelation, SourceSpan, TimePoint,
};
use powerio_tx::{Bus, BusId, BusType, Generator, Load};

fn small_network() -> BalancedNetwork {
    let mut bus1 = Bus::new(BusId(1), BusType::Ref, 345.0);
    bus1.vm = 1.02;
    let bus2 = Bus::new(BusId(2), BusType::Pq, 345.0);
    let mut network = BalancedNetwork::in_memory(
        "selection",
        100.0,
        vec![bus1, bus2],
        vec![powerio_tx::Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    network.loads_mut().push(Load::new(BusId(2), 40.0, 10.0));
    network.generators_mut().push(Generator::new(BusId(1)));
    network
}

fn point_series() -> PioValue {
    let series = powerio_prob::BalancedStateBuilder::new(
        small_network(),
        vec![
            TimePoint::new("h0", Some(std::time::Duration::from_secs(3600))).unwrap(),
            TimePoint::new("h1", None).unwrap(),
        ],
    )
    .load_active_powers(vec![40.0, 55.0])
    .bus_voltage_angles(vec![0.0, 0.0, 0.0, 0.5])
    .build()
    .unwrap();
    PioValue::BalancedOperatingPointTimeSeries(series)
}

fn scenario_set() -> PioValue {
    let mut peak = small_network();
    peak.loads_mut()[0].p = 80.0;
    PioValue::BalancedNetworkScenarioSet(
        ScenarioSet::new(vec![
            Scenario::new(ScenarioId::new("base").unwrap(), Some(0.7), small_network()),
            Scenario::new(ScenarioId::new("peak").unwrap(), Some(0.3), peak),
        ])
        .unwrap(),
    )
}

#[test]
fn inventories_state_the_exact_typed_keys() {
    let points_value = point_series();
    assert_eq!(
        list_states(&points_value).unwrap(),
        list_states(&points_value).unwrap()
    );
    let StateInventory::TimePoints(points) = list_states(&points_value).unwrap() else {
        panic!("expected a time inventory");
    };
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].position, 0);
    assert_eq!(points[0].label, "h0");
    assert_eq!(
        points[0].duration,
        Some(std::time::Duration::from_secs(3600))
    );
    assert_eq!(points[1].label, "h1");
    assert_eq!(points[1].duration, None);

    let StateInventory::Scenarios(scenarios) = list_states(&scenario_set()).unwrap() else {
        panic!("expected a scenario inventory");
    };
    assert_eq!(
        scenarios,
        vec![
            ScenarioEntry {
                id: "base".to_owned(),
                probability: Some(0.7)
            },
            ScenarioEntry {
                id: "peak".to_owned(),
                probability: Some(0.3)
            },
        ]
    );
}

#[test]
fn a_static_value_refuses_selection_by_code() {
    let value = PioValue::BalancedNetwork(small_network());
    let error = list_states(&value).unwrap_err();
    assert_eq!(
        error.diagnostics()[0].code(),
        "REQUEST.STATE.NOT_A_COLLECTION"
    );
    let error = select_state(&value, StateSelector::TimePosition(0)).unwrap_err();
    assert_eq!(
        error.diagnostics()[0].code(),
        "REQUEST.STATE.NOT_A_COLLECTION"
    );
}

#[test]
fn invalid_keys_return_structured_refusals() {
    let series = point_series();
    let error = select_state(&series, StateSelector::TimePosition(9)).unwrap_err();
    assert_eq!(error.diagnostics()[0].code(), "REQUEST.STATE.OUT_OF_RANGE");
    assert!(error.to_string().contains("2 point axis"), "{error}");

    let error = select_state(&series, StateSelector::Scenario("base")).unwrap_err();
    assert_eq!(
        error.diagnostics()[0].code(),
        "REQUEST.STATE.WRONG_SELECTOR"
    );

    let set = scenario_set();
    let error = select_state(&set, StateSelector::Scenario("winter")).unwrap_err();
    assert_eq!(
        error.diagnostics()[0].code(),
        "REQUEST.STATE.UNKNOWN_SCENARIO"
    );
    assert!(error.to_string().contains("base, peak"), "{error}");
}

#[test]
fn repeated_selection_shares_the_stored_owners() {
    let series = point_series();
    let SelectedState::BalancedOperatingPoint(first) =
        select_state(&series, StateSelector::TimePosition(1)).unwrap()
    else {
        panic!("expected an operating point");
    };
    let SelectedState::BalancedOperatingPoint(second) =
        select_state(&series, StateSelector::TimePosition(1)).unwrap()
    else {
        panic!("expected an operating point");
    };
    // The same stored item both times: the series' own network handle, no
    // copy of any table.
    assert!(std::ptr::eq(first, second));
    assert!(std::ptr::eq(
        first.network().buses().as_ptr(),
        second.network().buses().as_ptr()
    ));
    assert_eq!(first.load_active_power("loads:0"), Some(55.0));

    let set = scenario_set();
    let SelectedState::BalancedNetwork(peak) =
        select_state(&set, StateSelector::Scenario("peak")).unwrap()
    else {
        panic!("expected a network");
    };
    assert!((peak.loads()[0].p - 80.0).abs() < 1e-12);
}

#[test]
fn export_applies_typed_state_and_shares_untouched_tables() {
    let series = point_series();
    let module = export_state(&series, StateSelector::TimePosition(1)).unwrap();
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("expected a static network");
    };
    assert!((network.loads()[0].p - 55.0).abs() < 1e-12);
    // The vocabulary's radians became the table's degrees.
    assert!((network.buses()[1].va - 0.5_f64.to_degrees()).abs() < 1e-12);
    // The generator table was never stated, so the export shares it with the
    // series' network instead of copying it.
    let PioValue::BalancedOperatingPointTimeSeries(stored) = &series else {
        unreachable!();
    };
    let shared = stored.values()[0].network();
    assert!(std::ptr::eq(
        network.generators().as_ptr(),
        shared.generators().as_ptr()
    ));
    assert!(!std::ptr::eq(
        network.loads().as_ptr(),
        shared.loads().as_ptr()
    ));
    // The module's history states the selection.
    assert!(
        module
            .history()
            .iter()
            .any(|entry| entry.kind() == HistoryKind::Transform
                && entry.name() == "export_selected_state")
    );
}

#[test]
fn module_export_preserves_common_records_and_severs_value_bound_records() {
    let source = Source::from_bytes("states.pio.json", b"state collection".to_vec()).unwrap();
    let buffer = source.primary_buffer().unwrap();
    let source_id = buffer.id().clone();
    let source_len = buffer.bytes().len() as u64;
    let mut module = PioModule::new(point_series())
        .with_producer(Producer::new("selection-test", "1").unwrap())
        .with_source(source);
    module
        .add_source_descriptor(
            SourceDescriptor::new(source_id.clone(), "states.pio.json", source_len).unwrap(),
        )
        .unwrap();
    let span = SourceSpan::new(source_id, 0, 5).unwrap();
    module
        .add_source_map_entry(
            SourceMapEntry::new(
                "/network/buses/0/vm",
                SourceRelation::Exact,
                vec![span.clone()],
            )
            .unwrap(),
        )
        .unwrap();
    module
        .add_diagnostic(
            Diagnostic::new(
                DiagnosticCode::new("READ.TEST.SELECTION").unwrap(),
                DiagnosticSeverity::Note,
                "kept finding",
            )
            .with_target("/network/buses/0/vm")
            .unwrap()
            .with_span(span)
            .unwrap(),
        )
        .unwrap();
    module
        .add_history_entry(
            HistoryEntry::new(
                HistoryId::new("export-selected-state").unwrap(),
                HistoryKind::Parse,
                "parse_states",
            )
            .unwrap(),
        )
        .unwrap();
    module
        .insert_extension("org.example.selection", serde_json::json!({"kept": true}))
        .unwrap();

    let exported = export_module_state(&module, StateSelector::TimePosition(1)).unwrap();
    let PioValue::BalancedNetwork(network) = exported.value() else {
        panic!("expected a static network");
    };
    assert!((network.loads()[0].p - 55.0).abs() < 1e-12);
    assert_eq!(exported.producer().name(), "selection-test");
    assert_eq!(exported.sources(), module.sources());
    assert!(exported.source_map().is_empty());
    assert_eq!(exported.diagnostics().len(), 1);
    assert_eq!(exported.diagnostics()[0].target(), None);
    assert_eq!(
        exported.diagnostics()[0].spans(),
        &[SourceSpan::new(module.sources()[0].id().clone(), 0, 5,).unwrap()]
    );
    assert_eq!(exported.history().len(), 2);
    assert_eq!(exported.history()[0].id().as_str(), "export-selected-state");
    assert_eq!(
        exported.history()[1].id().as_str(),
        "export-selected-state-2"
    );
    assert_eq!(
        exported.extensions().get("org.example.selection"),
        Some(&serde_json::json!({"kept": true}))
    );
    assert!(exported.source().is_none());
    assert!(module.source().is_some());
    assert_eq!(module.source_map().len(), 1);
    assert_eq!(
        module.diagnostics()[0].target(),
        Some("/network/buses/0/vm")
    );
}

#[test]
fn an_exported_module_is_accepted_by_matrix_construction() {
    let module = export_state(&point_series(), StateSelector::TimePosition(0)).unwrap();
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("expected a static network");
    };
    let view = powerio_matrix::IndexedNetwork::new(network);
    let ybus =
        powerio_matrix::calc_admittance_matrix(&view, &powerio_matrix::BuildOptions::default())
            .unwrap();
    assert_eq!(ybus.g.rows(), 2);
}

#[test]
fn a_stated_injection_refuses_static_export_by_name() {
    let series = powerio_prob::BalancedStateBuilder::new(
        small_network(),
        vec![TimePoint::new("h0", None).unwrap()],
    )
    .bus_active_injections(vec![1.0, -1.0])
    .build()
    .unwrap();
    let value = PioValue::BalancedOperatingPointTimeSeries(series);
    let error = export_state(&value, StateSelector::TimePosition(0)).unwrap_err();
    assert_eq!(
        error.diagnostics()[0].code(),
        "TRANSFORM.STATE.UNREPRESENTED"
    );
    assert!(
        error.to_string().contains("bus_active_injection"),
        "{error}"
    );
}

#[test]
fn a_network_series_item_exports_through_the_shared_handle() {
    let mut second = small_network();
    second.loads_mut()[0].p = 70.0;
    let value = PioValue::BalancedNetworkTimeSeries(
        powerio_core::TimeSeries::new(
            vec![
                TimePoint::new("h0", None).unwrap(),
                TimePoint::new("h1", None).unwrap(),
            ],
            vec![small_network(), second],
        )
        .unwrap(),
    );
    let SelectedState::BalancedNetwork(item) =
        select_state(&value, StateSelector::TimePosition(1)).unwrap()
    else {
        panic!("expected a network");
    };
    assert!((item.loads()[0].p - 70.0).abs() < 1e-12);
    let module = export_state(&value, StateSelector::TimePosition(1)).unwrap();
    let PioValue::BalancedNetwork(exported) = module.value() else {
        panic!("expected a static network");
    };
    // The export is the cheap handle clone: every table stays shared.
    assert!(std::ptr::eq(
        exported.loads().as_ptr(),
        item.loads().as_ptr()
    ));
}

#[test]
fn a_multiconductor_series_is_a_selectable_collection() {
    let dss = "New Circuit.c basekv=12.47 pu=1 phases=3 bus1=a\n\
               New Line.l1 bus1=a.1.2.3 bus2=b.1.2.3 phases=3 r1=0.1 x1=0.2 length=1 units=km\n\
               New Load.ld bus1=b.1.2.3 phases=3 conn=wye kv=7.2 kw=30 kvar=9\n";
    let source = powerio_core::Source::from_bytes("<memory>", dss.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("dss").unwrap());
    let net = powerio_dist::parse(source).unwrap().into_value();
    let terminals: usize = net.buses().iter().map(|b| b.terminals.len()).sum();
    let labels: Vec<powerio_core::TimePoint> = (0..2)
        .map(|k| powerio_core::TimePoint::new(k.to_string(), None).unwrap())
        .collect();
    let series = powerio_prob::MulticonductorStateBuilder::new(net, labels)
        .terminal_voltage_magnitudes(vec![7200.0; 2 * terminals])
        .build()
        .unwrap();
    let value = PioValue::MulticonductorOperatingPointTimeSeries(series);

    // The inventory lists its time axis rather than refusing it as static.
    let inventory = list_states(&value).expect("a time-indexed collection");
    let StateInventory::TimePoints(points) = inventory else {
        panic!("time points expected");
    };
    assert_eq!(points.len(), 2);

    // Selection lands on the point; the unbound static export refuses with
    // its own code, never the static-value wording.
    let selected = select_state(&value, StateSelector::TimePosition(1)).unwrap();
    assert!(matches!(
        selected,
        SelectedState::MulticonductorOperatingPoint(_)
    ));
    let error = export_state(&value, StateSelector::TimePosition(1)).unwrap_err();
    assert_eq!(
        error.diagnostics().first().map(|d| d.code().to_owned()),
        Some("REQUEST.STATE.UNBOUND_EXPORT".to_owned())
    );
}

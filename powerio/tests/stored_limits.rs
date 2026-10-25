//! Decode time limits on PowerIO IR. Hostile documents are refused
//! at their stated bounds, record decode scales past six figure counts, and
//! a solution over six figure terminal counts round trips.

use std::sync::Arc;

use powerio::{BalancedNetwork, PioValue};
use powerio_core::{PioModule, Scenario, ScenarioId, ScenarioSet, TimePoint, TimeSeries};
use powerio_dist::{DistBus, DistLine, DistLineCode, MulticonductorNetwork, VoltageSource};
use powerio_prob::{
    BalancedOperatingPointBuilder, BalancedOperatingPointFlag, BalancedOperatingPointQuantity,
    McAcPfInstance, McAcPfSolution, Termination,
};
use powerio_tx::{Bus, BusId, BusType, Generator, Load, Switch};

mod helpers;
use helpers::{deserialize_module_text, serialize_module_text};

fn small_network() -> BalancedNetwork {
    let bus1 = Bus::new(BusId(1), BusType::Ref, 345.0);
    let bus2 = Bus::new(BusId(2), BusType::Pq, 345.0);
    let mut network = BalancedNetwork::in_memory(
        "stored-limits",
        100.0,
        vec![bus1, bus2],
        vec![powerio_tx::Branch::new(BusId(1), BusId(2), 0.01, 0.1)],
    );
    network.loads_mut().push(Load::new(BusId(2), 40.0, 10.0));
    network.generators_mut().push(Generator::new(BusId(1)));
    network
}

/// DESER-003: six figure record counts decode through the identity indexes.
/// Every diagnostic's span names the last declared source, so a linear source
/// scan per span would be quadratic and blow far past the ceiling.
#[test]
fn six_figure_record_counts_decode_within_the_ceiling() {
    const COUNT: usize = 131_072;
    let module = PioModule::new(PioValue::BalancedNetwork(small_network()));
    let mut raw: serde_json::Value =
        serde_json::from_str(&serialize_module_text(&module).unwrap()).unwrap();
    let mut sources = Vec::with_capacity(COUNT);
    for index in 0..COUNT {
        sources.push(serde_json::json!({
            "id": format!("s{index}"),
            "name": "case.m",
            "byte_length": 64
        }));
    }
    let last = format!("s{}", COUNT - 1);
    let mut diagnostics = Vec::with_capacity(COUNT);
    for index in 0..COUNT {
        diagnostics.push(serde_json::json!({
            "id": format!("d{index}"),
            "severity": "note",
            "code": "READ.TEST.NOTE",
            "message": "kept",
            "spans": [{"source": last, "byte_start": 0, "byte_end": 8}]
        }));
    }
    raw["sources"] = serde_json::Value::Array(sources);
    raw["diagnostics"] = serde_json::Value::Array(diagnostics);
    let text = raw.to_string();

    let started = std::time::Instant::now();
    let module = deserialize_module_text(&text).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(module.sources().len(), COUNT);
    assert_eq!(module.diagnostics().len(), COUNT);
    // Keep a generous ceiling for shared CI runners; this still catches the
    // quadratic regression this six-figure fixture is designed to detect.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "decode took {elapsed:?}"
    );
}

fn series_module_json() -> serde_json::Value {
    let network = small_network();
    let time_points = vec![
        TimePoint::new("h0", None).unwrap(),
        TimePoint::new("h1", None).unwrap(),
    ];
    let series = BalancedOperatingPointBuilder::new(network, time_points)
        .bus_voltage_magnitudes(vec![1.0, 1.01, 0.99, 1.02])
        .build()
        .unwrap();
    let module = PioModule::new(PioValue::from(series));
    serde_json::from_str(&serialize_module_text(&module).unwrap()).unwrap()
}

/// DESER-004: a stored quantity's values bind to the identities the document
/// states; a permuted identity list is refused rather than rebound.
#[test]
fn a_permuted_identity_list_is_refused() {
    let mut raw = series_module_json();
    let quantities = &mut raw["value"]["data"]["values"][0]["quantities"];
    assert_eq!(
        quantities["bus_voltage_magnitude"]["identities"],
        serde_json::json!(["1", "2"])
    );
    quantities["bus_voltage_magnitude"]["identities"] = serde_json::json!(["2", "1"]);
    let error = deserialize_module_text(&raw.to_string()).unwrap_err();
    assert_eq!(
        error.info().map(|info| info.code),
        Some("READ.MODULE.INVALID")
    );
    assert!(
        error.to_string().contains("bus_voltage_magnitude"),
        "{error}"
    );

    // The unpermuted document round trips and each identity reads its own
    // value back.
    let module = deserialize_module_text(&series_module_json().to_string()).unwrap();
    let PioValue::TimeSeries(series) = &module.value() else {
        panic!("wrong kind");
    };
    let PioValue::BalancedOperatingPoint(point0) = series.get(0).unwrap() else {
        panic!("wrong element type");
    };
    let PioValue::BalancedOperatingPoint(point1) = series.get(1).unwrap() else {
        panic!("wrong element type");
    };
    assert_eq!(point0.bus_voltage_magnitude(BusId(1)), Some(1.0));
    assert_eq!(point0.bus_voltage_magnitude(BusId(2)), Some(1.01));
    assert_eq!(point1.bus_voltage_magnitude(BusId(1)), Some(0.99));
}

/// DESER-005: the writer's vocabulary is the reader's, from one definition,
/// and `switch_closed` survives the round trip.
#[test]
fn the_switch_position_survives_the_round_trip() {
    let mut network = small_network();
    network
        .switches_mut()
        .push(Switch::new(BusId(1), BusId(2), true));
    let time_points = vec![
        TimePoint::new("h0", None).unwrap(),
        TimePoint::new("h1", None).unwrap(),
    ];
    let series = BalancedOperatingPointBuilder::new(network, time_points)
        .switch_closed(vec![true, false])
        .build()
        .unwrap();
    let module = PioModule::new(PioValue::from(series));
    let text = serialize_module_text(&module).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        raw["value"]["data"]["values"][0]["quantities"]
            .as_object()
            .unwrap()
            .contains_key("switch_closed"),
        "the stored document must carry the switch quantity"
    );
    let back = deserialize_module_text(&text).unwrap();
    let PioValue::TimeSeries(series) = &back.value() else {
        panic!("wrong kind");
    };
    let PioValue::BalancedOperatingPoint(point0) = series.get(0).unwrap() else {
        panic!("wrong element type");
    };
    let PioValue::BalancedOperatingPoint(point1) = series.get(1).unwrap() else {
        panic!("wrong element type");
    };
    let switch_id = point0.network().switches()[0].uid.as_deref().unwrap();
    assert_eq!(point0.switch_closed(switch_id), Some(true));
    assert_eq!(point1.switch_closed(switch_id), Some(false));
    assert_eq!(serialize_module_text(&back).unwrap(), text);
}

/// The typed operating point vocabulary has the stable IR spellings.
#[test]
fn typed_operating_point_vocabulary_has_stable_names() {
    use BalancedOperatingPointFlag as Flag;
    use BalancedOperatingPointQuantity as Quantity;

    let names: Vec<&str> = [
        Quantity::BusVoltageMagnitude,
        Quantity::BusVoltageAngle,
        Quantity::BusActiveInjection,
        Quantity::BusReactiveInjection,
        Quantity::GeneratorActivePower,
        Quantity::GeneratorReactivePower,
        Quantity::GeneratorVoltageSetpoint,
        Quantity::LoadActivePower,
        Quantity::LoadReactivePower,
        Quantity::BranchTapRatio,
        Quantity::BranchPhaseShift,
    ]
    .into_iter()
    .map(Quantity::name)
    .chain(
        [
            Flag::GeneratorInService,
            Flag::BranchInService,
            Flag::SwitchClosed,
        ]
        .into_iter()
        .map(Flag::name),
    )
    .collect();
    assert_eq!(names.len(), 14);
    assert!(names.contains(&"bus_voltage_magnitude"));
    assert!(names.contains(&"switch_closed"));
}

fn base_module_json() -> serde_json::Value {
    let module = PioModule::new(PioValue::BalancedNetwork(small_network()));
    serde_json::from_str(&serialize_module_text(&module).unwrap()).unwrap()
}

fn expect_refused(raw: &serde_json::Value) {
    let error = deserialize_module_text(&raw.to_string()).unwrap_err();
    assert_eq!(
        error.info().map(|info| info.code),
        Some("READ.MODULE.INVALID"),
        "{error}"
    );
}

/// DESER-007: every bounded sequence, map, and string in a stored record is
/// refused one element or byte past its limit and accepted exactly at it.
#[test]
// One deliberate sweep over the whole limit table; splitting it would
// scatter the boundary pairs.
#[allow(clippy::too_many_lines)]
fn record_document_bounds_refuse_past_and_accept_at_each_limit() {
    let span = |source: &str| serde_json::json!({"source": source, "byte_start": 0, "byte_end": 1});
    let source = |id: &str| serde_json::json!({"id": id, "name": "case.m", "byte_length": 64});
    let diagnostic = |id: String| {
        serde_json::json!({
            "id": id, "severity": "note", "code": "READ.TEST.NOTE", "message": "kept"
        })
    };
    let history =
        |id: String| serde_json::json!({"id": id, "kind": "parse", "name": "parse_matpower"});

    // Identifier byte bound (refusal only: an at-limit 64 KiB identifier is
    // valid but adds nothing over the boundary refusal).
    let mut raw = base_module_json();
    raw["sources"] = serde_json::json!([source(&"s".repeat(65_537))]);
    expect_refused(&raw);

    // Diagnostic code bytes: 255 accepted, 256 refused. The code grammar caps
    // segment lengths, so build a valid boundary code from dotted segments.
    let long_code = |bytes: usize| {
        let mut code = String::from("READ.TEST");
        while code.len() + 64 <= bytes {
            code.push('.');
            code.push_str(&"A".repeat(63));
        }
        if code.len() < bytes {
            let remainder = bytes - code.len() - 1;
            code.push('.');
            code.push_str(&"B".repeat(remainder));
        }
        code
    };
    let mut raw = base_module_json();
    let mut ok = diagnostic("d1".into());
    ok["code"] = serde_json::json!(long_code(255));
    raw["diagnostics"] = serde_json::json!([ok]);
    assert_eq!(long_code(255).len(), 255);
    deserialize_module_text(&raw.to_string()).unwrap();
    let mut raw = base_module_json();
    let mut over = diagnostic("d1".into());
    over["code"] = serde_json::json!(long_code(256));
    raw["diagnostics"] = serde_json::json!([over]);
    expect_refused(&raw);

    // Diagnostic message: truncated rather than refused past its decode bound.
    let mut raw = base_module_json();
    let mut noisy = diagnostic("d1".into());
    noisy["message"] = serde_json::json!("m".repeat(65_537));
    raw["diagnostics"] = serde_json::json!([noisy]);
    let module = deserialize_module_text(&raw.to_string()).unwrap();
    assert!(module.diagnostics()[0].message().len() <= 16_384);

    // Diagnostic target bytes: one past the bound is refused while decoding.
    let mut raw = base_module_json();
    let mut targeted = diagnostic("d1".into());
    targeted["target"] = serde_json::json!(format!("/{}", "t".repeat(8_192 + 1)));
    raw["diagnostics"] = serde_json::json!([targeted]);
    expect_refused(&raw);

    // Diagnostic spans: 256 accepted, 257 refused.
    let mut raw = base_module_json();
    raw["sources"] = serde_json::json!([source("s1")]);
    let mut spanned = diagnostic("d1".into());
    spanned["spans"] = serde_json::Value::Array((0..256).map(|_| span("s1")).collect());
    raw["diagnostics"] = serde_json::json!([spanned.clone()]);
    deserialize_module_text(&raw.to_string()).unwrap();
    spanned["spans"] = serde_json::Value::Array((0..257).map(|_| span("s1")).collect());
    raw["diagnostics"] = serde_json::json!([spanned]);
    expect_refused(&raw);

    // Related diagnostics: 256 accepted, 257 refused.
    let mut raw = base_module_json();
    let mut related_ok: Vec<serde_json::Value> = (0..256)
        .map(|index| diagnostic(format!("r{index}")))
        .collect();
    let mut lead = diagnostic("d1".into());
    lead["related"] = serde_json::Value::Array(
        (0..256)
            .map(|index| serde_json::json!(format!("r{index}")))
            .collect(),
    );
    related_ok.push(lead.clone());
    raw["diagnostics"] = serde_json::Value::Array(related_ok);
    deserialize_module_text(&raw.to_string()).unwrap();
    lead["related"] = serde_json::Value::Array(
        (0..257)
            .map(|index| serde_json::json!(format!("r{index}")))
            .collect(),
    );
    raw["diagnostics"] = serde_json::json!([lead]);
    expect_refused(&raw);

    // Detail keys: 256 accepted, 257 refused.
    let mut raw = base_module_json();
    let mut detailed = diagnostic("d1".into());
    let details_at = |count: usize| {
        serde_json::Value::Object(
            (0..count)
                .map(|index| (format!("k{index}"), serde_json::json!(1)))
                .collect(),
        )
    };
    detailed["details"] = details_at(256);
    raw["diagnostics"] = serde_json::json!([detailed.clone()]);
    deserialize_module_text(&raw.to_string()).unwrap();
    detailed["details"] = details_at(257);
    raw["diagnostics"] = serde_json::json!([detailed]);
    expect_refused(&raw);

    // Source map spans: 256 accepted, 257 refused.
    let mut raw = base_module_json();
    raw["sources"] = serde_json::json!([source("s1")]);
    let map_entry = |count: usize| {
        serde_json::json!([{
            "target": "/buses/0/vm",
            "relation": "exact",
            "spans": (0..count).map(|_| span("s1")).collect::<Vec<_>>()
        }])
    };
    raw["source_map"] = map_entry(256);
    deserialize_module_text(&raw.to_string()).unwrap();
    raw["source_map"] = map_entry(257);
    expect_refused(&raw);

    // History parameters, assumptions, and losses: 256 accepted, 257 refused.
    for field in ["assumptions", "losses"] {
        let mut raw = base_module_json();
        let mut entry = history("h1".into());
        entry[field] =
            serde_json::Value::Array((0..256).map(|_| serde_json::json!("noted")).collect());
        raw["history"] = serde_json::json!([entry.clone()]);
        deserialize_module_text(&raw.to_string()).unwrap();
        entry[field] =
            serde_json::Value::Array((0..257).map(|_| serde_json::json!("noted")).collect());
        raw["history"] = serde_json::json!([entry]);
        expect_refused(&raw);
    }
    let mut raw = base_module_json();
    let mut entry = history("h1".into());
    let parameters_at = |count: usize| {
        serde_json::Value::Object(
            (0..count)
                .map(|index| (format!("p{index}"), serde_json::json!(1)))
                .collect(),
        )
    };
    entry["parameters"] = parameters_at(256);
    raw["history"] = serde_json::json!([entry.clone()]);
    deserialize_module_text(&raw.to_string()).unwrap();
    entry["parameters"] = parameters_at(257);
    raw["history"] = serde_json::json!([entry]);
    expect_refused(&raw);
}

/// Module level record counts: each list is refused one element past its
/// stated maximum and accepted exactly at it. The two quarter million entry
/// lists are exercised through the count that matters, history and extensions
/// through their exact boundaries.
#[test]
fn module_record_counts_are_bounded() {
    // History: 65,536 accepted, 65,537 refused.
    let mut raw = base_module_json();
    let history_at = |count: usize| {
        serde_json::Value::Array(
            (0..count)
                .map(|index| {
                    serde_json::json!({
                        "id": format!("h{index}"), "kind": "parse", "name": "parse_matpower"
                    })
                })
                .collect(),
        )
    };
    raw["history"] = history_at(65_536);
    deserialize_module_text(&raw.to_string()).unwrap();
    raw["history"] = history_at(65_537);
    expect_refused(&raw);

    // Extensions: 4,096 accepted; a document declaring 2,000,000 keys is
    // refused at the 4,097th key, before the map is retained.
    let mut raw = base_module_json();
    let extensions_at = |count: usize| {
        serde_json::Value::Object(
            (0..count)
                .map(|index| (format!("org.example.k{index}"), serde_json::json!(1)))
                .collect(),
        )
    };
    raw["extensions"] = extensions_at(4_096);
    deserialize_module_text(&raw.to_string()).unwrap();
    raw["extensions"] = extensions_at(2_000_000);
    expect_refused(&raw);

    // Sources and diagnostics share one maximum; refusal one past it.
    let mut raw = base_module_json();
    raw["sources"] = serde_json::Value::Array(
        (0..262_145)
            .map(|index| {
                serde_json::json!({
                    "id": format!("s{index}"), "name": "case.m", "byte_length": 64
                })
            })
            .collect(),
    );
    expect_refused(&raw);
    let mut raw = base_module_json();
    raw["diagnostics"] = serde_json::Value::Array(
        (0..262_145)
            .map(|index| {
                serde_json::json!({
                    "id": format!("d{index}"), "severity": "note",
                    "code": "READ.TEST.NOTE", "message": "kept"
                })
            })
            .collect(),
    );
    expect_refused(&raw);
    let mut raw = base_module_json();
    raw["source_map"] = serde_json::Value::Array(
        (0..262_145)
            .map(|_| {
                serde_json::json!({
                    "target": "/buses/0/vm", "relation": "defaulted", "spans": []
                })
            })
            .collect(),
    );
    expect_refused(&raw);
}

#[test]
fn a_nested_network_that_fails_validation_is_refused_per_kind() {
    // A rogue branch inside a calculation instance is refused at read.
    let rogue_network_json = |mut net: powerio_tx::BalancedNetwork| {
        let mut rogue = net.branches()[0].clone();
        rogue.to = powerio_tx::BusId(9_999);
        net.branches_mut().push(rogue);
        net
    };

    // ac_scuc_instance and ac_scuc_solution, from the GOC3 fixture.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../powerio-prob/tests/data/goc3_small.json"
    );
    let text = std::fs::read_to_string(path).unwrap();
    let module = powerio::parse_with_options(
        powerio_core::Source::from_memory("goc3_small.json", text.into_bytes()).unwrap(),
        &powerio::ParseOptions::default()
            .format("goc3-json")
            .unwrap(),
    )
    .unwrap();
    let PioValue::AcScucInstance(instance) = module.value() else {
        panic!("GO Challenge 3 problem did not produce powerio.AcScucInstance");
    };
    let broken_instance = powerio_prob::AcScucInstance::new(
        rogue_network_json(instance.network().clone()),
        instance.inputs().clone(),
    );
    if let Ok(broken) = broken_instance {
        let doc = serialize_module_text(&powerio_core::PioModule::new(
            powerio::PioValue::AcScucInstance(broken),
        ))
        .unwrap();
        let error = deserialize_module_text(&doc).unwrap_err();
        assert!(error.to_string().contains("fails validation"), "{error}");
    }
}

#[test]
fn a_decoded_network_that_fails_validation_is_refused() {
    // A branch naming an undeclared bus passes document decoding but fails the
    // model's own validation; the stored read refuses it instead of yielding
    // the value.
    let mut net = small_network();
    let mut rogue = net.branches()[0].clone();
    rogue.to = powerio_tx::BusId(9_999);
    net.branches_mut().push(rogue);
    let mut raw: serde_json::Value = serde_json::from_str(
        &serialize_module_text(&powerio_core::PioModule::new(
            powerio::PioValue::BalancedNetwork(net),
        ))
        .unwrap(),
    )
    .unwrap();
    let _ = &mut raw;
    let error = deserialize_module_text(&raw.to_string()).unwrap_err();
    assert!(
        error.to_string().contains("fails validation"),
        "unexpected refusal: {error}"
    );
}

/// Diagnostics a reader raises carry no id, so the writer mints one each.
/// Minting must stay linear: a six figure list encodes well within the
/// ceiling the decode side already holds.
#[test]
fn six_figure_unidentified_diagnostics_encode_within_the_ceiling() {
    const COUNT: usize = 131_072;
    let mut module = PioModule::new(PioValue::BalancedNetwork(small_network()));
    let code = powerio_core::DiagnosticCode::new("READ.TEST.NOTE").unwrap();
    for _ in 0..COUNT {
        module
            .add_diagnostic(powerio_core::Diagnostic::new(
                code.clone(),
                powerio_core::DiagnosticSeverity::Note,
                "kept",
            ))
            .unwrap();
    }

    let started = std::time::Instant::now();
    let text = serialize_module_text(&module).unwrap();
    let elapsed = started.elapsed();
    // Keep the matching encode check resilient to shared-runner variance while
    // still catching non-linear behavior at this input size.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "encode took {elapsed:?}"
    );
    let back = deserialize_module_text(&text).unwrap();
    assert_eq!(back.diagnostics().len(), COUNT);
    assert_eq!(serialize_module_text(&back).unwrap(), text);
}

/// Four wire and two wire bus counts of the BMOPF feeder in #544: 27,239
/// buses carrying 106,038 terminals.
const FOUR_WIRE_BUSES: usize = 25_780;
const TWO_WIRE_BUSES: usize = 1_459;
const FEEDER_TERMINALS: usize = 4 * FOUR_WIRE_BUSES + 2 * TWO_WIRE_BUSES;

fn diagonal(n: usize, value: f64) -> Vec<Vec<f64>> {
    (0..n)
        .map(|row| {
            (0..n)
                .map(|col| if row == col { value } else { 0.0 })
                .collect()
        })
        .collect()
}

/// A radial feeder of the #544 dimensions, generated rather than committed:
/// the four wire buses form a binary tree under a three phase source, and
/// each two wire bus hangs off one of them through phase 1 and the neutral.
fn large_feeder() -> MulticonductorNetwork {
    let four: Vec<String> = ["1", "2", "3", "4"].map(String::from).to_vec();
    let two: Vec<String> = ["1", "4"].map(String::from).to_vec();
    let mut net = MulticonductorNetwork::named("large-feeder");
    net.line_codes_mut()
        .push(DistLineCode::new("lc4", diagonal(4, 0.2), diagonal(4, 0.4)));
    net.line_codes_mut()
        .push(DistLineCode::new("lc2", diagonal(2, 0.3), diagonal(2, 0.5)));
    for index in 0..FOUR_WIRE_BUSES {
        net.buses_mut()
            .push(DistBus::new(format!("b{index}"), four.clone()));
        if index > 0 {
            net.lines_mut().push(DistLine::new(
                format!("l{index}"),
                format!("b{}", (index - 1) / 2),
                format!("b{index}"),
                four.clone(),
                four.clone(),
                "lc4",
                100.0,
            ));
        }
    }
    for index in 0..TWO_WIRE_BUSES {
        net.buses_mut()
            .push(DistBus::new(format!("s{index}"), two.clone()));
        net.lines_mut().push(DistLine::new(
            format!("t{index}"),
            format!("b{}", FOUR_WIRE_BUSES - 1 - index),
            format!("s{index}"),
            two.clone(),
            two.clone(),
            "lc2",
            30.0,
        ));
    }
    net.sources_mut().push(VoltageSource::new(
        "vs",
        "b0",
        four[..3].to_vec(),
        vec![2_400.0; 3],
        vec![0.0, -2.094, 2.094],
    ));
    net
}

/// One distinct, non integral value per feeder terminal.
fn terminal_values(offset: f64, step: f64) -> Vec<f64> {
    (0..FEEDER_TERMINALS)
        .map(|index| offset + step * index as f64)
        .collect()
}

/// #544: a multiconductor solution whose terminal count passes the generic
/// stored collection bound round trips through `serialize` and `deserialize`,
/// optional current and power columns included.
#[test]
fn a_solution_over_106038_terminals_round_trips() {
    let network = large_feeder();
    let terminals: Vec<(String, String)> = network
        .buses()
        .iter()
        .flat_map(|bus| {
            bus.terminals
                .iter()
                .map(|terminal| (bus.id.clone(), terminal.clone()))
        })
        .collect();
    assert_eq!(network.buses().len(), 27_239);
    assert_eq!(terminals.len(), 106_038);

    let instance = Arc::new(McAcPfInstance::from_network(network).unwrap());
    let solution = McAcPfSolution::new(
        instance,
        Termination::Converged,
        terminal_values(2_400.0, 1.0e-3),
        terminal_values(-2.094, 1.0e-5),
        vec![1.0e5, 0.9e5, 1.1e5],
    )
    .unwrap()
    .with_terminal_currents(terminal_values(0.25, 0.5))
    .unwrap()
    .with_terminal_powers(terminal_values(-1.0e4, 0.125))
    .unwrap();

    let text =
        serialize_module_text(&PioModule::new(PioValue::McAcPfSolution(solution.clone()))).unwrap();
    let back = deserialize_module_text(&text).unwrap();
    let PioValue::McAcPfSolution(decoded) = back.value() else {
        panic!("wrong kind: {}", back.value().type_name());
    };
    for (bus, terminal) in &terminals {
        assert_eq!(
            decoded.terminal_voltage_magnitude(bus, terminal),
            solution.terminal_voltage_magnitude(bus, terminal)
        );
        assert_eq!(
            decoded.terminal_voltage_angle(bus, terminal),
            solution.terminal_voltage_angle(bus, terminal)
        );
        assert_eq!(
            decoded.terminal_current_magnitude(bus, terminal),
            solution.terminal_current_magnitude(bus, terminal)
        );
        assert_eq!(
            decoded.terminal_active_power(bus, terminal),
            solution.terminal_active_power(bus, terminal)
        );
    }
    let (bus, terminal) = terminals.last().unwrap();
    assert_eq!(
        decoded.terminal_active_power(bus, terminal),
        Some(-1.0e4 + 0.125 * (FEEDER_TERMINALS - 1) as f64)
    );
    assert_eq!(
        decoded.source_active_injections(),
        solution.source_active_injections()
    );
}

fn repeated_time_points(count: usize) -> Vec<TimePoint> {
    (0..count)
        .map(|index| TimePoint::new(format!("h{index}"), None).unwrap())
        .collect()
}

fn nested_balanced_network_series_module(count: usize) -> PioModule<PioValue> {
    let series = TimeSeries::new(repeated_time_points(count), vec![small_network(); count])
        .expect("time points and values have equal lengths");
    let scenarios = ScenarioSet::new(vec![Scenario::new(
        ScenarioId::new("base").unwrap(),
        Some(1.0),
        series,
    )])
    .unwrap();
    PioModule::new(PioValue::from(scenarios))
}

fn small_multiconductor_network() -> MulticonductorNetwork {
    let terminals: Vec<String> = ["1", "2", "3", "n"].map(String::from).to_vec();
    let mut network = MulticonductorNetwork::named("stored-nested-series");
    network
        .line_codes_mut()
        .push(DistLineCode::new("lc", diagonal(4, 0.2), diagonal(4, 0.4)));
    network
        .buses_mut()
        .push(DistBus::new("b0", terminals.clone()));
    network.sources_mut().push(VoltageSource::new(
        "vs",
        "b0",
        terminals[..3].to_vec(),
        vec![2_400.0; 3],
        vec![0.0, -2.094, 2.094],
    ));
    network
}

fn nested_multiconductor_network_series_module(count: usize) -> PioModule<PioValue> {
    let network = small_multiconductor_network();
    let series = TimeSeries::new(repeated_time_points(count), vec![network; count])
        .expect("time points and values have equal lengths");
    let scenarios = ScenarioSet::new(vec![Scenario::new(
        ScenarioId::new("base").unwrap(),
        Some(1.0),
        series,
    )])
    .unwrap();
    PioModule::new(PioValue::from(scenarios))
}

fn assert_emit_record_cap(module: &PioModule<PioValue>) {
    let error = serialize_module_text(module)
        .expect_err("the writer must reject a nested series over the reader's bound");
    assert_eq!(
        error.info().map(|info| info.code),
        Some("EMIT.MODULE.RECORD_CAP"),
        "unexpected error: {error}"
    );
}

#[test]
fn nested_balanced_network_series_observes_the_storage_bound() {
    const LIMIT: usize = 65_536;
    let module = nested_balanced_network_series_module(LIMIT);
    let text = serialize_module_text(&module).expect("the exact limit is accepted");
    deserialize_module_text(&text).expect("the reader accepts the exact limit");

    let oversized = nested_balanced_network_series_module(LIMIT + 1);
    assert_emit_record_cap(&oversized);
}

#[test]
fn nested_multiconductor_network_series_observes_the_storage_bound() {
    const LIMIT: usize = 65_536;
    let module = nested_multiconductor_network_series_module(LIMIT);
    let text = serialize_module_text(&module).expect("the exact limit is accepted");
    deserialize_module_text(&text).expect("the reader accepts the exact limit");

    let oversized = nested_multiconductor_network_series_module(LIMIT + 1);
    assert_emit_record_cap(&oversized);
}

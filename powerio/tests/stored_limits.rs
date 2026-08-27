//! Decode time limits on the `.pio.json` version 1 wire and the 0.9 upgrade:
//! hostile documents are refused at their stated bounds, and record decode
//! scales past six figure counts.

use powerio::stored::{read_module, write_module};
use powerio::{BalancedNetwork, PioValue};
use powerio_core::{PioModule, TimePoint};
use powerio_prob::BALANCED_STATE_QUANTITIES;
use powerio_tx::{Bus, BusId, BusType, Generator, Load, Switch};

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
    let mut raw: serde_json::Value = serde_json::from_str(&write_module(&module).unwrap()).unwrap();
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
    let module = read_module(&text).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(module.sources().len(), COUNT);
    assert_eq!(module.diagnostics().len(), COUNT);
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
    let series = powerio_prob::BalancedStateBuilder::new(network, time_points)
        .dense_by_name("bus_voltage_magnitude", vec![1.0, 1.01, 0.99, 1.02])
        .unwrap()
        .build()
        .unwrap();
    let module = PioModule::new(PioValue::BalancedOperatingPointTimeSeries(series));
    serde_json::from_str(&write_module(&module).unwrap()).unwrap()
}

/// DESER-004: a stored quantity's values bind to the identities the document
/// states; a permuted identity list is refused rather than rebound.
#[test]
fn a_permuted_identity_list_is_refused() {
    let mut raw = series_module_json();
    let quantities = &mut raw["value"]["data"]["quantities"];
    assert_eq!(
        quantities["bus_voltage_magnitude"]["identities"],
        serde_json::json!(["1", "2"])
    );
    quantities["bus_voltage_magnitude"]["identities"] = serde_json::json!(["2", "1"]);
    let error = read_module(&raw.to_string()).unwrap_err();
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
    let module = read_module(&series_module_json().to_string()).unwrap();
    let PioValue::BalancedOperatingPointTimeSeries(series) = module.value() else {
        panic!("wrong kind");
    };
    assert_eq!(
        series.values()[0].bus_voltage_magnitude(BusId(1)),
        Some(1.0)
    );
    assert_eq!(
        series.values()[0].bus_voltage_magnitude(BusId(2)),
        Some(1.01)
    );
    assert_eq!(
        series.values()[1].bus_voltage_magnitude(BusId(1)),
        Some(0.99)
    );
}

/// DESER-005: the writer's vocabulary is the reader's, from one definition,
/// and `switch_closed` survives the round trip.
#[test]
fn the_switch_state_survives_the_round_trip() {
    let mut network = small_network();
    network
        .switches_mut()
        .push(Switch::new(BusId(1), BusId(2), true));
    let time_points = vec![
        TimePoint::new("h0", None).unwrap(),
        TimePoint::new("h1", None).unwrap(),
    ];
    let series = powerio_prob::BalancedStateBuilder::new(network, time_points)
        .dense_by_name("switch_closed", vec![1.0, 0.0])
        .unwrap()
        .build()
        .unwrap();
    let module = PioModule::new(PioValue::BalancedOperatingPointTimeSeries(series));
    let text = write_module(&module).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        raw["value"]["data"]["quantities"]
            .as_object()
            .unwrap()
            .contains_key("switch_closed"),
        "the stored document must carry the switch quantity"
    );
    let back = read_module(&text).unwrap();
    let PioValue::BalancedOperatingPointTimeSeries(series) = back.value() else {
        panic!("wrong kind");
    };
    assert_eq!(series.values()[0].switch_closed("switches:0"), Some(true));
    assert_eq!(series.values()[1].switch_closed("switches:0"), Some(false));
    assert_eq!(write_module(&back).unwrap(), text);
}

/// The exported vocabulary is exactly the set the builder accepts.
#[test]
fn the_exported_vocabulary_matches_what_the_builder_accepts() {
    let builder = powerio_prob::BalancedStateBuilder::new(
        small_network(),
        vec![TimePoint::new("h0", None).unwrap()],
    );
    for name in BALANCED_STATE_QUANTITIES {
        assert!(builder.identity_order(name).is_ok(), "{name} not accepted");
    }
    assert!(builder.identity_order("bus_voltage").is_err());
    assert_eq!(BALANCED_STATE_QUANTITIES.len(), 14);
}

fn base_module_json() -> serde_json::Value {
    let module = PioModule::new(PioValue::BalancedNetwork(small_network()));
    serde_json::from_str(&write_module(&module).unwrap()).unwrap()
}

fn expect_refused(raw: &serde_json::Value) {
    let error = read_module(&raw.to_string()).unwrap_err();
    assert_eq!(
        error.info().map(|info| info.code),
        Some("READ.MODULE.INVALID"),
        "{error}"
    );
}

/// DESER-007: every bounded sequence, map, and string on the record wire is
/// refused one element or byte past its limit and accepted exactly at it.
#[test]
// One deliberate sweep over the whole limit table; splitting it would
// scatter the boundary pairs.
#[allow(clippy::too_many_lines)]
fn record_wire_bounds_refuse_past_and_accept_at_each_limit() {
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
    read_module(&raw.to_string()).unwrap();
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
    let module = read_module(&raw.to_string()).unwrap();
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
    read_module(&raw.to_string()).unwrap();
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
    read_module(&raw.to_string()).unwrap();
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
    read_module(&raw.to_string()).unwrap();
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
    read_module(&raw.to_string()).unwrap();
    raw["source_map"] = map_entry(257);
    expect_refused(&raw);

    // History parameters, assumptions, and losses: 256 accepted, 257 refused.
    for field in ["assumptions", "losses"] {
        let mut raw = base_module_json();
        let mut entry = history("h1".into());
        entry[field] =
            serde_json::Value::Array((0..256).map(|_| serde_json::json!("noted")).collect());
        raw["history"] = serde_json::json!([entry.clone()]);
        read_module(&raw.to_string()).unwrap();
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
    read_module(&raw.to_string()).unwrap();
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
    read_module(&raw.to_string()).unwrap();
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
    read_module(&raw.to_string()).unwrap();
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

fn legacy_package_json() -> serde_json::Value {
    // The released 0.9 stored layout, spelled at the JSON level: the frozen
    // decoder is crate private, so the wire shape is the test's fixture.
    let network = serde_json::to_value(small_network()).unwrap();
    serde_json::json!({
        "powerio_version": "0.9.0",
        "producer": {"tool": "powerio", "version": "0.9.0"},
        "model_kind": "balanced",
        "model": {"kind": "balanced", "balanced_network": network},
        "origin": {"kind": "in_memory"},
        "validation": {"status": "ok", "counts": {}}
    })
}

#[test]
fn legacy_record_lists_are_held_to_the_module_maxima() {
    // A released 0.9 package one element past a module maximum is refused by
    // the same bound the version 1 wire applies, before the list is retained.
    let mut raw = legacy_package_json();
    raw["diagnostics"] = serde_json::Value::Array(
        (0..262_145)
            .map(|index| {
                serde_json::json!({
                    "code": "READ.CASE.NOTE",
                    "severity": "info",
                    "stage": "parse",
                    "message": format!("finding {index}")
                })
            })
            .collect(),
    );
    let error = read_module(&raw.to_string()).unwrap_err();
    let code = error
        .diagnostics()
        .first()
        .map(|d| d.code().to_owned())
        .unwrap_or_default();
    // The legacy decode wraps the bound refusal in its own reader code; the
    // message names the list that hit its maximum.
    assert!(
        code.contains("TOO_LARGE") || code.contains("MALFORMED") || code.contains("INVALID"),
        "refusal code: {code}"
    );
    assert!(error.to_string().contains("diagnostics"), "{error}");

    // An accepted upgrade satisfies the write/read law the fuzz target
    // asserts: what read_module accepts, write_module emits and read_module
    // accepts again.
    let module = read_module(&legacy_package_json().to_string()).unwrap();
    let written = write_module(&module).unwrap();
    read_module(&written).expect("written module reads back");
}

#[test]
fn a_decoded_network_that_fails_validation_is_refused() {
    // A branch naming an undeclared bus passes the wire decode but fails the
    // model's own validation; the stored read refuses it instead of yielding
    // the value.
    let mut net = small_network();
    let mut rogue = net.branches()[0].clone();
    rogue.to = powerio_tx::BusId(9_999);
    net.branches_mut().push(rogue);
    let mut raw: serde_json::Value = serde_json::from_str(
        &write_module(&powerio_core::PioModule::new(
            powerio::PioValue::BalancedNetwork(net),
        ))
        .unwrap(),
    )
    .unwrap();
    let _ = &mut raw;
    let error = read_module(&raw.to_string()).unwrap_err();
    assert!(
        error.to_string().contains("fails validation"),
        "unexpected refusal: {error}"
    );
}

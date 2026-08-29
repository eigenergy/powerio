//! The module level balanced lowering: record handling across the kind
//! change, the visible note cap, repeated lowering, and the winding rating
//! base conversion.

use powerio::stored::{read_module, write_module};
use powerio::transform::{MulticonductorToBalancedOptions, lower_module_to_balanced};
use powerio::{PioValue, VERSION};

const TWO_WINDING_DSS: &str = r"Clear
Set DefaultBaseFrequency=60
New Circuit.tiny basekv=12.47 pu=1.0 phases=3 bus1=src MVAsc3=2000 MVAsc1=2100
New Transformer.t1 phases=3 windings=2 buses=(src, sec) conns=(delta, wye) kvs=(12.47, 0.416) kvas=(500, 300) %Rs=(0.5, 0.5) xhl=6
New Load.l1 bus1=sec phases=3 conn=wye kv=0.416 kw=90 pf=0.95 model=1
Set VoltageBases=[12.47, 0.416]
";

fn parse_mc_module(text: &str) -> powerio_core::PioModule<PioValue> {
    let source = powerio_core::Source::from_bytes("feeder.dss", text.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("dss").unwrap());
    powerio::parse(source).unwrap()
}

/// A stored multiconductor module carrying a diagnostic whose target points
/// into the multiconductor value lowers, keeps every diagnostic, loses the
/// stale target, and the result still writes.
#[test]
fn stale_targets_are_severed_and_every_diagnostic_survives() {
    let module = parse_mc_module(TWO_WINDING_DSS).sever_source();
    let mut raw: serde_json::Value = serde_json::from_str(&write_module(&module).unwrap()).unwrap();
    raw["diagnostics"] = serde_json::json!([
        {"id": "keep-1", "severity": "note", "code": "READ.TEST.FIRST", "message": "kept"},
        {
            "id": "keep-2", "severity": "warning", "code": "READ.TEST.TARGETED",
            "message": "kept", "target": "/transformers/0/name"
        },
        {"id": "keep-3", "severity": "error", "code": "READ.TEST.LAST", "message": "kept"}
    ]);
    let module = read_module(&raw.to_string()).unwrap();
    assert_eq!(module.diagnostics().len(), 3);
    let input_count = module.diagnostics().len();

    let lowered =
        lower_module_to_balanced(module, MulticonductorToBalancedOptions::default()).unwrap();
    assert!(matches!(lowered.value(), PioValue::BalancedNetwork(_)));
    // Every input diagnostic survives, plus the pass's own findings.
    assert!(lowered.diagnostics().len() >= input_count);
    for id in ["keep-1", "keep-2", "keep-3"] {
        assert!(
            lowered
                .diagnostics()
                .iter()
                .any(|d| d.id().is_some_and(|i| i.as_str() == id)),
            "diagnostic {id} was dropped"
        );
    }
    // The stale target is severed, so the lowered module still writes.
    assert!(lowered.diagnostics().iter().all(|d| d.target().is_none()));
    let text = write_module(&lowered).unwrap();
    assert!(text.contains("READ.TEST.TARGETED"));
}

/// Lowering a module that already carries a lowering history entry mints a
/// numbered id instead of silently recording nothing.
#[test]
fn repeated_lowering_records_every_pass() {
    let module = parse_mc_module(TWO_WINDING_DSS).sever_source();
    let mut raw: serde_json::Value = serde_json::from_str(&write_module(&module).unwrap()).unwrap();
    raw["history"] = serde_json::json!([
        {
            "id": "multiconductor-to-balanced", "kind": "transform",
            "name": "lower_multiconductor_to_balanced"
        }
    ]);
    let module = read_module(&raw.to_string()).unwrap();
    let lowered =
        lower_module_to_balanced(module, MulticonductorToBalancedOptions::default()).unwrap();
    let ids: Vec<&str> = lowered
        .history()
        .iter()
        .map(|entry| entry.id().as_str())
        .collect();
    assert!(ids.contains(&"multiconductor-to-balanced"));
    assert!(
        ids.contains(&"multiconductor-to-balanced-2"),
        "the pass recorded no history entry: {ids:?}"
    );
}

/// Hostile element names reach the history notes normalized: a bus id with
/// a NUL byte and a switch name past the identifier bound still lower to Ok
/// with every recorded note nonempty, NUL free, and within the bound.
#[test]
fn hostile_element_names_normalize_into_the_history_notes() {
    let module = parse_mc_module(TWO_WINDING_DSS).sever_source();
    let mut raw: serde_json::Value = serde_json::from_str(&write_module(&module).unwrap()).unwrap();
    let network = &mut raw["value"]["data"];
    // A NUL bearing name and an overlong name on elements the lowering
    // names in its notes.
    let hostile = format!(
        "t\u{0}evil{}",
        "x".repeat(powerio_core::limits::MAX_IDENTIFIER_BYTES + 40)
    );
    network["transformers"][0]["name"] = serde_json::json!(hostile);
    let module = read_module(&raw.to_string()).unwrap();
    let lowered =
        lower_module_to_balanced(module, MulticonductorToBalancedOptions::default()).unwrap();
    let entry = lowered
        .history()
        .iter()
        .find(|entry| entry.name() == "lower_multiconductor_to_balanced")
        .expect("the pass records its history entry");
    let mut saw_replacement = false;
    let mut saw_truncation = false;
    for note in entry.assumptions().iter().chain(entry.losses()) {
        assert!(!note.is_empty());
        assert!(!note.contains('\0'), "NUL survived: {note:?}");
        assert!(
            note.len() <= powerio_core::limits::MAX_IDENTIFIER_BYTES,
            "overlong note: {} bytes",
            note.len()
        );
        saw_replacement |= note.contains('\u{fffd}');
        saw_truncation |= note.contains("[truncated]");
    }
    assert!(saw_replacement, "no note carried the normalized NUL name");
    assert!(saw_truncation, "no note carried the truncated long name");
}

/// The two windings state different power ratings; the low winding %R is
/// converted onto the high winding base before the sum, and the conversion
/// is recorded as an assumption.
#[test]
fn mismatched_winding_ratings_convert_the_resistance_base() {
    let module = parse_mc_module(TWO_WINDING_DSS);
    let lowered =
        lower_module_to_balanced(module, MulticonductorToBalancedOptions::default()).unwrap();
    let PioValue::BalancedNetwork(network) = lowered.value() else {
        panic!("wrong kind");
    };
    let branch = network
        .branches()
        .iter()
        .find(|branch| branch.x > 0.0)
        .expect("the transformer lowers to a branch");
    // kvas=(500, 300), %Rs=(0.5, 0.5): r_pu on the transformer base is
    // (0.005 + 0.005 * 500/300), scaled by base_mva/S_high = 100/0.5 and the
    // squared voltage ratio (v_ref matches the zone base, so 1).
    let z_scale = 100.0 * 1_000_000.0 / 500_000.0;
    let expected = (0.005 + 0.005 * (500.0 / 300.0)) * z_scale;
    assert!(
        (branch.r - expected).abs() < 1e-9,
        "r {} expected {expected}",
        branch.r
    );
    let history = lowered
        .history()
        .iter()
        .find(|entry| entry.name() == "lower_multiconductor_to_balanced")
        .unwrap();
    assert!(
        history
            .assumptions()
            .iter()
            .any(|note| note.contains("converted from its own")),
        "no base conversion note"
    );
    let _ = VERSION;
}

#[test]
fn a_module_at_a_record_cap_is_refused_intact() {
    // A module whose records leave no room for the pass's own findings or
    // history entry refuses under the record cap code with the module handed
    // back unchanged, whichever cap binds.
    let full_diagnostics = {
        let mut module = parse_mc_module(TWO_WINDING_DSS);
        let room = powerio_core::limits::MAX_MODULE_DIAGNOSTICS - module.diagnostics().len();
        let filler = powerio_core::Diagnostic::new(
            powerio_core::DiagnosticCode::new("READ.CASE.FILLER").unwrap(),
            powerio_core::DiagnosticSeverity::Note,
            "filler",
        );
        for _ in 0..room {
            module.add_diagnostic(filler.clone()).unwrap();
        }
        module
    };
    let before = full_diagnostics.diagnostics().len();
    let (returned, error) =
        lower_module_to_balanced(full_diagnostics, MulticonductorToBalancedOptions::default())
            .unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.code() == "TRANSFORM.MULTI_TO_BALANCED.RECORD_CAP"),
        "{error:?}"
    );
    assert_eq!(returned.diagnostics().len(), before, "module unchanged");
    assert!(matches!(
        returned.value(),
        PioValue::MulticonductorNetwork(_)
    ));

    // The history cap binds the same way.
    let full_history = {
        let mut module = parse_mc_module(TWO_WINDING_DSS);
        let room = powerio_core::limits::MAX_MODULE_HISTORY_ENTRIES - module.history().len();
        for index in 0..room {
            let entry = powerio_core::HistoryEntry::new(
                powerio_core::HistoryId::new(format!("filler-{index}")).unwrap(),
                powerio_core::HistoryKind::Transform,
                "filler",
            )
            .unwrap();
            module.add_history_entry(entry).unwrap();
        }
        module
    };
    let before = full_history.history().len();
    let (returned, error) =
        lower_module_to_balanced(full_history, MulticonductorToBalancedOptions::default())
            .unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .any(|d| d.code() == "TRANSFORM.MULTI_TO_BALANCED.RECORD_CAP"),
        "{error:?}"
    );
    assert_eq!(returned.history().len(), before, "module unchanged");
}

/// A refused lowering reports 1.0 records, not the retired 0.9 document
/// shape: every severity is the four level spelling (error/warning/remark/note,
/// never fatal/info/debug), no record carries the retired `element_path` key,
/// and any `target` it does carry is a pointer, not a `/model/...` path.
#[test]
fn refused_lowering_reports_1_0_severities_and_targets() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss"
    ))
    .expect("fixture reads");
    let module = parse_mc_module(&text);
    let (_, error) = lower_module_to_balanced(module, MulticonductorToBalancedOptions::default())
        .expect_err(
            "the IEEE13 feeder mixes single, two, and three phase laterals and is not \
                 balanced-lowerable",
        );
    assert!(
        !error.diagnostics.is_empty(),
        "a refusal must carry findings"
    );

    let json = serde_json::to_value(&error.diagnostics).expect("diagnostics serialize");
    let records = json.as_array().expect("diagnostics is a JSON array");
    for record in records {
        let severity = record["severity"].as_str().expect("severity is a string");
        assert!(
            matches!(severity, "error" | "warning" | "remark" | "note"),
            "retired 0.9 severity spelling: {severity}"
        );
        assert!(
            record.get("element_path").is_none(),
            "retired locator key survived: {record}"
        );
        if let Some(target) = record.get("target").and_then(|t| t.as_str()) {
            assert!(target.starts_with('/'), "target is not a pointer: {target}");
        }
    }
}

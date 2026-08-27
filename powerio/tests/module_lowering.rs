//! The module level balanced lowering: record handling across the kind
//! change, the visible note cap, repeated lowering, and the winding rating
//! base conversion.

use powerio::package::{MulticonductorToBalancedOptions, lower_module_to_balanced};
use powerio::stored::{read_module, write_module};
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

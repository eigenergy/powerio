//! Schema lock for the `powerio-json` snapshot.
//!
//! The C ABI and the Julia bridge ride on this transport, and `PIO_ABI_VERSION`
//! ties the snapshot schema to the ABI version (`powerio-capi/include/powerio.h`).
//! So an accidental serde rename, retype, or removed default is a forced C ABI
//! break, not a quiet change. These tests pin three things:
//!   1. a committed v4-vintage snapshot keeps parsing under the current `from_json`
//!      (a rename/retype of any present field breaks parsing the frozen bytes),
//!   2. generator `caps` stays a name-keyed object on the wire (a length-exact
//!      array would force a v5 the day `GEN_EXTRA_KEYS` grows), and
//!   3. `deny_unknown_fields` stays off and `caps` keeps its `serde(default)`, so a
//!      newer snapshot's extra key and an older snapshot that predates `caps` both
//!      parse.

use std::path::Path;

use powerio::{Branch, Bus, BusId, BusType, Extras, GenCaps, Generator, Network, SourceFormat};

/// A v4-vintage snapshot, written by `powerio convert case30.m --to powerio-json`.
/// Regenerate ONLY on a deliberate schema change, and then bump `PIO_ABI_VERSION`.
fn golden_v4() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/powerio-json/case30_v4.json");
    std::fs::read_to_string(&path).expect("the committed v4 golden snapshot must exist")
}

#[test]
fn golden_v4_snapshot_still_parses() {
    let text = golden_v4();
    let net = Network::from_json(&text).expect("the v4 golden must still parse");
    assert_eq!(net.buses.len(), 30);
    assert_eq!(net.branches.len(), 41);
    assert_eq!(net.generators.len(), 6);
    assert_eq!(net.loads.len(), 20);
    assert_eq!(net.shunts.len(), 2);
    assert_eq!(net.base_mva.to_bits(), 100.0_f64.to_bits());
    assert!(
        net.generators.iter().all(|g| g.cost.is_some()),
        "case30 gen costs must survive the round trip"
    );

    // The freeze-critical wire form: caps is a name-keyed object, never an array.
    assert!(
        text.contains(r#""caps":{"#) && !text.contains(r#""caps":["#),
        "generator caps must serialize as an object"
    );

    // Re-serialize and read back: the schema round-trips without drift, and the
    // CURRENT serializer (not just the frozen golden bytes) still emits caps as a
    // name-keyed object, so a writer-side wire-form regression fails here too.
    let again = net.to_json().unwrap();
    assert!(
        again.contains(r#""caps":{"#) && !again.contains(r#""caps":["#),
        "the live serializer must still emit caps as an object"
    );
    let back = Network::from_json(&again).unwrap();
    assert_eq!(back.buses.len(), net.buses.len());
    assert_eq!(back.generators.len(), net.generators.len());
}

#[test]
fn snapshot_ignores_unknown_fields_and_defaults_omitted_caps() {
    // Build a minimal valid net, drop it to JSON, then mutate the JSON so it looks
    // like a snapshot from a different schema vintage and confirm it still parses.
    let net = small_net();
    let mut v: serde_json::Value = serde_json::from_str(&net.to_json().unwrap()).unwrap();

    // (a) an unknown future top-level field is ignored (deny_unknown_fields off).
    v["future_field_v5"] = serde_json::json!("ignored");
    // (b) a generator that omits caps entirely still parses (caps defaults empty).
    v["generators"][0].as_object_mut().unwrap().remove("caps");

    let text = serde_json::to_string(&v).unwrap();
    let parsed =
        Network::from_json(&text).expect("an unknown field and an omitted caps must still parse");
    assert_eq!(parsed.generators.len(), 1);
    assert!(
        !parsed.generators[0].has_caps(),
        "an omitted caps field defaults to the empty set"
    );
}

fn small_net() -> Network {
    let bus = |id, kind| Bus {
        id: BusId(id),
        kind,
        vm: 1.0,
        va: 0.0,
        base_kv: 230.0,
        vmax: 1.1,
        vmin: 0.9,
        evhi: None,
        evlo: None,
        area: 1,
        zone: 1,
        name: None,
        extras: Extras::new(),
    };
    // Length-agnostic: GEN_EXTRA_KEYS is pub(crate), so the integration crate
    // can't write `[None; GEN_EXTRA_KEYS.len()]`; `GenCaps::default()` tracks the
    // array length so this test still compiles when a capability column is added.
    let mut caps: GenCaps = GenCaps::default();
    caps[8] = Some(1.5); // ramp_30
    let g = Generator {
        bus: BusId(1),
        pg: 10.0,
        qg: 0.0,
        pmax: 100.0,
        pmin: 0.0,
        qmax: 50.0,
        qmin: -50.0,
        vg: 1.0,
        mbase: 100.0,
        in_service: true,
        cost: None,
        caps,
        regulated_bus: None,
    };
    let branch = Branch {
        from: BusId(1),
        to: BusId(2),
        r: 0.01,
        x: 0.1,
        b: 0.0,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap: 0.0,
        shift: 0.0,
        in_service: true,
        angmin: -360.0,
        angmax: 360.0,
        control: None,
        extras: Extras::new(),
    };
    Network {
        name: "schema_lock".into(),
        base_mva: 100.0,
        base_frequency: 60.0,
        buses: vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        loads: vec![],
        shunts: vec![],
        branches: vec![branch],
        generators: vec![g],
        storage: vec![],
        hvdc: vec![],
        transformers_3w: vec![],
        areas: vec![],
        solver: None,
        source_format: SourceFormat::InMemory,
        source: None,
    }
}

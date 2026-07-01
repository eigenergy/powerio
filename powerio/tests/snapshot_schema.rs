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

use powerio::{Branch, Bus, BusId, BusType, GenCaps, Generator, Network, SourceFormat};

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
    let bus = |id, kind| Bus::new(BusId(id), kind, 230.0);
    // Length-agnostic: GEN_EXTRA_KEYS is pub(crate), so the integration crate
    // can't write `[None; GEN_EXTRA_KEYS.len()]`; `GenCaps::default()` tracks the
    // array length so this test still compiles when a capability column is added.
    let mut caps: GenCaps = GenCaps::default();
    caps[8] = Some(1.5); // ramp_30
    let mut g = Generator::new(BusId(1));
    g.pg = 10.0;
    g.pmax = 100.0;
    g.qmax = 50.0;
    g.qmin = -50.0;
    g.mbase = 100.0;
    g.caps = caps;
    let branch = Branch::new(BusId(1), BusId(2), 0.01, 0.1);
    let mut net = Network::new("schema_lock", 100.0);
    net.buses = vec![bus(1, BusType::Ref), bus(2, BusType::Pq)];
    net.branches = vec![branch];
    net.generators = vec![g];
    net.source_format = SourceFormat::InMemory;
    net
}

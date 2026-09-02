//! Schema checks for the balanced network JSON document.
//!
//! Version 1 writes canonical source format names and stable component IDs. It
//! does not carry the prerelease C ABI snapshots forward as a second wire
//! format. Additive fields keep their Serde defaults so a reader can accept a
//! version 1 document written by a newer PowerIO build.

use std::path::Path;

use powerio_tx::{
    BalancedNetwork, Branch, Bus, BusId, BusType, CoordinateSpace, CoordsKind, GenCaps, Generator,
    GeoMeta, Location, SourceFormat, TerminalReference,
};

fn prerelease_snapshot() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/model-json/case30_v4.json");
    std::fs::read_to_string(&path).expect("the committed v4 golden snapshot must exist")
}

#[test]
fn prerelease_source_format_spelling_is_not_a_version_1_document() {
    let error = BalancedNetwork::from_json(&prerelease_snapshot())
        .expect_err("the prerelease enum spelling must not define a second wire format");
    assert!(error.to_string().contains("unknown variant `Matpower`"));
}

#[test]
fn snapshot_ignores_unknown_fields_and_defaults_omitted_caps() {
    // Build a minimal valid net, drop it to JSON, then mutate the JSON so it looks
    // like a snapshot from a different schema vintage and confirm it still parses.
    let net = small_net();
    let mut v: serde_json::Value = serde_json::from_str(&net.to_json().unwrap()).unwrap();

    // (a) an unknown future top-level field is ignored (deny_unknown_fields off).
    v["future_field_v5"] = serde_json::json!("ignored");
    // (b) a generator that omits additive fields still receives their released defaults.
    let generator = v["generators"][0].as_object_mut().unwrap();
    generator.remove("caps");
    generator.remove("voltage_regulation_on");
    generator.remove("regulating_terminal");

    let text = serde_json::to_string(&v).unwrap();
    let parsed = BalancedNetwork::from_json(&text)
        .expect("an unknown field and an omitted caps must still parse");
    assert_eq!(parsed.generators().len(), 1);
    assert!(
        !parsed.generators()[0].has_caps(),
        "an omitted caps field defaults to the empty set"
    );
    assert!(parsed.generators()[0].voltage_regulation_on);
    assert_eq!(parsed.generators()[0].regulating_terminal, None);
}

#[test]
fn generator_voltage_control_survives_snapshot_round_trip() {
    let mut net = small_net();
    let reference: TerminalReference = serde_json::from_value(serde_json::json!({
        "equipment": {
            "component_type": "load",
            "local_id": "remote-load"
        },
        "terminal": 1
    }))
    .unwrap();
    net.generators_mut()[0].voltage_regulation_on = false;
    net.generators_mut()[0].regulated_bus = Some(BusId(2));
    net.generators_mut()[0].regulating_terminal = Some(reference.clone());

    let parsed = BalancedNetwork::from_json(&net.to_json().unwrap()).unwrap();
    let generator = &parsed.generators()[0];
    assert!(!generator.voltage_regulation_on);
    assert_eq!(generator.regulated_bus, Some(BusId(2)));
    assert_eq!(generator.regulating_terminal, Some(reference));
}

fn small_net() -> BalancedNetwork {
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
    let mut net = BalancedNetwork::new("schema_lock", 100.0);
    *net.buses_mut() = vec![bus(1, BusType::Ref), bus(2, BusType::Pq)];
    *net.branches_mut() = vec![branch];
    *net.generators_mut() = vec![g];
    *net.source_format_mut() = SourceFormat::InMemory;
    net
}

#[test]
fn component_ids_survive_snapshot_roundtrip_and_are_assigned_when_absent() {
    let mut net = small_net();
    net.generators_mut()[0].uid = Some("gen-a".to_owned());

    let v: serde_json::Value = serde_json::from_str(&net.to_json().unwrap()).unwrap();
    assert_eq!(v["generators"][0]["uid"], serde_json::json!("gen-a"));
    let bus_uid = v["buses"][0]["uid"]
        .as_str()
        .expect("version 1 serialization assigns a stable component ID");
    assert!(!bus_uid.is_empty());

    let parsed = BalancedNetwork::from_json(&serde_json::to_string(&v).unwrap()).unwrap();
    assert_eq!(parsed.generators()[0].uid.as_deref(), Some("gen-a"));
    assert_eq!(parsed.buses()[0].uid.as_deref(), Some(bus_uid));
}

#[test]
fn geo_fields_roundtrip_and_stay_off_the_wire_when_absent() {
    let net = small_net();
    let text = net.to_json().unwrap();
    assert!(!text.contains(r#""geo""#));
    assert!(!text.contains(r#""location""#));
    let parsed = BalancedNetwork::from_json(&text).unwrap();
    assert_eq!(parsed.to_json().unwrap(), text);

    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v.get("geo").is_none());
    assert!(v["buses"][0].get("location").is_none());

    let mut with_geo = net;
    *with_geo.geo_mut() = Some(GeoMeta {
        space: CoordinateSpace::Geographic { crs: None },
        kind: Some(CoordsKind::Source),
    });
    with_geo.buses_mut()[0].location = Some(Location {
        x: -80.0,
        y: 35.0,
        kind: None,
    });

    let v: serde_json::Value = serde_json::from_str(&with_geo.to_json().unwrap()).unwrap();
    assert_eq!(
        v["geo"],
        serde_json::json!({"space": "geographic", "kind": "source"})
    );
    assert_eq!(v["buses"][0]["location"]["x"], serde_json::json!(-80.0));
    assert_eq!(v["buses"][0]["location"]["y"], serde_json::json!(35.0));

    let parsed = BalancedNetwork::from_json(&serde_json::to_string(&v).unwrap()).unwrap();
    assert_eq!(parsed.geo(), with_geo.geo());
    assert_eq!(parsed.buses()[0].location, with_geo.buses()[0].location);
}

use powerio_core::{Destination, PioModule};
use powerio_dist::{DistBus, DistTargetFormat, MulticonductorNetwork, VoltageSource};

fn source() -> VoltageSource {
    VoltageSource::new("s", "b", vec!["1".into()], vec![230.0], vec![0.0])
}

fn network() -> MulticonductorNetwork {
    let mut network = MulticonductorNetwork::new();
    network
        .buses_mut()
        .push(DistBus::new("b", vec!["1".into(), "n".into()]));
    network
        .sources_mut()
        .push(source().with_reference_terminal("n"));
    network
}

#[test]
fn source_wire_keeps_old_records_and_rejects_new_physics_in_the_published_schema() {
    let legacy: serde_json::Value =
        serde_json::from_str(include_str!("../../docs/schema/pio-ir/2/schema.json")).unwrap();
    let schema = serde_json::json!({"$ref": "#/$defs/VoltageSource", "$defs": legacy["$defs"]});
    let validator = jsonschema::validator_for(&schema).unwrap();
    let earth = source();
    let earth_json = serde_json::to_value(&earth).unwrap();
    assert!(validator.is_valid(&earth_json));
    assert!(earth_json.get("type").is_none());
    assert!(earth_json.get("reference_terminal").is_none());
    let mut bare_reference = earth_json.clone();
    bare_reference["reference_terminal"] = "n".into();
    assert!(serde_json::from_value::<VoltageSource>(bare_reference).is_err());
    assert_eq!(
        serde_json::from_value::<VoltageSource>(earth_json).unwrap(),
        earth
    );
    let floating = source().with_reference_terminal("n");
    let json = serde_json::to_value(&floating).unwrap();
    assert_eq!(json["type"], "powerio.ReferencedVoltageSource");
    assert!(!validator.is_valid(&json));
    assert_eq!(
        serde_json::from_value::<VoltageSource>(json.clone()).unwrap(),
        floating
    );
    let mut missing = json.clone();
    missing["value"]
        .as_object_mut()
        .unwrap()
        .remove("reference_terminal");
    assert!(serde_json::from_value::<VoltageSource>(missing).is_err());
    let mut unknown = json;
    unknown["type"] = "powerio.UnknownSource".into();
    assert!(serde_json::from_value::<VoltageSource>(unknown).is_err());
}

#[test]
fn reference_readiness_validates_endpoints_and_complete_phasors() {
    let valid = network();
    powerio_dist::require_electrical_readiness(&valid).unwrap();
    for reference in ["missing", "1"] {
        let mut net = valid.clone();
        net.sources_mut()[0].reference_terminal = Some(reference.into());
        assert!(powerio_dist::require_electrical_readiness(&net).is_err());
    }
    let mut net = valid;
    net.sources_mut()[0].v_angle.clear();
    assert!(powerio_dist::require_electrical_readiness(&net).is_err());
}

#[test]
fn unsupported_format_writers_refuse_to_ground_a_referenced_source() {
    let module = PioModule::new(network());
    for target in [
        DistTargetFormat::Dss,
        DistTargetFormat::PmdJson,
        DistTargetFormat::BmopfJson,
    ] {
        let error =
            powerio_dist::emit(&module, target, Destination::memory("case").unwrap()).unwrap_err();
        assert!(error.to_string().contains("reference-terminal"), "{error}");
    }
}

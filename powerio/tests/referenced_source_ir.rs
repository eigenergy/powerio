mod helpers;

use helpers::{deserialize_module_text, serialize_module_text};
use powerio::{PioModule, PioValue};
use powerio_dist::{DistBus, MulticonductorNetwork, VoltageSource};

fn network() -> MulticonductorNetwork {
    let mut net = MulticonductorNetwork::new();
    net.buses_mut()
        .push(DistBus::new("b", vec!["1".into(), "n".into()]));
    net.sources_mut().push(
        VoltageSource::new("s", "b", vec!["1".into()], vec![230.0], vec![0.0])
            .with_reference_terminal("n"),
    );
    net
}

#[test]
fn referenced_source_ir_roundtrips_in_network_collection_and_calculation_envelopes() {
    let net = network();
    let series = powerio_core::TimeSeries::new(
        vec![powerio_core::TimePoint::new("t", None).unwrap()],
        vec![net.clone()],
    )
    .unwrap();
    let instance = powerio_prob::McAcOpfInstance::from_network(net.clone()).unwrap();
    let pf = powerio_prob::McAcPfInstance::from_network(net.clone()).unwrap();
    for value in [
        PioValue::from(net),
        PioValue::from(series),
        PioValue::from(instance),
        PioValue::from(pf),
    ] {
        let original = PioModule::new(value);
        let text = serialize_module_text(&original).unwrap();
        assert!(text.contains("powerio.ReferencedVoltageSource"));
        let restored = deserialize_module_text(&text).unwrap();
        let again = serialize_module_text(&restored).unwrap();
        let a: serde_json::Value = serde_json::from_str(&text).unwrap();
        let b: serde_json::Value = serde_json::from_str(&again).unwrap();
        assert_eq!(a["value"], b["value"]);
        assert_eq!(original.value().type_name(), restored.value().type_name());
    }
}

#[test]
fn nonfinite_source_values_roundtrip_in_both_source_records() {
    // 0.11.5 read nonfinite source floats back from their JSON spellings;
    // both the historical body and the referenced record must keep doing so.
    let mut earth = network();
    earth.sources_mut()[0].reference_terminal = None;
    for mut net in [earth, network()] {
        let source = &mut net.sources_mut()[0];
        source.v_angle = vec![f64::NAN];
        source.energy_cost_rate = Some(vec![f64::INFINITY]);
        let text = serialize_module_text(&PioModule::new(PioValue::from(net))).unwrap();
        assert!(text.contains("\"NaN\"") && text.contains("\"Infinity\""));
        let restored = deserialize_module_text(&text).unwrap();
        let PioValue::MulticonductorNetwork(restored) = restored.value() else {
            panic!("the network type is preserved");
        };
        let source = &restored.sources()[0];
        assert!(source.v_angle[0].is_nan());
        assert_eq!(source.energy_cost_rate, Some(vec![f64::INFINITY]));
    }
}

#[test]
fn source_records_refuse_mixed_or_unknown_reference_forms() {
    let text = serialize_module_text(&PioModule::new(PioValue::from(network()))).unwrap();
    let mut document: serde_json::Value = serde_json::from_str(&text).unwrap();
    let source = &mut document["value"]["data"]["sources"][0];
    let body = source["value"].clone();
    for broken in [
        // A reference outside the tagged record would be silently dropped by
        // a historical reader, so it is refused.
        {
            let mut flat = body.clone();
            flat["reference_terminal"] = "n".into();
            flat
        },
        // The tag admits only the referenced record.
        serde_json::json!({"type": "powerio.Other", "value": body}),
        // A tagged record carries its fields only inside value.
        {
            let mut mixed = source.clone();
            mixed["name"] = "s".into();
            mixed
        },
    ] {
        let mut document = document.clone();
        document["value"]["data"]["sources"][0] = broken;
        assert!(deserialize_module_text(&document.to_string()).is_err());
    }
}

#[test]
fn prescribed_source_boundary_cannot_omit_the_reference() {
    let instance = powerio_prob::McAcPfInstance::from_network(network()).unwrap();
    let source = instance.source_boundary(0).unwrap();
    assert_eq!(source.source, "s");
    assert_eq!(source.bus, "b");
    assert_eq!(source.reference_terminal, Some("n"));
    assert_eq!(source.v_magnitude, [230.0]);
    assert!(std::ptr::eq(
        source.v_magnitude.as_ptr(),
        instance.sources()[0].v_magnitude.as_ptr()
    ));
    assert!(instance.source_boundary(1).is_none());
    let mut changed = network();
    changed.sources_mut()[0].reference_terminal = None;
    let changed = instance.with_network(changed).unwrap();
    assert!(
        changed
            .source_boundary(0)
            .unwrap()
            .reference_terminal
            .is_none()
    );
}

#[test]
fn balanced_projection_reports_the_unsupported_source_reference() {
    let report = powerio::transform::to_balanced_network_report(
        &network(),
        powerio::transform::MulticonductorToBalancedOptions::default(),
    );
    assert!(!report.is_ready());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message().contains("reference-terminal"))
    );
}

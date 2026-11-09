// Exact assertions below check fixed declarations and exact unit conversions.
#![allow(clippy::float_cmp)]
use powerio::*;
use std::sync::Arc;
const CASE: &str = include_str!("../../tests/data/pglib-hvdc/case5_3_he.m");
fn instance() -> AcDcOpfInstance {
    let module = parse(Source::from_memory("case5.m", CASE.as_bytes().to_vec()).unwrap()).unwrap();
    let PioValue::BalancedNetwork(n) = module.value() else {
        panic!()
    };
    AcDcOpfInstance::from_network(n.clone(), AcDcOpfProfile::PowerModelsAcDc0101AcpVscV1).unwrap()
}
fn point(i: &AcDcOpfInstance) -> AcDcOperatingPoint {
    AcDcOperatingPoint::new(
        i,
        i.coordinates()
            .into_iter()
            .map(|(quantity, identity)| AcDcValue {
                quantity,
                identity,
                value: 0.75,
            })
            .collect(),
    )
    .unwrap()
}
fn stored(value: PioValue) -> serde_json::Value {
    let out = serialize(
        &PioModule::new(value),
        Destination::memory("x.json").unwrap(),
    )
    .unwrap();
    let EmittedOutput::Memory { artifacts } = out.into_output() else {
        panic!()
    };
    serde_json::from_slice(artifacts[0].bytes()).unwrap()
}
fn read(doc: &serde_json::Value) -> Result<PioModule<PioValue>> {
    deserialize(Source::from_memory("x.json", doc.to_string().into_bytes()).unwrap())
}
#[test]
fn coupled_instance_and_result_roundtrip_with_capability_and_typed_units() {
    let i = instance();
    let p = point(&i);
    assert_eq!(p.values().len(), 95);
    let i = i.with_initial_point(p.clone()).unwrap();
    let solution = AcDcOpfSolution::new(
        Arc::new(i.clone()),
        p,
        42.0,
        Termination::Converged,
        Some("test producer".into()),
    )
    .unwrap();
    for value in [PioValue::from(i), PioValue::from(solution)] {
        let doc = stored(value);
        assert_eq!(
            doc["required_capabilities"],
            serde_json::json!(["balanced-acdc-equivalent-v1"])
        );
        let restored = read(&doc).unwrap();
        assert_eq!(stored(restored.value().clone()), doc);
        let mut no_gate = doc.clone();
        no_gate
            .as_object_mut()
            .unwrap()
            .remove("required_capabilities");
        assert!(read(&no_gate).is_err());
        assert!(emit(&restored, "matpower", Destination::memory("x.m").unwrap()).is_err());
    }
    assert_eq!(AcDcQuantity::PccActivePower.unit(), "MW");
    assert_eq!(AcDcQuantity::PccReactivePower.unit(), "MVAr");
    assert_eq!(AcDcQuantity::ValveVoltageAngle.unit(), "rad");
}
#[test]
fn stored_contract_refuses_unknown_profile_fields_quantities_and_wrong_points() {
    let i = instance();
    let doc = stored(i.clone().with_initial_point(point(&i)).unwrap().into());
    for (field, value) in [
        ("profile", serde_json::json!("unknown")),
        ("profile", serde_json::Value::Null),
        ("silent_projection", serde_json::json!(true)),
    ] {
        let mut bad = doc.clone();
        bad["value"]["data"][field] = value;
        assert!(read(&bad).is_err());
    }
    for (field, value) in [
        ("identity", serde_json::json!("missing")),
        ("quantity", serde_json::json!("solver_column")),
        ("value", serde_json::Value::Null),
    ] {
        let mut bad = doc.clone();
        bad["value"]["data"]["initial_point"][0][field] = value;
        assert!(read(&bad).is_err());
    }
    let mut bad = doc.clone();
    bad["value"]["data"]["initial_point"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(read(&bad).is_err());
    let mut bad = doc.clone();
    bad["value"]["data"]["initial_point"][0] = bad["value"]["data"]["initial_point"][1].clone();
    assert!(read(&bad).is_err());
    let mut values = point(&i).values().to_vec();
    values[0].value = f64::NAN;
    assert!(AcDcOperatingPoint::new(&i, values).is_err());
}
#[test]
fn identity_mapping_survives_reordering_and_legacy_guards_remain() {
    let i = instance();
    let p = point(&i);
    let mut net = i.network().clone();
    net.buses_mut().reverse();
    net.branches_mut().reverse();
    net.generators_mut().reverse();
    let mut owner = net.ac_dc_equivalent().unwrap().clone();
    owner.stations.reverse();
    owner.buses.reverse();
    owner.branches.reverse();
    net.set_ac_dc_equivalent(Some(owner)).unwrap();
    let reordered = AcDcOpfInstance::from_network(net.clone(), i.profile()).unwrap();
    assert_eq!(i.coordinates(), reordered.coordinates());
    reordered.with_initial_point(p).unwrap();
    assert!(AcOpfInstance::from_network(net.clone()).is_err());
    assert!(net.to_normalized().is_err());
}
#[cfg(feature = "matrix")]
#[test]
fn preparation_preserves_source_and_records_reference_adjustments_and_starts() {
    let i = instance();
    let before = serde_json::to_value(i.network()).unwrap();
    let p = build_ac_dc_opf_preparation(&i).unwrap();
    assert_eq!(before, serde_json::to_value(i.network()).unwrap());
    assert_eq!(p.stations.len(), 3);
    for s in &p.stations {
        assert_eq!(s.policy.converter_base_kv, 345.0);
        assert_eq!(s.policy.pcc_base_kv, 230.0);
        assert_eq!(s.policy.declared_current_limit_pu, 1.1);
        assert!((s.policy.effective_current_limit_pu - 1.25_f64.sqrt()).abs() < 1e-12);
        let loss = s.policy.loss_coefficients_pu;
        assert!((loss[1] - s.source.losses.linear_kv / (3.0_f64.sqrt() * 345.0)).abs() < 1e-15);
        assert!(
            (loss[2] - s.source.losses.inverter_quadratic_ohm * 100.0 / (3.0 * 345.0_f64.powi(2)))
                .abs()
                < 1e-15
        );
        assert_eq!(s.bounds(AcDcQuantity::PccActivePower).unwrap().max, 2.0);
    }
    let start = point(&i);
    let i = i.with_initial_point(start).unwrap();
    let p = build_ac_dc_opf_preparation(&i).unwrap();
    assert_eq!(p.ac.buses.initial_vm, vec![0.75; 5]);
    assert!(
        p.ac.generators
            .pg
            .iter()
            .all(|v| (*v - 0.0075).abs() < 1e-15)
    );
}

#[test]
fn malformed_result_and_duplicate_source_identities_refuse() {
    let i = instance();
    assert!(
        AcDcOpfSolution::new(
            Arc::new(i.clone()),
            point(&i),
            f64::INFINITY,
            Termination::Converged,
            None
        )
        .is_err()
    );
    let s = AcDcOpfSolution::new(
        Arc::new(i.clone()),
        point(&i),
        0.0,
        Termination::Converged,
        None,
    )
    .unwrap();
    let mut doc = stored(s.into());
    doc["value"]["data"]["point"].as_array_mut().unwrap().pop();
    assert!(read(&doc).is_err());
    let mut n = i.network().clone();
    n.generators_mut()[1].uid = n.generators()[0].uid.clone();
    assert!(AcDcOpfInstance::from_network(n, i.profile()).is_err());
}

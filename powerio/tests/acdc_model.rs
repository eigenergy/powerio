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

#[cfg(feature = "matrix")]
#[test]
fn numerical_angle_references_are_opt_in_stable_and_preserve_roles() {
    let i = instance();
    let mut net = i.network().clone();
    // Bus 2 becomes an AC island even though its DC-connected converter remains.
    for branch in net.branches_mut() {
        if branch.from.0 == 2 || branch.to.0 == 2 {
            branch.in_service = false;
        }
    }
    let original = serde_json::to_value(&net).unwrap();
    let declared = AcDcOpfInstance::from_network(net.clone(), i.profile()).unwrap();
    assert!(build_ac_dc_opf_preparation(&declared).is_err());
    let selected =
        declared.with_angle_reference_policy(AcDcAngleReferencePolicy::LowestBusIdPerIsland);
    let prepared = build_ac_dc_opf_preparation(&selected).unwrap();
    let added: Vec<_> = prepared
        .angle_references
        .iter()
        .filter(|r| !r.declared)
        .collect();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].bus, BusId(2));
    assert_eq!(added[0].island_buses, vec![BusId(2)]);
    assert_eq!(original, serde_json::to_value(selected.network()).unwrap());
    let mut ac_only = net.clone();
    ac_only.set_ac_dc_equivalent(None).unwrap();
    let ac_only = AcOpfInstance::from_network(ac_only).unwrap();
    assert!(
        matrix::build_ac_opf_preparation(&ac_only, &matrix::AcOpfAssemblyOptions::default())
            .is_err()
    );
    net.buses_mut().reverse();
    net.branches_mut().reverse();
    let reordered = AcDcOpfInstance::from_network(net, i.profile())
        .unwrap()
        .with_angle_reference_policy(AcDcAngleReferencePolicy::LowestBusIdPerIsland);
    assert_eq!(
        prepared.angle_references,
        build_ac_dc_opf_preparation(&reordered)
            .unwrap()
            .angle_references
    );
}

#[test]
fn angle_reference_policy_roundtrips_and_unknown_policy_refuses() {
    let i = instance();
    let old_shape = stored(i.clone().into());
    assert!(
        old_shape["value"]["data"]
            .get("angle_reference_policy")
            .is_none()
    );
    let selected = i.with_angle_reference_policy(AcDcAngleReferencePolicy::LowestBusIdPerIsland);
    let p = point(&selected);
    let selected = selected.with_initial_point(p.clone()).unwrap();
    let solution = AcDcOpfSolution::new(
        Arc::new(selected.clone()),
        p,
        0.0,
        Termination::Converged,
        None,
    )
    .unwrap();
    for value in [PioValue::from(selected), PioValue::from(solution)] {
        let doc = stored(value);
        assert_eq!(stored(read(&doc).unwrap().value().clone()), doc);
    }
    let mut bad = old_shape;
    bad["value"]["data"]["angle_reference_policy"] = serde_json::json!("automatic_slack_dispatch");
    assert!(read(&bad).is_err());
    bad["value"]["data"]["angle_reference_policy"] = serde_json::Value::Null;
    assert!(read(&bad).is_err());
}

#[test]
fn declared_current_profile_roundtrips_without_changing_compatibility_selection() {
    let old = instance();
    assert_eq!(
        old.profile().current_limit_policy(),
        AcDcCurrentLimitPolicy::PowerModelsAcDc0101Repair
    );
    let selected = AcDcOpfInstance::from_network(
        old.network().clone(),
        AcDcOpfProfile::DeclaredCurrentAcpVscV1,
    )
    .unwrap();
    assert_eq!(
        selected.profile().current_limit_policy(),
        AcDcCurrentLimitPolicy::Declared
    );
    let p = point(&selected);
    let selected = selected.with_initial_point(p.clone()).unwrap();
    let solution = AcDcOpfSolution::new(
        Arc::new(selected.clone()),
        p,
        0.0,
        Termination::Converged,
        None,
    )
    .unwrap();
    for value in [PioValue::from(selected), PioValue::from(solution)] {
        let doc = stored(value);
        assert_eq!(stored(read(&doc).unwrap().value().clone()), doc);
    }
    let doc = stored(old.into());
    assert_eq!(
        doc["value"]["data"]["profile"],
        "power_models_ac_dc0101_acp_vsc_v1"
    );
    // Bound semantics cannot be overridden beneath the compatibility profile.
    let mut bad = doc.clone();
    bad["value"]["data"]["current_limit_policy"] = serde_json::json!("declared");
    assert!(read(&bad).is_err());
    let mut bad = doc;
    bad["value"]["data"]["profile"] = serde_json::json!("declared_current_acp_vsc_v2");
    assert!(read(&bad).is_err());
}

#[cfg(feature = "matrix")]
#[test]
fn declared_current_bounds_never_increase_and_preserve_other_preparation() {
    let old = instance();
    for limit in [0.0, 0.05, 1.1, 5.0] {
        let mut net = old.network().clone();
        let mut owner = net.ac_dc_equivalent().unwrap().clone();
        for s in &mut owner.stations {
            s.valve_current_limit_pu = limit;
        }
        net.set_ac_dc_equivalent(Some(owner)).unwrap();
        let before = serde_json::to_value(&net).unwrap();
        let compatibility = AcDcOpfInstance::from_network(net.clone(), old.profile()).unwrap();
        let declared =
            AcDcOpfInstance::from_network(net, AcDcOpfProfile::DeclaredCurrentAcpVscV1).unwrap();
        let a = build_ac_dc_opf_preparation(&compatibility).unwrap();
        let b = build_ac_dc_opf_preparation(&declared).unwrap();
        for (a, b) in a.stations.iter().zip(&b.stations) {
            assert_eq!(
                a.policy.current_limit_policy,
                AcDcCurrentLimitPolicy::PowerModelsAcDc0101Repair
            );
            assert_eq!(
                b.policy.current_limit_policy,
                AcDcCurrentLimitPolicy::Declared
            );
            assert_eq!(
                a.policy.effective_current_limit_pu,
                limit.max(1.25_f64.sqrt())
            );
            assert_eq!(
                b.bounds(AcDcQuantity::ValveCurrentMagnitude).unwrap().max,
                limit
            );
            assert_eq!(b.policy.declared_current_limit_pu, limit);
            assert_eq!(a.policy.loss_coefficients_pu, b.policy.loss_coefficients_pu);
            for q in [
                AcDcQuantity::PccActivePower,
                AcDcQuantity::ValveReactivePower,
                AcDcQuantity::StationDcActivePower,
                AcDcQuantity::FilterVoltageMagnitude,
            ] {
                assert_eq!(a.bounds(q), b.bounds(q));
            }
        }
        assert_eq!(before, serde_json::to_value(declared.network()).unwrap());
        assert!(declared.network().to_normalized().is_err());
        assert!(AcOpfInstance::from_network(declared.network().clone()).is_err());
    }
}

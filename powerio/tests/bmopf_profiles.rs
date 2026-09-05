//! Explicit BMOPF schema versions and IR persistence bypass retained-source emission.

use powerio::{Destination, EmittedOutput, PioValue, Source};
use serde_json::{Value, json};

fn bytes(result: &powerio::EmitResult) -> &[u8] {
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("expected memory output")
    };
    assert_eq!(artifacts.len(), 1);
    artifacts[0].bytes()
}

fn case() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "meta": {"$schema": powerio::BmopfSchemaVersion::Bmopf010.schema_id(),
            "provenance": {"research": {"id": "independent-example", "revision": 3}}},
        "bus": {"b": {"terminal_names": ["a", "b", "c", "n"],
            "v_min": [210.0, 212.0, 214.0], "v_max": [250.0, 248.0, 246.0]}},
        "voltage_source": {"s": {"bus": "b", "terminal_map": ["a", "b", "c", "n"],
            "v_magnitude": [230.0,230.0,230.0,0.0], "v_angle": [0.0,-2.094,2.094,0.0]}}
    }))
    .unwrap()
}

#[test]
fn explicit_schema_version_reencodes_and_plain_format_echoes() {
    let source = case();
    let module =
        powerio::parse(Source::from_memory("case.bmopf.json", source.clone()).unwrap()).unwrap();
    let echoed = powerio::emit(
        &module,
        "bmopf-json",
        Destination::memory("case.json").unwrap(),
    )
    .unwrap();
    assert_eq!(bytes(&echoed), source);
    for profile in ["0.1.0", "0.2.0"] {
        let emitted = powerio::emit(
            &module,
            &format!("bmopf-json@{profile}"),
            Destination::memory("case.json").unwrap(),
        )
        .unwrap();
        let output: Value = serde_json::from_slice(bytes(&emitted)).unwrap();
        assert!(
            output["meta"]["$schema"]
                .as_str()
                .unwrap()
                .contains(profile)
        );
        assert_eq!(output["bus"]["b"]["v_min"], json!([210.0, 212.0, 214.0]));
        assert_eq!(output["meta"]["provenance"]["research"]["revision"], 3);
    }
    assert!(
        powerio::emit(
            &module,
            "bmopf-json@9.0.0",
            Destination::memory("case.json").unwrap()
        )
        .is_err()
    );
}

#[test]
fn generation_two_ir_preserves_phase_limits_without_source_bytes() {
    let module = powerio::parse(Source::from_memory("case.bmopf.json", case()).unwrap()).unwrap();
    let ir = powerio::serialize(&module, Destination::memory("case.pio.json").unwrap()).unwrap();
    let restored =
        powerio::deserialize(Source::from_memory("case.pio.json", bytes(&ir).to_vec()).unwrap())
            .unwrap();
    let PioValue::MulticonductorNetwork(network) = restored.value() else {
        panic!("expected network")
    };
    assert_eq!(
        network.buses()[0].v_min_phase.as_deref(),
        Some([210.0, 212.0, 214.0].as_slice())
    );
    let output = powerio::emit(
        &restored,
        "bmopf-json",
        Destination::memory("case.json").unwrap(),
    )
    .unwrap();
    let value: Value = serde_json::from_slice(bytes(&output)).unwrap();
    assert_eq!(value["bus"]["b"]["v_min"], json!([210.0, 212.0, 214.0]));
}

#[test]
fn dss_four_winding_reactances_survive_ir_and_bmopf() {
    let source = b"New Circuit.test basekv=115\nNew Transformer.t phases=3 windings=4 buses=[h m l v] conns=[delta wye wye wye] kvs=[115 24.9 4.16 2.4] kvas=[30000 30000 30000 30000] xscarray=[8 9 10 6 7 4]";
    let module = powerio::parse(Source::from_memory("case.dss", source.to_vec()).unwrap()).unwrap();
    let ir = powerio::serialize(&module, Destination::memory("case.pio.json").unwrap()).unwrap();
    let restored =
        powerio::deserialize(Source::from_memory("case.pio.json", bytes(&ir).to_vec()).unwrap())
            .unwrap();
    let emitted = powerio::emit(
        &restored,
        "bmopf-json@0.2.0",
        Destination::memory("case.json").unwrap(),
    )
    .unwrap();
    let value: Value = serde_json::from_slice(bytes(&emitted)).unwrap();
    let table = &value["transformer"]["n_winding"]["t"]["x_sc"];
    for (key, percent) in [
        ("1_2", 8.0),
        ("1_3", 9.0),
        ("1_4", 10.0),
        ("2_3", 6.0),
        ("2_4", 7.0),
        ("3_4", 4.0),
    ] {
        assert!((table[key].as_f64().unwrap() - percent / 100.0 * 1322.5).abs() < 1e-9);
    }
    let back = powerio::parse(
        Source::from_memory("roundtrip.bmopf.json", bytes(&emitted).to_vec()).unwrap(),
    )
    .unwrap();
    let PioValue::MulticonductorNetwork(network) = back.value() else {
        panic!("expected network")
    };
    for (actual, expected) in network.transformers()[0]
        .xsc_pct
        .iter()
        .zip([8.0, 9.0, 10.0, 6.0, 7.0, 4.0])
    {
        assert!((actual - expected).abs() < 1e-9);
    }
    let mut changed = network.clone();
    changed.transformers_mut()[0].xsc_pct[5] = 5.5;
    let changed = powerio::PioModule::new(PioValue::MulticonductorNetwork(changed));
    for target in ["bmopf-json@0.2.0", "pmd"] {
        let emitted = powerio::emit(
            &changed,
            target,
            Destination::memory("changed.json").unwrap(),
        )
        .unwrap();
        let restored =
            powerio::parse(Source::from_memory("changed.json", bytes(&emitted).to_vec()).unwrap())
                .unwrap();
        let PioValue::MulticonductorNetwork(network) = restored.value() else {
            panic!("expected network")
        };
        for (actual, expected) in network.transformers()[0]
            .xsc_pct
            .iter()
            .zip([8.0, 9.0, 10.0, 6.0, 7.0, 5.5])
        {
            assert!((actual - expected).abs() < 1e-9, "{target}");
        }
    }
}

#[test]
fn energy_costs_keep_phase_order_through_mutation_ir_and_schema_conversion() {
    let mut input: Value = serde_json::from_slice(&case()).unwrap();
    input["meta"]["$schema"] = json!(powerio::BmopfSchemaVersion::Bmopf020.schema_id());
    input["meta"]["schema_version"] = json!("0.2.0");
    input["voltage_source"]["s"]["energy_cost_rate"] = json!([0.10, 0.20, 0.30]);
    input["generator"] = json!({"g": {"bus":"b", "configuration":"WYE",
        "terminal_map":["a","b","c","n"], "p_min":[0,0,0], "p_max":[100,100,100],
        "energy_cost_rate":[0.03,0.04,0.05]}});
    let module = powerio::parse(
        Source::from_memory("rates.json", serde_json::to_vec(&input).unwrap()).unwrap(),
    )
    .unwrap();
    let PioValue::MulticonductorNetwork(network) = module.value() else {
        panic!("expected network")
    };
    let mut changed = network.clone();
    changed.sources_mut()[0].energy_cost_rate.as_mut().unwrap()[1] = 0.25;
    let changed = powerio::PioModule::new(PioValue::MulticonductorNetwork(changed));
    let ir = powerio::serialize(&changed, Destination::memory("rates.pio.json").unwrap()).unwrap();
    let restored =
        powerio::deserialize(Source::from_memory("rates.pio.json", bytes(&ir).to_vec()).unwrap())
            .unwrap();
    for version in ["0.1.0", "0.2.0"] {
        let output = powerio::emit(
            &restored,
            &format!("bmopf-json@{version}"),
            Destination::memory("rates.json").unwrap(),
        )
        .unwrap();
        let emitted: Value = serde_json::from_slice(bytes(&output)).unwrap();
        if version == "0.2.0" {
            assert_eq!(
                emitted["voltage_source"]["s"]["energy_cost_rate"],
                json!([0.1, 0.25, 0.3])
            );
            assert_eq!(
                emitted["generator"]["g"]["energy_cost_rate"],
                json!([0.03, 0.04, 0.05])
            );
            assert!(emitted["generator"]["g"].get("cost").is_none());
        } else {
            assert!(
                emitted["voltage_source"]["s"]
                    .get("energy_cost_rate")
                    .is_none()
            );
            assert_eq!(
                emitted["extras"]["voltage_source"]["s"]["energy_cost_rate"],
                json!([0.1, 0.25, 0.3])
            );
            assert_eq!(emitted["generator"]["g"]["cost"], json!([0.03, 0.04, 0.05]));
        }
        let back =
            powerio::parse(Source::from_memory("rates.json", bytes(&output).to_vec()).unwrap())
                .unwrap();
        let PioValue::MulticonductorNetwork(network) = back.value() else {
            panic!("expected network")
        };
        assert_eq!(
            network.sources()[0].energy_cost_rate,
            Some(vec![0.1, 0.25, 0.3])
        );
        assert_eq!(network.generators()[0].cost, Some(vec![0.03, 0.04, 0.05]));
    }
}

#[test]
fn unresolved_geometry_cannot_reach_instances_matrices_or_canonical_exports() {
    let text = b"New Circuit.test phases=1 basekv=19.1\nNew Line.l bus1=sourcebus.1 bus2=load.1 phases=1 geometry=unresolved length=1 units=m\n";
    let module =
        powerio::parse(Source::from_memory("geometry.dss", text.to_vec()).unwrap()).unwrap();
    assert!(
        module
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.DSS.GEOMETRY_UNRESOLVED")
    );
    let PioValue::MulticonductorNetwork(network) = module.value() else {
        panic!("expected network")
    };
    assert!(network.lines().is_empty());
    assert!(network.line_codes().is_empty());
    assert!(powerio_prob::McAcPfInstance::from_network(network.clone()).is_err());
    assert!(powerio_prob::McAcOpfInstance::from_network(network.clone()).is_err());
    assert!(powerio_matrix::calc_multiconductor_admittance_matrix(network).is_err());
    let echoed = powerio::emit(&module, "dss", Destination::memory("source").unwrap()).unwrap();
    assert_eq!(bytes(&echoed), text);
    let ir =
        powerio::serialize(&module, Destination::memory("geometry.pio.json").unwrap()).unwrap();
    let restored = powerio::deserialize(
        Source::from_memory("geometry.pio.json", bytes(&ir).to_vec()).unwrap(),
    )
    .unwrap();
    for format in ["bmopf-json@0.2.0", "pmd", "dss"] {
        let error =
            powerio::emit(&restored, format, Destination::memory("out").unwrap()).unwrap_err();
        assert!(error.to_string().contains("geometry"), "{format}: {error}");
    }
}

#[test]
fn inverter_energy_prices_survive_ir_and_legacy_relocation() {
    let mut input: Value = serde_json::from_slice(&case()).unwrap();
    input["meta"]["$schema"] = json!(powerio::BmopfSchemaVersion::Bmopf020.schema_id());
    input["ibr"] = json!({"pv":{"bus":"b", "terminal_map":["a","b","c","n"],
        "topology":"FOUR_LEG", "prime_mover":"PV", "s_max":[1000,1000,1000],
        "energy_cost_rate":[0.01,0.02,0.03]}});
    let module = powerio::parse(
        Source::from_memory("rates.json", serde_json::to_vec(&input).unwrap()).unwrap(),
    )
    .unwrap();
    let ir = powerio::serialize(&module, Destination::memory("rates.pio.json").unwrap()).unwrap();
    let restored =
        powerio::deserialize(Source::from_memory("rates.pio.json", bytes(&ir).to_vec()).unwrap())
            .unwrap();
    for version in ["0.1.0", "0.2.0"] {
        let output = powerio::emit(
            &restored,
            &format!("bmopf-json@{version}"),
            Destination::memory("rates.json").unwrap(),
        )
        .unwrap();
        let doc: Value = serde_json::from_slice(bytes(&output)).unwrap();
        let table = if version == "0.1.0" {
            &doc["extras"]["ibr"]
        } else {
            &doc["ibr"]
        };
        assert_eq!(table["pv"]["energy_cost_rate"], json!([0.01, 0.02, 0.03]));
        let back =
            powerio::parse(Source::from_memory("rates.json", bytes(&output).to_vec()).unwrap())
                .unwrap();
        let PioValue::MulticonductorNetwork(network) = back.value() else {
            panic!("expected network")
        };
        assert_eq!(
            network.ibrs()[0].extras["energy_cost_rate"],
            json!([0.01, 0.02, 0.03])
        );
    }
}

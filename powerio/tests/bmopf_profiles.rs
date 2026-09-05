//! Explicit BMOPF profiles and IR persistence bypass retained-source emission.

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
        "meta": {"$schema": powerio::BmopfProfile::Bmopf010.schema_id(),
            "provenance": {"research": {"id": "independent-example", "revision": 3}}},
        "bus": {"b": {"terminal_names": ["a", "b", "c", "n"],
            "v_min": [210.0, 212.0, 214.0], "v_max": [250.0, 248.0, 246.0]}},
        "voltage_source": {"s": {"bus": "b", "terminal_map": ["a", "b", "c", "n"],
            "v_magnitude": [230.0,230.0,230.0,0.0], "v_angle": [0.0,-2.094,2.094,0.0]}}
    }))
    .unwrap()
}

#[test]
fn explicit_profile_reencodes_and_plain_format_echoes() {
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

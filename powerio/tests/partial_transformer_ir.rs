//! A winding outage is essential physics, never an ignorable IR extension.
mod helpers;

use helpers::{deserialize_module_text as deserialize, serialize_module_text as serialize};
use powerio::{BalancedNetwork, PioModule, PioValue};
use powerio_core::{
    HistoryEntry, HistoryId, HistoryKind, Scenario, ScenarioId, ScenarioSet, TimePoint, TimeSeries,
};
use powerio_matrix::{BuildOptions, IndexedNetwork, calc_admittance_matrix};
use powerio_tx::{Bus, BusId, BusType, Generator, Impedance, Transformer3W, Winding};
use serde_json::{Value, json};

const PARTIAL: &str = "powerio.PartialTransformer3W";
const RAW: &str = include_str!("../../tests/data/psse/partial_3w_status.raw");

fn network(flags: [bool; 3], in_service: bool) -> BalancedNetwork {
    let ids = [BusId(10), BusId(20), BusId(30)];
    let mut net = BalancedNetwork::in_memory(
        "partial-transformer",
        75.0,
        ids.map(|id| Bus::new(id, BusType::Ref, 230.0)).to_vec(),
        Vec::new(),
    );
    let mut transformer = Transformer3W::new(
        ids.map(Winding::new),
        [
            Impedance::new(0.01, 0.12, 50.0),
            Impedance::new(0.02, 0.16, 75.0),
            Impedance::new(0.03, 0.20, 100.0),
        ],
    );
    transformer.uid = Some("three-winding-physical-row".into());
    transformer.name = Some("original equipment".into());
    transformer.in_service = in_service;
    transformer.star_va = 17.0;
    transformer.mag_b = -0.001;
    transformer.windings[1].shift = 12.0;
    transformer.windings[2].rate_a = 150.0;
    transformer.extras.insert(
        "source_record".into(),
        json!({"index": 7, "note": "original"}),
    );
    transformer.set_winding_in_service(flags);
    net.transformers_3w_mut().push(transformer);
    let mut generator = Generator::new(ids[0]);
    generator.pmax = 100.0;
    net.generators_mut().push(generator);
    net.assign_missing_component_ids();
    net
}

fn roundtrip(net: &BalancedNetwork) -> BalancedNetwork {
    let mut module = PioModule::new(PioValue::from(net.clone()));
    module
        .add_history_entry(
            HistoryEntry::new(
                HistoryId::new("preserved-history").unwrap(),
                HistoryKind::Transform,
                "original_operation",
            )
            .unwrap(),
        )
        .unwrap();
    module
        .insert_extension("org.example.preserved", json!({"original": true}))
        .unwrap();
    let text = serialize(&module).unwrap();
    let restored = deserialize(&text).unwrap();
    assert_eq!(
        serialize(&restored).unwrap(),
        text,
        "the module, history and identities must be byte stable"
    );
    let PioValue::BalancedNetwork(back) = restored.into_value() else {
        panic!("balanced network")
    };
    assert_eq!(back.transformers_3w(), net.transformers_3w());
    assert_eq!(back.base_mva(), net.base_mva());
    assert_eq!(back.source_format(), net.source_format());
    assert_eq!(
        IndexedNetwork::new(&back).branches(),
        IndexedNetwork::new(net).branches()
    );
    back
}

#[test]
fn all_winding_and_whole_transformer_states_preserve_physics_units_and_rows() {
    for mask in 0_u8..8 {
        let flags = [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0];
        for in_service in [false, true] {
            let net = network(flags, in_service);
            let back = roundtrip(&net);
            let row = serde_json::to_value(&net.transformers_3w()[0]).unwrap();
            assert_eq!(row.get("type").is_some(), flags != [true; 3]);
            if flags != [true; 3] {
                assert_eq!(row["type"], PARTIAL);
                assert!(
                    row.get("windings").is_none(),
                    "old readers must lack mandatory fields"
                );
                assert_eq!(row["value"]["extras"]["winding_in_service"], json!(flags));
            }
            let before_y =
                calc_admittance_matrix(&IndexedNetwork::new(&net), &BuildOptions::default())
                    .unwrap();
            let after_y =
                calc_admittance_matrix(&IndexedNetwork::new(&back), &BuildOptions::default())
                    .unwrap();
            assert_eq!(before_y.g, after_y.g);
            assert_eq!(before_y.b, after_y.b);
            let before_dc = powerio_matrix::DcOperators::build(
                &powerio_prob::DcPfInstance::from_network(net.clone()).unwrap(),
            )
            .unwrap();
            let after_dc = powerio_matrix::DcOperators::build(
                &powerio_prob::DcPfInstance::from_network(back.clone()).unwrap(),
            )
            .unwrap();
            assert_eq!(
                before_dc.calc_incidence_matrix(),
                after_dc.calc_incidence_matrix()
            );
            assert_eq!(
                before_dc.calc_branch_susceptances(),
                after_dc.calc_branch_susceptances()
            );
            assert_eq!(before_dc.branch_identities(), after_dc.branch_identities());
            assert_eq!(before_dc.analysis_sources(), after_dc.analysis_sources());
            assert_eq!(
                after_dc.branch_rows().len(),
                if in_service {
                    flags.into_iter().filter(|on| *on).count()
                } else {
                    0
                }
            );
            // Normalization is an explicit derivation, not a serialization projection.
            let (normalized, rows) = net
                .to_normalized_with_source_rows(&powerio_tx::NormalizeOptions::default())
                .unwrap();
            let (normalized_back, back_rows) = back
                .to_normalized_with_source_rows(&powerio_tx::NormalizeOptions::default())
                .unwrap();
            assert_eq!(rows.transformers_3w, back_rows.transformers_3w);
            assert_eq!(rows.branches, back_rows.branches);
            assert_eq!(rows.buses, back_rows.buses);
            let normalized = normalized.network;
            let normalized_back = normalized_back.network;
            assert_eq!(
                normalized.transformers_3w(),
                normalized_back.transformers_3w()
            );
            roundtrip(&normalized);
        }
    }
}

#[test]
fn legacy_default_bodies_keep_exact_field_order_and_bytes() {
    #[derive(serde::Serialize)]
    struct Legacy<'a> {
        windings: &'a [Winding; 3],
        z: &'a [Impedance; 3],
        star_vm: f64,
        star_va: f64,
        mag_g: f64,
        mag_b: f64,
        in_service: bool,
        name: &'a Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        uid: &'a Option<String>,
        extras: &'a powerio_tx::Extras,
    }
    for in_service in [false, true] {
        let net = network([true; 3], in_service);
        for uid in [None, Some("stable".to_owned())] {
            let mut t = net.transformers_3w()[0].clone();
            t.uid = uid;
            let legacy = serde_json::to_string(&Legacy {
                windings: &t.windings,
                z: &t.z,
                star_vm: t.star_vm,
                star_va: t.star_va,
                mag_g: t.mag_g,
                mag_b: t.mag_b,
                in_service,
                name: &t.name,
                uid: &t.uid,
                extras: &t.extras,
            })
            .unwrap();
            assert_eq!(serde_json::to_string(&t).unwrap(), legacy);
            assert_eq!(serde_json::from_str::<Transformer3W>(&legacy).unwrap(), t);
            // Historical readers ignored unrelated unknown fields; preserve this.
            let mut extended: Value = serde_json::from_str(&legacy).unwrap();
            extended["unrecognized_source_hint"] = json!(123);
            assert_eq!(
                serde_json::from_value::<Transformer3W>(extended).unwrap(),
                t
            );
        }
    }
}

#[test]
fn partial_transformer_roundtrips_in_collection_and_calculation_envelopes() {
    let net = network([true, false, true], true);
    let series =
        TimeSeries::new(vec![TimePoint::new("t", None).unwrap()], vec![net.clone()]).unwrap();
    let scenarios = ScenarioSet::new(vec![Scenario::new(
        ScenarioId::new("base").unwrap(),
        None,
        net.clone(),
    )])
    .unwrap();
    for value in [
        PioValue::from(net.clone()),
        PioValue::from(series),
        PioValue::from(scenarios),
        PioValue::from(powerio_prob::AcPfInstance::from_network(net.clone()).unwrap()),
        PioValue::from(powerio_prob::DcPfInstance::from_network(net.clone()).unwrap()),
        PioValue::from(powerio_prob::AcOpfInstance::from_network(net.clone()).unwrap()),
        PioValue::from(powerio_prob::DcOpfInstance::from_network(net).unwrap()),
    ] {
        let text = serialize(&PioModule::new(value)).unwrap();
        assert!(text.contains(PARTIAL));
        assert_eq!(serialize(&deserialize(&text).unwrap()).unwrap(), text);
    }
}

#[test]
fn raw_status_and_source_records_survive_ir_without_changing_format_writers() {
    let statuses = [
        (false, [true; 3]),
        (true, [true; 3]),
        (true, [true, false, true]),
        (true, [true, true, false]),
        (true, [false, true, true]),
    ];
    for (stat, expected) in statuses.into_iter().enumerate() {
        let raw = RAW.replace("'T3W 2       ', 2,", &format!("'T3W 2       ', {stat},"));
        let module = powerio::parse_with_options(
            powerio::Source::from_memory("partial.raw", raw.as_bytes().to_vec()).unwrap(),
            &powerio::ParseOptions::default().format("psse").unwrap(),
        )
        .unwrap();
        let text = serialize(&module).unwrap();
        let restored = deserialize(&text).unwrap();
        assert_eq!(
            serialize(&restored).unwrap(),
            text,
            "source records and history changed"
        );
        let PioValue::BalancedNetwork(parsed) = restored.value() else {
            panic!("balanced network")
        };
        let back = roundtrip(parsed);
        assert_eq!(
            (
                back.transformers_3w()[0].in_service,
                back.transformers_3w()[0].winding_in_service()
            ),
            expected
        );
        // Exchange-format writers consume physical records, never the IR wrapper.
        for format in ["psse", "powermodels-json"] {
            let emitted = powerio::emit(
                &PioModule::new(back.clone()),
                format,
                powerio::Destination::memory("case").unwrap(),
            )
            .unwrap();
            let powerio::EmittedOutput::Memory { artifacts } = emitted.into_output() else {
                panic!("memory")
            };
            assert!(
                artifacts
                    .iter()
                    .all(|artifact| !String::from_utf8_lossy(artifact.bytes()).contains(PARTIAL))
            );
        }
    }
}

#[test]
fn malformed_or_untagged_partial_records_fail_closed() {
    let net = network([true, false, true], true);
    let record = serde_json::to_value(&net.transformers_3w()[0]).unwrap();
    let body = record["value"].clone();
    let mut cases = vec![
        body.clone(),
        json!({"type": PARTIAL}),
        json!({"value": body.clone()}),
        json!({"type": "powerio.FutureTransformer3W", "value": body.clone()}),
        json!({"type": PARTIAL, "value": record.clone()}),
        json!({"type": null, "value": body.clone()}),
        json!({"type": PARTIAL, "value": null}),
    ];
    let mut mixed = record.clone();
    mixed["windings"] = body["windings"].clone();
    cases.push(mixed);
    for flags in [
        Value::Null,
        json!([true, true]),
        json!([true, false, true, false]),
        json!("open"),
        json!([true, true, true]),
    ] {
        let mut malformed = record.clone();
        malformed["value"]["extras"]["winding_in_service"] = flags;
        cases.push(malformed);
    }
    let mut oversized = record.clone();
    oversized["value"]["windings"]
        .as_array_mut()
        .unwrap()
        .push(body["windings"][0].clone());
    cases.push(oversized);
    let mut nested_tag = record.clone();
    nested_tag["value"]["type"] = json!(PARTIAL);
    cases.push(nested_tag);
    for case in cases {
        assert!(
            serde_json::from_value::<Transformer3W>(case.clone()).is_err(),
            "accepted {case}"
        );
        let mut doc: Value =
            serde_json::from_str(&serialize(&PioModule::new(PioValue::from(net.clone()))).unwrap())
                .unwrap();
        doc["value"]["data"]["transformers_3w"][0] = case;
        assert!(deserialize(&doc.to_string()).is_err());
    }
    let duplicate = format!(r#"{{"type":"{PARTIAL}","type":"{PARTIAL}","value":{body}}}"#);
    assert!(
        serde_json::from_str::<Transformer3W>(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("duplicate field")
    );
    let error = serde_json::from_value::<Transformer3W>(body)
        .unwrap_err()
        .to_string();
    assert!(error.contains("require a powerio.PartialTransformer3W record"));
}

#[test]
fn partial_transformer_keeps_typed_nonfinite_spelling() {
    let mut net = network([true, false, true], true);
    net.transformers_3w_mut()[0].windings[1].rate_a = f64::INFINITY;
    let text = serialize(&PioModule::new(PioValue::from(net))).unwrap();
    assert!(text.contains("Infinity"));
    let restored = deserialize(&text).unwrap();
    assert_eq!(serialize(&restored).unwrap(), text);
}

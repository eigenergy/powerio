//! Numeric zero constraints, infinity, and native format boundaries.
mod helpers;

use powerio_core::__implementation::nonfinite::{NonFiniteDe, NonFiniteSer};
use powerio_tx::{
    BalancedNetwork, Branch, BranchRatingSet, BusId, Impedance, MagnetizingConnection,
    TargetFormat, Transformer3W, Winding,
};
use serde::{Deserialize, Serialize};

fn nonfinite_json(value: &impl Serialize) -> String {
    let mut bytes = Vec::new();
    value
        .serialize(NonFiniteSer(&mut serde_json::Serializer::new(&mut bytes)))
        .unwrap();
    String::from_utf8(bytes).unwrap()
}

fn network() -> BalancedNetwork {
    helpers::parse_matpower_file(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case14.m"),
    )
    .unwrap()
}

#[test]
fn numeric_ratings_roundtrip_without_record_wrappers() {
    let mut branch = Branch::new(BusId(1), BusId(2), 0.01, 0.1);
    branch.rate_a = 0.0;
    branch.rate_b = 150.0;
    let text = nonfinite_json(&branch);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(json.get("type").is_none());
    assert_eq!(json["rate_a"], 0.0);
    assert_eq!(json["rate_b"], 150.0);
    assert_eq!(json["rate_c"], "Infinity");
    let mut deserializer = serde_json::Deserializer::from_str(&text);
    assert_eq!(
        Branch::deserialize(NonFiniteDe(&mut deserializer)).unwrap(),
        branch
    );
}

#[test]
fn nonfinite_adapter_preserves_unrelated_fields_and_infinity() {
    let mut branch = Branch::new(BusId(1), BusId(2), 0.01, 0.1);
    branch.rate_a = 0.0;
    branch.angmax = f64::INFINITY;
    let json = nonfinite_json(&branch);
    let mut deserializer = serde_json::Deserializer::from_str(&json);
    let back = Branch::deserialize(NonFiniteDe(&mut deserializer)).unwrap();
    assert_eq!(back, branch);
    branch.rate_a = f64::INFINITY;
    let text = nonfinite_json(&branch);
    assert!(text.contains("\"rate_a\":\"Infinity\""));
    let mut deserializer = serde_json::Deserializer::from_str(&text);
    assert_eq!(
        Branch::deserialize(NonFiniteDe(&mut deserializer)).unwrap(),
        branch
    );
}

#[test]
fn zero_winding_and_named_ratings_preserve_partial_states() {
    for partial in [false, true] {
        for named in [false, true] {
            let mut transformer = Transformer3W::new(
                [
                    Winding::new(BusId(1)),
                    Winding::new(BusId(2)),
                    Winding::new(BusId(3)),
                ],
                [Impedance::new(0.01, 0.1, 100.0); 3],
            );
            transformer.magnetizing_connection = MagnetizingConnection::Winding1Terminal;
            if partial {
                transformer.set_winding_in_service([true, false, true]);
            }
            if named {
                transformer.set_winding_rating_sets(2, vec![BranchRatingSet::new("winter", 0.0)]);
            } else {
                transformer.windings[2].rate_a = 0.0;
            }
            let text = nonfinite_json(&transformer);
            assert!(!text.contains("WithThermalLimits"));
            let mut deserializer = serde_json::Deserializer::from_str(&text);
            assert_eq!(
                Transformer3W::deserialize(NonFiniteDe(&mut deserializer)).unwrap(),
                transformer
            );
            let (_, branches) = transformer.to_star_expansion(BusId(100));
            if named {
                assert_eq!(branches[2].rating_sets[0].rate_mva, (0.0));
            } else {
                assert_eq!(branches[2].rate_a, (0.0));
            }
        }
    }
}

#[test]
fn normalization_preserves_zero_constraint_and_electrical_service() {
    let mut net = network();
    let branch = &mut net.branches_mut()[0];
    branch.rate_a = 0.0;
    branch.rate_b = 150.0;
    branch.rate_c = f64::INFINITY;
    let normalized = net.to_normalized().unwrap();
    let branch = &normalized.branches()[0];
    assert_eq!(branch.rate_a, (0.0));
    assert_eq!(branch.rate_b, (1.5));
    assert_eq!(branch.rate_c, f64::INFINITY);
    assert!(branch.in_service);
    assert!(branch.x != 0.0);
}

#[test]
fn named_winding_ratings_are_validated_before_use() {
    for winding in 0..3 {
        for rating in [0.0, 75.0, f64::INFINITY, -1.0, f64::NEG_INFINITY, f64::NAN] {
            let mut net = network();
            let mut transformer = Transformer3W::new(
                [
                    Winding::new(BusId(1)),
                    Winding::new(BusId(2)),
                    Winding::new(BusId(3)),
                ],
                [Impedance::new(0.01, 0.1, 100.0); 3],
            );
            transformer
                .set_winding_rating_sets(winding, vec![BranchRatingSet::new("winter", rating)]);
            let stored = transformer.winding_rating_sets(winding)[0].rate_mva;
            assert_eq!(stored.to_bits(), rating.to_bits());
            net.transformers_3w_mut().push(transformer);
            if rating >= 0.0 {
                net.validate().unwrap();
                let normalized = net.to_normalized().unwrap();
                assert_eq!(
                    normalized.transformers_3w()[0].winding_rating_sets(winding)[0].rate_mva,
                    rating / 100.0
                );
            } else {
                assert!(net.validate().is_err());
                assert!(net.to_normalized().is_err());
                assert!(helpers::emit_value(&net, TargetFormat::EgretJson).is_err());
            }
        }
    }
}

#[test]
fn egret_preserves_zero_but_legacy_writers_diagnose_it() {
    let mut net = network();
    net.branches_mut()[0].rate_a = 0.0;
    let emitted = helpers::emit_value(&net, TargetFormat::EgretJson).unwrap();
    let back = helpers::parse_egret_json(&emitted.text).unwrap();
    assert_eq!(back.branches()[0].rate_a, (0.0));
    assert!(back.branches()[0].in_service);
    for target in [
        TargetFormat::Matpower,
        TargetFormat::PowerModelsJson,
        TargetFormat::Psse { rev: 35 },
        TargetFormat::PsseRawx,
        TargetFormat::Pslf,
        TargetFormat::PowerWorld,
        TargetFormat::PandapowerJson,
        TargetFormat::SurgeJson,
        TargetFormat::RavensJson,
        TargetFormat::Ucte,
    ] {
        let output = helpers::emit_value(&net, target).unwrap();
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.message().contains("finite-zero rate_a")),
            "{target}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn invalid_numeric_ratings_are_refused_before_output() {
    for magnitude in [-1.0, f64::NEG_INFINITY, f64::NAN] {
        let mut net = network();
        net.branches_mut()[0].rate_a = magnitude;
        assert!(net.validate().is_err());
        for target in [
            TargetFormat::Matpower,
            TargetFormat::EgretJson,
            TargetFormat::SiennaJson { version: 4 },
        ] {
            assert!(helpers::emit_value(&net, target).is_err());
        }
    }
}

#[test]
fn sienna_winding_limits_preserve_zero_and_the_unbounded_marker() {
    let mut net = helpers::parse_matpower_file(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m"),
    )
    .unwrap();
    let mut transformer = Transformer3W::new(
        [
            Winding::new(BusId(1)),
            Winding::new(BusId(2)),
            Winding::new(BusId(3)),
        ],
        [Impedance::new(0.01, 0.1, 100.0); 3],
    );
    transformer.windings[0].rate_a = 0.0;
    transformer.windings[1].rate_a = 75.0;
    transformer.windings[2].rate_a = f64::INFINITY;
    transformer.windings[1].rate_b = 0.0;
    transformer.windings[2].rate_c = 175.0;
    transformer.set_winding_rating_sets(
        0,
        vec![
            BranchRatingSet::new("winter", 0.0),
            BranchRatingSet::new("no-bound", f64::INFINITY),
        ],
    );
    net.transformers_3w_mut().push(transformer);
    net.branches_mut()[0].rating_sets = vec![
        BranchRatingSet::new("zero", 0.0),
        BranchRatingSet::new("no-bound", f64::INFINITY),
        BranchRatingSet::new("emergency", 125.0),
    ];
    for version in 1..=4 {
        let output = helpers::emit_value(&net, TargetFormat::SiennaJson { version }).unwrap();
        let back = powerio_tx::parse(
            powerio_core::Source::from_memory("limits.json", output.text.into_bytes()).unwrap(),
        )
        .unwrap();
        let actual = &back.value().transformers_3w()[0];
        assert!(actual.in_service);
        assert_eq!(actual.winding_in_service(), [true; 3]);
        assert_eq!(
            actual.winding_rating_sets(0),
            net.transformers_3w()[0].winding_rating_sets(0)
        );
        assert_eq!(
            back.value().branches()[0].rating_sets,
            net.branches()[0].rating_sets
        );
        for (actual, expected) in actual
            .windings
            .iter()
            .zip(&net.transformers_3w()[0].windings)
        {
            assert_eq!(actual.rate_a, expected.rate_a);
            assert_eq!(actual.rate_b, expected.rate_b);
            assert_eq!(actual.rate_c, expected.rate_c);
        }
    }
    // PowerSystems requires each winding rating; an unlimited one is its
    // INFINITE_BOUND, which reads back as unlimited.
    let output = helpers::emit_value(&net, TargetFormat::PowerSystemsJson).unwrap();
    let back = powerio_tx::parse(
        powerio_core::Source::from_memory("limits.json", output.text.into_bytes()).unwrap(),
    )
    .unwrap();
    let windings = &back.value().transformers_3w()[0].windings;
    assert_eq!(windings[0].rate_a, 0.0);
    assert_eq!(windings[2].rate_a, f64::INFINITY);
    net.transformers_3w_mut()[0].windings[2].rate_a = 125.0;
    let output = helpers::emit_value(&net, TargetFormat::PowerSystemsJson).unwrap();
    assert!(output.diagnostics.iter().any(|d| {
        d.message()
            .contains("without native winding constraint semantics")
    }));
    if let Some(path) = std::env::var_os("POWERIO_THERMAL_SIENNA_OUTPUT") {
        std::fs::write(path, &output.text).unwrap();
    }
    let back = powerio_tx::parse(
        powerio_core::Source::from_memory("native.json", output.text.into_bytes()).unwrap(),
    )
    .unwrap();
    let actual = &back.value().transformers_3w()[0];
    assert_eq!(actual.winding_in_service(), [true; 3]);
    assert_eq!(
        actual.winding_rating_sets(0),
        net.transformers_3w()[0].winding_rating_sets(0)
    );
    for (actual, expected) in actual
        .windings
        .iter()
        .zip(&net.transformers_3w()[0].windings)
    {
        assert_eq!(actual.rate_a, expected.rate_a);
        assert_eq!(actual.rate_b, expected.rate_b);
        assert_eq!(actual.rate_c, expected.rate_c);
    }
}

fn network_with_an_unlimited_rating_set() -> BalancedNetwork {
    let mut net = network();
    net.branches_mut()[0].rating_sets = vec![
        BranchRatingSet::new("summer", 80.0),
        BranchRatingSet::new("winter", f64::INFINITY),
    ];
    net
}

#[test]
fn json_extensions_read_an_unlimited_rating_set_back() {
    // An unlimited set's rate is JSON null; the reader refused the whole
    // array, so the finite set became an untyped extra.
    let net = network_with_an_unlimited_rating_set();
    for (target, token) in [
        (TargetFormat::PowerModelsJson, "powermodels-json"),
        (TargetFormat::EgretJson, "egret-json"),
    ] {
        let emitted = helpers::emit_value(&net, target).unwrap();
        assert!(
            emitted
                .diagnostics
                .iter()
                .all(|d| !d.code().ends_with("NOT_A_NUMBER")),
            "{token}: {:?}",
            emitted.render_diagnostics()
        );
        let back = helpers::parse_str(&emitted.text, token).unwrap().network;
        assert_eq!(
            back.branches()[0].rating_sets,
            net.branches()[0].rating_sets,
            "{token}"
        );
        assert!(
            !back.branches()[0].extras.contains_key("rating_sets"),
            "{token}"
        );
    }
}

#[test]
fn xiidm_reports_an_unlimited_rating_set_instead_of_writing_nan() {
    let net = network_with_an_unlimited_rating_set();
    let emitted = helpers::emit_value(&net, TargetFormat::Xiidm).unwrap();
    assert!(!emitted.text.contains("NaN"));
    assert!(
        emitted
            .diagnostics
            .iter()
            .any(|d| d.code().ends_with("RATING_SET_DROPPED") && d.message().contains("winter")),
        "{:?}",
        emitted.render_diagnostics()
    );
    let back = helpers::parse_str(&emitted.text, "xiidm").unwrap().network;
    // XIIDM groups branches by kind, so find the edited one by its identity.
    let branch = back
        .branches()
        .iter()
        .find(|branch| branch.uid == net.branches()[0].uid)
        .unwrap();
    assert_eq!(
        branch.rating_sets,
        vec![BranchRatingSet::new("summer", 80.0)]
    );

    let mut transformer = Transformer3W::new(
        [
            Winding::new(BusId(1)),
            Winding::new(BusId(2)),
            Winding::new(BusId(3)),
        ],
        [Impedance::new(0.01, 0.1, 100.0); 3],
    );
    transformer.set_winding_rating_sets(
        1,
        vec![
            BranchRatingSet::new("summer", 80.0),
            BranchRatingSet::new("winter", f64::INFINITY),
        ],
    );
    let mut net = network();
    net.transformers_3w_mut().push(transformer);
    let emitted = helpers::emit_value(&net, TargetFormat::Xiidm).unwrap();
    assert!(!emitted.text.contains("NaN"));
    let back = helpers::parse_str(&emitted.text, "xiidm").unwrap().network;
    assert_eq!(
        back.transformers_3w()[0].winding_rating_sets(1),
        [BranchRatingSet::new("summer", 80.0)]
    );
}

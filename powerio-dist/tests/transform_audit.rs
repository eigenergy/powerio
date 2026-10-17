use powerio_dist::{
    DistBus, DistComponentLocation, DistGeographicPoint, DistLocationTarget, DistSwitch,
    LinDist3FlowPreparationPolicy, MulticonductorNetwork, NeutralKronOptions, neutral_kron_reduce,
    prepare_lindist3flow_network,
};
use serde_json::{Value, json};

fn network() -> MulticonductorNetwork {
    let mut network = MulticonductorNetwork::new();
    for name in ["a", "b"] {
        network
            .buses_mut()
            .push(DistBus::new(name, vec!["1".into()]));
    }
    network
}

fn add_switch(network: &mut MulticonductorNetwork, name: &str, terminal: &str) {
    network.switches_mut().push(DistSwitch::new(
        name,
        "a",
        "b",
        vec![terminal.into()],
        vec![terminal.into()],
        true,
    ));
    network
        .component_locations_mut()
        .push(DistComponentLocation::new(
            DistLocationTarget::Switch,
            name,
            DistGeographicPoint::new(Some(0.0), None),
        ));
}

fn restore(network: &MulticonductorNetwork) -> MulticonductorNetwork {
    let stored = serde_json::to_value(network).unwrap();
    let restored = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), stored);
    restored
}

#[test]
fn repeated_preparation_preserves_prior_omission_points_without_recursive_history() {
    let mut source = network();
    add_switch(&mut source, "first", "1");
    let first =
        prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Lower).unwrap();
    let initial = first.network().extras()["powerio_lindist3flow_preparation"].clone();
    assert_eq!(
        initial["actions"][0]["details"]["source_location"]["name"],
        "first"
    );
    let mut current = restore(first.network());
    for _ in 0..128 {
        let next =
            prepare_lindist3flow_network(&current, LinDist3FlowPreparationPolicy::Lower).unwrap();
        assert!(
            next.report().actions.is_empty(),
            "the report describes only this call"
        );
        current = next.into_parts().0;
    }
    let history = current.extras()["powerio_transform_history"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(
        history,
        &[json!({"metadata_key": "powerio_lindist3flow_preparation", "record": initial})]
    );
    add_switch(&mut current, "second", "1");
    let next =
        prepare_lindist3flow_network(&current, LinDist3FlowPreparationPolicy::Lower).unwrap();
    let restored = restore(next.network());
    assert_eq!(
        restored.extras()["powerio_transform_history"][0],
        history[0]
    );
    assert_eq!(
        restored.extras()["powerio_lindist3flow_preparation"]["actions"][0]["details"]["source_location"]
            ["name"],
        "second"
    );
    assert_eq!(source.component_locations().len(), 1);
}

#[test]
fn repeated_neutral_reductions_keep_each_removed_component_point() {
    let mut current = network();
    let mut records = Vec::new();
    for name in ["first", "second", "third"] {
        for bus in current.buses_mut() {
            bus.terminals.push("n".into());
            bus.grounded.push("n".into());
        }
        add_switch(&mut current, name, "n");
        let reduced = neutral_kron_reduce(&current, &NeutralKronOptions::default()).unwrap();
        current = restore(reduced.network());
        let record = current.extras()["powerio_neutral_kron"].clone();
        assert!(
            record["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["details"]["source_location"]["name"] == name)
        );
        records.push(record);
    }
    let history = current.extras()["powerio_transform_history"]
        .as_array()
        .unwrap();
    assert_eq!(history.len(), 2);
    for (archived, record) in history.iter().zip(&records) {
        assert_eq!(
            archived,
            &json!({"metadata_key": "powerio_neutral_kron", "record": record})
        );
    }
}

#[test]
fn malformed_or_full_history_cannot_silently_replace_prior_audits() {
    for history in [json!({"unexpected": true}), json!([])] {
        let mut source = network();
        add_switch(&mut source, "switch", "1");
        source.extras_mut().insert(
            "powerio_lindist3flow_preparation".into(),
            json!({"original": true}),
        );
        let history = if history.is_array() {
            Value::Array(vec![
                Value::Null;
                powerio_core::limits::MAX_MODULE_HISTORY_ENTRIES
            ])
        } else {
            history
        };
        source
            .extras_mut()
            .insert("powerio_transform_history".into(), history);
        let original = serde_json::to_value(&source).unwrap();
        let error = prepare_lindist3flow_network(&source, LinDist3FlowPreparationPolicy::Lower)
            .unwrap_err();
        assert!(error.to_string().contains("transformation history"));
        assert_eq!(serde_json::to_value(&source).unwrap(), original);
    }
}

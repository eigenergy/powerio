//! Check the formats listed in the AC power flow benchmark against the facade.

use std::collections::BTreeSet;

#[test]
fn every_source_format_is_listed_in_the_power_flow_corpus() {
    let corpus: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../evals/power-flow/corpus.json")).unwrap();
    let formats: BTreeSet<_> = corpus
        .iter()
        .map(|case| case["format"].as_str().unwrap())
        .collect();
    for entry in powerio::grid_formats().filter(|entry| entry.can_read) {
        assert!(
            formats.contains(entry.format.token),
            "{}",
            entry.format.token
        );
    }
}

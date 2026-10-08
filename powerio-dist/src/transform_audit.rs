//! Retain superseded network-transformation metadata without recursive nesting.

use serde_json::{Value, json};

use crate::MulticonductorNetwork;

const HISTORY: &str = "powerio_transform_history";

/// The latest record keeps its established metadata key. Previous records use
/// a flat archive, bounded like module history. Do not invent past reports or
/// interpret externally supplied records while preserving them.
pub(crate) fn store(
    network: &mut MulticonductorNetwork,
    key: &'static str,
    record: Value,
) -> Result<(), String> {
    let history_len = match network.extras().get(HISTORY) {
        None => 0,
        Some(Value::Array(history)) => history.len(),
        Some(_) => return Err("transformation history must be an array".to_owned()),
    };
    let limit = powerio_core::limits::MAX_MODULE_HISTORY_ENTRIES;
    if history_len > limit {
        return Err(format!("transformation history exceeds {limit} records"));
    }
    let previous = network.extras().get(key);
    if previous == Some(&record) {
        return Ok(());
    }
    if previous.is_some() && history_len == limit {
        return Err(format!(
            "transformation history cannot exceed {limit} records"
        ));
    }
    let extras = network.extras_mut();
    if let Some(previous) = extras.remove(key) {
        let history = extras
            .entry(HISTORY.to_owned())
            .or_insert_with(|| json!([]));
        history
            .as_array_mut()
            .expect("history was checked")
            .push(json!({
                "metadata_key": key,
                "record": previous,
            }));
    }
    extras.insert(key.to_owned(), record);
    Ok(())
}

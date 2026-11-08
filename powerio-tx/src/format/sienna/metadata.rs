//! Only fields without a Sienna schema counterpart belong in `ext`.
//! Electrical fields with a native counterpart always come from that field.
use super::{Result, Value, error, integer};
use serde::{Serialize, de::DeserializeOwned};

const KEY: &str = "org.eigenergy.powerio";

pub(super) fn record<T: Serialize>(
    doc: &mut Value,
    id: i64,
    value: &T,
    fields: &[&str],
) -> Result<()> {
    let object = serde_json::to_value(value).map_err(|e| error(e.to_string()))?;
    let mut extra = serde_json::Map::new();
    for key in fields {
        if let Some(v) = object.get(key) {
            // Required Sienna names are synthesized only when the source lacks
            // one. A supplied name is carried by the native schema field.
            if *key != "name" || v.is_null() {
                extra.insert((*key).into(), v.clone());
            }
        } else if matches!(*key, "uid" | "name") {
            extra.insert((*key).into(), Value::Null);
        }
    }
    if !extra.is_empty() {
        doc["ext"][id.to_string()][KEY] = Value::Object(extra);
    }
    Ok(())
}

pub(super) fn restore<T: Serialize + DeserializeOwned>(
    root: &Value,
    row: &Value,
    value: &mut T,
    fields: &[&str],
) -> Result<()> {
    let id = integer(row, "id")?;
    let Some(extra) = root["ext"][id.to_string()][KEY].as_object() else {
        return Ok(());
    };
    let mut object = serde_json::to_value(&*value).map_err(|e| error(e.to_string()))?;
    for key in fields {
        if let Some(v) = extra.get(*key) {
            if *key == "name" && !v.is_null() {
                return Err(error("ext must not override a native Sienna name"));
            }
            object[key] = v.clone();
        }
    }
    *value = serde_json::from_value(object)
        .map_err(|e| error(format!("invalid PowerIO metadata on component {id}: {e}")))?;
    Ok(())
}

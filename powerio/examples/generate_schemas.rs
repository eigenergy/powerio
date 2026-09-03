#[cfg(feature = "schema")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    generate::main()
}

#[cfg(not(feature = "schema"))]
fn main() {
    eprintln!("enable the `schema` feature to generate JSON Schemas");
    std::process::exit(1);
}

#[cfg(feature = "schema")]
mod generate {
    use std::{
        env, fs,
        path::{Path, PathBuf},
    };

    use serde_json::json;

    pub(super) fn main() -> Result<(), Box<dyn std::error::Error>> {
        let out = env::args_os()
            .nth(1)
            .map_or_else(|| PathBuf::from("docs/schema"), PathBuf::from);

        // Since 0.11.0, the IR schema version is the `powerio` crate version.
        let relative_path = format!("pio-module/{}", powerio::IR_SCHEMA_VERSION);
        let id = format!(
            "https://powerio.dev/schema/pio-module/{}/schema.json",
            powerio::IR_SCHEMA_VERSION
        );
        write_schema(
            serde_json::to_value(powerio::generate_ir_schema())?,
            &out,
            &relative_path,
            &id,
            &[],
        )?;

        Ok(())
    }

    fn write_schema(
        mut schema: serde_json::Value,
        out: &Path,
        rel: &str,
        id: &str,
        also_required: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        spell_nonfinite_floats(&mut schema)?;
        let root = schema
            .as_object_mut()
            .ok_or("schemars returned a non-object schema root")?;
        root.insert("$id".to_owned(), json!(id));

        let properties = root
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .ok_or("schemars returned a root with no properties")?;
        // A name that no longer exists would silently demand a field the
        // document cannot carry, so it is an error rather than a no-op.
        if let Some(missing) = also_required
            .iter()
            .find(|name| !properties.contains_key(**name))
        {
            return Err(format!("`{missing}` is required but is not a property of {rel}").into());
        }
        let required = root
            .entry("required")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or("schemars returned a non-array `required`")?;
        for name in also_required {
            if !required.iter().any(|value| value == name) {
                required.push(json!(name));
            }
        }

        let path = out.join(rel).join("schema.json");
        fs::create_dir_all(path.parent().ok_or("schema path has no parent")?)?;
        let mut text = serde_json::to_string_pretty(&schema)?;
        text.push('\n');
        fs::write(path, text)?;
        Ok(())
    }

    /// Every PowerIO IR document spells a nonfinite float as `"Infinity"`,
    /// `"-Infinity"`, or `"NaN"`, so each float position in the schema accepts
    /// one of those strings beside a number.
    fn spell_nonfinite_floats(
        schema: &mut serde_json::Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::Value;

        fn spell(node: &mut Value) {
            match node {
                Value::Object(map) => {
                    let is_double = map.get("format") == Some(&Value::String("double".into()));
                    let takes_number = match map.get("type") {
                        Some(Value::String(t)) => t == "number",
                        Some(Value::Array(ts)) => ts.iter().any(|t| t == "number"),
                        _ => false,
                    };
                    if is_double && takes_number {
                        let mut number_arm = serde_json::Map::new();
                        number_arm.insert("type".into(), map.remove("type").unwrap());
                        number_arm.insert("format".into(), map.remove("format").unwrap());
                        map.insert(
                            "anyOf".into(),
                            serde_json::json!([
                                number_arm,
                                { "type": "string", "enum": ["Infinity", "-Infinity", "NaN"] }
                            ]),
                        );
                    } else {
                        for v in map.values_mut() {
                            spell(v);
                        }
                    }
                }
                Value::Array(items) => {
                    for v in items {
                        spell(v);
                    }
                }
                _ => {}
            }
        }

        if schema.get("$defs").is_none() {
            return Err("schema has no $defs".into());
        }
        spell(schema);
        Ok(())
    }
}

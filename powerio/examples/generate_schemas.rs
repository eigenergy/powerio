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

        // The schema lives at the version this build writes, and its `$id` is
        // the address that directory is served from.
        write_schema(
            serde_json::to_value(powerio::generate_ir_schema())?,
            &out.join("pio-module")
                .join(powerio::IR_SCHEMA_VERSION)
                .join("schema.json"),
            powerio::IR_SCHEMA_ID,
        )
    }

    fn write_schema(
        mut schema: serde_json::Value,
        path: &Path,
        id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        spell_nonfinite_floats(&mut schema)?;
        let root = schema
            .as_object_mut()
            .ok_or("schemars returned a non-object schema root")?;
        root.insert("$id".to_owned(), json!(id));

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

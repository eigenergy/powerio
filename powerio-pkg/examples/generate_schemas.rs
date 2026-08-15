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

    use schemars::{JsonSchema, schema_for};
    use serde_json::json;

    pub(super) fn main() -> Result<(), Box<dyn std::error::Error>> {
        let out = env::args_os()
            .nth(1)
            .map_or_else(|| PathBuf::from("docs/schema"), PathBuf::from);

        // One published document per powerio lineage; it embeds every payload
        // type. The `$id` names the published location and is not written into
        // `.pio.json` files. The lineage is the same one the reader accepts, so
        // the path moves when and only when a document stops loading.
        let lineage = powerio::version::lineage_path();
        write_schema::<powerio_pkg::NetworkPackage>(
            &out,
            &format!("pio-package/{lineage}"),
            &format!("https://powerio.dev/schema/pio-package/{lineage}"),
            // `powerio_version` carries `serde(default)` so the reader can name
            // a missing field rather than fail on it, and schemars reads any
            // `serde` default as "optional" — `schemars(required)` does not
            // override it. Left alone, the published document would let a
            // producer omit the one field the version gate reads, validate
            // clean, and then be refused by `NetworkPackage::from_json`.
            &["powerio_version"],
        )?;

        Ok(())
    }

    fn write_schema<T: JsonSchema>(
        out: &Path,
        rel: &str,
        id: &str,
        also_required: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut schema = serde_json::to_value(schema_for!(T))?;
        let root = schema
            .as_object_mut()
            .ok_or("schemars returned a non-object schema root")?;
        root.insert("$id".to_owned(), json!(id));

        let properties: Vec<String> = root
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .ok_or("schemars returned a root with no properties")?
            .keys()
            .cloned()
            .collect();
        let required = root
            .entry("required")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or("schemars returned a non-array `required`")?;
        for name in also_required {
            // A name that no longer exists would silently demand a field the
            // document cannot carry, so it is an error rather than a no-op.
            if !properties.iter().any(|property| property == name) {
                return Err(format!("`{name}` is required but is not a property of {rel}").into());
            }
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
}

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

        write_schema::<powerio_pkg::NetworkPackage>(
            &out,
            "pio-package/0.1",
            powerio_pkg::PIO_PACKAGE_SCHEMA_URL,
        )?;
        write_schema::<powerio::BalancedNetwork>(
            &out,
            "pio-payload-balanced/1",
            powerio_pkg::PIO_PAYLOAD_BALANCED_SCHEMA_URL,
        )?;
        write_schema::<powerio_dist::MulticonductorNetwork>(
            &out,
            "pio-payload-multiconductor/2",
            powerio_pkg::PIO_PAYLOAD_MULTICONDUCTOR_SCHEMA_URL,
        )?;

        Ok(())
    }

    fn write_schema<T: JsonSchema>(
        out: &Path,
        rel: &str,
        id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut schema = serde_json::to_value(schema_for!(T))?;
        let root = schema
            .as_object_mut()
            .ok_or("schemars returned a non-object schema root")?;
        root.insert("$id".to_owned(), json!(id));

        let path = out.join(rel).join("schema.json");
        fs::create_dir_all(path.parent().ok_or("schema path has no parent")?)?;
        let mut text = serde_json::to_string_pretty(&schema)?;
        text.push('\n');
        fs::write(path, text)?;
        Ok(())
    }
}

//! The committed current PowerIO IR schema matches the implementation, and
//! the historical schema archive remains intact.

use std::path::Path;

const SCHEMA_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/schema");
const CURRENT_SCHEMA: &str = "pio-ir/2/schema.json";

fn read_schema_file(relative: &str) -> String {
    let path = Path::new(SCHEMA_ROOT).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} is not checked in: {error}. Generate it with \
             `cargo run -p powerio --example generate_schemas --features schema -- docs/schema`",
            path.display()
        )
    })
}

/// The directory holds one PowerIO IR history, not separate package and module
/// schema families.
#[test]
fn the_schema_directory_contains_the_documented_powerio_ir_history() {
    fn collect_files(root: &Path, path: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                collect_files(root, &path, out);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }

    let root = Path::new(SCHEMA_ROOT);
    let mut files = Vec::new();
    collect_files(root, root, &mut files);
    files.sort();
    assert_eq!(
        files,
        [
            "README.md",
            "pio-ir/0.1/schema.json",
            "pio-ir/0.2/schema.json",
            "pio-ir/0.9/schema.json",
            "pio-ir/1/schema.json",
            CURRENT_SCHEMA,
        ]
    );
}

/// Historical files preserve the identifiers that appeared in their releases.
#[test]
fn historical_schemas_preserve_their_original_identifiers() {
    for (path, expected_id) in [
        (
            "pio-ir/0.1/schema.json",
            "https://powerio.dev/schema/pio-package/0.1",
        ),
        (
            "pio-ir/0.2/schema.json",
            "https://powerio.dev/schema/pio-package/0.2",
        ),
        (
            "pio-ir/0.9/schema.json",
            "https://powerio.dev/schema/pio-package/0.9/schema.json",
        ),
        (
            "pio-ir/1/schema.json",
            "https://powerio.dev/schema/pio-module/1/schema.json",
        ),
    ] {
        let schema: serde_json::Value = serde_json::from_str(&read_schema_file(path)).unwrap();
        assert_eq!(schema["$id"], expected_id, "historical schema {path}");
    }
}

/// The current schema uses the public PowerIO IR identity and generation.
#[test]
fn the_current_powerio_ir_schema_is_committed() {
    let schema: serde_json::Value =
        serde_json::from_str(&read_schema_file(CURRENT_SCHEMA)).unwrap();
    assert_eq!(schema["$id"], powerio::IR_SCHEMA_ID);
    assert_eq!(
        schema["properties"]["schema"]["const"],
        powerio::IR_SCHEMA_NAME
    );
    assert_eq!(
        schema["properties"]["version"]["const"],
        powerio::IR_VERSION
    );
}

/// Every property of every value kind in the current document carries a type,
/// a `$ref`, or a composed schema.
#[test]
fn every_powerio_ir_property_is_typed() {
    fn walk(node: &serde_json::Value, path: &str, bad: &mut Vec<String>) {
        match node {
            serde_json::Value::Object(map) => {
                if map.get("type") == Some(&serde_json::Value::String("object".into()))
                    && let Some(serde_json::Value::Object(props)) = map.get("properties")
                {
                    for (name, prop) in props {
                        if let serde_json::Value::Object(keys) = prop
                            && keys.keys().all(|key| key == "description")
                        {
                            bad.push(format!("{path}.{name}"));
                        }
                    }
                }
                for (key, value) in map {
                    walk(value, &format!("{path}.{key}"), bad);
                }
            }
            serde_json::Value::Array(items) => {
                for (index, value) in items.iter().enumerate() {
                    walk(value, &format!("{path}[{index}]"), bad);
                }
            }
            _ => {}
        }
    }

    let schema: serde_json::Value =
        serde_json::from_str(&read_schema_file(CURRENT_SCHEMA)).unwrap();
    let mut bad = Vec::new();
    walk(&schema, "$", &mut bad);
    assert!(bad.is_empty(), "untyped properties: {bad:?}");
}

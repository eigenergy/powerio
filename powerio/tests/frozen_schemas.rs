//! The committed PowerIO 1.0 IR schema matches the implementation.

mod helpers;
use helpers::serialize_module_text;

fn current_ir_version() -> u64 {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = powerio::parse_with_options(
        powerio::Source::open(path).unwrap(),
        &powerio::ParseOptions::default().format("matpower").unwrap(),
    )
    .unwrap();
    let text = serialize_module_text(&module).unwrap();
    let document: serde_json::Value = serde_json::from_str(&text).unwrap();
    document["version"].as_u64().unwrap()
}

/// PowerIO 1.0 publishes one IR schema. Extra schema directories are stale
/// prerelease artifacts, not supported document generations.
#[test]
fn the_schema_directory_contains_only_powerio_1() {
    fn collect_files(root: &std::path::Path, path: &std::path::Path, out: &mut Vec<String>) {
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

    let root = std::path::Path::new("../docs/schema");
    let mut files = Vec::new();
    collect_files(root, root, &mut files);
    files.sort();
    assert_eq!(files, ["README.md", "pio-module/1/schema.json"]);
}

/// The document for this build's PowerIO IR version is committed.
#[test]
fn the_current_module_document_is_committed() {
    let version = current_ir_version();
    let path = format!("../docs/schema/pio-module/{version}/schema.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{path} is the schema document this build serves. Generate it with \
             `cargo run -p powerio --example generate_schemas --features schema -- \
             docs/schema` and commit it: {e}"
        )
    });
    let id = format!("https://powerio.dev/schema/pio-module/{version}/schema.json");
    assert!(
        text.contains(&id),
        "{path} does not declare `$id` {id}; regenerate it"
    );
}

/// Every property of every value kind in the current module document carries
/// a type, a `$ref`, or a composed schema. A field serialized through an
/// opaque `serde_json::Value` leaves a property whose only key is its
/// `description`, and a consumer generating types from the document gets a
/// hole where the data is.
#[test]
fn every_module_document_property_is_typed() {
    fn walk(node: &serde_json::Value, path: &str, bad: &mut Vec<String>) {
        match node {
            serde_json::Value::Object(map) => {
                if map.get("type") == Some(&serde_json::Value::String("object".into()))
                    && let Some(serde_json::Value::Object(props)) = map.get("properties")
                {
                    for (name, prop) in props {
                        if let serde_json::Value::Object(keys) = prop {
                            if keys.keys().all(|k| k == "description") {
                                bad.push(format!("{path}.{name}"));
                            }
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

    let path = format!(
        "../docs/schema/pio-module/{}/schema.json",
        current_ir_version()
    );
    let text = std::fs::read_to_string(path).unwrap();
    let schema: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut bad = Vec::new();
    walk(&schema, "$", &mut bad);
    assert!(bad.is_empty(), "untyped properties: {bad:?}");
}

//! The committed current PowerIO IR schema matches the implementation, and
//! the schema archive matches the history `docs/schema/README.md` documents.

use std::path::Path;

const SCHEMA_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/schema");

/// The `$id` each archived release wrote into its schema: evidence of what
/// those releases produced, so it is pinned here rather than derived.
const HISTORICAL_IDS: [(&str, &str); 4] = [
    ("0.1", "https://powerio.dev/schema/pio-package/0.1"),
    ("0.2", "https://powerio.dev/schema/pio-package/0.2"),
    (
        "0.9",
        "https://powerio.dev/schema/pio-package/0.9/schema.json",
    ),
    (
        "0.10.0",
        "https://powerio.dev/schema/pio-module/1/schema.json",
    ),
];

/// One row of the README's history table.
struct ArchivedSchema {
    path: String,
    current: bool,
}

/// The archive `docs/schema/README.md` documents, read from its history
/// table: one row per release, naming the archived file and whether this is
/// the document the current reader writes.
fn documented_archive() -> Vec<ArchivedSchema> {
    let readme = read_schema_file("README.md");
    let rows: Vec<ArchivedSchema> = readme
        .lines()
        .filter(|line| line.starts_with("| v"))
        .map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            assert_eq!(cells.len(), 6, "a history table row has four cells: {line}");
            ArchivedSchema {
                path: cells[3].trim_matches('`').to_owned(),
                current: cells[4] == "current",
            }
        })
        .collect();
    assert!(
        !rows.is_empty(),
        "docs/schema/README.md has a history table with one row per release"
    );
    rows
}

fn current_schema_path() -> String {
    format!("pio-module/{}/schema.json", powerio::IR_SCHEMA_VERSION)
}

fn read_schema_file(relative: &str) -> String {
    let path = Path::new(SCHEMA_ROOT).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} is not checked in: {error}. The current schema is generated with \
             `cargo run -p powerio --example generate_schemas --features schema -- docs/schema`",
            path.display()
        )
    })
}

fn declares_id(text: &str, id: &str) -> bool {
    text.contains(&format!("\"$id\": \"{id}\""))
}

/// The directory is one PowerIO IR history: exactly the files the README's
/// table documents, plus the README itself.
#[test]
fn the_schema_directory_is_the_documented_powerio_ir_history() {
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

    let mut documented: Vec<String> = documented_archive()
        .into_iter()
        .map(|row| row.path)
        .collect();
    documented.push("README.md".to_owned());
    documented.sort();
    assert_eq!(
        files, documented,
        "docs/schema holds exactly what its README documents"
    );
}

/// The README's one current row is this build's schema, and every other row
/// is a historical release whose `$id` is pinned above, in release order.
#[test]
fn the_documented_history_ends_at_this_build() {
    let rows = documented_archive();
    let current: Vec<&str> = rows
        .iter()
        .filter(|row| row.current)
        .map(|row| row.path.as_str())
        .collect();
    assert_eq!(current, [current_schema_path().as_str()]);

    let historical: Vec<&str> = rows
        .iter()
        .filter(|row| !row.current)
        .map(|row| row.path.as_str())
        .collect();
    let pinned: Vec<String> = HISTORICAL_IDS
        .iter()
        .map(|(version, _)| format!("pio-module/{version}/schema.json"))
        .collect();
    assert_eq!(historical, pinned);
}

/// Historical files preserve the identifiers their releases wrote, even though
/// the archive presents them as one PowerIO IR lineage.
#[test]
fn historical_schemas_preserve_their_original_identifiers() {
    for (version, id) in HISTORICAL_IDS {
        let text = read_schema_file(&format!("pio-module/{version}/schema.json"));
        assert!(
            declares_id(&text, id),
            "historical schema {version} does not declare `$id` {id}"
        );
    }
}

/// The document for this build's PowerIO IR version is committed and names
/// the address this build serves it from.
#[test]
fn the_current_module_document_is_committed() {
    let text = read_schema_file(&current_schema_path());
    assert!(
        declares_id(&text, powerio::IR_SCHEMA_ID),
        "{} does not declare `$id` {}; regenerate it",
        current_schema_path(),
        powerio::IR_SCHEMA_ID
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

    let text = read_schema_file(&current_schema_path());
    let schema: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut bad = Vec::new();
    walk(&schema, "$", &mut bad);
    assert!(bad.is_empty(), "untyped properties: {bad:?}");
}
